//! ferro-xgate: FerroOS Heavy's X11 gate.
//!
//! With X11 any program on the display can see all of it: every window on
//! every desktop, and every key typed. So programs don't get the display
//! itself (which runs as `:5`, its cookie root's alone) but the gate, as
//! `:0`. Through it they draw, take the keyboard when they're used, talk to
//! the window manager as before; to see the screen (a screenshot, a screen
//! share, recording it) they must be allowed, by the person, in a question
//! the gate puts up (with time to close what's private first). Pretend key
//! presses, recording what's typed and the like they can't do at all. See
//! policy.rs for exactly what's looked at.
//!
//! The window manager, the Snipping Tool (Print Screen) and the compositor
//! go through as before: the gate starts them itself (trust.rs, run.rs).
//!
//!   ferro-xgate serve --display 0 --upstream 5 --auth FILE --uid UID
//!   ferro-xgate run [--detach] PROGRAM   has the gate start one of those
//!   ferro-xgate probe [hold]   tries each way of seeing the screen, prints
//!                              what it got (and stays a few seconds more)
//!   ferro-xgate tell LINE...   a notice with an OK button

mod gate;
mod policy;
mod popup;
mod probe;
mod run;
mod sock;
mod trust;
mod wire;

use std::io::Write;
use std::os::linux::net::SocketAddrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{SocketAddr, UnixListener};
use std::path::{Path, PathBuf};

type Res<T> = Result<T, Box<dyn std::error::Error>>;

/// The cookie for display `number` in an Xauthority file (its
/// MIT-MAGIC-COOKIE-1 entry; the first such if none names it).
pub fn cookie(file: &[u8], number: &str) -> Option<Vec<u8>> {
    let mut at = 0;
    let field = |at: &mut usize| -> Option<Vec<u8>> {
        let n = usize::from(u16::from_be_bytes([*file.get(*at)?, *file.get(*at + 1)?]));
        let v = file.get(*at + 2..*at + 2 + n)?.to_vec();
        *at += 2 + n;
        Some(v)
    };
    let mut first = None;
    while at + 2 <= file.len() {
        at += 2; // the family
        let (_addr, num, name, data) = (field(&mut at)?, field(&mut at)?, field(&mut at)?, field(&mut at)?);
        if name == b"MIT-MAGIC-COOKIE-1" {
            if num == number.as_bytes() {
                return Some(data);
            }
            first.get_or_insert(data);
        }
    }
    first
}

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

/// What holds a display's lock file: a display server, or a gate. Any
/// other process with the number in a lock left behind isn't one.
pub fn holds_display(program: &str) -> bool {
    matches!(program.trim(), "Xorg" | "X" | "Xvfb" | "Xephyr" | "Xnest" | "Xvnc" | "Xwayland" | GATE)
}
const GATE: &str = "ferro-xgate";

/// Whether a process has ended (a zombie, or dead), from its line in
/// /proc/PID/stat: `PID (NAME) STATE ...`, where NAME may hold anything.
fn ended(stat: &str) -> bool {
    stat.rsplit_once(')').is_some_and(|(_, rest)| matches!(rest.trim_start().chars().next(), Some('Z' | 'X' | 'x')))
}

/// The program (its file's name) whose process holds the lock file, if
/// it's still running and is a display server or a gate.
fn holder(lock: &Path) -> Option<(u32, String)> {
    // A plain file only: nothing is read through a link put in /tmp.
    if !std::fs::symlink_metadata(lock).ok()?.file_type().is_file() {
        return None;
    }
    let pid: u32 = std::fs::read_to_string(lock).ok()?.trim().parse().ok()?;
    if pid == 0 || pid == std::process::id() {
        return None;
    }
    // One that has ended and only waits to be collected holds nothing.
    if ended(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?) {
        return None;
    }
    let name = std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().trim_end_matches(" (deleted)").to_owned()))
        .or_else(|| std::fs::read_to_string(format!("/proc/{pid}/comm")).ok().map(|c| c.trim().to_owned()))?;
    holds_display(&name).then_some((pid, name))
}

/// Display `n`'s sockets: the file in /tmp/.X11-unix and the abstract one
/// programs try first. Its lock file says it's taken, so no second display
/// server starts as it.
fn listen(n: &str) -> Res<Vec<UnixListener>> {
    let dir = PathBuf::from("/tmp/.X11-unix");
    if !dir.is_dir() {
        std::fs::create_dir_all(&dir)?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o1777))?;
    }
    // SAFETY: geteuid only reads this process's user.
    let me = unsafe { libc::geteuid() };
    // In a folder that's someone else's the socket could be swapped for theirs.
    if !std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir() && (m.uid() == 0 || m.uid() == me)) {
        return Err(format!("{} isn't root's", dir.display()).into());
    }
    let lock = PathBuf::from(format!("/tmp/.X{n}-lock"));
    // The gate of the logon before ends with its display: a moment for it.
    // A lock whose process is gone, or is now some other program (numbers
    // are used again), is left over.
    for tries in 0.. {
        match holder(&lock) {
            Some((_, name)) if name == GATE && tries < 12 => std::thread::sleep(std::time::Duration::from_millis(250)),
            Some((pid, name)) => return Err(format!("display :{n} is in use ({name}, process {pid})").into()),
            None => break,
        }
    }
    let _ = std::fs::remove_file(&lock);
    // Made new (never written through whatever is there), read-only.
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o444)
        .open(&lock)?
        .write_all(format!("{:>10}\n", std::process::id()).as_bytes())?;
    let bound = (|| -> Res<Vec<UnixListener>> {
        let path = dir.join(format!("X{n}"));
        // The abstract one first: programs try it first, so if another
        // program has it they'd talk to that one, not the gate.
        let hidden = SocketAddr::from_abstract_name(path.as_os_str().as_encoded_bytes())
            .and_then(|a| UnixListener::bind_addr(&a))
            .map_err(|e| format!("display :{n}'s socket is another program's ({e})"))?;
        let _ = std::fs::remove_file(&path);
        let file = UnixListener::bind(&path)?;
        // Anyone may knock; who comes in is checked (the person, or root).
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o777))?;
        Ok(vec![hidden, file])
    })();
    if bound.is_err() {
        let _ = std::fs::remove_file(&lock);
    }
    bound
}

fn serve(args: &[String]) -> Res<()> {
    // The gate collects the programs it starts and says how each ended,
    // whatever started the gate: with a child's end ignored there'd be
    // nothing to collect.
    // SAFETY: sets how this process takes one signal, before any thread.
    unsafe { libc::signal(libc::SIGCHLD, libc::SIG_DFL) };
    let display = arg(args, "--display").unwrap_or_else(|| "0".into());
    let upstream = arg(args, "--upstream").ok_or("--upstream N: the display's own number")?;
    let auth = arg(args, "--auth").ok_or("--auth FILE: the display's cookie")?;
    let uid: u32 = arg(args, "--uid").and_then(|u| u.parse().ok()).ok_or("--uid UID: whose session it is")?;
    let extra = arg(args, "--trusted").unwrap_or_else(|| trust::EXTRA.into());
    let cookie = cookie(&std::fs::read(&auth)?, &upstream).ok_or("no cookie in the --auth file")?;
    // Nobody may look inside the gate (it holds the cookie).
    // SAFETY: prctl with these arguments only changes a flag of this process.
    unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) };
    let gate = gate::Gate::new(PathBuf::from(format!("/tmp/.X11-unix/X{upstream}")), cookie, uid, PathBuf::from(extra))?;
    for l in listen(&display)? {
        let g = std::sync::Arc::clone(&gate);
        std::thread::spawn(move || g.serve(l));
    }
    gate.watch()
}

/// `ferro-xgate tell LINE...`: a notice on $DISPLAY with an OK button, for
/// the logon to say why there's no desktop (see logon-session).
fn tell(lines: &[String]) -> Res<()> {
    use x11rb::rust_connection::RustConnection;
    let (conn, screen) = RustConnection::connect(None)?;
    popup::tell(&conn, screen, lines)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let r = match args.get(1).map(String::as_str) {
        Some("serve") => serve(&args),
        Some("probe") if args.get(2).is_some_and(|a| a == "tricks") => probe::tricks(args.get(3).and_then(|s| s.parse().ok()).unwrap_or(3)),
        Some("probe") => probe::run(args.get(2).is_some_and(|a| a == "hold")),
        Some("run") => run::run(&args[2..]),
        Some("tell") => tell(&args[2..]),
        _ => Err("usage: ferro-xgate serve --display N --upstream N --auth FILE --uid UID [--trusted FILE] | run [--detach] PROGRAM [ARGUMENTS] | probe [hold | tricks] | tell LINE...".into()),
    };
    if let Err(e) = r {
        eprintln!("ferro-xgate: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(num: &str, name: &[u8], data: &[u8]) -> Vec<u8> {
        let mut b = vec![1, 0];
        for f in [&b"host"[..], num.as_bytes(), name, data] {
            b.extend_from_slice(&(f.len() as u16).to_be_bytes());
            b.extend_from_slice(f);
        }
        b
    }

    #[test]
    fn a_lock_left_behind_isnt_a_display() {
        assert!(holds_display("Xorg") && holds_display("Xvfb\n") && holds_display("ferro-xgate"));
        assert!(!holds_display("firefox-esr") && !holds_display("") && !holds_display("Xorg.wrap-not"));
        let dir = std::env::temp_dir().join(format!("xgate-lock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let lock = dir.join("lock");
        // This test's own parent is running, and isn't a display server:
        // its number in a lock is one that was used again.
        std::fs::write(&lock, format!("{:>10}\n", std::os::unix::process::parent_id())).unwrap();
        assert_eq!(holder(&lock), None);
        std::fs::write(&lock, "         0\n").unwrap();
        assert_eq!(holder(&lock), None);
        // A gate that has ended but hasn't been collected yet holds nothing.
        assert!(ended("19167 (ferro-xgate) Z 19005 19005"));
        assert!(!ended("19167 (ferro-xgate) S 19005 19005") && !ended("7 (a) Z b) R 1 1") && !ended(""));
        std::fs::write(&lock, "not a number").unwrap();
        assert_eq!(holder(&lock), None);
        // A link in the lock's place isn't followed.
        let target = dir.join("target");
        std::fs::write(&target, "1\n").unwrap();
        std::fs::remove_file(&lock).unwrap();
        std::os::unix::fs::symlink(&target, &lock).unwrap();
        assert_eq!(holder(&lock), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn finds_the_displays_cookie() {
        let mut file = entry("0", b"MIT-MAGIC-COOKIE-1", &[1; 16]);
        file.extend(entry("5", b"XDM-AUTHORIZATION-1", &[9; 8]));
        file.extend(entry("5", b"MIT-MAGIC-COOKIE-1", &[5; 16]));
        assert_eq!(cookie(&file, "5"), Some(vec![5; 16]));
        assert_eq!(cookie(&file, "7"), Some(vec![1; 16]), "the first, if none names it");
        assert_eq!(cookie(&file[..10], "5"), None, "cut short");
        assert_eq!(cookie(b"", "5"), None);
    }
}
