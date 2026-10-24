//! The drives "My Computer" shows: `C:` for the system, then one letter for
//! each real filesystem on a disk, USB stick, memory card or CD in the
//! machine, mounted so it opens like any folder.
//!
//! What's there comes from `lsblk`, which asks the kernel, so a drive is only
//! shown when it exists. Partitions that hold no files people use (EFI,
//! swap, Windows' reserved and recovery partitions, encrypted containers that
//! are still locked) and the medium FerroOS was started from are left out.

use ferro_gfx::icons::{self, Icon};
use ferro_path::DriveTable;
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetectedDrive {
    pub letter: char,
    pub label: String,
    pub path: PathBuf,
    pub icon: Icon,
}

/// One line of `lsblk -P`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Block {
    name: String,
    parent: String,
    kind: String,
    fstype: String,
    parttype: String,
    label: String,
    size: String,
    removable: bool,
    mountpoint: String,
}

const LSBLK_COLUMNS: &str = "NAME,PKNAME,TYPE,FSTYPE,PARTTYPE,LABEL,SIZE,RM,HOTPLUG,MOUNTPOINT";

/// Partition types that never hold people's files: EFI system, BIOS boot,
/// Microsoft reserved, Windows recovery, Linux swap.
const HIDDEN_PARTTYPES: [&str; 6] = [
    "c12a7328-f81f-11d2-ba4b-00a0c93ec93b",
    "21686148-6449-6e6f-744e-656564454649",
    "e3c9e316-0b5c-4db8-817d-f92df00215ae",
    "de94bba4-06d1-4d40-a16a-bfd50179d6ac",
    "0657fd6d-a4ab-43c4-84e5-0933c84b4f4f",
    "0xef",
];

/// Filesystems that aren't folders: swap, encrypted or RAID/LVM containers.
const NOT_FOLDERS: [&str; 7] = ["swap", "crypto_LUKS", "LVM2_member", "linux_raid_member", "BitLocker", "zfs_member", "squashfs"];

/// Where the system itself is mounted: already `C:`, or not for browsing.
fn is_system_mount(m: &str) -> bool {
    matches!(m, "/" | "/boot" | "/boot/efi" | "/efi" | "/usr" | "/var" | "/home" | "[SWAP]")
        || m.starts_with("/run/live")
        || m.starts_with("/lib/live")
        || m.starts_with("/opt/")
        || m.starts_with("/snap/")
}

/// `\x20`-style escapes lsblk uses inside its quoted values.
fn unescape(v: &str) -> String {
    let mut out = Vec::with_capacity(v.len());
    let b = v.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() && b[i + 1] == b'x' && b[i + 2].is_ascii_hexdigit() && b[i + 3].is_ascii_hexdigit() {
            if let Ok(n) = u8::from_str_radix(&v[i + 2..i + 4], 16) {
                out.push(n);
                i += 4;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_lsblk(text: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    for line in text.lines() {
        let mut b = Block::default();
        let mut rest = line.trim();
        while let Some(eq) = rest.find("=\"") {
            let key = rest[..eq].trim();
            let after = &rest[eq + 2..];
            let Some(end) = after.find('"') else { break };
            let value = unescape(&after[..end]);
            rest = &after[end + 1..];
            match key {
                "NAME" => b.name = value,
                "PKNAME" => b.parent = value,
                "TYPE" => b.kind = value,
                "FSTYPE" => b.fstype = value,
                "PARTTYPE" => b.parttype = value.to_ascii_lowercase(),
                "LABEL" => b.label = value,
                "SIZE" => b.size = value,
                "RM" | "HOTPLUG" => b.removable |= value == "1",
                "MOUNTPOINT" => b.mountpoint = value,
                _ => {}
            }
        }
        if !b.name.is_empty() {
            blocks.push(b);
        }
    }
    blocks
}

/// The disks (by name) that FerroOS itself runs from: the installation
/// medium in a live session, the system disk on an installed one. Their
/// other partitions (boot, EFI) are part of the system, not drives.
fn system_disks(blocks: &[Block]) -> Vec<String> {
    let mut disks = Vec::new();
    for b in blocks.iter().filter(|b| b.mountpoint.starts_with("/run/live") || matches!(b.mountpoint.as_str(), "/" | "/boot" | "/boot/efi")) {
        // Walk up through partitions and encryption to the disk itself.
        let mut cur = b;
        while let Some(p) = blocks.iter().find(|p| p.name == cur.parent) {
            cur = p;
        }
        if !disks.contains(&cur.name) {
            disks.push(cur.name.clone());
        }
    }
    disks
}

fn top_disk<'a>(blocks: &'a [Block], b: &'a Block) -> &'a Block {
    let mut cur = b;
    while let Some(p) = blocks.iter().find(|p| p.name == cur.parent) {
        cur = p;
    }
    cur
}

/// Which blocks are drives worth a letter, whether mounted yet or not.
fn candidates(blocks: &[Block]) -> Vec<&Block> {
    let system = system_disks(blocks);
    blocks
        .iter()
        .filter(|b| {
            if !matches!(b.kind.as_str(), "part" | "disk" | "crypt" | "rom" | "lvm") || b.name.contains("/zram") {
                return false;
            }
            if !b.mountpoint.is_empty() {
                return !is_system_mount(&b.mountpoint);
            }
            // Not mounted: only what we know is a filesystem people use.
            if b.fstype.is_empty() || NOT_FOLDERS.contains(&b.fstype.as_str()) || HIDDEN_PARTTYPES.contains(&b.parttype.as_str()) {
                return false;
            }
            // The system's own disk: its other partitions are boot and EFI.
            !system.contains(&top_disk(blocks, b).name)
        })
        .collect()
}

fn lsblk() -> Option<Vec<Block>> {
    let out = std::process::Command::new("lsblk").args(["-P", "-p", "-o", LSBLK_COLUMNS]).output().ok()?;
    out.status.success().then(|| parse_lsblk(&String::from_utf8_lossy(&out.stdout)))
}

fn label_for(b: &Block, removable: bool) -> String {
    let base = if b.kind == "rom" || matches!(b.fstype.as_str(), "iso9660" | "udf") {
        "CD-ROM Drive".to_owned()
    } else if !b.label.is_empty() {
        b.label.clone()
    } else if removable {
        "Removable Disk".to_owned()
    } else {
        "Local Disk".to_owned()
    };
    if b.size.is_empty() {
        base
    } else {
        format!("{base} {}", b.size)
    }
}

/// Network shares and the like that are mounted under /media or /mnt but
/// aren't block devices, from the kernel's list of mounts (so only ones that
/// really are mounted).
fn other_mounts(known: &[PathBuf]) -> Vec<(String, PathBuf)> {
    let mut found = Vec::new();
    for line in fs::read_to_string("/proc/mounts").unwrap_or_default().lines() {
        let mut f = line.split_whitespace();
        let (Some(_dev), Some(mnt), Some(fstype)) = (f.next(), f.next(), f.next()) else { continue };
        let mnt = unescape(&mnt.replace("\\040", " "));
        let shared = matches!(fstype, "cifs" | "smb3" | "nfs" | "nfs4" | "sshfs" | "fuse.sshfs" | "davfs" | "fuse.rclone");
        let path = PathBuf::from(&mnt);
        if shared && (mnt.starts_with("/media/") || mnt.starts_with("/mnt/") || mnt.starts_with("/run/media/")) && !known.contains(&path) {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("Network Drive").to_owned();
            found.push((name, path));
        }
    }
    found
}

pub fn detect_drives() -> Vec<DetectedDrive> {
    let mut drives = vec![DetectedDrive { letter: 'C', label: "Local Disk".to_string(), path: PathBuf::from("/"), icon: icons::DRIVE }];
    let mut letters = (b'D'..=b'Z').map(char::from);

    let mut blocks = lsblk().unwrap_or_default();
    // Mount what isn't yet, so every drive opens. udisks puts it under
    // /media/<user>/<label>; accounts in the sudo group may (harden.sh).
    #[cfg(unix)]
    {
        let unmounted: Vec<String> = candidates(&blocks).iter().filter(|b| b.mountpoint.is_empty()).map(|b| b.name.clone()).collect();
        if !unmounted.is_empty() {
            let mount = |dev: &str, extra: &[&str]| {
                std::process::Command::new("udisksctl")
                    .args(["mount", "-b", dev, "--no-user-interaction"])
                    .args(extra)
                    .stdin(std::process::Stdio::null())
                    .output()
                    .is_ok_and(|o| o.status.success())
            };
            for dev in &unmounted {
                // A Windows drive left "in use" by Fast Startup or
                // hibernation won't mount for writing; read-only, its files
                // can at least be opened and copied.
                if !mount(dev, &[]) {
                    mount(dev, &["-o", "ro"]);
                }
            }
            blocks = lsblk().unwrap_or(blocks);
        }
    }

    let mut known = Vec::new();
    for b in candidates(&blocks) {
        // One that couldn't be mounted (damaged, or a filesystem this
        // system can't read) would open as nothing; it isn't shown.
        if b.mountpoint.is_empty() {
            continue;
        }
        let Some(letter) = letters.next() else { break };
        let removable = b.removable || top_disk(&blocks, b).removable;
        let path = PathBuf::from(&b.mountpoint);
        known.push(path.clone());
        drives.push(DetectedDrive { letter, label: label_for(b, removable), path, icon: icons::DRIVE });
    }
    for (label, path) in other_mounts(&known) {
        let Some(letter) = letters.next() else { break };
        drives.push(DetectedDrive { letter, label, path, icon: icons::DRIVE });
    }
    drives
}

pub fn system_drives_with_detected() -> (DriveTable, Vec<DetectedDrive>) {
    let heavy = fs::read_to_string("/etc/os-release").is_ok_and(|r| r.lines().any(|l| l == "ID=ferroos-heavy"));
    let mut table = if heavy { DriveTable::heavy() } else { DriveTable::default() };

    let detected = detect_drives();
    for d in &detected {
        if d.letter != 'C' {
            table.mount(d.letter, &d.path.to_string_lossy());
        }
    }

    (table, detected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn c_drive_is_always_present() {
        let drives = detect_drives();
        assert!(!drives.is_empty());
        assert_eq!(drives[0].letter, 'C');
        assert_eq!(drives[0].path, Path::new("/"));
    }

    #[test]
    fn system_drives_mounts_all_detected() {
        let (table, detected) = system_drives_with_detected();
        for d in detected {
            assert!(table.root(d.letter).is_some());
        }
    }

    /// A live session from a USB stick (sdb) on a laptop with Windows on
    /// its NVMe disk and a second data disk (sda), plus a phone card.
    const LIVE: &str = r#"NAME="/dev/loop0" PKNAME="" TYPE="loop" FSTYPE="squashfs" PARTTYPE="" LABEL="" SIZE="870M" RM="0" HOTPLUG="0" MOUNTPOINT="/run/live/rootfs/filesystem.squashfs"
NAME="/dev/sda" PKNAME="" TYPE="disk" FSTYPE="" PARTTYPE="" LABEL="" SIZE="1.8T" RM="0" HOTPLUG="0" MOUNTPOINT=""
NAME="/dev/sda1" PKNAME="/dev/sda" TYPE="part" FSTYPE="ext4" PARTTYPE="0fc63daf-8483-4772-8e79-3d69d8477de4" LABEL="Data" SIZE="1.8T" RM="0" HOTPLUG="0" MOUNTPOINT=""
NAME="/dev/sdb" PKNAME="" TYPE="disk" FSTYPE="iso9660" PARTTYPE="" LABEL="FERROHEAVY" SIZE="28.7G" RM="1" HOTPLUG="1" MOUNTPOINT=""
NAME="/dev/sdb1" PKNAME="/dev/sdb" TYPE="part" FSTYPE="iso9660" PARTTYPE="0x0" LABEL="FERROHEAVY" SIZE="875M" RM="1" HOTPLUG="1" MOUNTPOINT="/run/live/medium"
NAME="/dev/sdb2" PKNAME="/dev/sdb" TYPE="part" FSTYPE="vfat" PARTTYPE="0xef" LABEL="FERROHEAVY" SIZE="8M" RM="1" HOTPLUG="1" MOUNTPOINT=""
NAME="/dev/nvme0n1" PKNAME="" TYPE="disk" FSTYPE="" PARTTYPE="" LABEL="" SIZE="476.9G" RM="0" HOTPLUG="0" MOUNTPOINT=""
NAME="/dev/nvme0n1p1" PKNAME="/dev/nvme0n1" TYPE="part" FSTYPE="vfat" PARTTYPE="c12a7328-f81f-11d2-ba4b-00a0c93ec93b" LABEL="SYSTEM" SIZE="100M" RM="0" HOTPLUG="0" MOUNTPOINT=""
NAME="/dev/nvme0n1p2" PKNAME="/dev/nvme0n1" TYPE="part" FSTYPE="" PARTTYPE="e3c9e316-0b5c-4db8-817d-f92df00215ae" LABEL="" SIZE="16M" RM="0" HOTPLUG="0" MOUNTPOINT=""
NAME="/dev/nvme0n1p3" PKNAME="/dev/nvme0n1" TYPE="part" FSTYPE="ntfs" PARTTYPE="ebd0a0a2-b9e5-4433-87c0-68b6b72699c7" LABEL="Windows\x20Disk" SIZE="475G" RM="0" HOTPLUG="0" MOUNTPOINT="/media/live/Windows Disk"
NAME="/dev/nvme0n1p4" PKNAME="/dev/nvme0n1" TYPE="part" FSTYPE="ntfs" PARTTYPE="de94bba4-06d1-4d40-a16a-bfd50179d6ac" LABEL="" SIZE="800M" RM="0" HOTPLUG="0" MOUNTPOINT=""
NAME="/dev/mmcblk0" PKNAME="" TYPE="disk" FSTYPE="" PARTTYPE="" LABEL="" SIZE="59.5G" RM="1" HOTPLUG="1" MOUNTPOINT=""
NAME="/dev/mmcblk0p1" PKNAME="/dev/mmcblk0" TYPE="part" FSTYPE="exfat" PARTTYPE="0x7" LABEL="" SIZE="59.5G" RM="1" HOTPLUG="1" MOUNTPOINT=""
NAME="/dev/zram0" PKNAME="" TYPE="disk" FSTYPE="swap" PARTTYPE="" LABEL="" SIZE="4G" RM="0" HOTPLUG="0" MOUNTPOINT="[SWAP]""#;

    #[test]
    fn real_drives_only() {
        let blocks = parse_lsblk(LIVE);
        let names: Vec<&str> = candidates(&blocks).iter().map(|b| b.name.as_str()).collect();
        // The data disk, Windows, and the memory card; not the USB stick
        // FerroOS runs from, EFI, Microsoft reserved, recovery or swap.
        assert_eq!(names, ["/dev/sda1", "/dev/nvme0n1p3", "/dev/mmcblk0p1"]);
    }

    #[test]
    fn labels_say_what_it_is() {
        let blocks = parse_lsblk(LIVE);
        let win = blocks.iter().find(|b| b.name == "/dev/nvme0n1p3").unwrap();
        assert_eq!(win.mountpoint, "/media/live/Windows Disk");
        assert_eq!(label_for(win, false), "Windows Disk 475G");
        let card = blocks.iter().find(|b| b.name == "/dev/mmcblk0p1").unwrap();
        assert_eq!(label_for(card, top_disk(&blocks, card).removable), "Removable Disk 59.5G");
    }

    /// An installed, encrypted FerroOS: its boot and EFI partitions and the
    /// opened system volume are all C:, a USB stick is the one drive.
    #[test]
    fn installed_system_disk_is_c() {
        let text = r#"NAME="/dev/nvme0n1" PKNAME="" TYPE="disk" FSTYPE="" PARTTYPE="" LABEL="" SIZE="476.9G" RM="0" HOTPLUG="0" MOUNTPOINT=""
NAME="/dev/nvme0n1p1" PKNAME="/dev/nvme0n1" TYPE="part" FSTYPE="vfat" PARTTYPE="c12a7328-f81f-11d2-ba4b-00a0c93ec93b" LABEL="" SIZE="512M" RM="0" HOTPLUG="0" MOUNTPOINT="/boot/efi"
NAME="/dev/nvme0n1p2" PKNAME="/dev/nvme0n1" TYPE="part" FSTYPE="ext4" PARTTYPE="0fc63daf-8483-4772-8e79-3d69d8477de4" LABEL="ferro-boot" SIZE="1G" RM="0" HOTPLUG="0" MOUNTPOINT="/boot"
NAME="/dev/nvme0n1p3" PKNAME="/dev/nvme0n1" TYPE="part" FSTYPE="crypto_LUKS" PARTTYPE="ca7d7ccb-63ed-4c53-861c-1742536059cc" LABEL="" SIZE="475.4G" RM="0" HOTPLUG="0" MOUNTPOINT=""
NAME="/dev/mapper/ferro-system" PKNAME="/dev/nvme0n1p3" TYPE="crypt" FSTYPE="ext4" PARTTYPE="" LABEL="ferro-system" SIZE="475.4G" RM="0" HOTPLUG="0" MOUNTPOINT="/"
NAME="/dev/sdc" PKNAME="" TYPE="disk" FSTYPE="" PARTTYPE="" LABEL="" SIZE="14.6G" RM="1" HOTPLUG="1" MOUNTPOINT=""
NAME="/dev/sdc1" PKNAME="/dev/sdc" TYPE="part" FSTYPE="vfat" PARTTYPE="0xc" LABEL="STICK" SIZE="14.6G" RM="1" HOTPLUG="1" MOUNTPOINT="/media/ana/STICK""#;
        let blocks = parse_lsblk(text);
        let names: Vec<&str> = candidates(&blocks).iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["/dev/sdc1"]);
    }

    #[test]
    fn unescapes_lsblk_values() {
        assert_eq!(unescape(r"My\x20Files"), "My Files");
        assert_eq!(unescape(r"plain"), "plain");
        assert_eq!(unescape(r"end\x2"), r"end\x2");
        assert_eq!(unescape("\\xé!"), "\\xé!");
    }
}
