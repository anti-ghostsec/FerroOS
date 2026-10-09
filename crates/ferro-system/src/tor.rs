//! The Tor service (Arti, Tor in Rust), running only while Tor is on.
//!
//! It runs as its own unprivileged user, the only one that the routing in
//! ferro-net lets reach the internet in Tor mode. Its program, state and
//! cache live in RAM (`/run`) and are deleted when Tor is switched off, so
//! nothing about Tor use is left on disk.

use crate::programs;
use std::fs;
use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const UID: u32 = 900;
const BIN_DIR: &str = "/run/tor-bin";
const DATA_DIR: &str = "/run/tor";
/// `state=off|starting|ready|error` and `detail=...`, for the desktop.
pub const STATUS: &str = "/run/ferro/tor-status";

const CONFIG: &str = r#"# Written by FerroOS each time Tor starts.
[proxy]
socks_listen = "127.0.0.1:9150"
dns_listen = "127.0.0.1:9053"

[storage]
cache_dir = "/run/tor/cache"
state_dir = "/run/tor/state"

[logging]
console = "info"
"#;

#[derive(Default)]
pub struct Tor {
    child: Option<Child>,
    state: Arc<Mutex<(String, String)>>,
    retry_at: Option<Instant>,
}

impl Tor {
    /// Starts or stops Tor to match the switch.
    pub fn set(&mut self, on: bool) {
        match (on, self.child.is_some()) {
            (true, false) => {
                if self.retry_at.is_some_and(|t| Instant::now() < t) {
                    return;
                }
                match start(&self.state) {
                    Ok(c) => {
                        self.child = Some(c);
                        self.retry_at = None;
                    }
                    Err(e) => {
                        eprintln!("ferro-system: Tor: {e}");
                        set_state(&self.state, "error", &e);
                        self.retry_at = Some(Instant::now() + Duration::from_secs(30));
                    }
                }
            }
            (false, true) => self.stop(),
            (false, false) if self.retry_at.take().is_some() => set_state(&self.state, "off", ""),
            _ => {}
        }
        // Restart if it died while wanted.
        if let Some(c) = self.child.as_mut() {
            if let Ok(Some(status)) = c.try_wait() {
                self.child = None;
                set_state(&self.state, "error", &format!("Tor stopped unexpectedly ({status}); restarting"));
                self.retry_at = Some(Instant::now() + Duration::from_secs(5));
            }
        }
    }

    fn stop(&mut self) {
        if let Some(mut c) = self.child.take() {
            // SAFETY: signalling our own child.
            unsafe { libc::kill(c.id() as i32, libc::SIGTERM) };
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline && matches!(c.try_wait(), Ok(None)) {
                std::thread::sleep(Duration::from_millis(50));
            }
            let _ = c.kill();
            let _ = c.wait();
        }
        // Guards, consensus, the program itself: all gone, RAM freed.
        unmount(DATA_DIR);
        unmount(BIN_DIR);
        set_state(&self.state, "off", "");
        eprintln!("ferro-system: Tor stopped");
    }
}

fn set_state(state: &Mutex<(String, String)>, s: &str, detail: &str) {
    *state.lock().unwrap() = (s.to_owned(), detail.to_owned());
    let text = format!("state={s}\ndetail={detail}\n");
    let tmp = format!("{STATUS}.tmp");
    if fs::write(&tmp, text).is_ok() {
        let _ = fs::rename(&tmp, STATUS);
    }
}

/// A RAM disk of its own, so Tor never fills `/run` (where status files and
/// sockets live), and unmounting it gives every byte back.
fn mount_tmpfs(dir: &str, opts: &str) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let (d, o) = (std::ffi::CString::new(dir).unwrap(), std::ffi::CString::new(opts).unwrap());
    // SAFETY: mount(2) with valid C strings.
    let r = unsafe { libc::mount(c"tmpfs".as_ptr(), d.as_ptr(), c"tmpfs".as_ptr(), libc::MS_NOSUID | libc::MS_NODEV, o.as_ptr().cast()) };
    if r != 0 {
        return Err(format!("can't make room for Tor: {}", std::io::Error::last_os_error()));
    }
    Ok(())
}

fn unmount(dir: &str) {
    let d = std::ffi::CString::new(dir).unwrap();
    // SAFETY: umount2(2) with a valid C string; MNT_DETACH in case of stragglers.
    unsafe { libc::umount2(d.as_ptr(), libc::MNT_DETACH) };
    let _ = fs::remove_dir(dir);
}

fn chown(p: &Path, uid: u32) {
    if let Ok(c) = std::ffi::CString::new(p.as_os_str().as_encoded_bytes()) {
        // SAFETY: chown with a valid C path.
        unsafe { libc::chown(c.as_ptr(), uid, uid) };
    }
}

fn start(state: &Arc<Mutex<(String, String)>>) -> Result<Child, String> {
    set_state(state, "starting", "loading Tor");
    unmount(DATA_DIR);
    unmount(BIN_DIR);
    mount_tmpfs(BIN_DIR, "size=24m,mode=0755")?;
    mount_tmpfs(DATA_DIR, &format!("size=96m,mode=0700,uid={UID},gid={UID}"))?;
    let bin = Path::new(BIN_DIR).join("arti");
    if let Err(e) = programs::extract("arti", &bin) {
        unmount(DATA_DIR);
        unmount(BIN_DIR);
        return Err(e.to_string());
    }
    let config = Path::new(BIN_DIR).join("arti.toml");
    fs::write(&config, CONFIG).map_err(|e| e.to_string())?;
    let _ = fs::set_permissions(&config, fs::Permissions::from_mode(0o644));
    for d in ["/run/tor/state", "/run/tor/cache"] {
        fs::create_dir_all(d).map_err(|e| e.to_string())?;
        let _ = fs::set_permissions(d, fs::Permissions::from_mode(0o700));
        chown(Path::new(d), UID);
    }

    let mut cmd = Command::new(&bin);
    cmd.args(["proxy", "-c"])
        .arg(&config)
        .env_clear()
        .env("HOME", DATA_DIR)
        .env("PATH", "/bin")
        .current_dir(DATA_DIR)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    // SAFETY: only async-signal-safe calls between fork and exec.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setgroups(0, std::ptr::null()) != 0 || libc::setgid(UID) != 0 || libc::setuid(UID) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0);
            Ok(())
        });
    }
    let mut child = cmd.spawn().map_err(|e| format!("can't start Tor: {e}"))?;
    let stderr = child.stderr.take().expect("piped");
    let st = state.clone();
    set_state(state, "starting", "connecting to the Tor network");
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            // Progress goes to the desktop; problems to the RAM-only console.
            if line.contains("Sufficiently bootstrapped") {
                set_state(&st, "ready", "connected to the Tor network");
                eprintln!("ferro-system: Tor ready");
            } else if ["warn", "error", "fatal", "panick", "usage"].iter().any(|w| line.to_ascii_lowercase().contains(w)) {
                eprintln!("tor: {line}");
            }
        }
    });
    eprintln!("ferro-system: Tor starting (pid {})", child.id());
    Ok(child)
}
