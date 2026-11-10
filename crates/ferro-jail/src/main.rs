//! ferro-jail: runs a program that opens files from anywhere (the video
//! player, the thumbnailer, the metadata tools) so that a booby-trapped
//! file that takes it over can do little with it:
//!
//! * no internet: sockets for IPv4, IPv6 and raw packets are refused
//!   (seccomp), so nothing can be sent anywhere or fetched;
//! * no changing files: everything can be read, as before, but nothing
//!   written, made, removed or renamed (Landlock), except what the program
//!   is there to do (below) and what `--write` names. So nothing can be left
//!   in the home folder to run at the next logon;
//! * no other programs: only the program itself and those named with
//!   `--exec` may be started (Landlock), so no shell, no downloader;
//! * no signals to programs outside (Landlock, from Linux 6.12), and no
//!   connecting to their abstract Unix sockets, unless the screen is one:
//!   a program that may draw keeps them when the display server listens on
//!   one, as Xorg does (the tools that draw nothing never do);
//! * none of the kernel's riskier corners (ptrace, io_uring, bpf, mounts,
//!   new namespaces...), and no gaining privileges through setuid programs.
//!
//! What it doesn't stop, so nobody counts on it:
//!
//! * Unix sockets that have a path stay reachable: the display server, and
//!   the session bus (the video player's media keys go through it). The
//!   kernels FerroOS runs on can't refuse those per program;
//! * a program that was taken over can still load another program's code
//!   through the dynamic loader (see ferro-sandbox), inside the same jail.
//!
//! The screen, the sound and reading the files it was given work as before.
//! Without Landlock in the kernel nothing is run at all.
//!
//!     ferro-jail [--exec PATH]... [--write PATH]... [--] PROGRAM [ARGS...]
//!
//! `--write FILE` lets the file be changed as it is; `--write FOLDER` lets
//! plain files be made, changed and removed in it and beneath it.
//!
//! What FerroOS's own jailed programs need is known here, from how they're
//! started:
//!
//! * mpv (the Video Player): Delete moves a video to the Recycle Bin, so
//!   files may be taken out of the folders of the videos it was given (not
//!   changed, not made) and put in the Bin; the pictures over the timeline
//!   go in a scratch folder of its own in memory;
//! * ffmpeg writing a file (the last argument, as when metadata is
//!   stripped): files may be made and changed in that file's folder. With
//!   nothing to write (thumbnails go to its output), nothing;
//! * exiftool changing a file (`-overwrite_original`; it writes a copy
//!   beside it and swaps them): the same, in the folder of each file named.
//!   Only reading, nothing.
//!
//! A folder's rights reach everything beneath it, so the last two are never
//! given for the home folder itself or anything above it: a file right in
//! the home folder is refused, with the reason.

use ferro_sandbox::linux::Jail;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{exit, Command};

fn main() {
    let mut args = std::env::args().skip(1);
    let mut jail = Jail::default();
    let program = loop {
        match args.next() {
            Some(a) if a == "--exec" => jail.exec.extend(args.next()),
            Some(a) if a == "--write" => jail.change.extend(args.next()),
            Some(a) if a == "--" => break args.next(),
            Some(a) => break Some(a),
            None => break None,
        }
    };
    let Some(program) = program else {
        eprintln!("usage: ferro-jail [--exec PATH]... [--write PATH]... [--] PROGRAM [ARGS...]");
        exit(2);
    };
    let Some(path) = resolve(&program) else {
        eprintln!("ferro-jail: {program}: not found");
        exit(127);
    };
    let args: Vec<String> = args.collect();
    jail.exec.push(path.to_string_lossy().into_owned());
    let place = Place::here();
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut env = Vec::new();
    let display = std::env::var("DISPLAY").ok().and_then(|d| display_number(&d));
    let on_abstract = || display.is_some_and(|n| std::fs::read_to_string("/proc/net/unix").is_ok_and(|s| listens_abstract(&s, n)));
    let needs = match name.as_str() {
        "mpv" => {
            jail.abstract_sockets = on_abstract();
            env = scratch(&place, &mut jail);
            player(&args, &place, &mut jail);
            Ok(())
        }
        "ffmpeg" => writer(ffmpeg_output(&args).into_iter().collect(), &place, &mut jail),
        "exiftool" => writer(exiftool_changes(&args), &place, &mut jail),
        _ => {
            jail.abstract_sockets = on_abstract();
            Ok(())
        }
    };
    if needs.is_err() {
        // One line, whole: it's what the window that asked shows.
        eprintln!(
            "The file is right in the home folder, where {name} may not change anything. Move it into a folder (Documents, Pictures...) first."
        );
        exit(126);
    }
    if let Err(e) = lock_down(&jail) {
        // Never run it unprotected: that's what it was asked not to do.
        eprintln!("ferro-jail: can't protect {program}: {e}");
        exit(126);
    }
    let err = Command::new(&path).args(args).envs(env).exec();
    eprintln!("ferro-jail: {program}: {err}");
    exit(126);
}

fn lock_down(jail: &Jail) -> std::io::Result<()> {
    use ferro_sandbox::linux;
    linux::lock_privileges()?;
    linux::install_seccomp()?;
    linux::install_net_gate(ferro_sandbox::NetMode::Deny)?;
    // No Landlock, no jail: the files and the list of programs are all its.
    let abi = linux::jail(jail)?;
    if abi < 6 {
        eprintln!("ferro-jail: this kernel is older than Linux 6.12: other programs' signals and abstract sockets aren't closed off");
    }
    Ok(())
}

/// Where the program is started: the folder paths are relative to, the
/// home folder and the session's folder in memory.
struct Place {
    cwd: PathBuf,
    home: Option<PathBuf>,
    runtime: Option<PathBuf>,
}

impl Place {
    fn here() -> Self {
        let dir = |name: &str| std::env::var_os(name).map(PathBuf::from).filter(|p| p.is_absolute()).and_then(|p| p.canonicalize().ok());
        Self { cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")), home: dir("HOME"), runtime: dir("XDG_RUNTIME_DIR") }
    }

    /// The folder `file` is in (or `file` itself, if it's a folder), with
    /// links followed: where Landlock's rule goes. None if there's none.
    fn folder_of(&self, file: &str) -> Option<PathBuf> {
        let path = self.cwd.join(file);
        let folder = if path.is_dir() { path } else { path.parent()?.to_path_buf() };
        folder.canonicalize().ok().filter(|f| f.is_dir())
    }

    /// The home folder, or one it's inside: rights there would reach the
    /// whole home folder, the files that run at logon among them.
    fn holds_home(&self, folder: &Path) -> bool {
        self.home.as_ref().map_or(folder == Path::new("/"), |home| home.starts_with(folder))
    }
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// The arguments that aren't options: all of them after `--`, and before
/// it those that don't start with a dash.
fn plain_args(args: &[String]) -> Vec<&str> {
    let end = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let before = args[..end].iter().filter(|a| !a.starts_with('-'));
    before.chain(args.iter().skip(end + 1)).map(String::as_str).collect()
}

/// The Video Player: files may leave the folders of the videos it was given
/// (Delete; the folder's other videos play too), for the Recycle Bin.
fn player(args: &[String], place: &Place, jail: &mut Jail) {
    let mut folders: Vec<PathBuf> = plain_args(args).into_iter().filter(|a| place.cwd.join(a).exists()).filter_map(|a| place.folder_of(a)).collect();
    folders.sort();
    folders.dedup();
    let bin = place.home.as_ref().map(|h| h.join(".local/share/Trash")).filter(|b| b.is_dir());
    if let Some(bin) = bin {
        let drive = |p: &Path| p.metadata().map(|m| m.dev()).ok();
        // A video of this drive is moved into the Bin as it is, and the Bin
        // needn't be writable for that. One of another drive (a USB stick)
        // has to be copied in: then the Bin is written to, and the kernel
        // has videos of this drive copied too (a file may not be moved to
        // where it could be changed, if it couldn't be where it was).
        if folders.iter().all(|f| drive(f) == drive(&bin)) {
            jail.move_into.push(text(&bin));
        } else {
            jail.change.push(text(&bin));
        }
    }
    jail.take_from.extend(folders.iter().map(|f| text(f)));
}

/// A folder of the player's own for what it makes while it runs (the
/// pictures over the timeline), instead of the session's folder in memory
/// with everyone else's files and sockets in it. Returns the environment
/// that sends it there, and keeps a sound server findable.
fn scratch(place: &Place, jail: &mut Jail) -> Vec<(String, String)> {
    let Some(runtime) = &place.runtime else { return Vec::new() };
    let dir = runtime.join("ferro-jail");
    let _ = std::fs::DirBuilder::new().mode(0o700).create(&dir);
    // Ours, and a folder: not a link to somewhere else.
    if !dir.symlink_metadata().is_ok_and(|m| m.is_dir()) {
        return Vec::new();
    }
    jail.change.push(text(&dir));
    let mut env = vec![("XDG_RUNTIME_DIR".to_owned(), text(&dir))];
    for (name, value) in [("PULSE_RUNTIME_PATH", runtime.join("pulse")), ("PIPEWIRE_RUNTIME_DIR", runtime.clone())] {
        if std::env::var_os(name).is_none() {
            env.push((name.to_owned(), text(&value)));
        }
    }
    env
}

/// The file ffmpeg writes: its last argument, unless that's its output
/// (`-`, `pipe:`) or an option.
fn ffmpeg_output(args: &[String]) -> Option<&str> {
    let last = args.last()?.as_str();
    (!last.starts_with('-') && !last.starts_with("pipe:") && !last.contains("://")).then_some(last)
}

/// The files exiftool changes: the ones named, when it's asked to write.
fn exiftool_changes(args: &[String]) -> Vec<&str> {
    if !args.iter().any(|a| a == "-overwrite_original" || a == "-overwrite_original_in_place") {
        return Vec::new();
    }
    plain_args(args)
}

/// A file whose folder is the home folder, or holds it.
#[derive(Debug, PartialEq, Eq)]
struct InHome;

/// Lets files be made and changed in the folder of each of `files`, unless
/// one's folder would open up the home folder.
fn writer(files: Vec<&str>, place: &Place, jail: &mut Jail) -> Result<(), InHome> {
    for file in files {
        let Some(folder) = place.folder_of(file) else { continue };
        if place.holds_home(&folder) {
            return Err(InHome);
        }
        jail.change.push(text(&folder));
    }
    Ok(())
}

/// The number of a display on this computer (`:0`, `:0.0`, `unix:0`).
fn display_number(display: &str) -> Option<u32> {
    let (host, rest) = display.rsplit_once(':')?;
    if !(host.is_empty() || host == "unix") {
        return None;
    }
    rest.split('.').next()?.parse().ok()
}

/// Whether display `n`'s server listens on an abstract socket, going by
/// the kernel's list of Unix sockets (/proc/net/unix).
fn listens_abstract(sockets: &str, n: u32) -> bool {
    let name = format!("@/tmp/.X11-unix/X{n}");
    sockets.lines().any(|l| l.split_whitespace().last() == Some(name.as_str()))
}

/// The program's full path: as given if it has a slash, else from $PATH.
fn resolve(program: &str) -> Option<PathBuf> {
    if program.contains('/') {
        return Path::new(program).is_file().then(|| PathBuf::from(program));
    }
    let paths = std::env::var_os("PATH").unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin".into());
    std::env::split_paths(&paths).map(|d| d.join(program)).find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_owned()).collect()
    }

    /// The folder tree of a test, removed when the test is over.
    struct Tree(PathBuf);

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A folder tree to try things in: home/ with Videos/ and the Bin.
    fn place(name: &str) -> (Tree, Place) {
        let root = std::env::temp_dir().join(format!("ferro-jail-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["home/Videos", "home/.local/share/Trash", "run"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        for f in ["home/Videos/a.mp4", "home/top.jpg"] {
            std::fs::write(root.join(f), b"x").unwrap();
        }
        let root = root.canonicalize().unwrap();
        let place = Place { cwd: root.join("home"), home: Some(root.join("home")), runtime: Some(root.join("run")) };
        (Tree(root), place)
    }

    #[test]
    fn displays_and_their_sockets() {
        assert_eq!(display_number(":0"), Some(0));
        assert_eq!(display_number(":5.0"), Some(5));
        assert_eq!(display_number("unix:12"), Some(12));
        assert_eq!(display_number("elsewhere:0"), None);
        assert_eq!(display_number("nonsense"), None);
        let sockets =
            "Num RefCount Protocol Flags Type St Inode Path\n0: 2 0 10000 1 1 77 /tmp/.X11-unix/X5\n0: 2 0 10000 1 1 78 @/tmp/.X11-unix/X50\n";
        assert!(!listens_abstract(sockets, 5), "only the file, and X50 isn't X5");
        assert!(listens_abstract(sockets, 50));
    }

    #[test]
    fn options_arent_files() {
        let args = strings(&["--script=/x/osc.lua", "-v", "a.mp4", "--", "-odd.mp4", "b.mp4"]);
        assert_eq!(plain_args(&args), ["a.mp4", "-odd.mp4", "b.mp4"]);
    }

    #[test]
    fn the_player_takes_videos_to_the_bin_and_nothing_else() {
        let (_tree, p) = place("player");
        let home = p.home.clone().unwrap();
        let mut jail = Jail::default();
        player(&strings(&["--osc=no", "--", "Videos/a.mp4", "Videos/gone.mp4"]), &p, &mut jail);
        assert_eq!(jail.take_from, [text(&home.join("Videos"))]);
        assert_eq!(jail.move_into, [text(&home.join(".local/share/Trash"))], "same drive: moved, not written");
        assert!(jail.change.is_empty() && jail.exec.is_empty());
    }

    #[test]
    fn the_player_gets_its_own_scratch_folder() {
        let (_tree, p) = place("scratch");
        let mut jail = Jail::default();
        let env = scratch(&p, &mut jail);
        let dir = p.runtime.clone().unwrap().join("ferro-jail");
        assert!(dir.is_dir());
        assert_eq!(jail.change, [text(&dir)]);
        assert_eq!(env[0], ("XDG_RUNTIME_DIR".to_owned(), text(&dir)));
    }

    #[test]
    fn ffmpeg_writes_only_when_it_names_a_file() {
        assert_eq!(ffmpeg_output(&strings(&["-i", "/v/a.mp4", "-f", "image2pipe", "-"])), None);
        assert_eq!(ffmpeg_output(&strings(&["-i", "/v/a.mp4", "pipe:1"])), None);
        assert_eq!(ffmpeg_output(&strings(&["-y", "-i", "/v/a.mp4", "/v/.tmp.mp4"])), Some("/v/.tmp.mp4"));
    }

    #[test]
    fn exiftool_writes_only_when_asked_to() {
        assert!(exiftool_changes(&strings(&["-args", "-G1", "--", "/p/a.jpg"])).is_empty());
        assert_eq!(exiftool_changes(&strings(&["-overwrite_original", "-P", "-all=", "--", "/p/a.jpg"])), ["/p/a.jpg"]);
    }

    #[test]
    fn a_writer_gets_the_files_folder_but_never_the_home_folder() {
        let (_tree, p) = place("writer");
        let home = p.home.clone().unwrap();
        let mut jail = Jail::default();
        assert_eq!(writer(vec!["Videos/.tmp.mp4"], &p, &mut jail), Ok(()));
        assert_eq!(jail.change, [text(&home.join("Videos"))]);
        let mut jail = Jail::default();
        assert_eq!(writer(vec!["top.jpg"], &p, &mut jail), Err(InHome));
        assert_eq!(writer(vec!["/etc/hostname"], &p, &mut jail), Ok(()), "not above the home folder");
        assert_eq!(writer(vec![&text(home.parent().unwrap().join("x").as_path())], &p, &mut jail), Err(InHome), "above it");
        assert_eq!(jail.change, ["/etc"]);
    }
}
