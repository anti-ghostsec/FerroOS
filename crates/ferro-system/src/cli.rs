//! `ferro-system`: the one root helper the unprivileged desktop talks to.
//! It owns the vault key (in RAM only), restores and auto-saves the
//! persisted folders, enforces kill switches and applies RAM budgets.

#[cfg(not(target_os = "linux"))]
pub fn main() {
    eprintln!("ferro-system runs on FerroOS (Linux)");
}

#[cfg(target_os = "linux")]
pub fn main() {
    daemon::run()
}

#[cfg(target_os = "linux")]
mod daemon {
    use crate::{skip_persist, valid_password, VaultState, PERSISTED, SOCKET, USER_GID, USER_UID};
    use ferro_vault::{archive, is_vault, Vault, VaultError, DEFAULT_KDF};
    use std::fs::{self, File, OpenOptions};
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::io::AsRawFd;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    static STOP: AtomicBool = AtomicBool::new(false);

    extern "C" fn on_term(_: libc::c_int) {
        STOP.store(true, Ordering::SeqCst);
    }

    enum State {
        NoDevice,
        New(PathBuf),
        Locked(PathBuf),
        Unlocked { vault: Vault<File>, fingerprint: u64 },
        Amnesic,
    }

    impl State {
        fn public(&self) -> VaultState {
            match self {
                State::NoDevice => VaultState::None,
                State::New(_) => VaultState::New,
                State::Locked(_) => VaultState::Locked,
                State::Unlocked { .. } => VaultState::Unlocked,
                State::Amnesic => VaultState::Amnesic,
            }
        }
    }

    fn live() -> bool {
        fs::read_to_string("/proc/cmdline").unwrap_or_default().split_whitespace().any(|w| w == "ferro.live")
    }

    /// The vault: `ferro.vault=/dev/X` (development), or the partition named
    /// `ferro-vault` that Setup creates. Never a guess: a disk that merely
    /// exists is not offered for formatting. Live sessions use none.
    fn vault_device() -> Option<PathBuf> {
        if live() {
            return None;
        }
        let cmdline = fs::read_to_string("/proc/cmdline").unwrap_or_default();
        let wanted = cmdline.split_whitespace().find_map(|w| w.strip_prefix("ferro.vault=")).unwrap_or("PARTLABEL=ferro-vault");
        // Drivers (USB, NVMe) may still be settling right after boot.
        for _ in 0..50 {
            let found = match wanted.strip_prefix("PARTLABEL=") {
                Some(label) => partition_by_name(label),
                None => Some(PathBuf::from(wanted)).filter(|p| p.exists()),
            };
            if found.is_some() {
                return found;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        None
    }

    /// A GPT partition by its name, from the kernel's device events.
    fn partition_by_name(label: &str) -> Option<PathBuf> {
        fs::read_dir("/sys/class/block").ok()?.flatten().find_map(|e| {
            let uevent = fs::read_to_string(e.path().join("uevent")).ok()?;
            let name = uevent.lines().find_map(|l| l.strip_prefix("PARTNAME="))?;
            let dev = uevent.lines().find_map(|l| l.strip_prefix("DEVNAME="))?;
            (name == label).then(|| PathBuf::from("/dev").join(dev))
        })
    }

    fn open_dev(p: &Path) -> std::io::Result<File> {
        OpenOptions::new().read(true).write(true).open(p)
    }

    fn snapshot() -> std::io::Result<Vec<u8>> {
        archive::pack(Path::new("/"), &PERSISTED, &skip_persist)
    }

    fn fingerprint() -> u64 {
        archive::fingerprint(Path::new("/"), &PERSISTED, &skip_persist)
    }

    pub fn run() {
        // SAFETY: async-signal-safe handler that only stores to an atomic.
        unsafe {
            libc::signal(libc::SIGTERM, on_term as *const () as libc::sighandler_t);
            libc::signal(libc::SIGHUP, on_term as *const () as libc::sighandler_t);
        }
        let initial = match vault_device() {
            None => State::NoDevice,
            // Never offer to format the programs disk as a vault.
            Some(p) if crate::programs::is_programs_disk(&p) => State::NoDevice,
            Some(p) => match open_dev(&p).and_then(|mut f| is_vault(&mut f)) {
                Ok(true) => State::Locked(p),
                Ok(false) => State::New(p),
                Err(e) => {
                    eprintln!("ferro-system: {}: {e}", p.display());
                    State::NoDevice
                }
            },
        };
        eprintln!("ferro-system: vault {}", initial.public().as_str());
        let state = Arc::new(Mutex::new(initial));
        let switches = Arc::new(Mutex::new(Enforcer::default()));
        // Privacy-first until the user's saved choices are known.
        switches.lock().unwrap().apply([true, false, false, false, false]);

        let _ = fs::remove_file(SOCKET);
        let listener = match UnixListener::bind(SOCKET) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("ferro-system: can't listen on {SOCKET}: {e}");
                std::process::exit(1);
            }
        };
        let _ = fs::set_permissions(SOCKET, fs::Permissions::from_mode(0o600));
        // SAFETY: chown with a valid C path.
        unsafe { libc::chown(c"/run/ferro/system.sock".as_ptr(), USER_UID, USER_GID) };

        // A live session from installation media offers Setup.
        if live() {
            std::thread::spawn(|| {
                for _ in 0..50 {
                    if crate::programs::find_disk().is_some_and(|(_, e)| e.iter().any(|e| e.name == "bzImage")) {
                        let _ = fs::write(crate::INSTALL_MEDIA, "");
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
            });
        }

        let saver_state = state.clone();
        std::thread::spawn(move || autosave(&saver_state));
        let sw = switches.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(1));
            sw.lock().unwrap().reapply(); // catch hot-plugged cameras/mics
            ferro_sandbox::linux::reap_manual_cgroups();
        });

        for conn in listener.incoming() {
            let Ok(conn) = conn else { continue };
            if !peer_allowed(&conn) {
                continue;
            }
            let reply = handle(&conn, &state, &switches);
            let mut conn = conn;
            let _ = writeln!(conn, "{reply}");
        }
    }

    /// Only root and the desktop session itself may talk to us. Apps run as
    /// the same user, but inside their own sandbox cgroups: they must not be
    /// able to switch off the VPN or the kill switches.
    fn peer_allowed(conn: &UnixStream) -> bool {
        let mut cred = libc::ucred { pid: 0, uid: u32::MAX, gid: 0 };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: SO_PEERCRED fills a ucred.
        let r = unsafe { libc::getsockopt(conn.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&mut cred as *mut libc::ucred).cast(), &mut len) };
        if r != 0 {
            return false;
        }
        if cred.uid == 0 {
            return true;
        }
        let cgroup = fs::read_to_string(format!("/proc/{}/cgroup", cred.pid)).unwrap_or_default();
        cred.uid == USER_UID && cgroup.lines().any(|l| l == "0::/ferro/desktop")
    }

    fn handle(conn: &UnixStream, state: &Mutex<State>, switches: &Mutex<Enforcer>) -> String {
        let _ = conn.set_read_timeout(Some(Duration::from_secs(5)));
        let mut line = String::new();
        if BufReader::new(conn).read_line(&mut line).is_err() {
            return "ERR bad request".into();
        }
        let line = line.trim_end_matches(['\r', '\n']);
        let (cmd, arg) = line.split_once(' ').unwrap_or((line, ""));
        match cmd {
            "STATUS" => state.lock().unwrap().public().as_str().into(),
            "CREATE" => create(state, arg),
            "UNLOCK" => unlock(state, arg),
            "AMNESIC" => {
                let mut st = state.lock().unwrap();
                if !matches!(*st, State::Unlocked { .. }) {
                    *st = State::Amnesic;
                }
                "OK".into()
            }
            "SAVE" => save(&mut state.lock().unwrap()),
            "SWITCHES" => {
                let mut v: Vec<bool> = arg.split_whitespace().map(|b| b == "1").collect();
                if v.len() == 3 {
                    v.extend([false, false]);
                }
                let Ok(v) = <[bool; 5]>::try_from(v) else { return "ERR expected five switches".into() };
                switches.lock().unwrap().apply(v);
                // The user's choices are known now: the network may be used.
                if !Path::new(ferro_net::SESSION_FILE).exists() {
                    let _ = fs::write(ferro_net::SESSION_FILE, "");
                }
                "OK".into()
            }
            "LIMIT" => limit(arg),
            // Erasing disks is for live sessions only, never an installed system.
            "DISKS" | "INSTALL" if !live() => "ERR Setup runs from the FerroOS USB stick".into(),
            "DISKS" => match crate::install::disks() {
                Ok(list) => list.iter().map(|d| format!("{}|{}|{}", d.name, d.bytes, d.model.replace(['|', ';'], " "))).collect::<Vec<_>>().join(";"),
                Err(e) => format!("ERR {e}"),
            },
            "INSTALL" => match crate::install::start(arg.trim()) {
                Ok(()) => "OK".into(),
                Err(e) => format!("ERR {e}"),
            },
            _ => "ERR unknown command".into(),
        }
    }

    fn create(state: &Mutex<State>, password: &str) -> String {
        if let Err(e) = valid_password(password) {
            return format!("ERR {e}");
        }
        let mut st = state.lock().unwrap();
        let State::New(path) = &*st else { return "ERR a vault already exists".into() };
        let result = open_dev(path).map_err(VaultError::from).and_then(|dev| {
            let data = snapshot().map_err(VaultError::from)?;
            Vault::create(dev, password, DEFAULT_KDF, &data)
        });
        match result {
            Ok(vault) => {
                *st = State::Unlocked { vault, fingerprint: fingerprint() };
                eprintln!("ferro-system: vault created");
                "OK".into()
            }
            Err(e) => format!("ERR {e}"),
        }
    }

    fn unlock(state: &Mutex<State>, password: &str) -> String {
        let mut st = state.lock().unwrap();
        let State::Locked(path) = &*st else { return "ERR the vault isn't locked".into() };
        let opened = open_dev(path).map_err(VaultError::from).and_then(|dev| Vault::unlock(dev, password));
        match opened {
            Ok((vault, data)) => {
                match archive::unpack(&data, Path::new("/")) {
                    Ok(paths) => {
                        for p in paths {
                            give_to_user(&p);
                        }
                    }
                    Err(e) => return format!("ERR saved files are damaged: {e}"),
                }
                *st = State::Unlocked { vault, fingerprint: fingerprint() };
                eprintln!("ferro-system: vault unlocked");
                "OK".into()
            }
            Err(VaultError::WrongPassword) => {
                drop(st);
                std::thread::sleep(Duration::from_secs(1)); // slow down guessing
                "ERR Wrong password.".into()
            }
            Err(e) => format!("ERR {e}"),
        }
    }

    fn give_to_user(p: &Path) {
        if let Ok(c) = std::ffi::CString::new(p.as_os_str().as_encoded_bytes()) {
            // SAFETY: lchown with a valid C path.
            unsafe { libc::lchown(c.as_ptr(), USER_UID, USER_GID) };
        }
    }

    fn save(st: &mut State) -> String {
        let State::Unlocked { vault, fingerprint: fp } = st else { return "OK".into() };
        match snapshot().map_err(VaultError::from).and_then(|data| vault.save(&data)) {
            Ok(()) => {
                *fp = fingerprint();
                "OK".into()
            }
            Err(e) => {
                eprintln!("ferro-system: save failed: {e}");
                format!("ERR {e}")
            }
        }
    }

    /// Saves within ~2 s of any change, and once more on shutdown (SIGTERM).
    fn autosave(state: &Mutex<State>) {
        let mut ticks = 0u32;
        loop {
            std::thread::sleep(Duration::from_millis(200));
            let stopping = STOP.load(Ordering::SeqCst);
            ticks += 1;
            if stopping || ticks.is_multiple_of(10) {
                let mut st = state.lock().unwrap();
                let changed = matches!(&*st, State::Unlocked { fingerprint, .. } if *fingerprint != self::fingerprint());
                if changed || stopping {
                    let _ = save(&mut st);
                }
            }
            if stopping {
                eprintln!("ferro-system: saved; exiting");
                std::process::exit(0);
            }
        }
    }

    fn limit(arg: &str) -> String {
        let mut it = arg.split_whitespace();
        let (Some(pid), Some(bytes)) = (it.next().and_then(|p| p.parse::<u32>().ok()), it.next()) else {
            return "ERR usage: LIMIT <pid> <bytes|none>".into();
        };
        // Only the desktop user's own programs; system services stay untouched.
        let owner = fs::read_to_string(format!("/proc/{pid}/status"))
            .ok()
            .and_then(|s| s.lines().find_map(|l| l.strip_prefix("Uid:")?.split_whitespace().next()?.parse::<u32>().ok()));
        if owner != Some(USER_UID) {
            return "ERR only your own programs can be given a RAM budget".into();
        }
        let bytes = if bytes == "none" { None } else { bytes.parse().ok() };
        match ferro_sandbox::linux::limit_process(pid, bytes) {
            Ok(()) => "OK".into(),
            Err(e) => format!("ERR {e}"),
        }
    }

    // ---- kill switches ---------------------------------------------------------------

    /// Network: ferro-net takes interfaces down while the kill file exists.
    /// Camera/microphone: devices are unbound from their drivers, so they
    /// disappear for everyone. On most PCs one sound chip serves both mic and
    /// speakers, so "microphone off" also silences audio output.
    /// VPN and Tor: ferro-net routes by their flag files; Tor's service runs
    /// here while it's on.
    #[derive(Default)]
    struct Enforcer {
        /// network, microphone, camera, VPN, Tor
        wanted: Option<[bool; 5]>,
        camera: Vec<(PathBuf, String)>,
        microphone: Vec<(PathBuf, String)>,
        tor: crate::tor::Tor,
    }

    fn flag(path: &str, on: bool) {
        if !on {
            let _ = fs::remove_file(path);
        } else if !Path::new(path).exists() {
            let _ = fs::write(path, "on\n");
        }
    }

    impl Enforcer {
        fn apply(&mut self, wanted: [bool; 5]) {
            self.wanted = Some(wanted);
            self.reapply();
        }

        fn reapply(&mut self) {
            let Some([network, mic, camera, vpn, tor]) = self.wanted else { return };
            flag(ferro_net::KILL_FILE, !network);
            flag(ferro_net::VPN_FILE, vpn);
            flag(ferro_net::TOR_FILE, tor);
            enforce(&mut self.camera, "video4linux", |_| true, camera);
            enforce(&mut self.microphone, "sound", |n| n.starts_with("card"), mic);
            self.tor.set(tor);
        }
    }

    fn enforce(unbound: &mut Vec<(PathBuf, String)>, class: &str, wanted: impl Fn(&str) -> bool, enabled: bool) {
        if enabled {
            for (driver, id) in unbound.drain(..) {
                let _ = fs::write(driver.join("bind"), &id);
            }
            return;
        }
        for entry in fs::read_dir(format!("/sys/class/{class}")).into_iter().flatten().flatten() {
            if !wanted(&entry.file_name().to_string_lossy()) {
                continue;
            }
            let Ok(device) = fs::canonicalize(entry.path().join("device")) else { continue };
            let Ok(driver) = fs::canonicalize(device.join("driver")) else { continue };
            let Some(id) = device.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
            if fs::write(driver.join("unbind"), &id).is_ok() {
                eprintln!("ferro-system: {class} device {id} disconnected by kill switch");
                if !unbound.iter().any(|(_, u)| *u == id) {
                    unbound.push((driver, id));
                }
            }
        }
    }
}
