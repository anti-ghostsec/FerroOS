//! The screen saver when the computer's been left: after the minutes
//! chosen in Display Properties (Screen Saver, Wait), the one chosen there
//! starts, and with "Password protected" the screen locks, as Win+L locks
//! it. Programs that play video keep it off as they play (they tell the
//! display they're in use: XResetScreenSaver, which also restarts the count).
//!
//! It runs on a thread of its own with a connection of its own, and asks
//! the display how long it's been since the last key or mouse movement
//! (MIT-SCREEN-SAVER), every few seconds.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
use x11rb::connection::Connection;
use x11rb::protocol::screensaver::ConnectionExt as _;

/// How often it looks.
const EVERY: Duration = Duration::from_secs(10);

/// What Display Properties chose.
#[derive(Debug, PartialEq, Eq)]
pub struct Choice {
    /// stars, matrix, blank (a custom one runs as blank); None: no saver.
    pub saver: Option<&'static str>,
    pub minutes: u64,
    pub lock: bool,
}

/// From appearance.conf's text. No saver and no lock: nothing happens.
pub fn choice(text: &str) -> Choice {
    let get = |key: &str| text.lines().find_map(|l| l.split_once('=').filter(|(k, _)| k.trim() == key).map(|(_, v)| v.trim().to_owned()));
    let saver = match get("screensaver").as_deref().map(str::to_ascii_lowercase).as_deref() {
        Some("stars") => Some("stars"),
        Some("matrix") => Some("matrix"),
        Some("blank") | Some("custom") => Some("blank"),
        _ => None,
    };
    let minutes = get("screensaver_timeout").and_then(|m| m.parse().ok()).unwrap_or(5u64).clamp(1, 60);
    let lock = get("screensaver_lock").is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
    // Password protection with no saver chosen: a black screen, locked.
    let saver = if lock { saver.or(Some("blank")) } else { saver };
    Choice { saver, minutes, lock }
}

/// The command that starts it: locked, as Win+L locks (see the window
/// manager's LockScreen), or just the saver.
/// FerroOS's own programs are in /usr/local/bin, which the session's PATH
/// needn't have.
pub fn command(c: &Choice) -> Option<String> {
    let saver = c.saver?;
    Some(if c.lock {
        format!("PATH=/usr/local/bin:$PATH; {}; ferro-screensaver --{saver} --lock || until {}; do sleep 1; done", crate::USB_LOCK, crate::LOGON)
    } else {
        format!("PATH=/usr/local/bin:$PATH; ferro-screensaver --{saver}")
    })
}

/// Whether what was started is still there. The window manager lets its
/// finished children go (SIGCHLD ignored, so no zombies), so it can't wait
/// for them: it looks for the process instead.
fn still_running(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

/// Starts it as the window manager starts everything (see `launch`): with
/// SIGCHLD back to normal, so its shell can wait for its own.
fn spawn(cmd: &str) -> Option<u32> {
    let mut c = Command::new("/bin/sh");
    c.args(["-c", cmd]).stdin(Stdio::null());
    // SAFETY: only signal(), which is async-signal-safe, between fork and exec.
    unsafe {
        c.pre_exec(|| {
            libc::signal(libc::SIGCHLD, libc::SIG_DFL);
            Ok(())
        });
    }
    c.spawn().map_err(|e| eprintln!("ferro-wm: screen saver not started: {e}")).ok().map(|child| child.id())
}

fn settings() -> Choice {
    let path = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/ferro/appearance.conf"));
    choice(&path.and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default())
}

/// Starts the watcher. Without the display's MIT-SCREEN-SAVER, nothing.
pub fn start() {
    std::thread::spawn(|| {
        let (conn, n) = match x11rb::connect(None) {
            Ok(c) => c,
            Err(e) => return eprintln!("ferro-wm: no idle screen saver: {e}"),
        };
        let root = conn.setup().roots[n].root;
        let mut running: Option<u32> = None;
        loop {
            std::thread::sleep(EVERY);
            // Still up (or still locked): not again.
            if running.is_some_and(still_running) {
                continue;
            }
            running = None;
            let info = match conn.screensaver_query_info(root).map(|c| c.reply()) {
                Ok(Ok(info)) => info,
                Ok(Err(e)) => return eprintln!("ferro-wm: no idle screen saver: {e}"),
                Err(e) => return eprintln!("ferro-wm: no idle screen saver: {e}"),
            };
            let c = settings();
            let Some(cmd) = command(&c) else { continue };
            if u64::from(info.ms_since_user_input) >= c.minutes * 60_000 {
                crate::mark_launch();
                running = spawn(&cmd);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_display_properties_chose() {
        assert_eq!(choice(""), Choice { saver: None, minutes: 5, lock: false }, "nothing chosen: nothing happens");
        assert_eq!(command(&choice("")), None);
        let c = choice("screensaver = stars\nscreensaver_timeout = 3\nscreensaver_lock = false\n");
        assert_eq!(c, Choice { saver: Some("stars"), minutes: 3, lock: false });
        assert_eq!(command(&c).as_deref(), Some("PATH=/usr/local/bin:$PATH; ferro-screensaver --stars"));
        let c = choice("screensaver = none\nscreensaver_timeout = 0\nscreensaver_lock = true\n");
        assert_eq!(c, Choice { saver: Some("blank"), minutes: 1, lock: true }, "locked with no saver: black");
        let cmd = command(&c).unwrap();
        assert!(cmd.contains("ferro-screensaver --blank --lock ||") && cmd.contains("until ") && cmd.contains("ferro-usb-lock lock"), "{cmd}");
        assert_eq!(choice("screensaver = matrix\nscreensaver_timeout = 999").minutes, 60);
    }
}
