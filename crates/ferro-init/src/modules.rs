//! Loads drivers for the hardware that's actually present.
//!
//! Every device the kernel finds exposes a `modalias` (e.g.
//! `pci:v00001234d00001111...`); `modules.alias` maps glob patterns of those
//! to driver modules. We load the matches (dependencies first), rescan in
//! case a bus driver revealed more devices, then free the RAM held by
//! module files nobody needs, keeping only drivers for buses where devices
//! can appear later (USB, HID, input). A uevent listener loads those on
//! hotplug.

use ferro_sys::glob_match;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Buses whose devices can show up after boot.
/// Kept after boot for devices plugged in later: USB, keyboards and mice,
/// and the disks behind USB sticks (SCSI).
const HOTPLUG_PREFIXES: [&str; 4] = ["usb:", "hid:", "input:", "scsi:"];
/// Drivers loaded later by name when a feature is switched on (the VPN).
const ON_DEMAND: [&str; 1] = ["wireguard"];

fn module_name(rel: &str) -> String {
    let file = rel.rsplit('/').next().unwrap_or(rel);
    let stem = file.split(".ko").next().unwrap_or(file);
    stem.replace('-', "_")
}

pub struct Modules {
    dir: PathBuf,
    deps: HashMap<String, Vec<String>>,
    /// (pattern, module name)
    aliases: Vec<(String, String)>,
    by_name: HashMap<String, String>,
    loaded: HashSet<String>,
    cmdline: String,
}

impl Modules {
    /// `None` when the kernel has no module tree (all built in, or the
    /// prebuilt kernel's fixed list is used instead).
    pub fn open() -> Option<Self> {
        let dir = fs::read_dir("/lib/modules").ok()?.flatten().map(|e| e.path()).find(|p| p.join("modules.dep").exists())?;
        let deps: HashMap<String, Vec<String>> = fs::read_to_string(dir.join("modules.dep"))
            .ok()?
            .lines()
            .filter_map(|l| l.split_once(':'))
            .map(|(m, d)| (m.to_owned(), d.split_whitespace().map(str::to_owned).collect()))
            .collect();
        let by_name = deps.keys().map(|rel| (module_name(rel), rel.clone())).collect();
        let aliases = fs::read_to_string(dir.join("modules.alias"))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                let mut w = l.split_whitespace();
                if w.next()? != "alias" {
                    return None;
                }
                Some((w.next()?.to_owned(), w.next()?.replace('-', "_")))
            })
            .collect();
        let cmdline = fs::read_to_string("/proc/cmdline").unwrap_or_default();
        Some(Self { dir, deps, aliases, by_name, loaded: HashSet::new(), cmdline })
    }

    /// Loads drivers for every device present; rescans until stable.
    pub fn load_for_present_hardware(&mut self) {
        for _ in 0..4 {
            let before = self.loaded.len();
            let mut found = Vec::new();
            collect_modaliases(Path::new("/sys/devices"), &mut found, 0);
            for ma in found {
                self.load_matching(&ma);
            }
            if self.loaded.len() == before {
                break;
            }
        }
        let mut names: Vec<String> = self.loaded.iter().map(|r| module_name(r)).collect();
        names.sort();
        eprintln!("init: drivers for present hardware: {}", names.join(" "));
    }

    pub fn load_matching(&mut self, modalias: &str) {
        let wanted: Vec<String> =
            self.aliases.iter().filter(|(pat, _)| glob_match(pat, modalias)).filter_map(|(_, m)| self.by_name.get(m).cloned()).collect();
        for rel in wanted {
            self.load(&rel, 0);
        }
    }

    fn load(&mut self, rel: &str, depth: usize) {
        if self.loaded.contains(rel) || depth > 16 {
            return;
        }
        for d in self.deps.get(rel).cloned().unwrap_or_default() {
            self.load(&d, depth + 1);
        }
        let path = self.dir.join(rel);
        match crate::system::finit(&path, &self.cmdline) {
            Ok(()) => {
                self.loaded.insert(rel.to_owned());
            }
            Err(e) => eprintln!("init: module {}: {e}", module_name(rel)),
        }
    }

    /// Frees RAM: removes loaded modules' files (the kernel has them) and
    /// every module that can't be needed later; keeps hotplug-bus drivers.
    pub fn prune(&self) {
        let hotplug: HashSet<&str> =
            self.aliases.iter().filter(|(pat, _)| HOTPLUG_PREFIXES.iter().any(|p| pat.starts_with(p))).map(|(_, m)| m.as_str()).collect();
        let mut kept = HashSet::new();
        for (rel, deps) in &self.deps {
            let name = module_name(rel);
            if !self.loaded.contains(rel) && (hotplug.contains(name.as_str()) || ON_DEMAND.contains(&name.as_str())) {
                kept.insert(rel.as_str());
                kept.extend(deps.iter().map(String::as_str));
            }
        }
        for rel in self.deps.keys() {
            if !kept.contains(rel.as_str()) {
                let _ = fs::remove_file(self.dir.join(rel));
            }
        }
    }
}

fn collect_modaliases(dir: &Path, out: &mut Vec<String>, depth: usize) {
    if depth > 24 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            collect_modaliases(&e.path(), out, depth + 1);
        } else if e.file_name() == "modalias" {
            if let Ok(s) = fs::read_to_string(e.path()) {
                let s = s.trim();
                if !s.is_empty() {
                    out.push(s.to_owned());
                }
            }
        }
    }
}

/// Subscribes to kernel device events. Open this *before* loading drivers:
/// drivers create devices asynchronously (the PS/2 mouse appears after
/// psmouse returns), and an event sent before anyone listens is lost.
pub fn uevent_socket() -> io::Result<i32> {
    // SAFETY: socket/bind with a zeroed, then filled, sockaddr_nl.
    let fd = unsafe { libc::socket(libc::AF_NETLINK, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, libc::NETLINK_KOBJECT_UEVENT) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut addr: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    addr.nl_family = libc::AF_NETLINK as u16;
    addr.nl_groups = 1;
    // SAFETY: addr is a valid sockaddr_nl.
    if unsafe { libc::bind(fd, (&addr as *const libc::sockaddr_nl).cast(), std::mem::size_of::<libc::sockaddr_nl>() as u32) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(fd)
}

/// Loads drivers for devices that appear later (and for those whose events
/// queued up on `fd` during boot), and hands their device files to the user.
pub fn hotplug_listener(fd: i32, mut modules: Modules) -> io::Result<()> {
    std::thread::Builder::new().name("hotplug".into()).spawn(move || {
        let mut buf = vec![0u8; 8192];
        loop {
            // SAFETY: reading into our buffer.
            let n = unsafe { libc::recv(fd, buf.as_mut_ptr().cast(), buf.len(), 0) };
            if n <= 0 {
                continue;
            }
            let msg = String::from_utf8_lossy(&buf[..n as usize]);
            let field = |k: &str| msg.split('\0').find_map(|kv| kv.strip_prefix(k)).map(str::to_owned);
            if field("ACTION=").as_deref() != Some("add") {
                continue;
            }
            if let Some(ma) = field("MODALIAS=") {
                modules.load_matching(&ma);
            }
            if let (Some(dev), Some(sub)) = (field("DEVNAME="), field("SUBSYSTEM=")) {
                if matches!(sub.as_str(), "input" | "drm" | "sound") {
                    crate::system::give_to_user(&format!("/dev/{dev}"));
                }
            }
        }
    })?;
    Ok(())
}
