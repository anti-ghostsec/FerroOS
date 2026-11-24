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

/// From appearance.conf's text. What isn't there is what Display Properties
/// shows before anything's chosen: Flying Stars after 5 minutes, password
/// protected. So a new account locks itself when it's left. Only a saver of
/// "none" with no lock does nothing.
pub fn choice(text: &str) -> Choice {
    let get = |key: &str| text.lines().find_map(|l| l.split_once('=').filter(|(k, _)| k.trim() == key).map(|(_, v)| v.trim().to_owned()));
    let saver = match get("screensaver").as_deref().map(str::to_ascii_lowercase).as_deref() {
        Some("stars") | None => Some("stars"),
        Some("matrix") => Some("matrix"),
        Some("blank") | Some("custom") => Some("blank"),
        _ => None,
    };
    let minutes = get("screensaver_timeout").and_then(|m| m.parse().ok()).unwrap_or(5u64).clamp(1, 60);
    let lock = get("screensaver_lock").is_none_or(|v| v == "1" || v.eq_ignore_ascii_case("true"));
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
        format!(
            "PATH=/usr/local/bin:$PATH; {}; {}; ferro-screensaver --{saver} --lock || until {}; do sleep 1; done; {}",
            crate::HUSH,
            crate::USB_LOCK,
            crate::LOGON,
            crate::UNHUSH
        )
    } else {
        format!("PATH=/usr/local/bin:$PATH; {}; ferro-screensaver --{saver}; {}", crate::HUSH, crate::UNHUSH)
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
        assert_eq!(choice(""), Choice { saver: Some("stars"), minutes: 5, lock: true }, "nothing chosen yet: stars, locked");
        assert_eq!(choice("wallpaper = x\n"), choice(""));
        assert_eq!(command(&choice("screensaver = none\nscreensaver_lock = false\n")), None, "turned off: nothing");
        let c = choice("screensaver = stars\nscreensaver_timeout = 3\nscreensaver_lock = false\n");
        assert_eq!(c, Choice { saver: Some("stars"), minutes: 3, lock: false });
        let cmd = command(&c).unwrap();
        assert!(cmd.starts_with("PATH=/usr/local/bin:$PATH; ") && cmd.contains("; ferro-screensaver --stars; "), "{cmd}");
        assert!(cmd.contains("dunstctl set-paused true") && cmd.ends_with(crate::UNHUSH), "notifications held while it's up: {cmd}");
        let c = choice("screensaver = none\nscreensaver_timeout = 0\nscreensaver_lock = true\n");
        assert_eq!(c, Choice { saver: Some("blank"), minutes: 1, lock: true }, "locked with no saver: black");
        let cmd = command(&c).unwrap();
        assert!(cmd.contains("ferro-screensaver --blank --lock ||") && cmd.contains("until ") && cmd.contains("ferro-usb-lock lock"), "{cmd}");
        assert_eq!(choice("screensaver = matrix\nscreensaver_timeout = 999").minutes, 60);
    }

    #[test]
    fn notifications_come_back_as_they_were() {
        // A stand-in dunstctl that notes what it's asked, and answers
        // is-paused as given.
        let dir = std::env::temp_dir().join(format!("ferro-hush-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("dunstctl");
        std::fs::write(
            &fake,
            "#!/bin/sh\nif [ \"$1\" = is-paused ]; then cat \"$(dirname \"$0\")/state\"; else echo \"$*\" >>\"$(dirname \"$0\")/log\"; fi\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let run = |state: &str| {
            std::fs::write(dir.join("state"), state).unwrap();
            let _ = std::fs::remove_file(dir.join("log"));
            let script = format!("PATH={}:$PATH; {}; true; {}", dir.display(), crate::HUSH, crate::UNHUSH);
            std::process::Command::new("/bin/sh").args(["-c", &script]).status().unwrap();
            std::fs::read_to_string(dir.join("log")).unwrap_or_default()
        };
        assert_eq!(run("false\n"), "set-paused true\nset-paused false\n", "held, then shown");
        assert_eq!(run("true\n"), "set-paused true\n", "paused before: left paused");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
