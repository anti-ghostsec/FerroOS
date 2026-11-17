//! USB Kill: the computer powers off the moment its USB ports change in a
//! way you haven't allowed: a stick or a "keyboard" plugged in, a device
//! taken out, or the key on a cord round your wrist pulled away (a tether).
//! For a server, or a laptop that may be taken from you while it's
//! unlocked: with the disk encrypted, powered off means the data is out of
//! reach again.
//!
//! The rules live in /etc/ferro/usbkill.conf (Settings > USB Kill writes
//! them, or edit the file). This library reads and writes them, lists the
//! USB devices from /sys, and judges a change; the root daemon
//! (`ferro-usbkill watch`) does the rest.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub const CONFIG: &str = "/etc/ferro/usbkill.conf";
/// What set it off last (or would have, testing), for Settings to show.
pub const LAST: &str = "/run/ferro/usbkill-last";
pub const LOG: &str = "/var/log/ferro-usbkill.log";
/// Where the kernel lists the devices on each bus.
pub const BUSES: &str = "/sys/bus";

/// When it watches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    Always,
    /// Only while the screen is locked (ferro-usb-lock's /run/ferro/usb-locked).
    Locked,
}

/// What it does when set off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    PowerOff,
    Restart,
    /// Only notes it (to try the rules out).
    Note,
}

/// Where a device is plugged in: a USB port (USB-A or USB-C, and what's on
/// hubs and docks), Thunderbolt (its PCIe devices, through a USB-C port
/// too), or FireWire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bus {
    Usb,
    Thunderbolt,
    FireWire,
}

/// A hex number from /sys or a rule ("0x8086", "8086"), as `width` digits.
fn hex(s: &str, width: usize) -> Option<String> {
    let h = s.trim().trim_start_matches("0x");
    (1..=8).contains(&h.len()).then_some(())?;
    let n = u32::from_str_radix(h, 16).ok()?;
    let out = format!("{n:0width$x}");
    (out.len() == width).then_some(out)
}

impl Bus {
    /// What a rule's id starts with: nothing for USB.
    pub fn prefix(self) -> &'static str {
        match self {
            Bus::Usb => "",
            Bus::Thunderbolt => "tb:",
            Bus::FireWire => "fw:",
        }
    }

    /// How many hex digits its vendor and product numbers have.
    fn width(self) -> usize {
        match self {
            Bus::Usb | Bus::Thunderbolt => 4,
            Bus::FireWire => 6,
        }
    }

    pub fn of(id: &str) -> Bus {
        if id.starts_with("tb:") {
            Bus::Thunderbolt
        } else if id.starts_with("fw:") {
            Bus::FireWire
        } else {
            Bus::Usb
        }
    }

    /// A device's id: vendor:product (USB), tb:vendor:device, fw:vendor:model.
    pub fn id(self, vendor: &str, product: &str) -> Option<String> {
        Some(format!("{}{}:{}", self.prefix(), hex(vendor, self.width())?, hex(product, self.width())?))
    }

    /// A rule's id, checked and written the one way.
    fn id_from(s: &str) -> Option<String> {
        let bus = Bus::of(s);
        let (v, p) = s[bus.prefix().len()..].split_once(':')?;
        // USB ids are always 4 digits each, as lsusb shows them.
        if bus == Bus::Usb && (v.len() != 4 || p.len() != 4) {
            return None;
        }
        bus.id(v, p)
    }
}

/// A device on a list: its USB id, and its serial number when given.
/// Ids can be copied onto another device; a serial makes that harder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub id: String,
    pub serial: Option<String>,
}

impl Rule {
    pub fn parse(s: &str) -> Option<Rule> {
        let s = s.trim();
        let (id, rest) = s.split_once(char::is_whitespace).map_or((s, ""), |(a, b)| (a, b.trim()));
        let id = Bus::id_from(&id.to_ascii_lowercase())?;
        let serial = match rest {
            "" => None,
            r => Some(r.strip_prefix("serial=")?.trim().to_owned()).filter(|s| !s.is_empty() && !s.contains(['\n', '\r'])),
        };
        if !rest.is_empty() && serial.is_none() {
            return None;
        }
        Some(Rule { id, serial })
    }

    pub fn matches(&self, d: &Device) -> bool {
        self.id == d.id && self.serial.as_ref().is_none_or(|s| d.serial.as_deref() == Some(s))
    }

    /// The rule for exactly this device: its id, and its serial if it has one.
    pub fn for_device(d: &Device) -> Rule {
        Rule { id: d.id.clone(), serial: d.serial.clone() }
    }
}

impl std::fmt::Display for Rule {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match &self.serial {
            Some(s) => write!(f, "{} serial={s}", self.id),
            None => f.write_str(&self.id),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub enabled: bool,
    pub when: When,
    /// A device that isn't allowed, plugged in, sets it off.
    pub on_new: bool,
    /// A device that isn't allowed, taken out, sets it off.
    pub on_removed: bool,
    pub action: Action,
    /// Everything written to the disks first (safer for what's open, slower).
    pub sync: bool,
    /// How often it looks, besides being told by the kernel (milliseconds).
    pub interval_ms: u64,
    /// Devices that may come and go.
    pub allow: Vec<Rule>,
    /// Devices whose taking out always sets it off.
    pub tether: Vec<Rule>,
    /// Files and folders overwritten and deleted first.
    pub wipe: Vec<PathBuf>,
    /// Commands run first, as root (each given 5 seconds).
    pub run: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            enabled: false,
            when: When::Always,
            on_new: true,
            on_removed: true,
            action: Action::PowerOff,
            sync: false,
            interval_ms: 250,
            allow: Vec::new(),
            tether: Vec::new(),
            wipe: Vec::new(),
            run: Vec::new(),
        }
    }
}

/// Folders a wipe must never be given: the system itself.
fn too_much(p: &Path) -> bool {
    const SYSTEM: [&str; 16] =
        ["/", "/bin", "/boot", "/dev", "/etc", "/home", "/lib", "/lib64", "/proc", "/root", "/run", "/sbin", "/sys", "/usr", "/var", "/Programs"];
    let s = p.to_string_lossy();
    let s = s.trim_end_matches('/');
    SYSTEM.contains(&if s.is_empty() { "/" } else { s })
}

fn yes(v: &str) -> Option<bool> {
    match v.to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

impl Config {
    /// From the file's text: the rules, and a line for each that made no
    /// sense (left out, the rest still apply).
    pub fn parse(text: &str) -> (Config, Vec<String>) {
        let mut c = Config::default();
        let mut bad = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                bad.push(format!("line {}: no '=' in it", n + 1));
                continue;
            };
            let (k, v) = (k.trim(), v.trim());
            let ok = match k {
                "enabled" => yes(v).map(|b| c.enabled = b).is_some(),
                "on_new" => yes(v).map(|b| c.on_new = b).is_some(),
                "on_removed" => yes(v).map(|b| c.on_removed = b).is_some(),
                "sync" => yes(v).map(|b| c.sync = b).is_some(),
                "when" => match v {
                    "always" => Some(When::Always),
                    "locked" => Some(When::Locked),
                    _ => None,
                }
                .map(|w| c.when = w)
                .is_some(),
                "action" => match v {
                    "poweroff" => Some(Action::PowerOff),
                    "restart" => Some(Action::Restart),
                    "note" => Some(Action::Note),
                    _ => None,
                }
                .map(|a| c.action = a)
                .is_some(),
                "interval_ms" => v.parse::<u64>().ok().filter(|ms| (50..=5000).contains(ms)).map(|ms| c.interval_ms = ms).is_some(),
                "allow" => Rule::parse(v).map(|r| c.allow.push(r)).is_some(),
                "tether" => Rule::parse(v).map(|r| c.tether.push(r)).is_some(),
                "wipe" => {
                    let p = PathBuf::from(v);
                    let fine = p.is_absolute() && !p.components().any(|c| matches!(c, std::path::Component::ParentDir)) && !too_much(&p);
                    if fine {
                        c.wipe.push(p);
                    }
                    fine
                }
                "run" => (!v.is_empty()).then(|| c.run.push(v.to_owned())).is_some(),
                _ => false,
            };
            if !ok {
                bad.push(format!("line {}: {line}", n + 1));
            }
        }
        (c, bad)
    }

    /// The file's text, with what each setting does.
    pub fn format(&self) -> String {
        let b = |v: bool| if v { "true" } else { "false" };
        let mut s = String::from(
            "# USB Kill (Settings > USB Kill): the computer powers off the moment its\n\
             # USB, Thunderbolt or FireWire ports change in a way not allowed here. `ferro-usbkill devices`\n\
             # lists what's plugged in, with the lines that would allow each.\n\n",
        );
        let _ = writeln!(s, "enabled = {}", b(self.enabled));
        let _ = writeln!(s, "# always, or locked: only while the screen is locked.");
        let _ = writeln!(s, "when = {}", if self.when == When::Locked { "locked" } else { "always" });
        let _ = writeln!(s, "# A device that isn't allowed: plugged in, taken out.");
        let _ = writeln!(s, "on_new = {}", b(self.on_new));
        let _ = writeln!(s, "on_removed = {}", b(self.on_removed));
        let _ = writeln!(s, "# poweroff, restart, or note (only writes it down, to try the rules).");
        let action = match self.action {
            Action::PowerOff => "poweroff",
            Action::Restart => "restart",
            Action::Note => "note",
        };
        let _ = writeln!(s, "action = {action}");
        let _ = writeln!(s, "# Write everything to the disks first: safer for open files, slower.");
        let _ = writeln!(s, "sync = {}", b(self.sync));
        let _ = writeln!(s, "interval_ms = {}", self.interval_ms);
        let _ = writeln!(
            s,
            "\n# Devices that may come and go: allow = vendor:product [serial=...]\n# (Thunderbolt: tb:vendor:device, FireWire: fw:vendor:model)"
        );
        for r in &self.allow {
            let _ = writeln!(s, "allow = {r}");
        }
        let _ = writeln!(s, "\n# Taking one of these out always sets it off (a key on a cord round your wrist).");
        for r in &self.tether {
            let _ = writeln!(s, "tether = {r}");
        }
        let _ = writeln!(s, "\n# First, before powering off: commands run as root (5 seconds each),");
        let _ = writeln!(s, "# then files and folders overwritten and deleted.");
        for r in &self.run {
            let _ = writeln!(s, "run = {r}");
        }
        for p in &self.wipe {
            let _ = writeln!(s, "wipe = {}", p.display());
        }
        s
    }

    pub fn allowed(&self, d: &Device) -> bool {
        self.allow.iter().chain(&self.tether).any(|r| r.matches(d))
    }

    pub fn tethered(&self, d: &Device) -> bool {
        self.tether.iter().any(|r| r.matches(d))
    }
}

/// A USB device plugged in now.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Device {
    /// Its place in /sys (the port), e.g. 3-1.2.
    pub port: String,
    /// The kernel's number for it, new each time it's plugged in: taken out
    /// and put back between two looks, it's still seen as a change.
    pub devnum: u32,
    pub id: String,
    pub serial: Option<String>,
    pub name: String,
}

/// What's plugged in, from /sys/bus (`root`): USB, Thunderbolt and
/// FireWire devices. The computer's own controllers (USB's usb1, usb2...,
/// Thunderbolt's host router, FireWire's own node) aren't devices you plug
/// in: left out.
pub fn scan(root: &Path) -> Vec<Device> {
    let mut found = Vec::new();
    for (bus, dir) in [(Bus::Usb, "usb"), (Bus::Thunderbolt, "thunderbolt"), (Bus::FireWire, "firewire")] {
        let Ok(entries) = std::fs::read_dir(root.join(dir).join("devices")) else { continue };
        for e in entries.flatten() {
            let port = e.file_name().to_string_lossy().into_owned();
            let p = e.path();
            let read = |f: &str| std::fs::read_to_string(p.join(f)).ok().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
            let name = |m: Option<String>, d: Option<String>, what: &str| match (m, d) {
                (Some(m), Some(d)) if d.starts_with(&m) => d,
                (Some(m), Some(d)) => format!("{m} {d}"),
                (m, d) => d.or(m).unwrap_or_else(|| what.into()),
            };
            let device = match bus {
                Bus::Usb => {
                    if port.contains(':') || port.starts_with("usb") {
                        continue;
                    }
                    let Some(id) = read("idVendor").zip(read("idProduct")).and_then(|(v, d)| bus.id(&v, &d)) else { continue };
                    let devnum = read("devnum").and_then(|n| n.parse().ok()).unwrap_or(0);
                    Device { devnum, id, serial: read("serial"), name: name(read("manufacturer"), read("product"), "USB device"), port }
                }
                // Routers are "domain-route": route 0 is the computer's own.
                Bus::Thunderbolt => {
                    if port.contains(':') || port.starts_with("domain") || port.ends_with("-0") {
                        continue;
                    }
                    let Some(id) = read("vendor").zip(read("device")).and_then(|(v, d)| bus.id(&v, &d)) else { continue };
                    Device {
                        devnum: 0,
                        id,
                        serial: read("unique_id"),
                        name: name(read("vendor_name"), read("device_name"), "Thunderbolt device"),
                        port,
                    }
                }
                // Nodes are fwN (their units fwN.M); the computer's own is_local.
                Bus::FireWire => {
                    if port.contains('.') || read("is_local").as_deref() == Some("1") {
                        continue;
                    }
                    let Some(id) = read("vendor").zip(read("model")).and_then(|(v, d)| bus.id(&v, &d)) else { continue };
                    Device { devnum: 0, id, serial: read("guid"), name: name(read("vendor_name"), read("model_name"), "FireWire device"), port }
                }
            };
            found.push(device);
        }
    }
    found.sort();
    found
}

/// What a change on the ports means.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Same,
    /// Changed, but as allowed: the new state is what's watched from now on.
    Allowed,
    /// Set off, and why.
    Fire(String),
}

fn named(d: &Device) -> String {
    format!("{} ({}{})", d.name, d.id, d.serial.as_ref().map(|s| format!(" serial={s}")).unwrap_or_default())
}

/// Judges the ports now against before.
pub fn judge(c: &Config, before: &[Device], now: &[Device]) -> Verdict {
    // A USB device is the one in that port with that number; FireWire
    // renumbers its nodes whenever the bus resets, so there it's only what
    // the device is.
    let key = |d: &Device| (if Bus::of(&d.id) == Bus::Usb { d.port.clone() } else { String::new() }, d.devnum, d.id.clone(), d.serial.clone());
    let (was, is): (BTreeSet<_>, BTreeSet<_>) = (before.iter().map(key).collect(), now.iter().map(key).collect());
    let gone: Vec<&Device> = before.iter().filter(|d| !is.contains(&key(d))).collect();
    let came: Vec<&Device> = now.iter().filter(|d| !was.contains(&key(d))).collect();
    if gone.is_empty() && came.is_empty() {
        return Verdict::Same;
    }
    if let Some(d) = gone.iter().find(|d| c.tethered(d)) {
        return Verdict::Fire(format!("tether taken out: {}", named(d)));
    }
    if c.on_removed {
        if let Some(d) = gone.iter().find(|d| !c.allowed(d)) {
            return Verdict::Fire(format!("taken out: {}", named(d)));
        }
    }
    if c.on_new {
        if let Some(d) = came.iter().find(|d| !c.allowed(d)) {
            return Verdict::Fire(format!("plugged in: {}", named(d)));
        }
    }
    Verdict::Allowed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(port: &str, devnum: u32, id: &str, serial: Option<&str>) -> Device {
        Device { port: port.into(), devnum, id: id.into(), serial: serial.map(Into::into), name: "Stick".into() }
    }

    #[test]
    fn rules_read_back_as_written() {
        let (c, bad) = Config::parse(
            "enabled = true\nwhen = locked\non_new = yes\non_removed = false\naction = note\nsync = true\ninterval_ms = 100\n\
             allow = 046D:C52B\nallow = 0781:5583 serial=ABC 1\ntether = 1234:abcd serial=X\nrun = /usr/bin/true\nwipe = /home/a/secret\n",
        );
        assert!(bad.is_empty(), "{bad:?}");
        assert!(c.enabled && c.on_new && !c.on_removed && c.sync);
        assert_eq!((c.when, c.action, c.interval_ms), (When::Locked, Action::Note, 100));
        assert_eq!(c.allow[0], Rule { id: "046d:c52b".into(), serial: None });
        assert_eq!(c.allow[1].serial.as_deref(), Some("ABC 1"));
        assert_eq!(Config::parse(&c.format()), (c, vec![]));
        assert_eq!(Config::parse(""), (Config::default(), vec![]), "nothing set: off");
    }

    #[test]
    fn nonsense_is_left_out_and_said() {
        let (c, bad) = Config::parse(
            "enabled = maybe\nallow = 46d:c52b\nallow = 046d:c52b sn=1\nwipe = relative\nwipe = /\nwipe = /etc/\nwipe = /home/../etc\ninterval_ms = 1\nfoo = 1\nnothing\n",
        );
        assert_eq!(bad.len(), 10, "{bad:?}");
        assert_eq!(c, Config::default());
    }

    #[test]
    fn a_change_sets_it_off_only_as_the_rules_say() {
        let mut c = Config::default();
        let kb = dev("3-5", 2, "1c4f:0002", None);
        let stick = dev("3-6", 7, "0781:5583", Some("S1"));
        let base = vec![kb.clone()];
        assert_eq!(judge(&c, &base, &base), Verdict::Same);
        assert!(matches!(judge(&c, &base, &[kb.clone(), stick.clone()]), Verdict::Fire(w) if w.starts_with("plugged in")));
        assert!(matches!(judge(&c, &base, &[]), Verdict::Fire(w) if w.starts_with("taken out")));
        // Out and back in between two looks: a new number, still a change.
        assert!(matches!(judge(&c, &base, &[dev("3-5", 9, "1c4f:0002", None)]), Verdict::Fire(_)));
        c.allow.push(Rule::parse("1c4f:0002").unwrap());
        c.allow.push(Rule::parse("0781:5583 serial=S1").unwrap());
        assert_eq!(judge(&c, &base, &[kb.clone(), stick.clone()]), Verdict::Allowed);
        assert_eq!(judge(&c, &base, &[]), Verdict::Allowed);
        // The same id with another serial isn't the allowed stick.
        assert!(matches!(judge(&c, &base, &[kb.clone(), dev("3-6", 8, "0781:5583", Some("S2"))]), Verdict::Fire(_)));
        c.on_new = false;
        assert_eq!(judge(&c, &base, &[kb.clone(), dev("3-6", 8, "0781:5583", Some("S2"))]), Verdict::Allowed);
        // A tether taken out always sets it off, even when nothing else would.
        c.on_removed = false;
        c.tether.push(Rule::parse("0781:5583 serial=S1").unwrap());
        let both = vec![kb.clone(), stick.clone()];
        assert!(matches!(judge(&c, &both, std::slice::from_ref(&kb)), Verdict::Fire(w) if w.starts_with("tether")));
    }

    #[test]
    fn lists_what_is_plugged_in_not_the_controllers() {
        let root = std::env::temp_dir().join(format!("ferro-usbkill-test-{}", std::process::id()));
        let mk = |name: &str, files: &[(&str, &str)]| {
            let d = root.join(name);
            std::fs::create_dir_all(&d).unwrap();
            for (f, v) in files {
                std::fs::write(d.join(f), format!("{v}\n")).unwrap();
            }
        };
        mk("usb/devices/usb1", &[("idVendor", "1d6b"), ("idProduct", "0002"), ("devnum", "1")]);
        mk(
            "usb/devices/3-5",
            &[("idVendor", "1C4F"), ("idProduct", "0002"), ("devnum", "2"), ("manufacturer", "SIGMACHIP"), ("product", "USB Keyboard")],
        );
        mk("usb/devices/3-5:1.0", &[("bInterfaceClass", "03")]);
        mk("usb/devices/3-7", &[("idVendor", "0b05"), ("idProduct", "19af"), ("devnum", "3"), ("product", "AURA"), ("serial", "987")]);
        mk("thunderbolt/devices/domain0", &[]);
        mk("thunderbolt/devices/0-0", &[("vendor", "0x8086"), ("device", "0x15ef")]);
        mk(
            "thunderbolt/devices/0-1",
            &[("vendor", "0x1"), ("device", "0x8"), ("vendor_name", "Dell"), ("device_name", "WD19TB"), ("unique_id", "u-1")],
        );
        mk("firewire/devices/fw0", &[("vendor", "0x001f11"), ("model", "0x0"), ("is_local", "1")]);
        mk("firewire/devices/fw1", &[("vendor", "0xa0b0"), ("model", "0x1"), ("guid", "0x0001"), ("model_name", "Audio")]);
        mk("firewire/devices/fw1.0", &[("specifier_id", "0x1")]);
        let found = scan(&root);
        std::fs::remove_dir_all(&root).unwrap();
        let got: Vec<(&str, &str, &str, Option<&str>)> =
            found.iter().map(|d| (d.port.as_str(), d.id.as_str(), d.name.as_str(), d.serial.as_deref())).collect();
        assert_eq!(
            got,
            [
                ("0-1", "tb:0001:0008", "Dell WD19TB", Some("u-1")),
                ("3-5", "1c4f:0002", "SIGMACHIP USB Keyboard", None),
                ("3-7", "0b05:19af", "AURA", Some("987")),
                ("fw1", "fw:00a0b0:000001", "Audio", Some("0x0001")),
            ]
        );
        assert_eq!(found[2].devnum, 3);
        // And rules for them read back as the scan writes them.
        assert_eq!(Rule::parse("tb:0x1:8 serial=u-1").map(|r| r.to_string()).as_deref(), Some("tb:0001:0008 serial=u-1"));
        assert_eq!(Rule::parse("fw:a0b0:1").unwrap().id, "fw:00a0b0:000001");
        assert!(Rule::parse("1c4f:2").is_none() && Rule::parse("tb:123456:1").is_none());
        assert!(Rule::parse("tb:0001:0008").unwrap().matches(&found[0]));
    }
}
