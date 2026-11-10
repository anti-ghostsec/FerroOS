//! Archives that come as several files: which file is which part, by name,
//! and a reader that runs through numbered pieces as if they were one file.
//!
//! - RAR volumes, each a RAR archive of its own: `name.part1.rar`,
//!   `name.part2.rar`... (or `part01`, `part001`), and the older `name.rar`,
//!   `name.r00`, `name.r01`... (after `r99` comes `s00`);
//! - any archive cut into pieces, as 7-Zip does: `name.7z.001`,
//!   `name.7z.002`...; put end to end the pieces are the archive.

use crate::Error;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Which part of a split RAR archive a file's name says it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RarPart {
    /// The name without the part of it that numbers the volume.
    pub base: String,
    /// `name.part1.rar` (not `name.rar`, `name.r00`).
    pub new_style: bool,
    /// Where it comes in the set, from 0.
    pub index: u64,
    /// How many digits its number has (`part01`: 2).
    pub width: usize,
}

pub(crate) fn rar_part(name: &str) -> Option<RarPart> {
    // ASCII only, so every index into `lower` is one into `name` too.
    let lower = name.to_ascii_lowercase();
    if let Some(rest) = lower.strip_suffix(".rar") {
        if let Some(i) = rest.rfind(".part") {
            let digits = &rest[i + 5..];
            if (1..=9).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_digit()) {
                let n: u64 = digits.parse().ok()?;
                if n >= 1 && i > 0 {
                    return Some(RarPart { base: name[..i].to_owned(), new_style: true, index: n - 1, width: digits.len() });
                }
            }
        }
        if rest.is_empty() {
            return None;
        }
        return Some(RarPart { base: name[..rest.len()].to_owned(), new_style: false, index: 0, width: 2 });
    }
    let dot = lower.rfind('.').filter(|d| *d > 0)?;
    let ext = &lower.as_bytes()[dot + 1..];
    if ext.len() == 3 && (b'r'..=b'z').contains(&ext[0]) && ext[1].is_ascii_digit() && ext[2].is_ascii_digit() {
        let index = 1 + u64::from(ext[0] - b'r') * 100 + u64::from(ext[1] - b'0') * 10 + u64::from(ext[2] - b'0');
        return Some(RarPart { base: name[..dot].to_owned(), new_style: false, index, width: 2 });
    }
    None
}

impl RarPart {
    /// The name the part at `index` of this set has.
    pub fn name_of(&self, index: u64) -> String {
        if self.new_style {
            format!("{}.part{:0w$}.rar", self.base, index + 1, w = self.width)
        } else if index == 0 {
            format!("{}.rar", self.base)
        } else {
            let (hundreds, rest) = ((index - 1) / 100, (index - 1) % 100);
            let letter = (b'r' + hundreds.min(8) as u8) as char;
            format!("{}.{letter}{rest:02}", self.base)
        }
    }
}

/// The parts of `part`'s set found in `dir`, by where they come in it.
/// `opened` (the file asked for) wins over another file numbered the same
/// (`part1` beside `part01`).
pub(crate) fn rar_siblings(dir: &Path, opened: &str, part: &RarPart) -> std::collections::BTreeMap<u64, PathBuf> {
    let mut found = std::collections::BTreeMap::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let Ok(name) = e.file_name().into_string() else { continue };
            let Some(p) = rar_part(&name) else { continue };
            if p.new_style != part.new_style || p.base != part.base {
                continue;
            }
            if name == opened || !found.contains_key(&p.index) {
                found.insert(p.index, e.path());
            }
        }
    }
    found
}

/// `name.7z.003`: the name without the number, the number (from 1) and how
/// many digits it has.
pub(crate) fn numbered(name: &str) -> Option<(&str, u64, usize)> {
    let dot = name.rfind('.').filter(|d| *d > 0)?;
    let digits = &name[dot + 1..];
    if !(3..=6).contains(&digits.len()) || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: u64 = digits.parse().ok()?;
    (n >= 1).then_some((&name[..dot], n, digits.len()))
}

fn file_name(path: &Path) -> Option<&str> {
    path.file_name()?.to_str()
}

/// Whether `path` is named as one piece of an archive cut into numbered
/// pieces (`.001`...).
pub fn is_numbered_piece(path: &Path) -> bool {
    file_name(path).and_then(numbered).is_some()
}

/// The first piece of the set `path` is one of (`path` itself when it
/// isn't one of a set).
pub(crate) fn first_piece(path: &Path) -> PathBuf {
    match file_name(path).and_then(numbered) {
        Some((base, _, width)) => path.with_file_name(format!("{base}.{:0width$}", 1)),
        None => path.to_path_buf(),
    }
}

/// The message for a part that isn't there.
pub(crate) fn missing(name: &str) -> Error {
    Error::Io(format!("'{name}' is missing. This archive is split into several files; all of them have to be in the same folder."))
}

/// One file, or numbered pieces read end to end as one. Only the piece
/// being read is held open.
pub(crate) struct Parts {
    /// Each piece, with where it starts in the whole.
    pieces: Vec<(PathBuf, u64, u64)>,
    total: u64,
    pos: u64,
    open: Option<(usize, File)>,
}

impl Parts {
    /// `path`, or the whole set when it's named as one piece of one
    /// (whichever piece it is).
    pub fn open(path: &Path) -> Result<Parts, Error> {
        let cant = |e: io::Error| Error::Io(format!("Couldn't open it: {e}"));
        let mut paths = Vec::new();
        match file_name(path).and_then(numbered) {
            Some((base, n, width)) => {
                let name_of = |i: u64| format!("{base}.{i:0width$}");
                let mut i = 1;
                while path.with_file_name(name_of(i)).is_file() {
                    paths.push(path.with_file_name(name_of(i)));
                    i += 1;
                }
                // The first one not there: missing, if the one opened
                // comes after it.
                if n >= i || path.with_file_name(name_of(i + 1)).is_file() {
                    return Err(missing(&name_of(i)));
                }
            }
            None => paths.push(path.to_path_buf()),
        }
        let mut pieces = Vec::with_capacity(paths.len());
        let mut total = 0u64;
        for p in paths {
            let len = std::fs::metadata(&p).map_err(cant)?.len();
            pieces.push((p, total, len));
            total = total.checked_add(len).ok_or_else(|| Error::Damaged("its parts are too big".into()))?;
        }
        let first = File::open(&pieces[0].0).map_err(cant)?;
        Ok(Parts { pieces, total, pos: 0, open: Some((0, first)) })
    }

    /// The size of all the pieces together.
    pub fn len(&self) -> u64 {
        self.total
    }
}

impl Read for Parts {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() || self.pos >= self.total {
            return Ok(0);
        }
        // The piece holding `pos` (pieces of no length hold nothing).
        let i = self.pieces.partition_point(|(_, start, len)| start + len <= self.pos);
        let (path, start, len) = &self.pieces[i];
        if self.open.as_ref().is_none_or(|(at, _)| *at != i) {
            self.open = Some((i, File::open(path)?));
        }
        let file = &mut self.open.as_mut().expect("just opened").1;
        let within = self.pos - start;
        file.seek(SeekFrom::Start(within))?;
        let room = (len - within).min(buf.len() as u64) as usize;
        let n = file.read(&mut buf[..room])?;
        if n == 0 {
            // Shorter than it was when the set was opened.
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "a part of the archive got shorter"));
        }
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for Parts {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let to = match from {
            SeekFrom::Start(p) => i128::from(p),
            SeekFrom::Current(d) => i128::from(self.pos) + i128::from(d),
            SeekFrom::End(d) => i128::from(self.total) + i128::from(d),
        };
        self.pos = u64::try_from(to).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        Ok(self.pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rar_parts_by_name() {
        let p = rar_part("Film.part1.rar").unwrap();
        assert_eq!((p.base.as_str(), p.new_style, p.index, p.width), ("Film", true, 0, 1));
        let p = rar_part("a.b.PART012.RAR").unwrap();
        assert_eq!((p.base.as_str(), p.new_style, p.index, p.width), ("a.b", true, 11, 3));
        assert_eq!(p.name_of(0), "a.b.part001.rar");
        assert_eq!(p.name_of(99), "a.b.part100.rar");
        let p = rar_part("Film.rar").unwrap();
        assert_eq!((p.base.as_str(), p.new_style, p.index), ("Film", false, 0));
        assert_eq!(p.name_of(1), "Film.r00");
        assert_eq!(p.name_of(100), "Film.r99");
        assert_eq!(p.name_of(101), "Film.s00");
        assert_eq!(rar_part("Film.r00").unwrap().index, 1);
        assert_eq!(rar_part("Film.R07").unwrap().index, 8);
        assert_eq!(rar_part("Film.s00").unwrap().index, 101);
        assert_eq!(rar_part("Film.r00").unwrap().name_of(0), "Film.rar");
        // Not numbered at all: the first of an old-style set, if any.
        assert!(!rar_part("x.part.rar").unwrap().new_style);
        assert!(!rar_part("x.part0.rar").unwrap().new_style);
        for n in ["Film.zip", "Film.r0", "Film.q00", "Film.r000", ".rar", "rar", "Film.rarx"] {
            assert_eq!(rar_part(n), None, "{n}");
        }
    }

    #[test]
    fn numbered_pieces_by_name() {
        assert_eq!(numbered("a.7z.001"), Some(("a.7z", 1, 3)));
        assert_eq!(numbered("a.zip.0012"), Some(("a.zip", 12, 4)));
        for n in ["a.7z", "a.7z.01", "a.7z.000", "a.7z.00a", ".001", "001"] {
            assert_eq!(numbered(n), None, "{n}");
        }
        assert_eq!(first_piece(Path::new("/x/a.7z.003")), PathBuf::from("/x/a.7z.001"));
        assert_eq!(first_piece(Path::new("/x/a.7z")), PathBuf::from("/x/a.7z"));
    }

    #[test]
    fn pieces_read_as_one() {
        let d = std::env::temp_dir().join(format!("ferro-archive-test-{}-parts", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let whole: Vec<u8> = (0..10_000u32).map(|i| (i * 7 % 256) as u8).collect();
        for (i, chunk) in whole.chunks(3000).enumerate() {
            std::fs::write(d.join(format!("w.bin.{:03}", i + 1)), chunk).unwrap();
        }
        // From any piece: the whole.
        for from in ["w.bin.001", "w.bin.003", "w.bin.004"] {
            let mut p = Parts::open(&d.join(from)).unwrap();
            assert_eq!(p.len(), 10_000);
            let mut got = Vec::new();
            p.read_to_end(&mut got).unwrap();
            assert_eq!(got, whole, "{from}");
            // Across a join, after seeking.
            p.seek(SeekFrom::Start(2990)).unwrap();
            let mut some = [0u8; 20];
            p.read_exact(&mut some).unwrap();
            assert_eq!(some, whole[2990..3010]);
            p.seek(SeekFrom::End(-5)).unwrap();
            let mut tail = Vec::new();
            p.read_to_end(&mut tail).unwrap();
            assert_eq!(tail, whole[9995..]);
        }
        // One gone: said by name.
        std::fs::remove_file(d.join("w.bin.002")).unwrap();
        let e = Parts::open(&d.join("w.bin.004")).err().unwrap();
        assert!(e.to_string().contains("'w.bin.002' is missing"), "{e}");
        std::fs::remove_file(d.join("w.bin.001")).unwrap();
        let e = Parts::open(&d.join("w.bin.003")).err().unwrap();
        assert!(e.to_string().contains("'w.bin.001' is missing"), "{e}");
        std::fs::remove_dir_all(&d).unwrap();
    }
}
