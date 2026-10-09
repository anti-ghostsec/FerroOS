//! Windows-style drive paths (`C:\Users\...`) for FerroOS.
//!
//! The kernel only understands POSIX paths, so every FerroOS component that
//! accepts a user-facing path translates it here first. A [`DriveTable`] maps
//! drive letters to POSIX mount points (`C:` is `/` by default).
//!
//! Case-insensitivity is deliberately *not* handled here: it belongs in the
//! filesystem (ext4 `casefold`), where it is both correct and fast.
//!
//! `no_std` + `alloc` so the same code can later live inside a libc shim.

#![no_std]
extern crate alloc;
#[cfg(test)]
extern crate std;

use alloc::borrow::ToOwned;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    Empty,
    /// The drive letter has no mount in the [`DriveTable`].
    UnknownDrive(char),
    /// `\\server\share` paths need a network redirector FerroOS doesn't have.
    Unc,
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathError::Empty => f.write_str("empty path"),
            PathError::UnknownDrive(d) => write!(f, "drive {d}: is not mounted"),
            PathError::Unc => f.write_str("UNC network paths are not supported"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveTable {
    /// (uppercase letter, POSIX root), kept sorted by letter.
    drives: Vec<(char, String)>,
}

impl Default for DriveTable {
    /// `C:` mounted at `/`.
    fn default() -> Self {
        let mut t = Self::empty();
        t.mount('C', "/");
        t
    }
}

impl DriveTable {
    pub fn empty() -> Self {
        Self { drives: Vec::new() }
    }

    /// Maps `letter:` to `root`, replacing any existing mapping.
    pub fn mount(&mut self, letter: char, root: &str) -> &mut Self {
        let letter = letter.to_ascii_uppercase();
        let root = normalize_root(root);
        match self.drives.binary_search_by_key(&letter, |(l, _)| *l) {
            Ok(i) => self.drives[i].1 = root,
            Err(i) => self.drives.insert(i, (letter, root)),
        }
        self
    }

    pub fn unmount(&mut self, letter: char) {
        let letter = letter.to_ascii_uppercase();
        self.drives.retain(|(l, _)| *l != letter);
    }

    pub fn root(&self, letter: char) -> Option<&str> {
        let letter = letter.to_ascii_uppercase();
        self.drives.iter().find(|(l, _)| *l == letter).map(|(_, r)| r.as_str())
    }

    pub fn drives(&self) -> impl Iterator<Item = (char, &str)> {
        self.drives.iter().map(|(l, r)| (*l, r.as_str()))
    }

    /// Translates a Windows-style path to a POSIX path.
    ///
    /// * `C:\Program Files\x` -> `/Program Files/x`
    /// * `\etc\hostname` -> relative to the `C:` root
    /// * `C:foo` is treated as `C:\foo` (there is no per-drive cwd)
    /// * `..` is resolved and can never climb above a drive's root
    /// * relative paths stay relative, with `/` separators
    /// * `NUL` and `CON` map to `/dev/null` and `/dev/tty`
    pub fn to_posix(&self, path: &str) -> Result<String, PathError> {
        if path.is_empty() {
            return Err(PathError::Empty);
        }
        let mut p = path;
        if let Some(rest) = p.strip_prefix(r"\\?\").or_else(|| p.strip_prefix(r"\\.\")) {
            if rest.len() >= 4 && rest[..4].eq_ignore_ascii_case(r"UNC\") {
                return Err(PathError::Unc);
            }
            p = rest;
        } else if p.starts_with(r"\\") {
            return Err(PathError::Unc);
        }

        let device = p.trim_end_matches(':');
        if device.eq_ignore_ascii_case("NUL") {
            return Ok("/dev/null".to_owned());
        }
        if device.eq_ignore_ascii_case("CON") {
            return Ok("/dev/tty".to_owned());
        }

        let bytes = p.as_bytes();
        let (base, rest) = if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
            let letter = (bytes[0] as char).to_ascii_uppercase();
            (Some(self.root(letter).ok_or(PathError::UnknownDrive(letter))?), &p[2..])
        } else if p.starts_with(['\\', '/']) {
            (Some(self.root('C').unwrap_or("/")), p)
        } else {
            (None, p)
        };

        let comps = normalize(rest, base.is_some());
        Ok(match base {
            Some(base) => join_posix(base, &comps),
            None if comps.is_empty() => ".".to_owned(),
            None => comps.join("/"),
        })
    }

    /// Translates a POSIX path to the Windows-style path the user should see.
    ///
    /// The longest matching drive root wins, so with `D:` at `/mnt/d`,
    /// `/mnt/d/x` is `D:\x` rather than `C:\mnt\d\x`. Returns `None` for an
    /// absolute path that no drive covers. Relative paths just swap separators.
    pub fn to_windows(&self, posix: &str) -> Option<String> {
        // Host-style roots (`C:/...`, used by the desktop preview) are absolute too.
        let host_absolute = posix.as_bytes().get(1) == Some(&b':');
        if !posix.starts_with('/') && !host_absolute {
            return Some(posix.replace('/', "\\"));
        }
        let comps = normalize(posix, true);
        let (letter, depth) = self
            .drives
            .iter()
            .filter_map(|(l, root)| {
                let rc = normalize(root, true);
                comps.starts_with(&rc).then_some((*l, rc.len()))
            })
            .max_by_key(|(_, depth)| *depth)?;
        Some(format!("{letter}:\\{}", comps[depth..].join("\\")))
    }
}

/// True if `s` looks like a Windows path rather than a POSIX one.
pub fn is_windows_path(s: &str) -> bool {
    let b = s.as_bytes();
    (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':') || s.contains('\\')
}

/// Appends `name` to a Windows-style directory path.
pub fn join(dir: &str, name: &str) -> String {
    let mut s = String::from(dir);
    if !s.is_empty() && !s.ends_with(['\\', '/']) {
        s.push('\\');
    }
    s.push_str(name);
    s
}

/// The parent of a Windows-style path, or `None` at a drive root.
pub fn parent(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches(['\\', '/']);
    let idx = trimmed.rfind(['\\', '/'])?;
    let p = &trimmed[..idx];
    if p.is_empty() || p.ends_with(':') {
        Some(format!("{p}\\"))
    } else {
        Some(p.to_owned())
    }
}

fn normalize_root(root: &str) -> String {
    let t = root.trim_end_matches('/');
    if t.is_empty() {
        "/".to_owned()
    } else if t.ends_with(':') {
        // A Windows host path like `C:/`, used by the desktop preview.
        format!("{t}/")
    } else {
        t.to_owned()
    }
}

/// Splits on both separators, dropping empty and `.` components and resolving
/// `..`. Absolute paths clamp `..` at the root; relative ones keep leading `..`.
fn normalize(path: &str, absolute: bool) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::new();
    for c in path.split(['\\', '/']) {
        match c {
            "" | "." => {}
            ".." => match out.last() {
                Some(&last) if last != ".." => {
                    out.pop();
                }
                _ if absolute => {}
                _ => out.push(".."),
            },
            c => out.push(c),
        }
    }
    out
}

fn join_posix(base: &str, comps: &[&str]) -> String {
    if comps.is_empty() {
        return base.to_owned();
    }
    let mut s = String::from(base);
    if !s.ends_with('/') {
        s.push('/');
    }
    s.push_str(&comps.join("/"));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> DriveTable {
        let mut t = DriveTable::default();
        t.mount('d', "/mnt/d/");
        t
    }

    #[test]
    fn drive_paths() {
        let t = table();
        assert_eq!(t.to_posix(r"C:\").unwrap(), "/");
        assert_eq!(t.to_posix(r"C:\Program Files\Steam").unwrap(), "/Program Files/Steam");
        assert_eq!(t.to_posix(r"c:/users//me\.\x").unwrap(), "/users/me/x");
        assert_eq!(t.to_posix(r"D:\Games\a.exe").unwrap(), "/mnt/d/Games/a.exe");
        assert_eq!(t.to_posix(r"D:").unwrap(), "/mnt/d");
        assert_eq!(t.to_posix(r"C:foo").unwrap(), "/foo");
        assert_eq!(t.to_posix(r"\\?\C:\x").unwrap(), "/x");
    }

    #[test]
    fn root_relative_and_relative() {
        let t = table();
        assert_eq!(t.to_posix(r"\etc\hostname").unwrap(), "/etc/hostname");
        assert_eq!(t.to_posix("/usr/bin").unwrap(), "/usr/bin");
        assert_eq!(t.to_posix(r"a\b\..\c").unwrap(), "a/c");
        assert_eq!(t.to_posix(r"..\..\x").unwrap(), "../../x");
        assert_eq!(t.to_posix(r".").unwrap(), ".");
    }

    #[test]
    fn dotdot_cannot_escape_drive() {
        let t = table();
        assert_eq!(t.to_posix(r"D:\..\..\etc").unwrap(), "/mnt/d/etc");
        assert_eq!(t.to_posix(r"C:\a\..\..").unwrap(), "/");
    }

    #[test]
    fn errors_and_devices() {
        let t = table();
        assert_eq!(t.to_posix(""), Err(PathError::Empty));
        assert_eq!(t.to_posix(r"Q:\x"), Err(PathError::UnknownDrive('Q')));
        assert_eq!(t.to_posix(r"\\server\share"), Err(PathError::Unc));
        assert_eq!(t.to_posix(r"\\?\UNC\server\share"), Err(PathError::Unc));
        assert_eq!(t.to_posix("nul").unwrap(), "/dev/null");
        assert_eq!(t.to_posix("CON:").unwrap(), "/dev/tty");
    }

    #[test]
    fn to_windows_longest_root_wins() {
        let t = table();
        assert_eq!(t.to_windows("/").unwrap(), r"C:\");
        assert_eq!(t.to_windows("/etc/hostname").unwrap(), r"C:\etc\hostname");
        assert_eq!(t.to_windows("/mnt/d/Games").unwrap(), r"D:\Games");
        assert_eq!(t.to_windows("/mnt/dx").unwrap(), r"C:\mnt\dx");
        assert_eq!(t.to_windows("a/b").unwrap(), r"a\b");
        assert_eq!(DriveTable::empty().to_windows("/x"), None);
    }

    #[test]
    fn round_trip() {
        let t = table();
        for p in [r"C:\", r"C:\bin\ferro-shell", r"D:\Games\Doom"] {
            assert_eq!(t.to_windows(&t.to_posix(p).unwrap()).unwrap(), p);
        }
    }

    #[test]
    fn host_style_roots() {
        let mut t = DriveTable::empty();
        t.mount('C', "C:/");
        assert_eq!(t.to_posix(r"C:\").unwrap(), "C:/");
        assert_eq!(t.to_posix(r"C:\Users").unwrap(), "C:/Users");
        assert_eq!(t.to_windows("C:/Users/me").unwrap(), r"C:\Users\me");

        let mut deep = DriveTable::empty();
        deep.mount('C', "C:/Temp/x");
        let posix = deep.to_posix(r"C:\sub").unwrap();
        assert_eq!(posix, "C:/Temp/x/sub");
        assert_eq!(deep.to_windows(&posix).unwrap(), r"C:\sub");
    }

    #[test]
    fn join_and_parent() {
        assert_eq!(join(r"C:\", "bin"), r"C:\bin");
        assert_eq!(join(r"C:\bin", "x"), r"C:\bin\x");
        assert_eq!(parent(r"C:\bin\x").unwrap(), r"C:\bin");
        assert_eq!(parent(r"C:\bin").unwrap(), r"C:\");
        assert_eq!(parent(r"C:\"), None);
        assert!(is_windows_path(r"C:\x") && is_windows_path(r"a\b") && !is_windows_path("/usr"));
    }
}
