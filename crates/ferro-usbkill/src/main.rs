//! ferro-usbkill: FerroOS's USB Kill (see the library for what it's for).
//!
//!   ferro-usbkill watch          the service: watches the ports (root)
//!   ferro-usbkill status         whether it is on (prints on or off)
//!   ferro-usbkill devices        what's plugged in, and whether it's allowed
//!   ferro-usbkill check          what in the rules file makes no sense
//!   ferro-usbkill allow-present  allows everything plugged in now (root)
//!   ferro-usbkill apply FILE     makes FILE the rules, if it all makes sense
//!                                (root; Settings > USB Kill uses it)
//!
//! The service is always running; with the rules switched off it only
//! waits for them to change. It notices a change at once (the kernel says
//! so) and looks every interval_ms besides. Once set off it notes why, runs
//! the rules' commands, wipes what they list, and powers off at once: no
//! programs are asked to close, nothing gets a chance to stop it.

use ferro_usbkill::{judge, scan, Action, Bus, Config, Rule, Verdict, When, BUSES, CONFIG, LAST, LOG};
use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

/// ferro-usb-lock's sign that the screen is locked.
const LOCKED: &str = "/run/ferro/usb-locked";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["watch"] => watch(),
        ["devices"] => devices(),
        ["check"] => check(),
        ["status"] => {
            println!("{}", if load().0.enabled { "on" } else { "off" });
            0
        }
        ["allow-present"] => allow_present(),
        ["apply", file] => apply(Path::new(file)),
        _ => {
            eprintln!("usage: ferro-usbkill watch | status | devices | check | allow-present | apply FILE");
            2
        }
    };
    std::process::exit(code);
}

/// Where the ports' devices are listed: /sys, or a made-up folder when
/// FerroOS's tests name one (FERRO_USBKILL_SYS).
fn ports_dir() -> std::path::PathBuf {
    std::env::var_os("FERRO_USBKILL_SYS").map_or_else(|| BUSES.into(), Into::into)
}

fn load() -> (Config, Vec<String>) {
    Config::parse(&std::fs::read_to_string(CONFIG).unwrap_or_default())
}

fn root() -> bool {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() == 0 }
}

/// Readable by everyone, whatever the umask (root's is 077): Settings reads
/// the rules as the person using it, and root-only rules read as off.
fn readable(f: &std::fs::File) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    f.set_permissions(std::fs::Permissions::from_mode(0o644))
}

/// The rules, written whole and then put in place, so the service never
/// reads half of them.
fn save(c: &Config) -> std::io::Result<()> {
    std::fs::create_dir_all("/etc/ferro")?;
    let tmp = format!("{CONFIG}.new");
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o644).open(&tmp)?;
    readable(&f)?;
    f.write_all(c.format().as_bytes())?;
    f.sync_all()?;
    std::fs::rename(&tmp, CONFIG)
}

fn devices() -> i32 {
    let (c, _) = load();
    let all = scan(&ports_dir());
    // The computer's own PCI devices are watched, but aren't plugged in.
    let (inside, found): (Vec<_>, Vec<_>) = all.iter().partition(|d| Bus::of(&d.id) == Bus::Pci && !d.external);
    if found.is_empty() {
        println!("No devices are plugged in.");
    }
    for d in found {
        let state = if c.tethered(d) {
            "tether"
        } else if c.allowed(d) {
            "allowed"
        } else {
            "not allowed"
        };
        println!("{:<10} {}  {}  [{state}]", d.port, d.id, d.name);
        println!("{:<10} allow = {}", "", Rule::for_device(d));
    }
    if !inside.is_empty() {
        println!("\nAnd {} PCI devices inside the computer, watched too.", inside.len());
    }
    println!("\nUSB Kill is {}.", if c.enabled { "on" } else { "off" });
    0
}

fn check() -> i32 {
    let (_, bad) = load();
    for b in &bad {
        println!("{CONFIG}: left out, {b}");
    }
    i32::from(!bad.is_empty())
}

fn allow_present() -> i32 {
    if !root() {
        eprintln!("ferro-usbkill: allow-present needs root (sudo)");
        return 1;
    }
    let (mut c, _) = load();
    for d in scan(&ports_dir()) {
        if !c.allowed(&d) {
            println!("Allowed: {} ({})", d.name, Rule::for_device(&d));
            c.allow.push(Rule::for_device(&d));
        }
    }
    match save(&c) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("ferro-usbkill: {CONFIG}: {e}");
            1
        }
    }
}

fn apply(file: &Path) -> i32 {
    if !root() {
        eprintln!("ferro-usbkill: apply needs root (sudo)");
        return 1;
    }
    let text = match std::fs::metadata(file) {
        Ok(m) if m.is_file() && m.len() <= 64 * 1024 => std::fs::read_to_string(file),
        Ok(_) => Err(std::io::Error::other("not a small file")),
        Err(e) => Err(e),
    };
    let text = match text {
        Ok(t) => t,
        Err(e) => {
            eprintln!("ferro-usbkill: {}: {e}", file.display());
            return 1;
        }
    };
    let (c, bad) = Config::parse(&text);
    if !bad.is_empty() {
        for b in &bad {
            eprintln!("ferro-usbkill: {b}");
        }
        eprintln!("ferro-usbkill: the rules weren't changed.");
        return 1;
    }
    match save(&c) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("ferro-usbkill: {CONFIG}: {e}");
            1
        }
    }
}

/// The date and time (UTC), for the log.
fn now_utc() -> String {
    let secs = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rest) = ((secs / 86400) as i64, secs % 86400);
    // Days to a date (Howard Hinnant's civil_from_days).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC", rest / 3600, rest / 60 % 60, rest % 60)
}

/// Noted in the log (on the encrypted disk) and for Settings.
fn note(line: &str) {
    let line = format!("{} {line}\n", now_utc());
    eprint!("ferro-usbkill: {line}");
    if let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).mode(0o600).open(LOG) {
        let _ = f.write_all(line.as_bytes());
        let _ = f.sync_data();
    }
}

fn fire(c: &Config, why: &str) {
    let what = match c.action {
        Action::PowerOff => "powering off",
        Action::Restart => "restarting",
        Action::Note => "only noted (testing)",
    };
    note(&format!("set off, {why}: {what}"));
    if std::fs::create_dir("/run/ferro").is_ok() {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions("/run/ferro", std::fs::Permissions::from_mode(0o755));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o644).open(LAST) {
        let _ = readable(&f);
        let _ = f.write_all(format!("{} {why}: {what}\n", now_utc()).as_bytes());
    }
    if c.action == Action::Note {
        return;
    }
    for cmd in &c.run {
        run(cmd);
    }
    for p in &c.wipe {
        wipe(p);
    }
    if c.sync {
        // SAFETY: sync has no preconditions.
        unsafe { libc::sync() };
    }
    let how = if c.action == Action::Restart { libc::RB_AUTOBOOT } else { libc::RB_POWER_OFF };
    // SAFETY: reboot with a valid command; it doesn't return when it works.
    unsafe { libc::reboot(how) };
    note("couldn't power off");
}

/// A command from the rules, given 5 seconds.
fn run(cmd: &str) {
    let Ok(mut child) = Command::new("/bin/sh").args(["-c", cmd]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() else {
        return;
    };
    let end = Instant::now() + Duration::from_secs(5);
    while Instant::now() < end {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
}

/// Overwritten (the first 64 MB of each file: it has to be quick, and on an
/// SSD overwriting can't reach every copy anyway: the disk's encryption is
/// what protects it) and deleted. Links are deleted, never followed.
fn wipe(p: &Path) {
    let Ok(m) = std::fs::symlink_metadata(p) else { return };
    if m.is_dir() {
        if let Ok(dir) = std::fs::read_dir(p) {
            for e in dir.flatten() {
                wipe(&e.path());
            }
        }
        let _ = std::fs::remove_dir(p);
        return;
    }
    if m.is_file() {
        if let Ok(mut f) = std::fs::OpenOptions::new().write(true).custom_flags(libc::O_NOFOLLOW).open(p) {
            let zeros = vec![0u8; 1 << 20];
            let mut left = m.len().min(64 << 20);
            while left > 0 {
                let n = left.min(zeros.len() as u64) as usize;
                if f.write_all(&zeros[..n]).is_err() {
                    break;
                }
                left -= n as u64;
            }
            let _ = f.sync_data();
        }
    }
    let _ = std::fs::remove_file(p);
}

/// The kernel's word that a device came or went (a netlink socket), so a
/// change is seen at once, not at the next look.
fn uevents() -> Option<i32> {
    // SAFETY: plain socket calls with a zeroed, then filled-in address.
    unsafe {
        let fd = libc::socket(libc::AF_NETLINK, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK, libc::NETLINK_KOBJECT_UEVENT);
        if fd < 0 {
            return None;
        }
        let mut sa: libc::sockaddr_nl = std::mem::zeroed();
        sa.nl_family = libc::AF_NETLINK as u16;
        sa.nl_groups = 1;
        if libc::bind(fd, (&raw const sa).cast(), std::mem::size_of::<libc::sockaddr_nl>() as u32) < 0 {
            libc::close(fd);
            return None;
        }
        Some(fd)
    }
}

/// Waits up to `ms`, or until the kernel says something happened.
fn wait(fd: Option<i32>, ms: u64) {
    let Some(fd) = fd else {
        std::thread::sleep(Duration::from_millis(ms));
        return;
    };
    let mut p = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
    // SAFETY: one valid pollfd; then reads into a buffer of its own size.
    unsafe {
        if libc::poll(&mut p, 1, ms as i32) > 0 {
            let mut buf = [0u8; 4096];
            while libc::recv(fd, buf.as_mut_ptr().cast(), buf.len(), 0) > 0 {}
        }
    }
}

/// How long the computer has slept (suspended) since it started.
fn slept() -> Duration {
    let read = |clock| {
        let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: a valid clock and timespec.
        unsafe { libc::clock_gettime(clock, &mut t) };
        Duration::new(t.tv_sec as u64, t.tv_nsec as u32)
    };
    read(libc::CLOCK_BOOTTIME).saturating_sub(read(libc::CLOCK_MONOTONIC))
}

/// Asked to stop (SIGTERM, SIGINT, SIGHUP).
static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_stop(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

/// Whether the computer is shutting down (then the service is stopped as
/// everything is, and that's no reason to set it off).
fn shutting_down() -> bool {
    Command::new("systemctl")
        .arg("is-system-running")
        .stderr(Stdio::null())
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "stopping")
}

fn watch() -> i32 {
    if !root() {
        eprintln!("ferro-usbkill: watch needs root");
        return 1;
    }
    // Last to be killed when memory runs out.
    let _ = std::fs::write("/proc/self/oom_score_adj", "-1000");
    // SAFETY: the handler only stores to an atomic.
    unsafe {
        for sig in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            libc::signal(sig, on_stop as *const () as libc::sighandler_t);
        }
    }
    let fd = uevents();
    // The rules as read (None: no file), and as understood.
    let mut text: Option<Vec<u8>> = None;
    let mut c = Config::default();
    // What's watched: the ports as they were when it started watching (or
    // last changed as allowed). None: not watching.
    let mut base = None;
    let mut asleep = slept();
    let mut quiet_until = Instant::now();
    loop {
        // Stopped while watching, other than by the computer shutting down:
        // someone getting it out of the way. That sets it off.
        if STOP.load(Ordering::SeqCst) {
            if base.is_some() && !shutting_down() {
                fire(&c, "the service was stopped while it was on");
            }
            note("stopped");
            return 0;
        }
        // The rules, read every time (not only when the file's date
        // changes, which can be set back). Changed while it's watching,
        // they're not taken: the change sets it off, with the rules it was
        // watching with. Someone at the computer can't allow their own
        // device first. (Only noting, to try rules out, the new ones are
        // then taken.)
        let now_text = std::fs::read(CONFIG).ok();
        if now_text != text {
            if base.is_some() {
                fire(&c, "the rules were changed while it was on");
            }
            text = now_text;
            let bad;
            (c, bad) = Config::parse(&String::from_utf8_lossy(text.as_deref().unwrap_or_default()));
            for b in bad {
                note(&format!("rules: left out, {b}"));
            }
            base = None;
        }
        let armed = c.enabled && (c.when == When::Always || Path::new(LOCKED).exists());
        if !armed {
            if base.take().is_some() {
                note("stopped watching");
            }
            wait(fd, 1000);
            continue;
        }
        // Back from sleep, the ports' devices are found all over again: a
        // few seconds for that, then what's there is watched.
        let s = slept();
        if s > asleep + Duration::from_secs(1) {
            asleep = s;
            quiet_until = Instant::now() + Duration::from_secs(5);
        }
        let ports = scan(&ports_dir());
        if Instant::now() < quiet_until {
            base = Some(ports);
        } else if let Some(before) = &base {
            match judge(&c, before, &ports) {
                Verdict::Same => {}
                Verdict::Allowed => base = Some(ports),
                Verdict::Fire(why) => {
                    fire(&c, &why);
                    base = Some(scan(&ports_dir()));
                }
            }
        } else {
            note(&format!("watching {} devices", ports.len()));
            base = Some(ports);
        }
        wait(fd, c.interval_ms);
    }
}
