//! Runs the real ferro-jail: what's inside may run itself, not others, and
//! changes no files but the ones it's there for. (Each test passes without
//! testing anything on a kernel without Landlock, where ferro-jail refuses
//! to run the program at all.)

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn jail() -> Command {
    // (Cargo sets it as the test runs, not as it's built.)
    Command::new(std::env::var_os("CARGO_BIN_EXE_ferro-jail").expect("cargo test sets CARGO_BIN_EXE_ferro-jail"))
}

fn no_landlock(out: &Output) -> bool {
    String::from_utf8_lossy(&out.stderr).contains("Landlock is not enabled")
}

/// A home folder of its own for a test, with Videos/, the Recycle Bin and
/// the session's folder in memory beside it: (root, home).
fn home(name: &str) -> (PathBuf, PathBuf) {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("jail-{name}"));
    let _ = fs::remove_dir_all(&root);
    for d in ["home/Videos", "home/.local/share/Trash", "run", "bin"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }
    let root = root.canonicalize().unwrap();
    let home = root.join("home");
    (root, home)
}

/// A shell script called `name`, standing in for the real program.
fn script(root: &Path, name: &str, body: &str) -> PathBuf {
    let path = root.join("bin").join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn only_the_jailed_program_runs() {
    // The shell itself runs; a program it tries to start doesn't, unless
    // allowed.
    let out = jail().args(["/bin/sh", "-c", "/bin/true && echo started"]).output().unwrap();
    if no_landlock(&out) {
        return;
    }
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(!String::from_utf8_lossy(&out.stdout).contains("started"), "another program ran: {said}");
    let out = jail().args(["--exec", "/bin/true", "/bin/sh", "-c", "/bin/true && echo started"]).output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("started"));
}

#[test]
fn missing_program_is_reported() {
    let out = jail().arg("no-such-program-anywhere").output().unwrap();
    assert_eq!(out.status.code(), Some(127));
}

#[test]
fn nothing_is_left_in_the_home_folder() {
    let (root, home) = home("home");
    fs::write(home.join(".profile"), "as it was\n").unwrap();
    let run = |extra: &[&str]| {
        let try_all = "echo x >\"$HOME/.bashrc\"; echo x >>\"$HOME/.profile\"; echo x >\"$HOME/Videos/new\"; \
                       mkdir \"$HOME/.config\"; ln -s /bin/sh \"$HOME/link\"; cat \"$HOME/.profile\"";
        let tools = ["--exec", "/bin/cat", "--exec", "/bin/mkdir", "--exec", "/bin/ln"];
        jail().args(tools).args(extra).args(["/bin/sh", "-c", try_all]).env("HOME", &home).current_dir(&root).output().unwrap()
    };
    let out = run(&[]);
    if no_landlock(&out) {
        return;
    }
    assert_eq!(String::from_utf8_lossy(&out.stdout), "as it was\n", "read as before, not changed");
    for made in [".bashrc", "Videos/new", ".config", "link"] {
        assert!(home.join(made).symlink_metadata().is_err(), "{made} was made");
    }
    // --write: a folder's files, or one file as it is; nothing else.
    let (videos, profile) = (home.join("Videos"), home.join(".profile"));
    let out = run(&["--write", videos.to_str().unwrap(), "--write", profile.to_str().unwrap()]);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "as it was\nx\n");
    assert!(home.join("Videos/new").is_file());
    for made in [".bashrc", ".config", "link"] {
        assert!(home.join(made).symlink_metadata().is_err(), "{made} was made");
    }
}

#[test]
fn the_video_player_moves_videos_to_the_bin_and_changes_nothing() {
    let (root, home) = home("player");
    for video in ["a.mp4", "b.mp4"] {
        fs::write(home.join("Videos").join(video), "film").unwrap();
    }
    // Delete, as ferro-delete.lua does it; then what a hijacked player
    // might try; then the scratch folder for the timeline's pictures.
    let mpv = script(
        &root,
        "mpv",
        "mv \"$2\" \"$HOME/.local/share/Trash/\" && echo moved\n\
         echo x >>\"$HOME/Videos/b.mp4\"\n\
         echo x >\"$HOME/Videos/new.desktop\"\n\
         echo x >\"$HOME/.bashrc\"\n\
         echo x >>\"$HOME/.local/share/Trash/a.mp4\"\n\
         echo x >\"$XDG_RUNTIME_DIR/ferro-thumb-1\" && echo \"scratch $XDG_RUNTIME_DIR\"",
    );
    let out = jail()
        .args(["--exec", "/bin/sh", "--exec", "/bin/mv"])
        .arg(&mpv)
        .args(["--", "Videos/a.mp4"])
        .env("HOME", &home)
        .env("XDG_RUNTIME_DIR", root.join("run"))
        .current_dir(&home)
        .output()
        .unwrap();
    if no_landlock(&out) {
        return;
    }
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(said.contains("moved"), "{said} {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(fs::read_to_string(home.join(".local/share/Trash/a.mp4")).unwrap(), "film", "in the Bin, as it was");
    assert!(!home.join("Videos/a.mp4").exists());
    assert_eq!(fs::read_to_string(home.join("Videos/b.mp4")).unwrap(), "film", "the folder's other files aren't changed");
    assert!(!home.join("Videos/new.desktop").exists() && !home.join(".bashrc").exists());
    let scratch = root.join("run/ferro-jail");
    assert!(said.contains(&format!("scratch {}", scratch.display())), "{said}");
    assert!(scratch.join("ferro-thumb-1").is_file() && !root.join("run/ferro-thumb-1").exists());
}

#[test]
fn a_tool_writes_beside_the_file_it_changes_but_not_in_the_home_folder() {
    let (root, home) = home("tool");
    // ffmpeg copying a video over without its metadata: the last argument
    // is what it writes.
    let ffmpeg = script(&root, "ffmpeg", "for last; do :; done\necho copy >\"$last\"; echo x >\"$HOME/.bashrc\"");
    let run = |out_file: &Path| {
        jail()
            .args(["--exec", "/bin/sh"])
            .arg(&ffmpeg)
            .args(["-y", "-i", "in.mp4"])
            .arg(out_file)
            .env("HOME", &home)
            .current_dir(&home)
            .output()
            .unwrap()
    };
    let out = run(&home.join("Videos/.tmp.mp4"));
    if no_landlock(&out) {
        return;
    }
    assert_eq!(fs::read_to_string(home.join("Videos/.tmp.mp4")).unwrap(), "copy\n");
    assert!(!home.join(".bashrc").exists());
    // Right in the home folder: refused before it starts, with the reason.
    let out = run(&home.join(".tmp.mp4"));
    assert_eq!(out.status.code(), Some(126));
    assert!(String::from_utf8_lossy(&out.stderr).starts_with("The file is right in the home folder"));
    assert!(!home.join(".tmp.mp4").exists() && !home.join(".bashrc").exists());
    // With nothing to write (a thumbnail goes to its output), nothing is.
    let out =
        jail().args(["--exec", "/bin/sh"]).arg(&ffmpeg).args(["-i", "Videos/in.mp4", "-"]).env("HOME", &home).current_dir(&home).output().unwrap();
    assert!(out.status.success() || !home.join("-").exists());
    assert!(!home.join("-").exists() && !home.join(".bashrc").exists());
}

#[test]
fn the_sockets_that_would_let_it_out_arent_there() {
    // A session's folder of its own: the bus, a sound server's socket.
    let (root, _home) = home("sockets");
    let run = root.join("run");
    fs::write(run.join("bus"), "").unwrap();
    fs::create_dir_all(run.join("pulse")).unwrap();
    fs::write(run.join("pulse/native"), "").unwrap();
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    let look = "for p in \"$R/bus\" \"$R/pulse/native\" /run/dbus/system_bus_socket \"/run/user/$U/bus\" \"/run/user/$U/systemd/private\"; do \
                [ -e \"$p\" ] && echo \"there $p\"; done; echo looked";
    let out = jail().args(["/bin/sh", "-c", look]).env("XDG_RUNTIME_DIR", &run).env("R", &run).env("U", uid.to_string()).output().unwrap();
    let said = String::from_utf8_lossy(&out.stderr);
    if no_landlock(&out) || said.contains("can't be hidden") {
        return;
    }
    assert_eq!(String::from_utf8_lossy(&out.stdout), format!("there {}/pulse/native\nlooked\n", run.display()), "{said}");
    // Outside, all as it was.
    assert!(run.join("bus").exists() && run.join("pulse/native").exists());
}
