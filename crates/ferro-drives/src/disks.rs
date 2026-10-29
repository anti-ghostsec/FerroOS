//! Which disks Encrypt a Drive offers, and what it accepts as a name and a
//! passphrase. The same rules are checked again by ferro-drive (as root);
//! these are so the window only offers what will work, and says why not.

/// One line of `lsblk -P`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Block {
    pub name: String,
    pub parent: String,
    pub kind: String,
    pub bytes: u64,
    pub model: String,
    pub tran: String,
    pub mountpoint: String,
    pub fstype: String,
    pub label: String,
}

pub const LSBLK_COLUMNS: &str = "NAME,PKNAME,TYPE,SIZE,MODEL,TRAN,MOUNTPOINT,FSTYPE,LABEL";

/// A disk that could be encrypted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Disk {
    pub name: String,
    pub bytes: u64,
    pub model: String,
    pub usb: bool,
    /// Its partitions mounted under /media (a USB stick opened in My
    /// Computer): closed before it's erased.
    pub to_unmount: Vec<String>,
}

impl Disk {
    pub fn line(&self) -> String {
        let model = if self.model.is_empty() { "Disk" } else { &self.model };
        let usb = if self.usb { " (USB)" } else { "" };
        format!("{:<9} {:>9}  {model}{usb}", self.name, size_text(self.bytes))
    }
}

/// 931.5 GB, 14.9 GB, 512 MB.
pub fn size_text(bytes: u64) -> String {
    let gb = bytes as f64 / 1_073_741_824.0;
    if gb >= 1.0 {
        format!("{gb:.1} GB")
    } else {
        format!("{} MB", bytes / 1_048_576)
    }
}

/// lsblk -P's KEY="value" pairs (values may hold spaces and \xNN escapes).
pub fn parse_lsblk(text: &str) -> Vec<Block> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut b = Block::default();
        let mut rest = line.trim();
        while let Some((key, after)) = rest.split_once("=\"") {
            let Some(end) = after.find('"') else { break };
            let value = unescape(&after[..end]);
            match key.trim() {
                "NAME" => b.name = value,
                "PKNAME" => b.parent = value,
                "TYPE" => b.kind = value,
                "SIZE" => b.bytes = value.parse().unwrap_or(0),
                "MODEL" => b.model = value.trim().to_owned(),
                "TRAN" => b.tran = value,
                "MOUNTPOINT" => b.mountpoint = value,
                "FSTYPE" => b.fstype = value,
                "LABEL" => b.label = value,
                _ => {}
            }
            rest = &after[end + 1..];
        }
        if !b.name.is_empty() {
            out.push(b);
        }
    }
    out
}

fn unescape(v: &str) -> String {
    let mut out = String::new();
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            let code: String = chars.by_ref().skip(1).take(2).collect();
            if let Ok(n) = u8::from_str_radix(&code, 16) {
                out.push(n as char);
                continue;
            }
        }
        out.push(c);
    }
    out
}

/// Where FerroOS itself is: a disk holding one of these is never offered.
fn is_system_mount(m: &str) -> bool {
    matches!(m, "/" | "/boot" | "/boot/efi" | "/efi" | "[SWAP]") || m.starts_with("/run/live")
}

/// The disk `b` is on (itself if it's one), through partitions and
/// encryption.
fn disk_of<'a>(blocks: &'a [Block], b: &'a Block) -> &'a Block {
    let mut at = b;
    for _ in 0..8 {
        match blocks.iter().find(|p| p.name == at.parent) {
            Some(p) => at = p,
            None => break,
        }
    }
    at
}

/// The disks that can be offered: not FerroOS's own, nothing on them in use
/// except what's open from /media (that's closed first).
pub fn offered(blocks: &[Block]) -> Vec<Disk> {
    let mut out = Vec::new();
    // Not floppies, not empty card readers: a disk of 64 MB or more.
    for d in blocks.iter().filter(|b| b.kind == "disk" && b.bytes >= 64 << 20 && !b.name.starts_with("fd")) {
        let on_it: Vec<&Block> = blocks.iter().filter(|b| b.name != d.name && disk_of(blocks, b).name == d.name).collect();
        let all = std::iter::once(d).chain(on_it.iter().copied());
        if all.clone().any(|b| is_system_mount(&b.mountpoint)) {
            continue;
        }
        // Open encryption, LVM or RAID on it: in use.
        if on_it.iter().any(|b| !matches!(b.kind.as_str(), "part")) {
            continue;
        }
        let mounted: Vec<&Block> = all.filter(|b| !b.mountpoint.is_empty()).collect();
        if mounted.iter().any(|b| !b.mountpoint.starts_with("/media/")) {
            continue;
        }
        out.push(Disk {
            name: d.name.clone(),
            bytes: d.bytes,
            model: d.model.clone(),
            usb: d.tran == "usb",
            to_unmount: mounted.iter().map(|b| format!("/dev/{}", b.name)).collect(),
        });
    }
    out
}

/// What's wrong with the name, if anything (ext4 allows 16 bytes).
pub fn check_name(name: &str) -> Result<(), &'static str> {
    if name.is_empty() {
        return Err("Give the drive a name.");
    }
    if name.len() > 16 {
        return Err("The name can be at most 16 characters.");
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '_')) {
        return Err("The name can have letters, digits, spaces, - and _.");
    }
    Ok(())
}

pub fn check_passphrase(pass: &str, again: &str) -> Result<(), &'static str> {
    if pass.chars().count() < 12 {
        return Err("The passphrase needs at least 12 characters.");
    }
    if pass != again {
        return Err("The two passphrases aren't the same.");
    }
    Ok(())
}

/// "Unlocked /dev/sdb1 as /dev/dm-3." -> /dev/dm-3
pub fn unlocked_as(said: &str) -> Option<String> {
    let dev = said.trim().rsplit_once(" as ")?.1.trim_end_matches('.');
    dev.starts_with("/dev/").then(|| dev.to_owned())
}

/// "Mounted /dev/dm-3 at /media/you/Matrix" -> /media/you/Matrix
pub fn mounted_at(said: &str) -> Option<String> {
    let at = said.trim().rsplit_once(" at ")?.1.trim_end_matches('.');
    at.starts_with('/').then(|| at.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = r#"NAME="nvme0n1" PKNAME="" TYPE="disk" SIZE="256060514304" MODEL="SAMSUNG MZVLB256" TRAN="nvme" MOUNTPOINT="" FSTYPE="" LABEL=""
NAME="nvme0n1p1" PKNAME="nvme0n1" TYPE="part" SIZE="536870912" MODEL="" TRAN="nvme" MOUNTPOINT="/boot/efi" FSTYPE="vfat" LABEL=""
NAME="nvme0n1p3" PKNAME="nvme0n1" TYPE="part" SIZE="254000000000" MODEL="" TRAN="nvme" MOUNTPOINT="" FSTYPE="crypto_LUKS" LABEL=""
NAME="ferro-system" PKNAME="nvme0n1p3" TYPE="crypt" SIZE="254000000000" MODEL="" TRAN="" MOUNTPOINT="/" FSTYPE="ext4" LABEL=""
NAME="sda" PKNAME="" TYPE="disk" SIZE="1000204886016" MODEL="Samsung SSD 870" TRAN="sata" MOUNTPOINT="" FSTYPE="" LABEL=""
NAME="sdb" PKNAME="" TYPE="disk" SIZE="30943995904" MODEL="SD/MMC" TRAN="usb" MOUNTPOINT="" FSTYPE="" LABEL=""
NAME="sdb1" PKNAME="sdb" TYPE="part" SIZE="30943995904" MODEL="" TRAN="usb" MOUNTPOINT="/media/you/STICK" FSTYPE="vfat" LABEL="STICK"
NAME="sdc" PKNAME="" TYPE="disk" SIZE="500107862016" MODEL="Old\x20Disk" TRAN="sata" MOUNTPOINT="" FSTYPE="" LABEL=""
NAME="sdc1" PKNAME="sdc" TYPE="part" SIZE="500107862016" MODEL="" TRAN="sata" MOUNTPOINT="/srv/data" FSTYPE="ext4" LABEL=""
NAME="sdd" PKNAME="" TYPE="disk" SIZE="1000204886016" MODEL="Vault" TRAN="usb" MOUNTPOINT="" FSTYPE="" LABEL=""
NAME="sdd1" PKNAME="sdd" TYPE="part" SIZE="1000204886016" MODEL="" TRAN="usb" MOUNTPOINT="" FSTYPE="crypto_LUKS" LABEL="Vault"
NAME="luks-1234" PKNAME="sdd1" TYPE="crypt" SIZE="1000204886016" MODEL="" TRAN="" MOUNTPOINT="/media/you/Vault" FSTYPE="ext4" LABEL="Vault""#;

    #[test]
    fn offers_only_disks_that_are_free() {
        let disks = offered(&parse_lsblk(LIST));
        let names: Vec<&str> = disks.iter().map(|d| d.name.as_str()).collect();
        // Not the system disk, not one mounted outside /media, not one
        // whose encryption is open.
        assert_eq!(names, ["sda", "sdb"]);
        assert_eq!(disks[1].to_unmount, ["/dev/sdb1"]);
        assert!(disks[1].usb && !disks[0].usb);
        assert_eq!(disks[0].line(), "sda        931.5 GB  Samsung SSD 870");
    }

    #[test]
    fn names_and_passphrases() {
        assert!(check_name("Matrix").is_ok());
        assert!(check_name("my projects_1").is_ok());
        assert!(check_name("").is_err());
        assert!(check_name("seventeen-chars!!").is_err());
        assert!(check_name("a/b").is_err());
        assert!(check_passphrase("short", "short").is_err());
        assert!(check_passphrase("twelve chars", "twelve chars").is_ok());
        assert!(check_passphrase("twelve chars", "twelve charz").is_err());
    }

    #[test]
    fn reads_what_udisks_says() {
        assert_eq!(unlocked_as("Unlocked /dev/sdb1 as /dev/dm-3.\n").as_deref(), Some("/dev/dm-3"));
        assert_eq!(mounted_at("Mounted /dev/dm-3 at /media/you/Matrix\n").as_deref(), Some("/media/you/Matrix"));
        assert_eq!(unlocked_as("Error unlocking"), None);
    }
}
