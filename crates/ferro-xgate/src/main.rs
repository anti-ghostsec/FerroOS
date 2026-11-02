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
//! go through as before (trust.rs).
//!
//!   ferro-xgate serve --display 0 --upstream 5 --auth FILE --uid UID
//!   ferro-xgate probe [hold]   tries each way of seeing the screen, prints
//!                              what it got (and stays a few seconds more)

mod gate;
mod policy;
mod popup;
mod probe;
mod sock;
mod trust;
mod wire;

use std::os::linux::net::SocketAddrExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{SocketAddr, UnixListener};
use std::path::PathBuf;

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

/// Display `n`'s sockets: the file in /tmp/.X11-unix and the abstract one
/// programs try first. Its lock file says it's taken, so no second display
/// server starts as it.
fn listen(n: &str) -> Res<Vec<UnixListener>> {
    let dir = PathBuf::from("/tmp/.X11-unix");
    if !dir.is_dir() {
        std::fs::create_dir_all(&dir)?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o1777))?;
    }
    let lock = format!("/tmp/.X{n}-lock");
    if let Ok(text) = std::fs::read_to_string(&lock) {
        let pid: u32 = text.trim().parse().unwrap_or(0);
        if pid != 0 && pid != std::process::id() && PathBuf::from(format!("/proc/{pid}")).exists() {
            return Err(format!("display :{n} is in use (process {pid})").into());
        }
    }
    std::fs::write(&lock, format!("{:>10}\n", std::process::id()))?;
    std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o444))?;
    let path = dir.join(format!("X{n}"));
    let _ = std::fs::remove_file(&path);
    let file = UnixListener::bind(&path)?;
    // Anyone may knock; who comes in is checked (the person, or root).
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o777))?;
    let mut all = vec![file];
    if let Ok(l) = SocketAddr::from_abstract_name(path.as_os_str().as_encoded_bytes()).and_then(|a| UnixListener::bind_addr(&a)) {
        all.push(l);
    }
    Ok(all)
}

fn serve(args: &[String]) -> Res<()> {
    let display = arg(args, "--display").unwrap_or_else(|| "0".into());
    let upstream = arg(args, "--upstream").ok_or("--upstream N: the display's own number")?;
    let auth = arg(args, "--auth").ok_or("--auth FILE: the display's cookie")?;
    let uid: u32 = arg(args, "--uid").and_then(|u| u.parse().ok()).ok_or("--uid UID: whose session it is")?;
    let cookie = cookie(&std::fs::read(&auth)?, &upstream).ok_or("no cookie in the --auth file")?;
    // Nobody may look inside the gate (it holds the cookie).
    // SAFETY: prctl with these arguments only changes a flag of this process.
    unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) };
    let gate = gate::Gate::new(PathBuf::from(format!("/tmp/.X11-unix/X{upstream}")), cookie, uid)?;
    for l in listen(&display)? {
        let g = std::sync::Arc::clone(&gate);
        std::thread::spawn(move || g.serve(l));
    }
    gate.watch()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let r = match args.get(1).map(String::as_str) {
        Some("serve") => serve(&args),
        Some("probe") => probe::run(args.get(2).is_some_and(|a| a == "hold")),
        _ => Err("usage: ferro-xgate serve --display N --upstream N --auth FILE --uid UID | probe".into()),
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
