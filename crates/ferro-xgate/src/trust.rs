//! Which programs reach the display as before, without the gate looking:
//! the ones whose job is the whole screen. The window manager draws every
//! window's frame and the taskbar; the Snipping Tool (Print Screen) is the
//! person taking a screenshot; picom, when Smooth drawing is on, paints
//! every window.
//!
//! What a program is, is the file it runs (`/proc/PID/exe`), which must be
//! one of those, root's, and not writable by anyone else. It mustn't be
//! being debugged (another program could make it do anything), nor started
//! with libraries of someone else's choosing (LD_PRELOAD). FerroOS's own
//! (`ferro-wm`, `ferro-snip`) say at start that they can't be debugged at
//! all (`PR_SET_DUMPABLE` 0), so nothing can take them over later either:
//! the files in its /proc then belong to root, which is checked.
//!
//! More (another compositor, a screen recorder the person uses) can be
//! listed in `/etc/ferro/xgate-trusted`, one path a line; root's only.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// (program, it says it can't be debugged).
const BUILT_IN: [(&str, bool); 4] = [
    ("/usr/local/bin/ferro-wm", true),
    ("/usr/local/bin/ferro-snip", true),
    // The clipboard history: it keeps what's copied, so it reads it as it's copied.
    ("/usr/local/bin/ferro-clipboard", true),
    ("/usr/bin/picom", false),
];
const FILE_MANAGER: &str = "/usr/local/bin/ferro-files";
const EXTRA: &str = "/etc/ferro/xgate-trusted";

/// Root's, and nobody else may change it.
fn roots(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.uid() == 0 && m.mode() & 0o022 == 0)
}

/// The paths listed in the extra file (comments and blank lines aside).
pub fn parse_extra(text: &str) -> Vec<String> {
    text.lines().map(str::trim).filter(|l| l.starts_with('/')).map(String::from).collect()
}

fn extra() -> Vec<String> {
    if !roots(Path::new(EXTRA)) {
        return Vec::new();
    }
    fs::read_to_string(EXTRA).map(|t| parse_extra(&t)).unwrap_or_default()
}

/// No library chosen by whoever started it.
pub fn clean_environment(environ: &[u8]) -> bool {
    !environ.split(|&b| b == 0).any(|v| v.starts_with(b"LD_PRELOAD=") || v.starts_with(b"LD_LIBRARY_PATH=") || v.starts_with(b"LD_AUDIT="))
}

fn traced(status: &str) -> bool {
    status.lines().find_map(|l| l.strip_prefix("TracerPid:")).is_some_and(|v| v.trim() != "0")
}

/// Whether `pid` goes through as it is; if not, why not (for the log).
pub fn trusted(pid: u32) -> Result<(), &'static str> {
    let proc = format!("/proc/{pid}");
    let exe = fs::read_link(format!("{proc}/exe")).map_err(|_| "its program can't be told")?;
    let exe_s = exe.to_string_lossy();
    let undebuggable = match BUILT_IN.iter().find(|(p, _)| *p == exe_s) {
        Some(&(_, u)) => u,
        None if extra().iter().any(|p| *p == exe_s) => false,
        None => return Err("not one of the trusted programs"),
    };
    if !roots(&exe) {
        return Err("its program file isn't root's alone");
    }
    // A program run by the person whose /proc files are root's can't be
    // debugged (the folder itself stays theirs).
    if undebuggable && fs::metadata(format!("{proc}/status")).map_or(true, |m| m.uid() != 0) {
        return Err("it can be debugged");
    }
    let status = fs::read_to_string(format!("{proc}/status")).unwrap_or_default();
    let environ = fs::read(format!("{proc}/environ")).unwrap_or_default();
    if status.is_empty() || traced(&status) {
        return Err("it's being debugged");
    }
    if !clean_environment(&environ) {
        return Err("started with chosen libraries (LD_PRELOAD)");
    }
    Ok(())
}

/// It's the File Manager (root's), which may read which folder its other windows show.
pub fn is_file_manager(pid: u32) -> bool {
    fs::read_link(format!("/proc/{pid}/exe")).is_ok_and(|e| e.as_os_str() == FILE_MANAGER && roots(&e))
}

/// What to call it in the question: its program's name.
pub fn name(pid: u32) -> String {
    fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().trim_end_matches(" (deleted)").to_owned()))
        .or_else(|| fs::read_to_string(format!("/proc/{pid}/comm")).ok().map(|c| c.trim().to_owned()))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "A program".into())
}

/// When a process started (clock ticks after boot): with its number, which
/// one it is, since numbers are used again.
pub fn started(pid: u32) -> u64 {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
    // After the name in parentheses (which may itself hold any of them), field 22.
    stat.rsplit_once(')').and_then(|(_, rest)| rest.split_whitespace().nth(19)).and_then(|v| v.parse().ok()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extra_list() {
        assert_eq!(parse_extra("# recorders\n/usr/bin/obs\n\n  /opt/x/y  \nnot-a-path\n"), ["/usr/bin/obs", "/opt/x/y"]);
    }

    #[test]
    fn chosen_libraries_and_debuggers() {
        assert!(clean_environment(b"HOME=/home/me\0PATH=/bin\0"));
        assert!(!clean_environment(b"HOME=/home/me\0LD_PRELOAD=/tmp/x.so\0"));
        assert!(!traced("Name:\tpicom\nTracerPid:\t0\n"));
        assert!(traced("Name:\tpicom\nTracerPid:\t812\n"));
    }

    #[test]
    fn this_process() {
        let me = std::process::id();
        assert!(started(me) > 0);
        assert!(!name(me).is_empty());
        assert_eq!(trusted(me), Err("not one of the trusted programs"));
    }
}
