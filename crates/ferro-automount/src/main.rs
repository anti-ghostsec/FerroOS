//! ferro-automount: USB sticks and SD cards appear in My Computer (under
//! /media/<you>) as soon as they're plugged in. Every two seconds it looks
//! at the kernel's list of drives and asks udisks to mount anything new on
//! USB or a card reader; udisks decides how (NTFS left dirty by an unplug is
//! repaired by ntfs-3g, see harden.sh) and who may. What can't be mounted
//! (an encrypted volume, a blank card) is tried once, not again until it's
//! plugged in anew. Instead of udiskie: the same job in a few hundred KB of
//! memory rather than a Python program's 45 MB.

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
                if !removable(&name) || !candidate(&name) {
                    continue;
                }
                let size = fs::read_to_string(e.path().join("size")).unwrap_or_default().trim().to_owned();
                let key = (name.clone(), size);
                present.insert(key.clone());
                if now.contains(&name) || tried.contains(&key) {
                    continue;
                }
                tried.insert(key);
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
                    let _ = Command::new("notify-send")
                        .args(["-a", "Drives", "-i", "drive-removable-media", &format!("{label} is ready"), "Open it from My Computer."])
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .spawn();
                    // Backup, if this is the drive it backs up to by itself.
                    let _ = Command::new("ferro-backup").arg("--auto").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
                }
            }
        }
        tried.retain(|k| present.contains(k));
        std::thread::sleep(Duration::from_secs(2));
    }
}
