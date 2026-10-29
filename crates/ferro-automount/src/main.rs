//! ferro-automount: USB sticks and SD cards appear in My Computer (under
//! /media/<you>) as soon as they're plugged in. Every two seconds it looks
//! at the kernel's list of drives and asks udisks to mount anything new on
//! USB or a card reader; udisks decides how (NTFS left dirty by an unplug is
//! repaired by ntfs-3g, see harden.sh) and who may. What can't be mounted
//! (an encrypted volume, a blank card) is tried once, not again until it's
//! plugged in anew. Instead of udiskie: the same job in a few hundred KB of
//! memory rather than a Python program's 45 MB.
//!
//! Encrypted drives (LUKS), on USB or inside the PC, that are still locked:
//! one this PC keeps a key for (Unlock automatically, see ferro-drive) is
//! opened with it; any other gets the box that asks for its passphrase
//! (ferro-drives --unlock), once, when it's plugged in or at logon. The
//! system's own encrypted disk is open already, so it's never asked about.

use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

/// Drives on USB or a card reader (by where the kernel found them), never
/// the built-in disk, a loop file or the encrypted system itself.
fn removable(name: &str) -> bool {
    if ["loop", "ram", "zram", "dm-", "md", "sr", "nvme"].iter().any(|p| name.starts_with(p)) {
        return false;
    }
    let Ok(real) = fs::canonicalize(Path::new("/sys/class/block").join(name)) else { return false };
    let real = real.to_string_lossy();
    real.contains("/usb") || real.contains("/mmc")
}

/// Something that could hold a file system: a partition, or a whole disk
/// with none (a stick formatted as one big volume), with room on it.
fn candidate(name: &str) -> bool {
    let dir = Path::new("/sys/class/block").join(name);
    let size = fs::read_to_string(dir.join("size")).ok().and_then(|s| s.trim().parse::<u64>().ok()).unwrap_or(0);
    if size == 0 {
        return false;
    }
    if dir.join("partition").exists() {
        return true;
    }
    // A whole disk: only if it has no partitions of its own.
    !fs::read_dir(&dir).map(|d| d.flatten().any(|e| e.file_name().to_string_lossy().starts_with(name))).unwrap_or(false)
}

/// A locked LUKS volume's id: an encrypted partition (or whole disk) that
/// nothing has opened yet. What udev found is in its database.
fn locked_luks(name: &str) -> Option<String> {
    if ["loop", "ram", "zram", "dm-", "md", "sr"].iter().any(|p| name.starts_with(p)) {
        return None;
    }
    let dir = Path::new("/sys/class/block").join(name);
    let dev = fs::read_to_string(dir.join("dev")).ok()?;
    let db = fs::read_to_string(format!("/run/udev/data/b{}", dev.trim())).ok()?;
    if !db.lines().any(|l| l == "E:ID_FS_TYPE=crypto_LUKS") {
        return None;
    }
    // Open already (the system disk, or one opened at startup).
    if fs::read_dir(dir.join("holders")).map(|mut d| d.next().is_some()).unwrap_or(true) {
        return None;
    }
    db.lines().find_map(|l| l.strip_prefix("E:ID_FS_UUID=")).map(str::to_owned)
}

/// Being encrypted right now: ferro-drive marks the disk it works on, and
/// it's left alone (not even counted as tried) until that's done.
fn busy(name: &str) -> bool {
    let dir = Path::new("/sys/class/block").join(name);
    let disk = if dir.join("partition").exists() {
        fs::canonicalize(&dir).ok().and_then(|p| p.parent().and_then(|d| d.file_name()).map(|d| d.to_string_lossy().into_owned()))
    } else {
        Some(name.to_owned())
    };
    disk.is_some_and(|d| Path::new("/run/ferro-drive").join(format!("busy-{d}")).exists())
}

/// Starts `program` without waiting for it (nor leaving it a zombie).
fn start(program: &str, args: &[&str]) -> Option<std::process::Child> {
    Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().ok()
}

fn notify(title: &str, body: &str) {
    if let Some(mut child) = start("notify-send", &["-a", "Drives", "-i", "drive-removable-media", title, body]) {
        std::thread::spawn(move || child.wait());
    }
}

/// An encrypted drive: with this PC's key if it keeps one (no password:
/// ferro-drive's `open` is allowed without, see harden.sh), else the box.
fn unlock(name: &str, uuid: &str) {
    let (name, uuid) = (name.to_owned(), uuid.to_owned());
    std::thread::spawn(move || {
        let opened = Command::new("sudo")
            .args(["-n", "/usr/local/sbin/ferro-drive", "open", &uuid])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success());
        match opened {
            Some(o) => {
                let at = String::from_utf8_lossy(&o.stdout).trim().to_owned();
                let label = at.rsplit('/').next().unwrap_or("The drive").to_owned();
                notify(&format!("{label} is ready"), &format!("It unlocked by itself, at {at}."));
            }
            None => {
                if let Some(mut child) = start("ferro-drives", &["--unlock", &format!("/dev/{name}")]) {
                    let _ = child.wait();
                }
            }
        }
    });
}

/// The devices mounted now, as /dev/NAME.
fn mounted() -> HashSet<String> {
    fs::read_to_string("/proc/self/mounts")
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .filter_map(|d| d.strip_prefix("/dev/"))
        .map(str::to_owned)
        .collect()
}

fn main() {
    // What's been tried, as name and size: tried once until it goes away
    // (or another card goes in the same reader, with another size).
    let mut tried: HashSet<(String, String)> = HashSet::new();
    loop {
        let now = mounted();
        let mut present = HashSet::new();
        if let Ok(entries) = fs::read_dir("/sys/class/block") {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if !candidate(&name) || busy(&name) {
                    continue;
                }
                let luks = locked_luks(&name);
                if luks.is_none() && !removable(&name) {
                    continue;
                }
                let size = fs::read_to_string(e.path().join("size")).unwrap_or_default().trim().to_owned();
                let key = (name.clone(), size);
                present.insert(key.clone());
                if now.contains(&name) || tried.contains(&key) {
                    continue;
                }
                tried.insert(key);
                if let Some(uuid) = luks {
                    unlock(&name, &uuid);
                    continue;
                }
                let out = Command::new("udisksctl")
                    .args(["mount", "--no-user-interaction", "-b"])
                    .arg(format!("/dev/{name}"))
                    .stdin(Stdio::null())
                    .stderr(Stdio::null())
                    .output();
                // "Mounted /dev/sdb1 at /media/nully/STICK": said, as Windows does.
                if let Some(out) = out.ok().filter(|o| o.status.success()) {
                    let said = String::from_utf8_lossy(&out.stdout);
                    let place = said.trim().rsplit_once(" at ").map_or(String::new(), |(_, at)| at.trim_end_matches('.').to_owned());
                    let label = place.rsplit('/').next().filter(|l| !l.is_empty()).unwrap_or(&name).to_owned();
                    notify(&format!("{label} is ready"), "Open it from My Computer.");
                    // Backup, if this is the drive it backs up to by itself.
                    let _ = Command::new("ferro-backup").arg("--auto").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
                }
            }
        }
        tried.retain(|k| present.contains(k));
        std::thread::sleep(Duration::from_secs(2));
    }
}
