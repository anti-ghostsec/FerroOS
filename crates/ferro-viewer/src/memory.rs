//! The page each comic was left at, kept only if the person said yes when
//! first asked, and without anything that says which comic it was: no names,
//! no folders, no times. A comic is known by its fingerprint (`Comic::id`),
//! and only a scrambled form of that is kept: HMAC-SHA256 of it under a
//! random key made on this computer, cut to 16 bytes. Without the key, the
//! file can't be matched against a comic someone has.
//!
//! It all lives in one file, `~/.config/ferro/viewer/pages`, only ever
//! written by the viewer (the only place outside the picture's folder that
//! its sandbox lets it change):
//!
//! ```text
//! remember yes
//! key <64 hex digits>
//! <32 hex digits> <page>      newest first, at most MAX of them
//! ```
//!
//! "remember no" is kept too, so the question isn't asked again; then
//! there's no key and no pages.

use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The most comics whose page is kept; the one read longest ago goes first.
pub const MAX: usize = 500;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Choice {
    /// Not asked yet: ask on the first comic.
    Unasked,
    Yes,
    No,
}

pub struct Memory {
    /// Where it's kept; `None` with no home folder (then nothing is kept).
    file: Option<PathBuf>,
    pub choice: Choice,
    key: Option<[u8; 32]>,
    /// (mark, page), newest first.
    places: Vec<(String, usize)>,
}

/// The viewer's own folder, `~/.config/ferro/viewer`.
pub fn folder() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join(".config/ferro/viewer"))
}

impl Memory {
    /// What's kept in `folder` (nothing, if there's nothing there yet).
    pub fn load(folder: Option<&Path>) -> Self {
        let file = folder.map(|f| f.join("pages"));
        let text = file.as_ref().and_then(|f| fs::read_to_string(f).ok()).unwrap_or_default();
        let mut m = Self::parse(&text);
        m.file = file;
        // Kept on, but the key's lost: a new one (the pages went with it).
        if m.choice == Choice::Yes && m.key.is_none() {
            m.key = new_key().ok();
        }
        m
    }

    fn parse(text: &str) -> Self {
        let mut m = Self { file: None, choice: Choice::Unasked, key: None, places: Vec::new() };
        for line in text.lines() {
            let mut w = line.split_whitespace();
            match (w.next(), w.next()) {
                (Some("remember"), Some("yes")) => m.choice = Choice::Yes,
                (Some("remember"), Some("no")) => m.choice = Choice::No,
                (Some("key"), Some(k)) => m.key = unhex::<32>(k),
                (Some(mark), Some(page)) if unhex::<16>(mark).is_some() && m.places.len() < MAX => {
                    if let Ok(page) = page.parse() {
                        if !m.places.iter().any(|(k, _)| k == mark) {
                            m.places.push((mark.to_string(), page));
                        }
                    }
                }
                _ => {}
            }
        }
        // Pages without their key are no use to anyone; nor is anything kept
        // after "no".
        if m.choice != Choice::Yes || m.key.is_none() {
            m.places.clear();
        }
        if m.choice != Choice::Yes {
            m.key = None;
        }
        m
    }

    /// The page this comic was left at, if pages are kept and it was.
    pub fn page(&self, id: &[u8; 32]) -> Option<usize> {
        let key = self.remembering()?;
        let mark = mark(key, id);
        self.places.iter().find(|(m, _)| *m == mark).map(|&(_, p)| p)
    }

    /// Keeps this comic's page (when pages are kept at all).
    pub fn keep(&mut self, id: &[u8; 32], page: usize) -> io::Result<()> {
        let Some(key) = self.remembering() else { return Ok(()) };
        let mark = mark(key, id);
        if self.places.first().is_some_and(|(m, p)| *m == mark && *p == page) {
            return Ok(());
        }
        self.places.retain(|(m, _)| *m != mark);
        self.places.insert(0, (mark, page));
        self.places.truncate(MAX);
        self.save()
    }

    /// The answer to "Remember your page?" (or M, changing it). No forgets
    /// every page kept, and the key.
    pub fn choose(&mut self, yes: bool) -> io::Result<()> {
        if yes {
            if self.key.is_none() {
                self.key = Some(new_key()?);
            }
            self.choice = Choice::Yes;
        } else {
            self.choice = Choice::No;
            self.key = None;
            self.places.clear();
        }
        self.save()
    }

    fn remembering(&self) -> Option<&[u8; 32]> {
        if self.choice == Choice::Yes {
            self.key.as_ref()
        } else {
            None
        }
    }

    fn text(&self) -> String {
        let mut s = String::new();
        match (self.choice, &self.key) {
            (Choice::Yes, Some(key)) => {
                s += "remember yes\n";
                s += &format!("key {}\n", hex(key));
                for (m, p) in &self.places {
                    s += &format!("{m} {p}\n");
                }
            }
            (Choice::No, _) => s += "remember no\n",
            _ => {}
        }
        s
    }

    /// Written whole to a file beside it, then put in its place, so it's
    /// never half written; only its owner can read it.
    fn save(&self) -> io::Result<()> {
        let Some(file) = &self.file else { return Err(io::Error::new(io::ErrorKind::NotFound, "no home folder")) };
        let new = file.with_extension("new");
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        let mut f = opts.open(&new)?;
        f.write_all(self.text().as_bytes())?;
        f.sync_all()?;
        drop(f);
        fs::rename(&new, file)
    }
}

/// A comic's scrambled mark: what's kept in place of its fingerprint.
fn mark(key: &[u8; 32], id: &[u8; 32]) -> String {
    hex(&hmac_sha256(key, id)[..16])
}

/// HMAC-SHA256 (RFC 2104), for a key of up to 64 bytes.
fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    k[..key.len()].copy_from_slice(key);
    let inner = Sha256::new().chain_update(k.map(|b| b ^ 0x36)).chain_update(msg).finalize();
    Sha256::new().chain_update(k.map(|b| b ^ 0x5c)).chain_update(inner).finalize().into()
}

/// 32 random bytes from the kernel.
fn new_key() -> io::Result<[u8; 32]> {
    let mut key = [0u8; 32];
    let mut got = 0;
    while got < key.len() {
        // SAFETY: the pointer and length are of the part of `key` not yet filled.
        let n = unsafe { libc::getrandom(key[got..].as_mut_ptr().cast(), key.len() - got, 0) };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        got += n as usize;
    }
    Ok(key)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex<const N: usize>(s: &str) -> Option<[u8; N]> {
    if s.len() != N * 2 || !s.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; N];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_matches_rfc_4231() {
        // RFC 4231, test case 2.
        let got = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(hex(&got), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ferro-viewer-memory-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn pages_are_kept_only_after_yes_and_without_names() {
        let d = temp("yes");
        let (a, b) = ([1u8; 32], [2u8; 32]);
        let mut m = Memory::load(Some(&d));
        assert_eq!(m.choice, Choice::Unasked);
        m.keep(&a, 5).unwrap();
        assert!(!d.join("pages").exists(), "nothing written before it's asked");
        m.choose(true).unwrap();
        m.keep(&a, 5).unwrap();
        m.keep(&b, 9).unwrap();
        m.keep(&a, 6).unwrap();
        let m = Memory::load(Some(&d));
        assert_eq!((m.page(&a), m.page(&b), m.page(&[3; 32])), (Some(6), Some(9), None));
        let text = fs::read_to_string(d.join("pages")).unwrap();
        assert_eq!(text.lines().count(), 4, "the choice, the key and two comics: {text}");
        assert!(!text.contains(&hex(&a)), "the fingerprint itself isn't kept");
        assert_eq!(text.lines().nth(2).unwrap().split(' ').nth(1), Some("6"), "newest first");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(d.join("pages")).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn no_forgets_everything_and_is_remembered() {
        let d = temp("no");
        let mut m = Memory::load(Some(&d));
        m.choose(true).unwrap();
        m.keep(&[1; 32], 3).unwrap();
        m.choose(false).unwrap();
        assert_eq!(fs::read_to_string(d.join("pages")).unwrap(), "remember no\n");
        let mut m = Memory::load(Some(&d));
        assert_eq!(m.choice, Choice::No);
        m.keep(&[1; 32], 4).unwrap();
        assert_eq!(m.page(&[1; 32]), None);
        // Yes again: a new key, so nothing from before could match.
        m.choose(true).unwrap();
        assert_eq!(Memory::load(Some(&d)).page(&[1; 32]), None);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn at_most_max_comics() {
        let d = temp("max");
        let mut m = Memory::load(Some(&d));
        m.choose(true).unwrap();
        for i in 0..MAX + 20 {
            let mut id = [0u8; 32];
            id[..8].copy_from_slice(&(i as u64).to_le_bytes());
            m.places.insert(0, (mark(m.key.as_ref().unwrap(), &id), i));
        }
        m.keep(&[9; 32], 1).unwrap();
        let m = Memory::load(Some(&d));
        assert_eq!(m.places.len(), MAX);
        assert_eq!(m.page(&[9; 32]), Some(1));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_damaged_file_keeps_nothing() {
        let m = Memory::parse("remember yes\nabc 4\n0123 x\n");
        assert_eq!((m.choice, m.places.len()), (Choice::Yes, 0), "no key: no pages");
        let m = Memory::parse("remember no\nkey 00\n00000000000000000000000000000000 1\n");
        assert_eq!((m.choice, m.places.len(), m.key), (Choice::No, 0, None));
    }
}
