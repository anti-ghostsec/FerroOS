//! Which programs reach the display as before, without the gate looking:
//! the ones whose job is the whole screen. The window manager draws every
//! window's frame and the taskbar; the Snipping Tool (Print Screen) is the
//! person taking a screenshot; picom, when Smooth drawing is on, paints
//! every window.
//!
//! The gate starts these itself (run.rs: `ferro-xgate run PROGRAM`), and
//! goes by that alone: a connection is one of theirs when it comes from a
//! process the gate started and hasn't seen end. What file a process runs
//! says nothing: a program can open a connection, hand it to another and
//! then become the window manager; or start the window manager with
//! libraries, a debugger or a sandbox of its own choosing around it. One
//! the gate started has none of that: the file is root's, the environment
//! is cleaned of everything that loads code, and no other program is its
//! parent, so (kernel.yama.ptrace_scope 1, as FerroOS sets it) none can
//! debug it. FerroOS's own (`ferro-wm`, `ferro-snip`) say at start that
//! they can't be debugged at all (`PR_SET_DUMPABLE` 0): the files in its
//! /proc then belong to root, which is still checked.
//!
//! More (another compositor, a screen recorder the person uses) can be
//! listed in `/etc/ferro/xgate-trusted`, one path a line; root's only.
//! They're started the same way: `ferro-xgate run /usr/bin/the-recorder`.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// What may follow a program's name when the gate starts it. Anyone may
/// ask for one to be started, so it's only ever started the way FerroOS
/// does: nothing that names a file of the asker's to load.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Args {
    /// One of these lists, exactly.
    Only(&'static [&'static [&'static str]]),
    /// Whatever the person gives it (the ones they listed themselves).
    Any,
    /// The taskbar's menus (ferro-flyout): which one, and where (a number).
    Flyout,
}

/// (program, it says it can't be debugged, its arguments).
const BUILT_IN: [(&str, bool, Args); 5] = [
    ("/usr/local/bin/ferro-wm", true, Args::Only(&[&[]])),
    ("/usr/local/bin/ferro-snip", true, Args::Only(&[&[]])),
    // The clipboard history: it keeps what's copied, so it reads it as it's copied.
    ("/usr/local/bin/ferro-clipboard", true, Args::Only(&[&["--daemon"], &["--pick"]])),
    ("/usr/bin/picom", false, Args::Only(&[&["--config", "/etc/xdg/picom.conf"]])),
    // The taskbar's network and volume menus: they take the keyboard and
    // the mouse as they open, beside the pointer, not under it (see the
    // policy's `may_take_input`), as the window manager's own menus do.
    ("/usr/local/bin/ferro-flyout", true, Args::Flyout),
];
const FILE_MANAGER: &str = "/usr/local/bin/ferro-files";
pub const EXTRA: &str = "/etc/ferro/xgate-trusted";

/// Root's (or, where the gate isn't run by root, whoever runs it), and
/// nobody else may change it.
fn roots(path: &Path) -> bool {
    // SAFETY: geteuid only reads this process's user.
    let me = unsafe { libc::geteuid() };
    fs::metadata(path).is_ok_and(|m| (m.uid() == 0 || m.uid() == me) && m.mode() & 0o022 == 0)
}

/// The paths listed in the extra file (comments and blank lines aside).
pub fn parse_extra(text: &str) -> Vec<String> {
    text.lines().map(str::trim).filter(|l| l.starts_with('/')).map(String::from).collect()
}

fn extra(list: &Path) -> Vec<String> {
    if !roots(list) {
        return Vec::new();
    }
    fs::read_to_string(list).map(|t| parse_extra(&t)).unwrap_or_default()
}

/// What `program` is to the gate, asked to start it with `args`: whether
/// it says it can't be debugged; or why it won't be started.
pub fn listed(program: &str, args: &[String], extra_list: &Path) -> Result<bool, &'static str> {
    let (undebuggable, takes) = match BUILT_IN.iter().find(|(p, ..)| *p == program) {
        Some(&(_, u, a)) => (u, a),
        None if extra(extra_list).iter().any(|p| p == program) => (false, Args::Any),
        None => return Err("not one of the trusted programs"),
    };
    let fits = match takes {
        Args::Only(ways) => ways.iter().any(|w| w.iter().eq(args.iter())),
        Args::Any => true,
        Args::Flyout => matches!(args, [kind, x] if (kind == "network" || kind == "volume") && x.parse::<u16>().is_ok()),
    };
    if !fits {
        return Err("not how that program is started");
    }
    if !roots(Path::new(program)) {
        return Err("its program file isn't root's alone");
    }
    Ok(undebuggable)
}

fn traced(status: &str) -> bool {
    status.lines().find_map(|l| l.strip_prefix("TracerPid:")).is_some_and(|v| v.trim() != "0")
}

/// A process the gate started is still as it was started; if not, why not
/// (for the log).
pub fn untouched(pid: u32, undebuggable: bool) -> Result<(), &'static str> {
    let proc = format!("/proc/{pid}");
    // A program run by the person whose /proc files are root's can't be
    // debugged (the folder itself stays theirs).
    if undebuggable && fs::metadata(format!("{proc}/status")).map_or(true, |m| m.uid() != 0) {
        return Err("it can be debugged");
    }
    let status = fs::read_to_string(format!("{proc}/status")).unwrap_or_default();
    if status.is_empty() || traced(&status) {
        return Err("it's being debugged");
    }
    Ok(())
}

/// Programs can't debug ones they didn't start (kernel.yama.ptrace_scope
/// 1 or more): what keeps the person's other programs out of the ones the
/// gate starts.
pub fn strangers_cant_debug() -> bool {
    fs::read_to_string("/proc/sys/kernel/yama/ptrace_scope").is_ok_and(|v| v.trim().parse::<u32>().is_ok_and(|n| n >= 1))
}

/// FerroOS's programs that take a password: while one of them has the
/// keyboard, no other program may take it away (see the policy).
const TAKE_PASSWORDS: [&str; 3] = ["/usr/local/bin/ferro-askpass", "/usr/local/bin/ferro-logon", "/usr/local/bin/ferro-screensaver"];

/// It's one of FerroOS's programs that take a password (root's files). Being
/// one only ever protects its own windows, so it needn't be started by the gate.
pub fn takes_passwords(pid: u32) -> bool {
    fs::read_link(format!("/proc/{pid}/exe")).is_ok_and(|e| TAKE_PASSWORDS.iter().any(|p| e.as_os_str() == *p) && roots(&e))
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
    fn debuggers() {
        assert!(!traced("Name:\tpicom\nTracerPid:\t0\n"));
        assert!(traced("Name:\tpicom\nTracerPid:\t812\n"));
    }

    #[test]
    fn this_process() {
        let me = std::process::id();
        assert!(started(me) > 0);
        assert!(!name(me).is_empty());
        // Running the window manager's file isn't being the window manager.
        assert_eq!(untouched(me, true), Err("it can be debugged"));
    }

    #[test]
    fn only_the_listed_started_the_listed_way() {
        let none = Path::new("/nonexistent/xgate-trusted");
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(listed("/usr/bin/xterm", &[], none), Err("not one of the trusted programs"));
        assert_eq!(listed("ferro-wm", &[], none), Err("not one of the trusted programs"), "by its full path only");
        // The compositor with a configuration of the asker's own.
        assert_eq!(listed("/usr/bin/picom", &args(&["--config", "/tmp/mine.conf"]), none), Err("not how that program is started"));
        assert_eq!(listed("/usr/bin/picom", &[], none), Err("not how that program is started"));
        assert_eq!(listed("/usr/local/bin/ferro-clipboard", &args(&["--daemon", "x"]), none), Err("not how that program is started"));
        assert_eq!(listed("/usr/local/bin/ferro-wm", &args(&["--anything"]), none), Err("not how that program is started"));
        // (Whether the file is there and root's comes after.)
        assert_ne!(listed("/usr/local/bin/ferro-wm", &[], none), Err("not how that program is started"));
        assert_ne!(listed("/usr/local/bin/ferro-clipboard", &args(&["--pick"]), none), Err("not how that program is started"));
        assert_ne!(listed("/usr/local/bin/ferro-flyout", &args(&["network", "512"]), none), Err("not how that program is started"));
        assert_eq!(listed("/usr/local/bin/ferro-flyout", &args(&["network", "--x"]), none), Err("not how that program is started"));
        assert_eq!(listed("/usr/local/bin/ferro-flyout", &args(&["other", "1"]), none), Err("not how that program is started"));
    }

    #[test]
    fn the_extra_list_must_be_roots() {
        // A list anyone could write names nothing.
        let dir = std::env::temp_dir().join(format!("xgate-trust-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let list = dir.join("trusted");
        fs::write(&list, "/bin/sh\n").unwrap();
        fs::set_permissions(&list, std::os::unix::fs::PermissionsExt::from_mode(0o666)).unwrap();
        assert_eq!(listed("/bin/sh", &[], &list), Err("not one of the trusted programs"));
        fs::set_permissions(&list, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
        assert_eq!(listed("/bin/sh", &args_none(), &list), Ok(false));
        let _ = fs::remove_dir_all(&dir);
    }

    fn args_none() -> Vec<String> {
        Vec::new()
    }
}
