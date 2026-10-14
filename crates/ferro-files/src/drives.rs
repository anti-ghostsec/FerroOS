//! Dynamic storage and mount point detection for FerroOS.
//!
//! Detects all mounted disks, USB storage, optical drives, and partitions
//! across `/proc/mounts`, `/media`, and `/mnt`, assigning them classic
//! drive letters (C:, D:, E:...) and retro icons.

use ferro_gfx::icons::{self, Icon};
use ferro_path::DriveTable;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetectedDrive {
    pub letter: char,
    pub label: String,
    pub path: PathBuf,
    pub icon: Icon,
}

pub fn detect_drives() -> Vec<DetectedDrive> {
    let mut drives = Vec::new();
    let mut seen_paths = HashSet::new();
    let mut seen_devs = HashSet::new();

    // 1. C: is always the system root
    drives.push(DetectedDrive { letter: 'C', label: "Local Disk".to_string(), path: PathBuf::from("/"), icon: icons::DRIVE });
    seen_paths.insert(PathBuf::from("/"));

    let mut next_letter = b'D';
    let mut assign_letter = || -> Option<char> {
        while next_letter <= b'Z' {
            let l = next_letter as char;
            next_letter += 1;
            if l != 'C' {
                return Some(l);
            }
        }
        None
    };

    // Trigger mounting of available unmounted storage devices on Linux
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("udiskie-mount").arg("-a").output();

        if let Ok(content) = fs::read_to_string("/proc/partitions") {
            let mounted_devs: HashSet<String> = fs::read_to_string("/proc/mounts")
                .unwrap_or_default()
                .lines()
                .filter_map(|l| l.split_whitespace().next().map(str::to_string))
                .collect();

            for line in content.lines() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() < 4 {
                    continue;
                }
                let name = parts[3];
                let is_part = (name.starts_with("sd") && name.chars().any(|c| c.is_ascii_digit()))
                    || (name.starts_with("nvme") && name.contains('p'))
                    || (name.starts_with("mmcblk") && name.contains('p'));

                if is_part {
                    let dev_path = format!("/dev/{name}");
                    if !mounted_devs.contains(&dev_path) {
                        let _ = std::process::Command::new("udisksctl").args(["mount", "-b", &dev_path, "--no-user-interaction"]).output();
                    }
                }
            }
        }
    }

    // 2. Scan /proc/mounts for real mounted filesystems
    if let Ok(content) = fs::read_to_string("/proc/mounts") {
        for line in content.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 3 {
                continue;
            }
            let dev = parts[0];
            let mnt = parts[1];
            let fstype = parts[2];

            // Ignore pseudo filesystems
            if fstype == "proc"
                || fstype == "sysfs"
                || fstype == "devpts"
                || fstype == "devtmpfs"
                || fstype == "cgroup"
                || fstype == "cgroup2"
                || fstype == "pstore"
                || fstype == "bpf"
                || fstype == "configfs"
                || fstype == "debugfs"
                || fstype == "tracefs"
                || fstype == "securityfs"
                || fstype == "autofs"
                || fstype == "mqueue"
                || fstype == "hugetlbfs"
                || fstype == "efivarfs"
                || fstype == "ramfs"
                || fstype == "rpc_pipefs"
                || fstype == "fusectl"
                || fstype == "binfmt_misc"
            {
                continue;
            }

            // Exclude system directories
            if mnt == "/"
                || mnt == "/proc"
                || mnt == "/sys"
                || mnt == "/dev"
                || mnt == "/run"
                || mnt == "/tmp"
                || mnt == "/var"
                || mnt == "/boot"
                || mnt == "/boot/efi"
                || mnt == "/etc"
                || mnt == "/usr"
                || mnt.starts_with("/sys/")
                || mnt.starts_with("/dev/")
                || mnt.starts_with("/run/user/")
            {
                continue;
            }

            let path = PathBuf::from(mnt);
            if seen_paths.contains(&path) {
                continue;
            }

            // Accept real block devices or mounts under /media, /run/media, /mnt
            let is_removable_or_external = mnt.starts_with("/media")
                || mnt.starts_with("/run/media")
                || mnt.starts_with("/mnt")
                || dev.starts_with("/dev/sd")
                || dev.starts_with("/dev/nvme")
                || dev.starts_with("/dev/mmcblk")
                || dev.starts_with("/dev/sr")
                || dev.starts_with("/dev/mapper/");

            if !is_removable_or_external {
                continue;
            }

            seen_devs.insert(dev.to_string());
            if let Some(letter) = assign_letter() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("Drive").to_string();
                let is_cd = fstype == "iso9660" || fstype == "udf" || dev.contains("sr") || dev.contains("cdrom");
                let label = if is_cd {
                    "CD-ROM Drive".to_string()
                } else if !name.is_empty() && name != "mnt" && name != "media" {
                    name
                } else {
                    "Removable Disk".to_string()
                };

                seen_paths.insert(path.clone());
                drives.push(DetectedDrive { letter, label, path, icon: icons::DRIVE });
            }
        }
    }

    // 3. Scan directories under /media, /run/media, /mnt
    let candidate_dirs = [PathBuf::from("/media"), PathBuf::from("/run/media"), PathBuf::from("/mnt")];
    for base in &candidate_dirs {
        if let Ok(entries) = fs::read_dir(base) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() && !seen_paths.contains(&p) {
                    // Check if it's a user subfolder like /media/live/STICK or /media/username/STICK
                    if let Ok(subentries) = fs::read_dir(&p) {
                        let sub_dirs: Vec<PathBuf> = subentries.flatten().map(|e| e.path()).filter(|sub| sub.is_dir()).collect();
                        if !sub_dirs.is_empty() {
                            for sub in sub_dirs {
                                if !seen_paths.contains(&sub) {
                                    if let Some(letter) = assign_letter() {
                                        let name = sub.file_name().and_then(|n| n.to_str()).unwrap_or("Removable").to_string();
                                        seen_paths.insert(sub.clone());
                                        drives.push(DetectedDrive { letter, label: name, path: sub, icon: icons::DRIVE });
                                    }
                                }
                            }
                            continue;
                        }
                    }

                    if let Some(letter) = assign_letter() {
                        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("Removable").to_string();
                        seen_paths.insert(p.clone());
                        drives.push(DetectedDrive { letter, label: name, path: p, icon: icons::DRIVE });
                    }
                }
            }
        }
    }

    // 4. Scan /proc/partitions for any unmounted physical partitions (internal HDDs, SSDs, USBs)
    if let Ok(content) = fs::read_to_string("/proc/partitions") {
        for line in content.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 4 {
                continue;
            }
            let name = parts[3];
            let is_part = (name.starts_with("sd") && name.chars().any(|c| c.is_ascii_digit()))
                || (name.starts_with("nvme") && name.contains('p'))
                || (name.starts_with("mmcblk") && name.contains('p'));

            if is_part {
                let dev_path = format!("/dev/{name}");
                if !seen_devs.contains(&dev_path) {
                    if let Some(letter) = assign_letter() {
                        let label = format!("Local Disk ({name})");
                        seen_devs.insert(dev_path.clone());
                        drives.push(DetectedDrive { letter, label, path: PathBuf::from(&dev_path), icon: icons::DRIVE });
                    }
                }
            }
        }
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
}
