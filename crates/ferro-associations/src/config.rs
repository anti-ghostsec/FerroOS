//! File association configuration parsing, resolution, and persistence.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssociationEntry {
    pub ext: String,
    pub desc: String,
    pub command: String,
}

pub const PRESET_APPS: [(&str, &str); 6] = [
    ("Geany Text Editor", "geany %f"),
    ("FerroOS Image Viewer", "ferro-viewer %f"),
    ("FerroOS Video Player", "ferro-video %f"),
    ("LibreWolf Browser", "librewolf %f"),
    ("Terminal Editor (Nano)", "ferro-terminal -e nano %f"),
    ("System Default (xdg-open)", "xdg-open %f"),
];

pub fn default_associations() -> Vec<AssociationEntry> {
    vec![
        AssociationEntry { ext: "txt".into(), desc: "Text Document".into(), command: "geany %f".into() },
        AssociationEntry { ext: "md".into(), desc: "Markdown Document".into(), command: "geany %f".into() },
        AssociationEntry { ext: "rs".into(), desc: "Rust Source File".into(), command: "geany %f".into() },
        AssociationEntry { ext: "c".into(), desc: "C Source File".into(), command: "geany %f".into() },
        AssociationEntry { ext: "h".into(), desc: "C Header File".into(), command: "geany %f".into() },
        AssociationEntry { ext: "sh".into(), desc: "Shell Script".into(), command: "geany %f".into() },
        AssociationEntry { ext: "conf".into(), desc: "Configuration File".into(), command: "geany %f".into() },
        AssociationEntry { ext: "ini".into(), desc: "INI Configuration".into(), command: "geany %f".into() },
        AssociationEntry { ext: "png".into(), desc: "PNG Image".into(), command: "ferro-viewer %f".into() },
        AssociationEntry { ext: "jpg".into(), desc: "JPEG Image".into(), command: "ferro-viewer %f".into() },
        AssociationEntry { ext: "jpeg".into(), desc: "JPEG Image".into(), command: "ferro-viewer %f".into() },
        AssociationEntry { ext: "bmp".into(), desc: "Bitmap Image".into(), command: "ferro-viewer %f".into() },
        AssociationEntry { ext: "webp".into(), desc: "WebP Image".into(), command: "ferro-viewer %f".into() },
        AssociationEntry { ext: "gif".into(), desc: "GIF Animation".into(), command: "ferro-viewer %f".into() },
        AssociationEntry { ext: "mp4".into(), desc: "MP4 Video".into(), command: "ferro-video %f".into() },
        AssociationEntry { ext: "m4v".into(), desc: "MP4 Video".into(), command: "ferro-video %f".into() },
        AssociationEntry { ext: "mkv".into(), desc: "Matroska Video".into(), command: "ferro-video %f".into() },
        AssociationEntry { ext: "webm".into(), desc: "WebM Video".into(), command: "ferro-video %f".into() },
        AssociationEntry { ext: "avi".into(), desc: "AVI Video".into(), command: "ferro-video %f".into() },
        AssociationEntry { ext: "mov".into(), desc: "QuickTime Video".into(), command: "ferro-video %f".into() },
        AssociationEntry { ext: "wmv".into(), desc: "Windows Media Video".into(), command: "ferro-video %f".into() },
        AssociationEntry { ext: "flv".into(), desc: "Flash Video".into(), command: "ferro-video %f".into() },
        AssociationEntry { ext: "mpg".into(), desc: "MPEG Video".into(), command: "ferro-video %f".into() },
        AssociationEntry { ext: "mpeg".into(), desc: "MPEG Video".into(), command: "ferro-video %f".into() },
        AssociationEntry { ext: "ts".into(), desc: "MPEG Transport Stream".into(), command: "ferro-video %f".into() },
        AssociationEntry { ext: "ogv".into(), desc: "Ogg Video".into(), command: "ferro-video %f".into() },
        AssociationEntry { ext: "3gp".into(), desc: "Phone Video".into(), command: "ferro-video %f".into() },
        AssociationEntry { ext: "mp3".into(), desc: "MP3 Audio".into(), command: "ferro-video %f".into() },
        AssociationEntry { ext: "wav".into(), desc: "Waveform Audio".into(), command: "ferro-video %f".into() },
        AssociationEntry { ext: "flac".into(), desc: "FLAC Audio".into(), command: "ferro-video %f".into() },
        AssociationEntry { ext: "ogg".into(), desc: "Ogg Audio".into(), command: "ferro-video %f".into() },
        AssociationEntry { ext: "m4a".into(), desc: "MPEG-4 Audio".into(), command: "ferro-video %f".into() },
        AssociationEntry { ext: "opus".into(), desc: "Opus Audio".into(), command: "ferro-video %f".into() },
        AssociationEntry { ext: "html".into(), desc: "HTML Document".into(), command: "librewolf %f".into() },
        AssociationEntry { ext: "pdf".into(), desc: "PDF Document".into(), command: "xdg-open %f".into() },
        AssociationEntry { ext: "appimage".into(), desc: "AppImage Application".into(), command: "sh -c 'chmod +x %f 2>/dev/null; exec %f'".into() },
    ]
}

pub fn config_path() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".config").join("ferro").join("associations.conf")
    } else {
        PathBuf::from("/etc/ferro/associations.conf")
    }
}

pub fn load_associations() -> Vec<AssociationEntry> {
    let mut map: BTreeMap<String, AssociationEntry> = default_associations().into_iter().map(|a| (a.ext.clone(), a)).collect();

    let path = config_path();
    if let Ok(content) = fs::read_to_string(&path) {
        for line in content.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((ext, cmd)) = line.split_once('=') {
                let ext = ext.trim().trim_start_matches('.').to_lowercase();
                let mut cmd = cmd.trim().to_string();
                // Map old/missing image viewers (viewnior, qview) to the built-in ferro-viewer
                if let Some(rest) = cmd.strip_prefix("viewnior").or_else(|| cmd.strip_prefix("qview")) {
                    if rest.is_empty() || rest.starts_with(' ') {
                        cmd = format!("ferro-viewer{rest}");
                    }
                }
                // And plain mpv to the Video Player: mpv as FerroOS sets it up,
                // inside ferro-jail.
                if let Some(rest) = cmd.strip_prefix("mpv") {
                    if rest.is_empty() || rest.starts_with(' ') {
                        cmd = format!("ferro-video{rest}");
                    }
                }
                if let Some(entry) = map.get_mut(&ext) {
                    entry.command = cmd;
                } else {
                    map.insert(ext.clone(), AssociationEntry { ext: ext.clone(), desc: format!("{ext} File"), command: cmd });
                }
            }
        }
    }

    map.into_values().collect()
}

pub fn save_associations(entries: &[AssociationEntry]) {
    let path = config_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut lines = vec!["# FerroOS File Associations".to_string(), "# Format: extension = command %f".to_string()];
    for e in entries {
        lines.push(format!("{} = {}", e.ext.to_lowercase(), e.command));
    }
    let _ = fs::write(&path, lines.join("\n") + "\n");
}

/// Programs chosen for one file only (Properties, Change..., "Only this
/// file"): `file-associations.conf` beside `associations.conf`, a line each:
/// the file's full path, a tab, the command.
pub fn file_config_path() -> PathBuf {
    config_path().with_file_name("file-associations.conf")
}

/// The file's path as it's noted: links followed, so any way to it counts.
fn file_key(path: &Path) -> String {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()).to_string_lossy().into_owned()
}

fn read_file_overrides(conf: &Path) -> Vec<(String, String)> {
    let text = fs::read_to_string(conf).unwrap_or_default();
    text.lines().filter_map(|l| l.split_once('\t')).map(|(p, c)| (p.to_owned(), c.trim().to_owned())).collect()
}

/// The program chosen for this one file, if one was.
pub fn file_override(path: &Path) -> Option<String> {
    file_override_in(&file_config_path(), path)
}

fn file_override_in(conf: &Path, path: &Path) -> Option<String> {
    let key = file_key(path);
    read_file_overrides(conf).into_iter().find(|(p, _)| *p == key).map(|(_, c)| c).filter(|c| !c.is_empty())
}

/// Chooses the program for this one file (`None`: back to its type's).
pub fn set_file_override(path: &Path, command: Option<&str>) {
    set_file_override_in(&file_config_path(), path, command);
}

fn set_file_override_in(conf: &Path, path: &Path, command: Option<&str>) {
    let key = file_key(path);
    let mut all: Vec<(String, String)> = read_file_overrides(conf).into_iter().filter(|(p, _)| *p != key).collect();
    if let Some(c) = command {
        all.push((key, c.replace('\n', " ")));
    }
    if let Some(parent) = conf.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut text = String::from("# FerroOS: programs chosen for single files (path, a tab, the command)\n");
    for (p, c) in all {
        text.push_str(&format!("{p}\t{c}\n"));
    }
    let _ = fs::write(conf, text);
}

/// Safely quotes an argument for POSIX shell execution, preventing command injection.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Strips FreeDesktop field codes (%f, %F, %u, %U, %i, %c, %k, %v, %m, etc.)
pub fn strip_field_codes(exec: &str) -> String {
    expand_field_codes(exec, None).split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `exec` with its field codes filled in: the file codes (`%f`, `%F`, `%u`,
/// `%U`) with `file`, if there's one, `%%` with `%`, and the rest (`%i`,
/// `%c`, `%k`...) with nothing.
fn expand_field_codes(exec: &str, file: Option<&str>) -> String {
    let mut out = String::new();
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('%') => out.push('%'),
            Some('f' | 'F' | 'u' | 'U') => out.push_str(file.unwrap_or("")),
            _ => {}
        }
    }
    out
}

pub fn parse_desktop_entry(path: &Path) -> Option<String> {
    let resolved = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let content = fs::read_to_string(&resolved).or_else(|_| fs::read_to_string(path)).ok()?;
    let parent = resolved.parent().or_else(|| path.parent());
    parse_desktop_entry_content(&content, parent)
}

pub fn parse_desktop_entry_content(content: &str, working_dir: Option<&Path>) -> Option<String> {
    let mut in_entry = false;
    let mut exec: Option<String> = None;
    let mut terminal = false;
    let mut url: Option<String> = None;
    let mut entry_type = String::new();
    let mut path_dir: Option<String> = None;

    for line in content.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let (key, value) = (key.trim(), value.trim());
        match key {
            "Type" => entry_type = value.to_string(),
            "Exec" => exec = Some(strip_field_codes(value)),
            "Terminal" => terminal = value.eq_ignore_ascii_case("true"),
            "URL" => url = Some(value.to_string()),
            "Path" if !value.is_empty() => {
                path_dir = Some(value.to_string());
            }
            _ => {}
        }
    }

    if entry_type.eq_ignore_ascii_case("Link") {
        if let Some(u) = url {
            return Some(format!("xdg-open {}", shell_quote(&u)));
        }
    }

    let raw_exec = exec?;
    let trimmed = raw_exec.trim();
    if trimmed.is_empty() {
        return None;
    }

    let lower = trimmed.to_lowercase();
    let cmd = if lower == "ferro-cmd"
        || lower == "cmd"
        || lower == "terminal"
        || lower == "x-terminal-emulator"
        || (lower.contains("ferro-cmd") && !lower.contains("xterm") && !lower.contains("lxterminal"))
    {
        "ferro-terminal || lxterminal".to_string()
    } else if lower.contains("librewolf") || lower.contains("firefox") || lower == "browser" {
        "librewolf || firefox-esr || firefox || x-www-browser".to_string()
    } else if terminal {
        let q = shell_quote(trimmed);
        format!("lxterminal -e {q} || x-terminal-emulator -e sh -c {q}")
    } else {
        trimmed.to_string()
    };

    let full_cmd = if let Some(dir) = path_dir.or_else(|| working_dir.map(|p| p.to_string_lossy().into_owned())) {
        format!("cd {} && ({})", shell_quote(&dir), cmd)
    } else {
        cmd
    };

    Some(full_cmd)
}

/// Whether `path` may be started as a program (or, for a `.desktop` file,
/// have its `Exec=` line run) when it's opened. Only what the user (or the
/// system) put there on purpose: a file of theirs or root's that nobody
/// else can change, marked executable, and not on a USB stick or other
/// removable or foreign file system, where every file can look executable.
/// The system's own application folders count as marked.
pub fn trusted_launcher(path: &Path) -> bool {
    let Ok(resolved) = fs::canonicalize(path) else { return false };
    let Ok(md) = fs::metadata(&resolved) else { return false };
    if !md.is_file() {
        return false;
    }
    let system_entry = ["/usr/share/applications/", "/usr/local/share/applications/"].iter().any(|d| resolved.starts_with(d))
        && resolved.extension().is_some_and(|e| e == "desktop");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: getuid has no preconditions.
        let me = unsafe { libc::getuid() };
        let owned = md.uid() == me || md.uid() == 0;
        let locked = md.mode() & 0o022 == 0;
        let marked = md.mode() & 0o111 != 0 || system_entry;
        owned && locked && marked && !on_removable_media(&resolved)
    }
    #[cfg(not(unix))]
    {
        system_entry
    }
}

/// USB sticks, SD cards, discs and other systems' file systems: what's on
/// them came from elsewhere.
#[cfg(unix)]
fn on_removable_media(path: &Path) -> bool {
    if ["/media/", "/run/media/", "/mnt/"].iter().any(|d| path.starts_with(d)) {
        return true;
    }
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) else { return true };
    // SAFETY: statfs fills the struct for a valid C path.
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return true;
    }
    const FOREIGN: [i64; 9] = [
        0x4d44,      // FAT
        0x2011_bab0, // exFAT
        0x5346_544e, // NTFS
        0x7366_746e, // NTFS3
        0x6573_5546, // FUSE (ntfs-3g and friends)
        0x9660,      // ISO 9660
        0x1501_3346, // UDF
        0x6969,      // NFS
        0xff53_4d42, // SMB/CIFS
    ];
    #[allow(clippy::unnecessary_cast)]
    FOREIGN.contains(&(st.f_type as i64))
}

/// Windows programs, started with Wine (see `windows_program_command`).
const WINDOWS_PROGRAMS: [&str; 5] = ["exe", "msi", "bat", "com", "pif"];

/// How a Windows program opens: with Wine, when it's installed. Never
/// straight from a CD or USB stick (as with Linux programs, see
/// `trusted_launcher`): installing Wine makes it the handler for every
/// .exe, and this keeps a double-click on a stick from running one.
/// Where programs installed into C:\\Programs are started from.
const PROGRAM_LAUNCHERS: &str = "/usr/local/lib/ferro-programs";

/// The program a chosen command starts, made runnable: `C:\\Programs\\x\\x`
/// as the path it is, and a program installed into C:\\Programs by its name
/// (`ruffle`) as its launcher, since it isn't on the PATH. `resolve` says
/// where a program is when that isn't what it says (`runnable_program`).
fn runnable_with(command: &str, resolve: &dyn Fn(&str) -> Option<String>) -> String {
    let (program, rest) = command.split_once(' ').unwrap_or((command, ""));
    let sep = if rest.is_empty() { "" } else { " " };
    match resolve(program) {
        Some(p) => format!("{}{sep}{rest}", shell_quote(&p)),
        None => command.to_owned(),
    }
}

/// Where `program`, a chosen command's first word, really is when that
/// isn't what it says: a C:\\ path, or a program installed into C:\\Programs.
fn runnable_program(program: &str) -> Option<String> {
    runnable_program_with(program, Path::new(PROGRAM_LAUNCHERS), &on_path)
}

fn runnable_program_with(program: &str, launchers: &Path, on_path: &dyn Fn(&str) -> bool) -> Option<String> {
    if ferro_path::is_windows_path(program) {
        if let Ok(posix) = ferro_path::DriveTable::heavy().to_posix(program) {
            return Some(posix);
        }
    }
    if !program.is_empty() && !program.contains(['/', '\'', '"', '$', '=']) && !on_path(program) {
        let launcher = launchers.join(program.to_lowercase());
        if launcher.is_file() {
            return Some(launcher.to_string_lossy().into_owned());
        }
    }
    None
}

/// Whether `name` is a program on the PATH.
fn on_path(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(name).is_file()))
}

fn windows_program_command(path: &Path, resolved: &Path) -> String {
    #[cfg(unix)]
    let removable = on_removable_media(resolved);
    #[cfg(not(unix))]
    let removable = {
        let _ = resolved;
        false
    };
    if removable {
        return "zenity --info --title 'Windows program' --text 'Programs are not started straight from a CD or USB stick. Copy the folder to your Documents first, or run it from a terminal: wine /path/to/SETUP.EXE'".into();
    }
    let quoted = shell_quote(&path.to_string_lossy());
    format!(
        "if command -v wine >/dev/null 2>&1; then wine start /unix {quoted}; else zenity --info --title 'Windows program' --text 'Install Wine from Add Programs to run Windows programs.'; fi"
    )
}

/// Whether a chosen command says where the file goes (`unrar x %f`). If it
/// doesn't, the file goes last (`geany` opens it as `geany FILE`).
pub fn has_file_code(command: &str) -> bool {
    ["%f", "%F", "%u", "%U"].iter().any(|c| command.contains(c))
}

/// What makes a chosen command a line for the shell (`sh -c '...'`,
/// `a || b`) rather than a program and its arguments.
const SHELL_CHARS: &[char] = &['|', '&', ';', '<', '>', '(', ')', '$', '`', '\\', '"', '\'', '*', '?', '[', ']', '{', '}', '#', '~'];

/// The command line (for `sh -c`) that opens `path` with `command`, the
/// program chosen for it in Open With or `associations.conf`. The command
/// may say where the file goes: `%f` (or freedesktop.org's `%F`, `%u`, `%U`)
/// is the file, so `unrar x %f` extracts it; without one, the file is added
/// last. A program and its arguments are taken word by word and the file
/// put in as one word of its own, quoted, whatever its name; a command-line
/// program (see `needs_terminal`) then runs in a Command Prompt window in
/// the file's folder, kept open when it ends so what it printed can be
/// read. A command with shell in it (`sh -c '...'`, `a || b`) is used as
/// it's written, the file quoted in where it says.
pub fn open_with_command(command: &str, path: &Path) -> String {
    open_with_command_using(command, path, &runnable_program, &needs_terminal)
}

fn open_with_command_using(command: &str, path: &Path, resolve: &dyn Fn(&str) -> Option<String>, terminal: &dyn Fn(&str) -> bool) -> String {
    let file = path.to_string_lossy();
    let mut words: Vec<String> = command.split_whitespace().map(str::to_owned).collect();
    let resolved = words.first().and_then(|p| resolve(p));
    let plain = words.iter().enumerate().all(|(i, w)| (i == 0 && resolved.is_some()) || !w.contains(SHELL_CHARS));
    if words.is_empty() || !plain {
        let cmd = runnable_with(command, resolve);
        let quoted = shell_quote(&file);
        return if has_file_code(&cmd) {
            ["%f", "%F", "%u", "%U"].iter().fold(cmd, |c, code| c.replace(code, &quoted))
        } else {
            format!("{cmd} {quoted}")
        };
    }
    if let Some(p) = resolved {
        words[0] = p;
    }
    let mut argv: Vec<String> = Vec::new();
    let mut placed = false;
    for w in &words {
        let filled = expand_field_codes(w, Some(&file));
        placed |= filled != expand_field_codes(w, None);
        if !filled.is_empty() {
            argv.push(filled);
        }
    }
    if !placed {
        argv.push(file.into_owned());
    }
    let line = |argv: &[String]| argv.iter().map(|a| shell_word(a)).collect::<Vec<_>>().join(" ");
    let program = argv.iter().find(|a| *a != "env" && !(a.contains('=') && !a.starts_with('-'))).cloned().unwrap_or_default();
    if !terminal(&program) {
        return line(&argv);
    }
    let held = [vec!["ferro-terminal".to_owned(), "--hold".to_owned(), "-e".to_owned()], argv].concat();
    match path.parent().filter(|d| !d.as_os_str().is_empty()) {
        Some(dir) => format!("cd {} && {}", shell_word(&dir.to_string_lossy()), line(&held)),
        None => line(&held),
    }
}

/// `s` as one word for the shell: as it is when that's safe, else quoted.
fn shell_word(s: &str) -> String {
    let safe = !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:=+,@%".contains(c));
    if safe {
        s.to_owned()
    } else {
        shell_quote(s)
    }
}

/// Whether `program` is a command-line one: run on its own it has nowhere
/// to show what it prints or to ask anything, so it's run in a Command
/// Prompt window instead. Decided cheaply, without starting it:
/// - FerroOS's own programs (`ferro-*`) open their own windows;
/// - a program with a `.desktop` entry says itself, by `Terminal=`;
/// - otherwise a compiled program is a windowed one only if it's linked to
///   a display or toolkit library (X11, xcb, Wayland, GTK, Qt, SDL, GL...);
/// - a small script that only starts another program (`exec /usr/lib/7zip/7z
///   "$@"`) is that program; any other script (`xdg-open` and the like), or
///   a program that isn't found, is started on its own as before.
fn needs_terminal(program: &str) -> bool {
    let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if name(Path::new(program)).starts_with("ferro-") {
        return false;
    }
    let found = if program.contains('/') {
        Some(PathBuf::from(program)).filter(|p| p.is_file())
    } else {
        std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).map(|d| d.join(program)).find(|p| p.is_file()))
    };
    let Some(mut exe) = found else { return false };
    let dirs = application_dirs();
    for _ in 0..4 {
        let real = fs::canonicalize(&exe).unwrap_or_else(|_| exe.clone());
        let names = [name(&exe), name(&real)];
        if let Some(terminal) = dirs.iter().find_map(|d| desktop_terminal_in(d, &names)) {
            return terminal;
        }
        if let Some(libs) = elf_needed(&real) {
            return !libs.iter().any(|l| is_display_library(l));
        }
        match wrapped_program(&real) {
            Some(next) => exe = next,
            None => return false,
        }
    }
    false
}

/// The program a wrapper script starts: a short script with a line
/// `exec /path/to/program ...`.
fn wrapped_program(path: &Path) -> Option<PathBuf> {
    if fs::metadata(path).ok()?.len() > 2048 {
        return None;
    }
    let text = fs::read_to_string(path).ok()?;
    if !text.starts_with("#!") {
        return None;
    }
    text.lines().find_map(|l| {
        let program = l.trim().strip_prefix("exec ")?.split_whitespace().next()?.trim_matches(['"', '\'']);
        Some(PathBuf::from(program)).filter(|p| p.is_absolute() && p.is_file())
    })
}

/// Where `.desktop` entries are: the user's own, then the system's.
fn application_dirs() -> Vec<PathBuf> {
    let home =
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")));
    let system = std::env::var("XDG_DATA_DIRS").ok().filter(|d| !d.is_empty()).unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    home.into_iter().chain(system.split(':').map(PathBuf::from)).map(|d| d.join("applications")).collect()
}

/// `Terminal=` of the first entry in `dir` that starts a program named one
/// of `names`.
fn desktop_terminal_in(dir: &Path, names: &[String]) -> Option<bool> {
    let mut files: Vec<PathBuf> =
        fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "desktop")).collect();
    files.sort();
    files.iter().find_map(|f| desktop_entry_terminal(&fs::read_to_string(f).ok()?, names))
}

/// For an application entry that starts a program named one of `names`
/// (by its `Exec=` or `TryExec=`), whether it runs in a terminal.
fn desktop_entry_terminal(content: &str, names: &[String]) -> Option<bool> {
    let (mut in_entry, mut application, mut matches, mut terminal) = (false, true, false, false);
    let base = |w: &str| w.trim_matches('"').rsplit('/').next().unwrap_or("").to_owned();
    for line in content.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        let Some((key, value)) = line.split_once('=').filter(|_| in_entry) else { continue };
        match key.trim() {
            "Type" => application = value.trim() == "Application",
            "Terminal" => terminal = value.trim().eq_ignore_ascii_case("true"),
            "TryExec" => matches |= names.contains(&base(value.trim())),
            "Exec" => {
                let program = value.split_whitespace().find(|w| *w != "env" && !w.contains('='));
                matches |= program.is_some_and(|p| names.contains(&base(p)));
            }
            _ => {}
        }
    }
    (application && matches).then_some(terminal)
}

/// Libraries only a program with windows links to.
fn is_display_library(lib: &str) -> bool {
    [
        "libX11",
        "libxcb",
        "libXt",
        "libwayland-client",
        "libgtk",
        "libgdk",
        "libQt",
        "libSDL",
        "libglfw",
        "libGL",
        "libEGL",
        "libOpenGL",
        "libvulkan",
        "libwx_",
        "libfltk",
        "libtk",
        "libelementary",
    ]
    .iter()
    .any(|p| lib.starts_with(p))
}

/// The libraries an ELF program is linked to (its `DT_NEEDED` entries;
/// none for one linked statically). `None`: it isn't an ELF program.
fn elf_needed(path: &Path) -> Option<Vec<String>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = fs::File::open(path).ok()?;
    let mut head = [0u8; 64];
    f.read_exact(&mut head).ok()?;
    if &head[..4] != b"\x7fELF" {
        return None;
    }
    let (wide, le) = (head[4] == 2, head[5] == 1);
    let num = |b: &[u8]| b.iter().enumerate().fold(0u64, |n, (i, &x)| n | (x as u64) << (8 * if le { i } else { b.len() - 1 - i }));
    let mut read_at = |off: u64, len: u64| -> Option<Vec<u8>> {
        if len > 16 << 20 {
            return None;
        }
        let mut buf = vec![0u8; len as usize];
        f.seek(SeekFrom::Start(off)).ok()?;
        f.read_exact(&mut buf).ok()?;
        Some(buf)
    };
    let (shoff, shentsize, shnum) = if wide {
        (num(&head[0x28..0x30]), num(&head[0x3a..0x3c]), num(&head[0x3c..0x3e]))
    } else {
        (num(&head[0x20..0x24]), num(&head[0x2e..0x30]), num(&head[0x30..0x32]))
    };
    if shentsize < if wide { 0x40 } else { 0x28 } {
        return Some(Vec::new());
    }
    let table = read_at(shoff, shentsize * shnum)?;
    // A section's type, offset, size and link.
    let section = |i: u64| -> Option<(u64, u64, u64, u64)> {
        let s = table.get((i * shentsize) as usize..((i + 1) * shentsize) as usize)?;
        Some(if wide {
            (num(&s[4..8]), num(&s[0x18..0x20]), num(&s[0x20..0x28]), num(&s[0x28..0x2c]))
        } else {
            (num(&s[4..8]), num(&s[0x10..0x14]), num(&s[0x14..0x18]), num(&s[0x18..0x1c]))
        })
    };
    const SHT_DYNAMIC: u64 = 6;
    let Some((_, off, size, link)) = (0..shnum).filter_map(section).find(|s| s.0 == SHT_DYNAMIC) else { return Some(Vec::new()) };
    let (_, str_off, str_size, _) = section(link)?;
    let (dynamic, strings) = (read_at(off, size)?, read_at(str_off, str_size)?);
    let entry = if wide { 16 } else { 8 };
    let mut libs = Vec::new();
    for e in dynamic.chunks_exact(entry) {
        let (tag, val) = (num(&e[..entry / 2]), num(&e[entry / 2..]));
        match tag {
            0 => break,
            1 => {
                let name = strings.get(val as usize..)?;
                let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
                libs.push(String::from_utf8_lossy(&name[..end]).into_owned());
            }
            _ => {}
        }
    }
    Some(libs)
}

pub fn resolve_command_for_path(path: &Path) -> String {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    let resolved = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let resolved_ext = resolved.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();

    let mut lookup = ext.clone();
    if ext == "desktop" || resolved_ext == "desktop" {
        if trusted_launcher(path) {
            if let Some(cmd) = parse_desktop_entry(path) {
                return cmd;
            }
        }
        lookup = "txt".into();
    }

    let entries = load_associations();
    let p_str = path.to_string_lossy();
    let quoted = shell_quote(&p_str);

    // A program chosen for this file alone comes before everything else.
    if let Some(cmd) = file_override(path) {
        return open_with_command(&cmd, path);
    }

    // Programs to install: the installer (it asks, for a tarball, whether
    // it's a program or just files).
    let lower = p_str.to_lowercase();
    if [".deb", ".tar.gz", ".tgz", ".tar.xz", ".txz", ".tar.bz2", ".tbz2", ".tar.zst"].iter().any(|e| lower.ends_with(e)) {
        return format!("ferro-terminal -e /usr/local/bin/ferro-installer {quoted}");
    }

    // A zip is unpacked beside itself (as the file manager does), unless it's
    // set to open with something other than unzip.
    if lookup == "zip" {
        let chosen = entries.iter().find(|e| e.ext.eq_ignore_ascii_case("zip"));
        if chosen.is_none_or(|e| e.command.split_whitespace().next() == Some("unzip")) {
            return format!("ferro-files --extract {quoted}");
        }
    }

    for e in &entries {
        if e.ext.eq_ignore_ascii_case(&lookup) {
            return open_with_command(&e.command, path);
        }
    }

    if WINDOWS_PROGRAMS.contains(&ext.as_str()) || WINDOWS_PROGRAMS.contains(&resolved_ext.as_str()) {
        return windows_program_command(path, &resolved);
    }

    if ext == "appimage" || resolved_ext == "appimage" {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(metadata) = fs::metadata(path) {
                let mut perms = metadata.permissions();
                let mode = perms.mode();
                if mode & 0o111 == 0 {
                    perms.set_mode(mode | 0o755);
                    let _ = fs::set_permissions(path, perms);
                }
            }
        }
        return format!("sh -c 'chmod +x {quoted} 2>/dev/null; exec {quoted}'");
    }

    format!("xdg-open {quoted}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_programs_by_name_or_c_path() {
        let d = std::env::temp_dir().join(format!("ferro-assoc-launch-{}", std::process::id()));
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("ruffle"), "").unwrap();
        let nowhere = |p: &str| runnable_program_with(p, &d, &|_| false);
        let launcher = shell_quote(&d.join("ruffle").to_string_lossy());
        assert_eq!(runnable_with("ruffle %f", &nowhere), format!("{launcher} %f"));
        assert_eq!(runnable_with("Ruffle", &nowhere), launcher);
        assert_eq!(runnable_with(r"C:\Programs\ruffle\ruffle %f", &nowhere), "'/Programs/ruffle/ruffle' %f");
        // On the PATH, or not installed: left as it is.
        assert_eq!(runnable_with("ruffle %f", &|p| runnable_program_with(p, &d, &|_| true)), "ruffle %f");
        assert_eq!(runnable_with("geany %f", &nowhere), "geany %f");
        assert_eq!(runnable_with("sh -c 'x'", &nowhere), "sh -c 'x'");
        // Opened with one installed there: its launcher, the file after it.
        let none = |_: &str| false;
        let bare = shell_word(&d.join("ruffle").to_string_lossy());
        assert_eq!(open_with_command_using("ruffle", Path::new("/a/b c.swf"), &nowhere, &none), format!("{bare} '/a/b c.swf'"));
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn chosen_commands_put_the_file_where_they_say() {
        let (as_is, gui, cli) = (|_: &str| None, |_: &str| false, |p: &str| p == "unrar" || p == "7z");
        let rar = Path::new("/home/atom/Downloads/My Stuff.rar");
        // No file code: the file goes last, as before.
        assert_eq!(open_with_command_using("geany", Path::new("/home/atom/notes.txt"), &as_is, &gui), "geany /home/atom/notes.txt");
        assert_eq!(open_with_command_using("geany %f", rar, &as_is, &gui), "geany '/home/atom/Downloads/My Stuff.rar'");
        // freedesktop.org's codes too, and where they are.
        for code in ["%f", "%F", "%u", "%U"] {
            assert_eq!(open_with_command_using(&format!("tool x {code} -y"), rar, &as_is, &gui), "tool x '/home/atom/Downloads/My Stuff.rar' -y");
        }
        assert_eq!(open_with_command_using("tool --in=%f %i", rar, &as_is, &gui), "tool '--in=/home/atom/Downloads/My Stuff.rar'");
        // A command-line program: in a Command Prompt window, in the file's folder.
        assert_eq!(
            open_with_command_using("unrar x %f", rar, &as_is, &cli),
            "cd /home/atom/Downloads && ferro-terminal --hold -e unrar x '/home/atom/Downloads/My Stuff.rar'"
        );
        assert_eq!(open_with_command_using("7z", Path::new("/d/a.7z"), &as_is, &cli), "cd /d && ferro-terminal --hold -e 7z /d/a.7z");
        // What's decided on is the program, after env and its settings.
        assert!(open_with_command_using("env LANG=C unrar x", rar, &as_is, &cli).contains("ferro-terminal --hold -e env LANG=C unrar x"));
        // A file's name is one word, whatever's in it.
        let evil = Path::new("/tmp/a'; rm -rf ~; echo '$(id)`x`.rar");
        let cmd = open_with_command_using("unrar x %f", evil, &as_is, &cli);
        assert!(cmd.ends_with(&shell_quote(&evil.to_string_lossy())), "{cmd}");
        assert!(std::process::Command::new("sh").args(["-n", "-c", &cmd]).status().unwrap().success(), "{cmd}");
        let out = std::process::Command::new("sh").args(["-c", &cmd.replace("ferro-terminal --hold -e unrar x", "printf %s")]).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), evil.to_string_lossy());
        // Shell in the command: used as written, never in a terminal.
        assert_eq!(open_with_command_using("unrar x %f || true", rar, &as_is, &cli), "unrar x '/home/atom/Downloads/My Stuff.rar' || true");
        assert_eq!(open_with_command_using("sh -c 'x %f'", rar, &as_is, &cli), "sh -c 'x '/home/atom/Downloads/My Stuff.rar''");
        assert!(has_file_code("unrar x %F") && !has_file_code("unrar x"));
        assert_eq!(strip_field_codes("steam %U  --x %%"), "steam --x %");
    }

    #[test]
    fn command_line_programs_are_told_apart() {
        let names = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let htop = "[Desktop Entry]\nType=Application\nName=Htop\nTryExec=htop\nExec=htop\nTerminal=true\n";
        assert_eq!(desktop_entry_terminal(htop, &names(&["htop"])), Some(true));
        let geany = "[Desktop Entry]\nType=Application\nExec=/usr/bin/geany %F\n\n[Desktop Action New]\nExec=other\n";
        assert_eq!(desktop_entry_terminal(geany, &names(&["geany"])), Some(false));
        assert_eq!(desktop_entry_terminal(geany, &names(&["other"])), None, "only the entry itself");
        let wrapped = "[Desktop Entry]\nType=Application\nExec=env GDK_SCALE=2 \"/opt/gimp/gimp\" %U\n";
        assert_eq!(desktop_entry_terminal(wrapped, &names(&["gimp"])), Some(false));
        assert_eq!(desktop_entry_terminal("[Desktop Entry]\nType=Link\nExec=htop\nTerminal=true\n", &names(&["htop"])), None);

        assert!(is_display_library("libX11.so.6") && is_display_library("libgtk-3.so.0") && is_display_library("libQt5Widgets.so.5"));
        assert!(!is_display_library("libc.so.6") && !is_display_library("libglib-2.0.so.0") && !is_display_library("libstdc++.so.6"));

        // This test program is ELF and links to libc but nothing with windows.
        let me = std::env::current_exe().unwrap();
        let libs = elf_needed(&me).unwrap();
        assert!(libs.iter().any(|l| l.starts_with("libc.")), "{libs:?}");
        assert!(!libs.iter().any(|l| is_display_library(l)));
        // A script isn't.
        let d = std::env::temp_dir().join(format!("ferro-assoc-elf-{}", std::process::id()));
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("launcher"), "#!/bin/sh\nexec x \"$@\"\n").unwrap();
        assert_eq!(elf_needed(&d.join("launcher")), None);
        assert!(!needs_terminal(&d.join("launcher").to_string_lossy()));
        // But one that only starts a program is that program.
        assert!(needs_terminal(&me.to_string_lossy()));
        fs::write(d.join("wrapper"), format!("#!/bin/sh\nexec \"{}\" \"$@\"\n", me.display())).unwrap();
        assert_eq!(wrapped_program(&d.join("wrapper")), Some(me.clone()));
        assert!(needs_terminal(&d.join("wrapper").to_string_lossy()));
        assert!(!needs_terminal("/nowhere/unrar"), "not found: started as before");
        assert!(!needs_terminal("ferro-viewer"), "FerroOS's own open windows");
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn one_file_keeps_its_own_program() {
        let d = std::env::temp_dir().join(format!("ferro-assoc-file-{}", std::process::id()));
        fs::create_dir_all(&d).unwrap();
        let (conf, a, b) = (d.join("file-associations.conf"), d.join("a.png"), d.join("b.png"));
        fs::write(&a, "").unwrap();
        fs::write(&b, "").unwrap();
        set_file_override_in(&conf, &a, Some("gimp %f"));
        assert_eq!(file_override_in(&conf, &a).as_deref(), Some("gimp %f"));
        assert_eq!(file_override_in(&conf, &b), None, "only that file");
        set_file_override_in(&conf, &a, Some("krita %f"));
        assert_eq!(file_override_in(&conf, &a).as_deref(), Some("krita %f"), "replaced, not added");
        set_file_override_in(&conf, &a, None);
        assert_eq!(file_override_in(&conf, &a), None);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn default_associations_contain_required_apps() {
        let defaults = default_associations();
        let txt = defaults.iter().find(|a| a.ext == "txt").unwrap();
        assert!(txt.command.contains("geany"));

        let png = defaults.iter().find(|a| a.ext == "png").unwrap();
        assert!(png.command.contains("ferro-viewer"));

        let mp4 = defaults.iter().find(|a| a.ext == "mp4").unwrap();
        assert!(mp4.command.contains("ferro-video"));

        let html = defaults.iter().find(|a| a.ext == "html").unwrap();
        assert!(html.command.contains("librewolf"));
    }

    #[test]
    fn resolve_command_replaces_placeholder() {
        let p = Path::new("/home/atom/notes.txt");
        let cmd = resolve_command_for_path(p);
        assert!(cmd.contains("/home/atom/notes.txt"));
        assert!(cmd.contains("geany"));
    }

    #[test]
    fn parse_desktop_entry_application_and_links() {
        let text_app = "[Desktop Entry]\nType=Application\nName=Steam\nExec=/usr/games/steam %U\nTerminal=false\n";
        let cmd = parse_desktop_entry_content(text_app, None).unwrap();
        assert_eq!(cmd, "/usr/games/steam");

        let text_term = "[Desktop Entry]\nType=Application\nName=Top\nExec=htop\nTerminal=true\n";
        let cmd_term = parse_desktop_entry_content(text_term, None).unwrap();
        assert!(cmd_term.contains("lxterminal -e"));

        let text_link = "[Desktop Entry]\nType=Link\nURL=https://debian.org\n";
        let cmd_link = parse_desktop_entry_content(text_link, None).unwrap();
        assert_eq!(cmd_link, "xdg-open 'https://debian.org'");
        let sneaky = "[Desktop Entry]\nType=Link\nURL=x\"$(id)\"\n";
        assert_eq!(parse_desktop_entry_content(sneaky, None).unwrap(), "xdg-open 'x\"$(id)\"'");
    }

    #[cfg(unix)]
    #[test]
    fn untrusted_shortcuts_and_lookalikes_open_as_text() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("ferro-assoc-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let entry = "[Desktop Entry]\nType=Application\nExec=touch /tmp/pwned\n";
        // Looks like a shortcut, named like a document: opened as one.
        let doc = dir.join("invoice.txt");
        fs::write(&doc, entry).unwrap();
        assert!(!resolve_command_for_path(&doc).contains("pwned"));
        // A downloaded shortcut, not marked executable: opened as text.
        let shortcut = dir.join("game.desktop");
        fs::write(&shortcut, entry).unwrap();
        fs::set_permissions(&shortcut, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!resolve_command_for_path(&shortcut).contains("pwned"));
        // Marked executable by its owner: it's a launcher.
        fs::set_permissions(&shortcut, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(resolve_command_for_path(&shortcut).contains("touch /tmp/pwned"));
        // But not if anyone else could have changed it.
        fs::set_permissions(&shortcut, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(!resolve_command_for_path(&shortcut).contains("pwned"));
        // Windows programs go to Wine, by a quoted path.
        let game = dir.join("Hokus Pokus $(id).exe");
        fs::write(&game, "MZ").unwrap();
        let cmd = resolve_command_for_path(&game);
        assert!(cmd.contains("wine start /unix '") && cmd.contains("$(id).exe'"), "{cmd}");
        // But never from a USB stick or CD.
        assert!(!resolve_command_for_path(Path::new("/media/user/CDROM/SETUP.EXE")).contains("wine start"));
        // Both commands are well-formed shell.
        for cmd in [resolve_command_for_path(&game), resolve_command_for_path(Path::new("/media/user/CDROM/SETUP.EXE"))] {
            assert!(std::process::Command::new("sh").args(["-n", "-c", &cmd]).status().unwrap().success(), "{cmd}");
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn shell_quote_neutralizes_injection() {
        assert_eq!(shell_quote("normal_file.txt"), "'normal_file.txt'");
        assert_eq!(shell_quote("file with spaces.txt"), "'file with spaces.txt'");
        assert_eq!(shell_quote("injected\"; rm -rf /; echo \".txt"), "'injected\"; rm -rf /; echo \".txt'");
        assert_eq!(shell_quote("it's cool.txt"), "'it'\\''s cool.txt'");

        let p = Path::new("/tmp/test\"; id; echo \".txt");
        let cmd = resolve_command_for_path(p);
        assert!(cmd.contains("'/tmp/test\"; id; echo \".txt'"));
    }
}
