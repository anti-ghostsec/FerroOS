//! Restricted folders: a folder's Properties > Restricted. Ticked, the
//! folder is given to root (root:root, 0700), so nothing running as you can
//! open it: not you in File Explorer, and not a program or AI assistant
//! running as you. Unticked, it's yours again, as it was.
//!
//! Why root, and not just taking its permissions away (chmod 000): the owner
//! can always give them back, so anything running as you could too. Root's
//! folder it can't touch. The change goes through `ferro-restrict` (the
//! binary of this crate, in /usr/local/sbin), run with sudo, which asks for
//! your password, both ways. There's no NOPASSWD rule for it: one for unlock
//! would let any program running as you undo it.
//!
//! What it doesn't stop: anything that can run sudo with your password, or
//! is root. It protects against programs running as you that don't know your
//! password, which on FerroOS includes the Claude desktop app (it runs with
//! no_new_privs, so sudo can't raise it however it's asked). Files inside
//! keep their owner: one with another hard link outside, or already open
//! before the folder was restricted, can still be read that way.
//!
//! What the folder was (owner, group, mode) is kept in a root-owned record,
//! /var/lib/ferro/restricted/<device>-<inode>: it follows the folder when it
//! is moved or renamed, and only the folder it was made for matches it.

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

/// The root helper: `ferro-restrict lock PATH` or `unlock PATH`, via sudo.
pub const HELPER: &str = "/usr/local/sbin/ferro-restrict";
/// Where what each restricted folder was is kept. Others may look up a
/// record (`is_restricted`), not list them: 0711.
pub const RECORDS: &str = "/var/lib/ferro/restricted";
/// FerroOS's password box for sudo, when it's there.
const ASKPASS: &str = "/usr/local/bin/ferro-askpass";
/// By its full path: not whatever `sudo` is first on a PATH someone changed.
const SUDO: &str = "/usr/bin/sudo";

/// Home folders the desktop itself needs: restricted, the next logon breaks.
const DESKTOP_NEEDS: [&str; 3] = [".config", ".local", ".cache"];

/// What a folder was before it was restricted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record {
    pub uid: u32,
    pub gid: u32,
    /// Permission bits, setgid and sticky included (`& 0o7777`).
    pub mode: u32,
}

impl Record {
    /// `uid gid mode(octal)`, then the folder's path then, for whoever reads it.
    pub fn to_text(&self, path: &Path) -> String {
        format!("{} {} {:o}\n{}\n", self.uid, self.gid, self.mode, path.display())
    }

    pub fn parse(text: &str) -> Option<Record> {
        let mut words = text.lines().next()?.split_whitespace();
        let uid = words.next()?.parse().ok()?;
        let gid = words.next()?.parse().ok()?;
        let mode = u32::from_str_radix(words.next()?, 8).ok()?;
        (words.next().is_none() && mode <= 0o7777).then_some(Record { uid, gid, mode })
    }
}

/// The record for the folder on device `dev` with inode `ino`.
pub fn record_path(dev: u64, ino: u64) -> PathBuf {
    Path::new(RECORDS).join(format!("{dev:x}-{ino:x}"))
}

/// Whether `path` is a restricted folder: root's, and recorded as restricted
/// (not just any folder of root's). Needs no privileges.
pub fn is_restricted(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(path).is_ok_and(|m| m.is_dir() && m.uid() == 0 && record_path(m.dev(), m.ino()).exists())
}

/// Whether `real` (absolute, links resolved) may be restricted by the user
/// `user` whose home is `home`: a folder inside the home (not the home
/// itself, nor the folders the desktop needs), or a folder on a drive
/// mounted under /media/<user> (not the whole drive). Nothing else: no
/// system folder, no other user's.
pub fn check_place(real: &Path, home: &Path, user: &str) -> Result<(), String> {
    let plain = |p: &Path| p.is_absolute() && p.components().all(|c| matches!(c, Component::RootDir | Component::Normal(_)));
    if !plain(real) {
        return Err(format!("'{}' isn't a full path.", real.display()));
    }
    // A home of / (or /home) would let anything through.
    if !plain(home) || home.components().count() < 3 {
        return Err("Your account's home folder is somewhere that can't hold restricted folders.".into());
    }
    if real == home {
        return Err("Your home folder itself can't be restricted: the desktop needs it. Restrict a folder inside it.".into());
    }
    if let Ok(inside) = real.strip_prefix(home) {
        if DESKTOP_NEEDS.iter().any(|d| inside == Path::new(d)) {
            return Err(format!("'{}' holds the desktop's own settings, so it can't be restricted.", inside.display()));
        }
        return Ok(());
    }
    if !user.is_empty() && !user.contains('/') {
        if let Ok(inside) = real.strip_prefix(Path::new("/media").join(user)) {
            return match inside.components().count() {
                0 => Err("That's where your drives appear, not a folder that can be restricted.".into()),
                1 => Err("A whole drive can't be restricted, only a folder on it.".into()),
                _ => Ok(()),
            };
        }
    }
    Err(format!("'{}' isn't in your home folder or on one of your drives, so it can't be restricted.", real.display()))
}

/// Whether a filesystem of type `f_type` (statfs's) keeps Unix owners and
/// permissions. FAT, exFAT and NTFS (USB sticks, Windows drives) make every
/// file the mounting user's whatever is asked, so a folder there can't be
/// given to root; network and FUSE filesystems may not either.
pub fn keeps_owners(f_type: u32) -> Result<(), String> {
    let name = match f_type {
        0x4d44 => "FAT",
        0x2011_bab0 => "exFAT",
        0x5346_544e | 0x7366_746e => "NTFS",
        0x6573_5546 => "FUSE (NTFS and other drives through a helper)",
        0x9660 | 0x1501_3346 => "CD/DVD",
        0x6969 => "NFS",
        0xff53_4d42 | 0xfe53_4d42 | 0x517b => "Windows network share",
        _ => return Ok(()),
    };
    Err(format!("This drive is {name}, which doesn't keep Unix owners: a folder on it can't be restricted. Use a Linux (ext4) drive, or an encrypted container."))
}

/// Restricts (`restrict`) or frees `path`, asking for the password: in
/// FerroOS's password box if there is one, else in a Command Prompt window.
/// The error is said to the person.
pub fn set(path: &Path, restrict: bool) -> Result<(), String> {
    let path = fs::canonicalize(path).map_err(|e| format!("Can't find '{}': {e}.", path.display()))?;
    let verb = if restrict { "lock" } else { "unlock" };
    if Path::new(ASKPASS).exists() {
        // -k: always asked, and no remembered password left behind for
        // whatever runs next.
        let out = Command::new(SUDO)
            .args(["-k", "-A", "--", HELPER, verb])
            .arg(&path)
            .env("SUDO_ASKPASS", ASKPASS)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("Can't run sudo: {e}."))?;
        if out.status.success() {
            return Ok(());
        }
        let said = String::from_utf8_lossy(&out.stderr);
        let last = said.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
        return Err(match last.strip_prefix("ferro-restrict: ") {
            Some(why) => why.to_owned(),
            None if last.starts_with("sudo:") => "The password wasn't given, so nothing was changed.".into(),
            None => format!("Nothing was changed: {last}"),
        });
    }
    // In a Command Prompt, where sudo asks for the password itself.
    let script = format!("{SUDO} -k -- {HELPER} \"$1\" \"$2\"; echo; printf 'Press Enter to close. '; read x");
    Command::new("ferro-terminal")
        .args(["-e", "sh", "-c", &script, "sh", verb])
        .arg(&path)
        .status()
        .map_err(|e| format!("Can't open a Command Prompt: {e}."))?;
    if is_restricted(&path) == restrict {
        Ok(())
    } else {
        Err("Not changed yet: finish in the Command Prompt window, then open Properties again.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/home/nully";

    fn place(p: &str) -> Result<(), String> {
        check_place(Path::new(p), Path::new(HOME), "nully")
    }

    #[test]
    fn folders_in_the_home_can_be_restricted() {
        assert!(place("/home/nully/Documents").is_ok());
        assert!(place("/home/nully/Documents/Taxes 2026").is_ok());
        assert!(place("/home/nully/.ssh").is_ok());
        assert!(place("/home/nully/.config/secret").is_ok());
    }

    #[test]
    fn the_home_itself_and_what_the_desktop_needs_cannot() {
        assert!(place("/home/nully").is_err());
        assert!(place("/home/nully/.config").is_err());
        assert!(place("/home/nully/.local").is_err());
        assert!(place("/home/nully/.cache").is_err());
    }

    #[test]
    fn system_folders_and_other_homes_cannot() {
        for p in ["/", "/etc", "/usr/local/bin", "/var/lib/ferro", "/root", "/home", "/home/other/Documents", "/home/nullyx/Documents", "/tmp/x"] {
            assert!(place(p).is_err(), "{p}");
        }
    }

    #[test]
    fn folders_on_drives_can_but_not_whole_drives() {
        assert!(place("/media/nully/STICK/Private").is_ok());
        assert!(place("/media/nully/STICK").is_err());
        assert!(place("/media/nully").is_err());
        assert!(place("/media/other/STICK/Private").is_err());
        assert!(check_place(Path::new("/media/STICK/x"), Path::new(HOME), "").is_err());
    }

    #[test]
    fn only_plain_full_paths() {
        assert!(place("Documents").is_err());
        assert!(place("/home/nully/../../etc").is_err());
        assert!(place("/home/nully/./Documents").is_ok(), "a . in the middle is dropped by Path");
        assert!(check_place(Path::new("/home/x/Documents"), Path::new("/"), "x").is_err());
        assert!(check_place(Path::new("/home/x"), Path::new("/home"), "x").is_err());
    }

    #[test]
    fn records_read_back() {
        let r = Record { uid: 1000, gid: 1000, mode: 0o2755 };
        let text = r.to_text(Path::new("/home/nully/Private"));
        assert_eq!(text, "1000 1000 2755\n/home/nully/Private\n");
        assert_eq!(Record::parse(&text), Some(r));
        assert_eq!(Record::parse("1000 1000"), None);
        assert_eq!(Record::parse("1000 1000 99999"), None);
        assert_eq!(Record::parse("1000 1000 755 extra"), None);
        assert_eq!(Record::parse(""), None);
    }

    #[test]
    fn drives_without_owners_are_refused() {
        assert!(keeps_owners(0xef53).is_ok(), "ext4");
        assert!(keeps_owners(0x9123_683e).is_ok(), "btrfs");
        assert!(keeps_owners(0x4d44).is_err(), "vfat");
        assert!(keeps_owners(0x2011_bab0).is_err(), "exfat");
        assert!(keeps_owners(0x6573_5546).is_err(), "fuseblk");
    }

    #[test]
    fn records_are_named_by_device_and_inode() {
        assert_eq!(record_path(0x803, 42), Path::new("/var/lib/ferro/restricted/803-2a"));
    }
}
