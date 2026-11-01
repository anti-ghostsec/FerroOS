//! Phones and tablets plugged in over USB in File transfer mode (MTP):
//! each is opened with aft-mtp-mount under /run/user/<you>/Phones/<its
//! name>, where My Computer shows it, as soon as it's plugged in. Nothing to
//! type. aft-mtp-mount reads from the phone as a file is read; jmtpfs, what
//! was used by hand before, copied each whole file into /tmp (memory, on
//! FerroOS) before handing over a byte.
//!
//! Unplugged, or plugged in again (a new connection, though the same phone),
//! the old mount is taken down at once (`fusermount -uz`), so nothing is
//! left pointing at a connection that's gone: that is what froze whatever
//! looked at it. One that can't be opened (locked, or File transfer not
//! chosen yet) is tried again while it stays plugged in, and said once.
//!
//! Which USB device is a phone, libmtp's udev rules say (ID_MTP_DEVICE, by
//! its list and mtp-probe); they also give it to whoever sits at the PC.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A phone as the kernel has it now. A new connection (plugged in again,
/// or the phone switched modes) is a new device number, so a new phone here.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Plugged {
    bus: u32,
    dev: u32,
    serial: String,
    product: String,
}

struct Mounted {
    at: PathBuf,
    child: Option<Child>,
    /// Said it's ready (or that it couldn't be opened): said once.
    said: bool,
    ready: bool,
    tries: u32,
    next_try: Instant,
}

pub struct Phones {
    root: PathBuf,
    known: HashMap<Plugged, Mounted>,
}

/// How long a phone gets to answer before it counts as not opened.
const OPEN_WITHIN: Duration = Duration::from_secs(20);

fn read(path: &Path) -> String {
    fs::read_to_string(path).map(|s| s.trim().to_owned()).unwrap_or_default()
}

/// Whether udev's record of a USB device says it's a phone (MTP).
fn is_mtp(db: &str) -> bool {
    db.lines().any(|l| l == "E:ID_MTP_DEVICE=1")
}

/// A USB device's character-device minor: what its udev record is named by.
fn minor(bus: u32, dev: u32) -> u32 {
    (bus.saturating_sub(1)) * 128 + dev.saturating_sub(1)
}

/// The phones plugged in now, from sysfs and udev's records.
fn plugged() -> Vec<Plugged> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir("/sys/bus/usb/devices") else { return out };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        // Interfaces ("1-2:1.0") and root hubs ("usb1") aren't devices of their own.
        if name.contains(':') || name.starts_with("usb") {
            continue;
        }
        let dir = e.path();
        let (Ok(bus), Ok(dev)) = (read(&dir.join("busnum")).parse::<u32>(), read(&dir.join("devnum")).parse::<u32>()) else { continue };
        let db = fs::read_to_string(format!("/run/udev/data/c189:{}", minor(bus, dev))).unwrap_or_default();
        if !is_mtp(&db) {
            continue;
        }
        let model = db.lines().find_map(|l| l.strip_prefix("E:ID_MODEL=")).unwrap_or("").replace('_', " ");
        let product = Some(read(&dir.join("product"))).filter(|p| !p.is_empty()).unwrap_or(model);
        out.push(Plugged { bus, dev, serial: read(&dir.join("serial")), product });
    }
    out
}

/// A folder name from what the phone calls itself: no slashes, nothing
/// hidden, never empty.
fn folder_name(product: &str) -> String {
    let clean: String = product.chars().map(|c| if c == '/' || c.is_control() { ' ' } else { c }).collect();
    let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    let clean = clean.trim_start_matches('.').trim();
    if clean.is_empty() {
        "Phone".into()
    } else {
        clean.to_owned()
    }
}

/// Whether something is mounted at `at` now.
fn is_mounted(at: &Path) -> bool {
    let want = at.to_string_lossy().replace(' ', "\\040");
    fs::read_to_string("/proc/self/mounts").unwrap_or_default().lines().any(|l| l.split_whitespace().nth(1) == Some(want.as_str()))
}

/// Takes down whatever is mounted at `at`, even if what serves it is gone
/// or stuck (lazy: it goes as soon as nothing is using it), then the folder.
fn unmount(at: &Path) {
    if is_mounted(at) {
        let _ = Command::new("fusermount").arg("-uz").arg(at).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
    let _ = fs::remove_dir(at);
}

fn stop(m: &mut Mounted) {
    if let Some(mut child) = m.child.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
    unmount(&m.at);
}

impl Phones {
    /// Phones are mounted under `root` (/run/user/<you>/Phones). What a
    /// session before this one left there (it ended, or crashed, with a
    /// phone mounted) is taken down first.
    pub fn new(root: PathBuf) -> Self {
        if let Ok(entries) = fs::read_dir(&root) {
            for e in entries.flatten() {
                unmount(&e.path());
            }
        }
        Self { root, known: HashMap::new() }
    }

    /// A free place for a phone: its name, or with a number when two of
    /// the same are plugged in.
    fn place_for(&self, product: &str) -> PathBuf {
        let base = folder_name(product);
        let taken = |p: &Path| self.known.values().any(|m| m.at == p);
        (1..).map(|n| self.root.join(if n == 1 { base.clone() } else { format!("{base} ({n})") })).find(|p| !taken(p)).expect("some name is free")
    }

    /// Once every couple of seconds: phones that went are taken down,
    /// new ones mounted, ones that couldn't be opened tried again.
    pub fn tick(&mut self, notify: &dyn Fn(&str, &str)) {
        let now = plugged();
        // Gone, or connected anew: down at once.
        let gone: Vec<Plugged> = self.known.keys().filter(|k| !now.contains(k)).cloned().collect();
        for k in gone {
            if let Some(mut m) = self.known.remove(&k) {
                stop(&mut m);
            }
        }
        let several = now.len() > 1;
        for p in now {
            if !self.known.contains_key(&p) {
                let at = self.place_for(&p.product);
                self.known.insert(p.clone(), Mounted { at, child: None, said: false, ready: false, tries: 0, next_try: Instant::now() });
            }
            let m = self.known.get_mut(&p).expect("just added");
            let name = folder_name(&p.product);
            match m.child.as_mut().map(|c| c.try_wait()) {
                // Running: ready once its folder is mounted.
                Some(Ok(None)) => {
                    if !m.ready && is_mounted(&m.at) {
                        m.ready = true;
                        m.tries = 0;
                        if !m.said {
                            m.said = true;
                            notify(&format!("{name} is ready"), "Open it from My Computer.");
                        }
                    } else if !m.ready && m.next_try.elapsed() > OPEN_WITHIN {
                        // Started but never opened: stopped, tried again.
                        stop(m);
                        Self::failed(m, &name, notify);
                    }
                }
                // It ended (the phone stopped answering, or wouldn't open).
                Some(_) => {
                    stop(m);
                    m.ready = false;
                    Self::failed(m, &name, notify);
                }
                None if Instant::now() >= m.next_try => Self::start(m, &p, several),
                None => {}
            }
        }
    }

    fn failed(m: &mut Mounted, name: &str, notify: &dyn Fn(&str, &str)) {
        m.tries += 1;
        m.next_try = Instant::now() + if m.tries < 6 { Duration::from_secs(5) } else { Duration::from_secs(30) };
        if !m.said && m.tries >= 2 {
            m.said = true;
            notify(
                &format!("{name} can't be opened yet"),
                "Unlock the phone, and on it choose USB: File transfer. It opens by itself as soon as it can.",
            );
        }
    }

    fn start(m: &mut Mounted, p: &Plugged, several: bool) {
        // Something left at this place (a crash) goes first.
        unmount(&m.at);
        if fs::create_dir_all(&m.at).is_err() {
            m.next_try = Instant::now() + Duration::from_secs(30);
            return;
        }
        let mut cmd = Command::new("aft-mtp-mount");
        // In the foreground, so it's ours to stop. With more than one
        // plugged in, this one only: by the serial number it gives over
        // MTP, which on phones is the USB one (not on everything, so only
        // then: alone, it's the one there is).
        cmd.arg("-f");
        if several && !p.serial.is_empty() {
            cmd.args(["-D", &p.serial]);
        }
        cmd.arg(&m.at).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        m.child = cmd.spawn().ok();
        m.ready = false;
        m.next_try = Instant::now();
        if m.child.is_none() {
            // aft-mtp-mount isn't there (an update not done yet): not again soon.
            m.next_try = Instant::now() + Duration::from_secs(300);
            let _ = fs::remove_dir(&m.at);
        }
    }
}

impl Drop for Phones {
    fn drop(&mut self) {
        for m in self.known.values_mut() {
            stop(m);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_device_is_a_phone() {
        assert!(is_mtp("E:ID_VENDOR=samsung\nE:ID_MTP_DEVICE=1\nG:uaccess\n"));
        assert!(!is_mtp("E:ID_VENDOR=Dell\nE:ID_INPUT_KEYBOARD=1\n"));
        // Bus 1 device 15 is /dev/bus/usb/001/015, char 189:14.
        assert_eq!(minor(1, 15), 14);
        assert_eq!(minor(2, 1), 128);
    }

    #[test]
    fn folder_names() {
        assert_eq!(folder_name("Galaxy A15 5G"), "Galaxy A15 5G");
        assert_eq!(folder_name("Pixel 8/Pro"), "Pixel 8 Pro");
        assert_eq!(folder_name("..hidden"), "hidden");
        assert_eq!(folder_name("  "), "Phone");
    }
}
