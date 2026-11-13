//! ferro-archive: reads .zip, .7z and .rar archives and unpacks them safely.
//! An archive split into several files opens from any one of them, with the
//! rest beside it: RAR volumes (`.part1.rar`..., or `.rar`, `.r00`...) and
//! archives cut into numbered pieces (`.7z.001`, `.zip.001`, `.rar.001`...).
//!
//! All three decoders are pure Rust and built into the program (the `zip`,
//! `sevenz-rust2` and `rars` crates), so nothing is downloaded or installed
//! to open an archive. Whatever an archive says, extracting it:
//! - writes only inside the folder chosen: names with `..`, a leading `/`, a
//!   drive letter or a NUL are skipped, and no file is ever written through
//!   a symbolic link (folders on the way must be real folders; files are
//!   created new, never followed);
//! - makes symbolic links last, and only those pointing inside that folder;
//! - stops at a limit on what comes out (the free space on the drive, and
//!   no more than 1000 times the archive past the first GB), and at any
//!   entry giving more than it said it holds: zip bombs end there;
//! - never replaces a file already there;
//! - asks for a password when one is needed ([`Error::NeedPassword`]) and
//!   says so when it's wrong ([`Error::WrongPassword`]).

pub mod create;
mod volumes;

pub use volumes::is_numbered_piece;

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Zip,
    SevenZ,
    Rar,
}

impl Format {
    pub fn name(self) -> &'static str {
        match self {
            Format::Zip => "ZIP",
            Format::SevenZ => "7-Zip",
            Format::Rar => "RAR",
        }
    }
}

/// One thing in an archive, as listed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The name as stored ('/' between folders).
    pub name: String,
    pub size: u64,
    /// What it takes up in the archive (0 where the format doesn't say,
    /// as in a solid 7z).
    pub packed: u64,
    /// Last changed, in seconds since 1970.
    pub modified: Option<i64>,
    pub is_dir: bool,
    pub is_link: bool,
    pub encrypted: bool,
}

#[derive(Clone, Debug)]
pub struct Listing {
    pub format: Format,
    pub entries: Vec<Entry>,
}

impl Listing {
    /// The total of what `which` (all, with `None`) holds.
    pub fn size_of(&self, which: Option<&[usize]>) -> u64 {
        match which {
            Some(w) => w.iter().filter_map(|&i| self.entries.get(i)).map(|e| e.size).sum(),
            None => self.entries.iter().map(|e| e.size).sum(),
        }
    }

    /// `which`, with everything inside any folder in it.
    pub fn with_contents(&self, which: &[usize]) -> Vec<usize> {
        let folders: Vec<String> =
            which.iter().filter_map(|&i| self.entries.get(i)).filter(|e| e.is_dir).map(|e| format!("{}/", e.name.trim_end_matches('/'))).collect();
        (0..self.entries.len()).filter(|i| which.contains(i) || folders.iter().any(|f| self.entries[*i].name.starts_with(f.as_str()))).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Encrypted: ask for the password and try again.
    NeedPassword,
    WrongPassword,
    /// More would come out than the limit allows (a zip bomb, or no room).
    TooBig(String),
    Cancelled,
    /// Not an archive this reads, or a kind of compression it can't.
    Unsupported(String),
    Damaged(String),
    Io(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NeedPassword => f.write_str("This archive is password-protected."),
            Error::WrongPassword => f.write_str("The password is not correct."),
            Error::TooBig(why) => f.write_str(why),
            Error::Cancelled => f.write_str("Cancelled."),
            Error::Unsupported(why) => write!(f, "This archive can't be opened: {why}."),
            Error::Damaged(why) => write!(f, "The archive is damaged: {why}."),
            Error::Io(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(match e.raw_os_error() {
            Some(libc::ENOSPC) => "The drive is full.".into(),
            Some(libc::EACCES) | Some(libc::EPERM) | Some(libc::EROFS) => "That folder can't be written to.".into(),
            _ => format!("Couldn't write: {e}"),
        })
    }
}

/// How far an extraction has come, shared with the window drawing it.
#[derive(Default)]
pub struct Progress {
    pub done: AtomicU64,
    pub total: AtomicU64,
    pub cancel: AtomicBool,
    pub current: Mutex<String>,
}

impl Progress {
    pub fn percent(&self) -> u32 {
        let (d, t) = (self.done.load(Ordering::Relaxed), self.total.load(Ordering::Relaxed));
        if t == 0 {
            0
        } else {
            (d.min(t) as u128 * 100 / t as u128) as u32
        }
    }
}

/// How much may come out.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Bytes in all.
    pub max_total: u64,
    /// Bytes out per byte of archive, once past `ratio_after`.
    pub max_ratio: u64,
    pub ratio_after: u64,
    pub max_entries: usize,
}

pub const GB: u64 = 1 << 30;

impl Limits {
    /// For unpacking into `dest`: what fits on its drive (keeping 64 MB
    /// free), at most 1000 times the archive past the first GB.
    pub fn for_dest(dest: &Path) -> Self {
        let room = free_space(dest).map_or(u64::MAX, |f| f.saturating_sub(64 << 20));
        Limits { max_total: room, max_ratio: 1000, ratio_after: GB, max_entries: 1_000_000 }
    }
}

/// The bytes free to us on `dir`'s drive.
pub fn free_space(dir: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    // SAFETY: statvfs fills the struct given from a NUL-ended path; both
    // live through the call.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    Some(st.f_bavail as u64 * st.f_frsize as u64)
}

/// What an extraction did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    pub files: usize,
    pub folders: usize,
    pub links: usize,
    /// Names that would have landed outside the folder (or through a link).
    pub unsafe_names: Vec<String>,
    /// Already there, so kept as they were.
    pub existing: usize,
    /// Links pointing outside the folder, or of a kind not made.
    pub skipped_links: usize,
}

impl Report {
    /// One line for the status bar.
    pub fn summary(&self) -> String {
        self.summary_into(None)
    }

    /// The same, saying where it went (`into 'Photos'`).
    pub fn summary_into(&self, place: Option<&str>) -> String {
        let mut s = format!("Extracted {} file(s)", self.files);
        if self.folders > 0 {
            s += &format!(" and {} folder(s)", self.folders);
        }
        if let Some(p) = place {
            s += &format!(" into '{p}'");
        }
        s.push('.');
        if !self.unsafe_names.is_empty() {
            s += &format!(" Skipped {} unsafe name(s).", self.unsafe_names.len());
        }
        if self.existing > 0 {
            s += &format!(" Kept {} already there.", self.existing);
        }
        if self.skipped_links > 0 {
            s += &format!(" Skipped {} link(s).", self.skipped_links);
        }
        s
    }
}

/// What kind of archive `path` is, by its first bytes (then its name).
pub fn detect(path: &Path) -> Option<Format> {
    let mut head = [0u8; 8];
    // Cut into numbered pieces (`.7z.001`...): the first one says what it is.
    let first = volumes::first_piece(path);
    let n = File::open(&first).and_then(|mut f| f.read(&mut head)).unwrap_or(0);
    let head = &head[..n];
    if head.starts_with(b"PK\x03\x04") || head.starts_with(b"PK\x05\x06") || head.starts_with(b"PK\x07\x08") {
        return Some(Format::Zip);
    }
    if head.starts_with(&[b'7', b'z', 0xBC, 0xAF, 0x27, 0x1C]) {
        return Some(Format::SevenZ);
    }
    if head.starts_with(b"Rar!\x1a\x07") || head.starts_with(b"RE~^") {
        return Some(Format::Rar);
    }
    // A self-extracting one, or a zip with something before it: by name
    // (without a piece's number).
    let name = path.file_name()?.to_str()?;
    let name = volumes::numbered(name).map_or(name, |(base, _, _)| base);
    let ext = Path::new(name).extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "zip" | "cbz" | "jar" => Some(Format::Zip),
        "7z" => Some(Format::SevenZ),
        "rar" | "cbr" => Some(Format::Rar),
        _ => None,
    }
}

/// The archive's contents. With headers encrypted (7z, RAR) this needs the
/// password already; zips list without one.
pub fn open(path: &Path, password: Option<&str>) -> Result<Listing, Error> {
    let format = detect(path).ok_or_else(|| Error::Unsupported("it isn't a ZIP, 7z or RAR archive".into()))?;
    let entries = match format {
        Format::Zip => zip_list(path)?,
        Format::SevenZ => sz_list(path, password)?,
        Format::Rar => rar_list(path, password)?,
    };
    Ok(Listing { format, entries })
}

/// Unpacks `which` entries of the archive (all, with `None`) into `dest`,
/// keeping their folders. See the top of this file for what's refused.
pub fn extract(
    path: &Path,
    listing: &Listing,
    dest: &Path,
    which: Option<&[usize]>,
    password: Option<&str>,
    limits: Limits,
    progress: &Arc<Progress>,
) -> Result<Report, Error> {
    extract_inner(path, listing, dest, which, password, limits, progress, None)
}

/// [`extract`], with `strip` (the one folder everything is in) left off
/// the front of every name.
#[allow(clippy::too_many_arguments)]
fn extract_inner(
    path: &Path,
    listing: &Listing,
    dest: &Path,
    which: Option<&[usize]>,
    password: Option<&str>,
    limits: Limits,
    progress: &Arc<Progress>,
    strip: Option<PathBuf>,
) -> Result<Report, Error> {
    let dest = fs::canonicalize(dest)?;
    if !dest.is_dir() {
        return Err(Error::Io("The folder to extract into isn't there.".into()));
    }
    if listing.entries.len() > limits.max_entries {
        return Err(Error::TooBig(format!("It holds {} entries; more than {} isn't unpacked.", listing.entries.len(), limits.max_entries)));
    }
    let wanted: Vec<bool> = match which {
        Some(w) => (0..listing.entries.len()).map(|i| w.contains(&i)).collect(),
        None => vec![true; listing.entries.len()],
    };
    let total = listing.entries.iter().zip(&wanted).filter(|(_, w)| **w).map(|(e, _)| e.size).fold(0u64, u64::saturating_add);
    if total > limits.max_total {
        return Err(Error::TooBig(format!(
            "It would take {} to unpack, more than there is room for ({}).",
            human_size(total),
            human_size(limits.max_total)
        )));
    }
    if password.is_none() && listing.entries.iter().zip(&wanted).any(|(e, w)| *w && e.encrypted) {
        return Err(Error::NeedPassword);
    }
    progress.total.store(total, Ordering::Relaxed);
    progress.done.store(0, Ordering::Relaxed);
    // Every part of a split one counts (RAR volumes are added as they're found).
    let archive_size = volumes::Parts::open(path).map(|p| p.len()).unwrap_or(0);
    let mut out = Out::new(dest, limits, archive_size, progress.clone());
    out.state.borrow_mut().strip = strip;
    let result = match listing.format {
        Format::Zip => zip_extract(path, listing, &wanted, password, &mut out),
        Format::SevenZ => sz_extract(path, &wanted, password, &mut out),
        Format::Rar => rar_extract(path, &wanted, password, &mut out),
    };
    // A file cut short by the error goes (it was only ever under a hidden
    // name); what came out whole stays.
    if result.is_err() {
        let mut st = out.state.borrow_mut();
        for (hidden, _) in [st.partial.take(), st.pending.take()].into_iter().flatten() {
            let _ = fs::remove_file(hidden);
        }
    }
    result?;
    out.settle_pending();
    out.finish_links();
    let report = out.state.borrow().report.clone();
    Ok(report)
}

/// Unpacks the whole archive beside itself, as File Explorer's Extract Here
/// does: one folder inside comes out as that folder; loose files come out
/// together in a folder named after the archive. Nothing there is touched:
/// it unpacks into a hidden folder first, then renames into a free name.
/// Gives the line to show, and where it went.
///
/// The hidden folder is only ever renamed where it is: once sandboxed
/// (Landlock), a program can't move things from one folder to another. So
/// the one folder inside is not moved out; its name is left off the front
/// of every entry as it's unpacked.
pub fn extract_here(path: &Path, password: Option<&str>, progress: &Arc<Progress>) -> Result<(String, PathBuf), Error> {
    let listing = open(path, password)?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let top = common_top(&listing);
    let want = top.clone().unwrap_or_else(|| archive_stem(&name).to_owned());
    let work = (0..).map(|n| dir.join(format!(".ferro-extract-{}-{n}", std::process::id()))).find(|p| !p.exists()).expect("a free name");
    fs::create_dir(&work)?;
    let result = extract_inner(path, &listing, &work, None, password, Limits::for_dest(dir), progress, top.map(PathBuf::from)).and_then(|report| {
        if fs::read_dir(&work)?.next().is_none() {
            return Err(Error::Io(format!("'{name}' is empty.")));
        }
        let to = rename_free(&work, dir, &want)?;
        let shown = to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut line = format!("Extracted '{name}' into '{shown}'.");
        if let Some(extra) = report.summary().split_once(". ").map(|(_, rest)| rest) {
            line = format!("{line} {extra}");
        }
        Ok((line, to))
    });
    let _ = fs::remove_dir_all(&work);
    result
}

/// The one folder every entry is in (`Game/` in `Game/run.sh`,
/// `Game/data/x`), if there is one: Extract Here then makes that folder,
/// not a folder of the same name around it.
pub fn common_top(listing: &Listing) -> Option<String> {
    let mut top: Option<String> = None;
    for e in &listing.entries {
        let Some(rel) = safe_path(&e.name) else { continue };
        let mut parts = rel.components();
        let first = parts.next()?.as_os_str().to_string_lossy().into_owned();
        // A file (or link) by that name, not a folder: no folder to keep.
        if parts.next().is_none() && !e.is_dir {
            return None;
        }
        match &top {
            Some(t) if *t != first => return None,
            Some(_) => {}
            None => top = Some(first),
        }
    }
    top
}

/// Renames `from` to `name` in `dir` (its own folder), or to the first of
/// `name (2)`, `name (3)`... not taken: never over anything there.
fn rename_free(from: &Path, dir: &Path, name: &str) -> Result<PathBuf, Error> {
    use std::os::unix::ffi::OsStrExt;
    let old = std::ffi::CString::new(from.as_os_str().as_bytes()).map_err(|_| Error::Io("A name has a NUL in it.".into()))?;
    for n in 1.. {
        let to = if n == 1 { dir.join(name) } else { dir.join(format!("{name} ({n})")) };
        if fs::symlink_metadata(&to).is_ok() {
            continue;
        }
        let new = std::ffi::CString::new(to.as_os_str().as_bytes()).map_err(|_| Error::Io("A name has a NUL in it.".into()))?;
        // SAFETY: renameat2 reads two NUL-ended paths that live through the call.
        let r = unsafe { libc::syscall(libc::SYS_renameat2, libc::AT_FDCWD, old.as_ptr(), libc::AT_FDCWD, new.as_ptr(), libc::RENAME_NOREPLACE) };
        if r == 0 {
            return Ok(to);
        }
        let e = io::Error::last_os_error();
        match e.raw_os_error() {
            // Taken just now: the next name.
            Some(libc::EEXIST) => continue,
            // A filesystem without RENAME_NOREPLACE: checked just above.
            Some(libc::EINVAL) => return fs::rename(from, &to).map(|()| to).map_err(Into::into),
            _ => return Err(e.into()),
        }
    }
    unreachable!()
}

/// `photos.zip` -> `photos`.
pub fn stem(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if i > 0 => &name[..i],
        _ => name,
    }
}

/// [`stem`] for an archive's name, without what numbers a part of a split
/// one either: `Film` for `Film.part2.rar`, `Film.r00` and `Film.7z.001`.
pub fn archive_stem(name: &str) -> &str {
    if let Some(p) = volumes::rar_part(name) {
        return &name[..p.base.len()];
    }
    match volumes::numbered(name) {
        Some((base, _, _)) => stem(base),
        None => stem(name),
    }
}

/// `name` in `dir`, or the first of `name (2)`, `name (3)`... not taken.
pub fn free_name(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if fs::symlink_metadata(&first).is_err() {
        return first;
    }
    (2..).map(|n| dir.join(format!("{name} ({n})"))).find(|p| fs::symlink_metadata(p).is_err()).expect("some name is free")
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u + 1 < UNITS.len() {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} bytes")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

// ---------------------------------------------------------------------------
// Safe names

/// An archive's name for something, as a path inside the folder: only plain
/// names, with `\` taken as a separator too (zips made on Windows). `None`
/// for anything that would leave the folder: `..`, a leading `/`, a drive
/// letter (`C:`), a NUL; and for a name with nothing left (`./`).
pub fn safe_path(name: &str) -> Option<PathBuf> {
    if name.contains('\0') {
        return None;
    }
    let name = name.replace('\\', "/");
    if name.starts_with('/') {
        return None;
    }
    let mut out = PathBuf::new();
    for (i, part) in name.split('/').enumerate() {
        match part {
            "" | "." => continue,
            ".." => return None,
            p if i == 0 && p.len() >= 2 && p.as_bytes()[0].is_ascii_alphabetic() && p.as_bytes()[1] == b':' => return None,
            p => out.push(p),
        }
    }
    // Belt and braces: what was built is plain names only.
    if out.as_os_str().is_empty() || !out.components().all(|c| matches!(c, Component::Normal(_))) {
        return None;
    }
    Some(out)
}

/// Whether a file by this name is a program, script, shortcut or installer:
/// those aren't opened straight from inside an archive (only extracted).
pub fn is_runnable_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    // No extension (README) opens as a file: it's never left executable.
    let ext = n.rsplit_once('.').map_or("", |(_, e)| e);
    [
        "desktop", "exe", "msi", "bat", "cmd", "com", "scr", "lnk", "pif", "appimage", "sh", "bash", "run", "bin", "deb", "rpm", "jar", "py", "pl",
        "rb", "js", "vbs", "ps1", "elf", "so", "ko",
    ]
    .contains(&ext)
}

/// Whether a link at `at` (inside the folder) pointing to `target` stays
/// inside: a relative target whose `..`s all come first and climb no higher
/// than the folder. Links are made last and folders on the way are real,
/// so every link checked this way points inside.
pub fn link_inside(at: &Path, target: &str) -> bool {
    if target.is_empty() || target.contains('\0') || target.starts_with('/') || target.contains('\\') {
        return false;
    }
    let depth = at.components().count().saturating_sub(1);
    let mut ups = 0;
    let mut down = false;
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." if down => return false,
            ".." => ups += 1,
            p if p.len() >= 2 && p.as_bytes()[1] == b':' => return false,
            _ => down = true,
        }
    }
    ups <= depth
}

// ---------------------------------------------------------------------------
// Writing out

struct OutState {
    dest: PathBuf,
    limits: Limits,
    archive_size: u64,
    written: u64,
    progress: Arc<Progress>,
    report: Report,
    links: Vec<(PathBuf, String)>,
    /// The file being written: its hidden name while it's written, and the
    /// name it gets when it's whole. Removed if it fails part way.
    partial: Option<(PathBuf, PathBuf)>,
    /// Written whole but not yet checked by the decoder (RAR checks after
    /// it lets go of the writer): renamed when the next file starts, or at
    /// the end; removed if the decoder fails.
    pending: Option<(PathBuf, PathBuf)>,
    /// For hidden names.
    serial: u64,
    /// Why writing stopped, if it was us (the decoders see only an io error).
    stopped: Option<Error>,
    /// Left off the front of every name (see `extract_here`).
    strip: Option<PathBuf>,
}

/// Where an entry goes.
enum Place {
    At(PathBuf),
    /// The folder being stripped off: the destination itself.
    Root,
    Unsafe,
}

/// Where entries go: shared with the writers handed to the decoders.
struct Out {
    state: Rc<RefCell<OutState>>,
}

impl Out {
    fn new(dest: PathBuf, limits: Limits, archive_size: u64, progress: Arc<Progress>) -> Self {
        let state = OutState {
            dest,
            limits,
            archive_size,
            written: 0,
            progress,
            report: Report::default(),
            links: Vec::new(),
            partial: None,
            pending: None,
            serial: 0,
            stopped: None,
            strip: None,
        };
        Out { state: Rc::new(RefCell::new(state)) }
    }

    fn cancelled(&self) -> bool {
        self.state.borrow().progress.cancel.load(Ordering::Relaxed)
    }

    /// The error that stopped things: ours if we stopped them, else `e`.
    fn why(&self, e: Error) -> Error {
        self.state.borrow_mut().stopped.take().unwrap_or(e)
    }

    /// Where entry `name` goes, inside the destination.
    fn place(&self, name: &str) -> Place {
        let Some(rel) = safe_path(name) else { return Place::Unsafe };
        match &self.state.borrow().strip {
            Some(top) => match rel.strip_prefix(top) {
                Ok(r) if r.as_os_str().is_empty() => Place::Root,
                Ok(r) => Place::At(r.to_path_buf()),
                Err(_) => Place::At(rel),
            },
            None => Place::At(rel),
        }
    }

    /// A folder, made (with those it's in) if it isn't there.
    fn folder(&self, name: &str) -> Result<(), Error> {
        let rel = match self.place(name) {
            Place::At(rel) => rel,
            Place::Root => return Ok(()),
            Place::Unsafe => {
                self.state.borrow_mut().report.unsafe_names.push(name.to_owned());
                return Ok(());
            }
        };
        let dest = self.state.borrow().dest.clone();
        match make_folders(&dest, &rel) {
            Ok(true) => self.state.borrow_mut().report.folders += 1,
            Ok(false) => {}
            Err(Unsafe) => self.state.borrow_mut().report.unsafe_names.push(name.to_owned()),
        }
        Ok(())
    }

    /// A new file for entry `name`, or `None` to skip it (unsafe, or
    /// something's already there). `size` is what it says it holds: more
    /// than that is refused.
    fn file(&self, name: &str, size: u64, modified: Option<i64>, executable: bool) -> Result<Option<Sink>, Error> {
        if self.cancelled() {
            return Err(Error::Cancelled);
        }
        let Place::At(rel) = self.place(name) else {
            self.state.borrow_mut().report.unsafe_names.push(name.to_owned());
            return Ok(None);
        };
        let dest = self.state.borrow().dest.clone();
        if let Some(parent) = rel.parent().filter(|p| !p.as_os_str().is_empty()) {
            if make_folders(&dest, parent).is_err() {
                self.state.borrow_mut().report.unsafe_names.push(name.to_owned());
                return Ok(None);
            }
        }
        // The decoder has moved on: the last file checked out.
        self.settle_pending();
        let path = dest.join(&rel);
        if fs::symlink_metadata(&path).is_ok() {
            self.state.borrow_mut().report.existing += 1;
            return Ok(None);
        }
        // Written under a hidden name beside where it goes, renamed when whole.
        let (file, hidden) = loop {
            let n = {
                let mut st = self.state.borrow_mut();
                st.serial += 1;
                st.serial
            };
            let hidden = path.with_file_name(format!(".ferro-part-{}-{n}", std::process::id()));
            match new_file(&hidden, executable) {
                Ok(f) => break (f, hidden),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        };
        {
            let mut st = self.state.borrow_mut();
            st.partial = Some((hidden.clone(), path.clone()));
            *st.progress.current.lock().unwrap_or_else(|p| p.into_inner()) = name.to_owned();
        }
        Ok(Some(Sink { file: Some(file), hidden, path, said: size, got: 0, modified, state: self.state.clone() }))
    }

    /// A link, made at the end if it points inside.
    fn link(&self, name: &str, target: &str) {
        let place = self.place(name);
        let mut st = self.state.borrow_mut();
        match place {
            Place::At(rel) if link_inside(&rel, target) => st.links.push((rel, target.to_owned())),
            Place::At(_) | Place::Root => st.report.skipped_links += 1,
            Place::Unsafe => st.report.unsafe_names.push(name.to_owned()),
        }
    }

    /// The file waiting for the decoder's check: given its name.
    fn settle_pending(&self) {
        let pending = self.state.borrow_mut().pending.take();
        if let Some((hidden, path)) = pending {
            publish(&self.state, &hidden, &path);
        }
    }

    fn skip_link(&self) {
        self.state.borrow_mut().report.skipped_links += 1;
    }

    fn finish_links(&self) {
        let mut st = self.state.borrow_mut();
        let links = std::mem::take(&mut st.links);
        let dest = st.dest.clone();
        for (rel, target) in links {
            let ok = rel.parent().filter(|p| !p.as_os_str().is_empty()).is_none_or(|p| make_folders(&dest, p).is_ok())
                && std::os::unix::fs::symlink(&target, dest.join(&rel)).is_ok();
            if ok {
                st.report.links += 1;
            } else {
                st.report.skipped_links += 1;
            }
        }
    }
}

struct Unsafe;

/// Makes `rel`'s folders inside `dest`, each a real folder (a link or a file
/// on the way is refused). Whether the last one was new.
fn make_folders(dest: &Path, rel: &Path) -> Result<bool, Unsafe> {
    let mut at = dest.to_path_buf();
    let mut made = false;
    for part in rel.components() {
        let Component::Normal(p) = part else { return Err(Unsafe) };
        at.push(p);
        match fs::symlink_metadata(&at) {
            Ok(m) if m.file_type().is_dir() => made = false,
            Ok(_) => return Err(Unsafe),
            Err(_) => {
                fs::create_dir(&at).map_err(|_| Unsafe)?;
                made = true;
            }
        }
    }
    Ok(made)
}

/// A new file, never through a link left there and never over a file.
/// Never runnable, whatever the archive says: FerroOS trusts a shortcut or a
/// program to run on a double-click by its execute bit, and what comes out
/// of someone else's archive mustn't have one (programs are installed with
/// Install, which keeps them).
fn new_file(path: &Path, _executable: bool) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new().write(true).create_new(true).custom_flags(libc::O_NOFOLLOW).mode(0o644).open(path)
}

/// Renames `from` to `to`, never over anything there. Same-folder renames
/// only: those are what the sandbox (Landlock) allows.
pub fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let c = |p: &Path| std::ffi::CString::new(p.as_os_str().as_bytes()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput));
    let (old, new) = (c(from)?, c(to)?);
    // SAFETY: renameat2 reads two NUL-ended paths that live through the call.
    let r = unsafe { libc::syscall(libc::SYS_renameat2, libc::AT_FDCWD, old.as_ptr(), libc::AT_FDCWD, new.as_ptr(), libc::RENAME_NOREPLACE) };
    if r == 0 {
        return Ok(());
    }
    let e = io::Error::last_os_error();
    if e.raw_os_error() == Some(libc::EINVAL) && fs::symlink_metadata(to).is_err() {
        // A filesystem without RENAME_NOREPLACE: checked just before.
        return fs::rename(from, to);
    }
    Err(e)
}

/// A whole file given its name; counted (or, if something got there first,
/// let go).
fn publish(state: &Rc<RefCell<OutState>>, hidden: &Path, path: &Path) {
    let mut st = state.borrow_mut();
    match rename_noreplace(hidden, path) {
        Ok(()) => st.report.files += 1,
        Err(_) => {
            let _ = fs::remove_file(hidden);
            st.report.existing += 1;
        }
    }
}

/// One file being written; counts every byte against the limits.
struct Sink {
    file: Option<File>,
    /// Where it's written, and where it goes when whole.
    hidden: PathBuf,
    path: PathBuf,
    said: u64,
    got: u64,
    modified: Option<i64>,
    state: Rc<RefCell<OutState>>,
}

impl Sink {
    fn stop(&self, why: Error) -> io::Error {
        let msg = why.to_string();
        self.state.borrow_mut().stopped = Some(why);
        io::Error::other(msg)
    }

    /// Done and checked: dated and given its name.
    fn finish(mut self) -> Result<(), Error> {
        self.close();
        self.state.borrow_mut().partial = None;
        publish(&self.state, &self.hidden, &self.path);
        Ok(())
    }

    /// Dated; written whole.
    fn close(&mut self) {
        let Some(file) = self.file.take() else { return };
        if let Some(t) = self.modified.and_then(to_system_time) {
            let _ = file.set_modified(t);
        }
    }
}

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = buf.len() as u64;
        {
            let st = self.state.borrow();
            if st.progress.cancel.load(Ordering::Relaxed) {
                drop(st);
                return Err(self.stop(Error::Cancelled));
            }
            if self.got + n > self.said {
                drop(st);
                return Err(self.stop(Error::Damaged("an entry holds more than it says".into())));
            }
            let total = st.written + n;
            if total > st.limits.max_total {
                let why = format!("Stopped: it unpacks to more than there is room for ({}).", human_size(st.limits.max_total));
                drop(st);
                return Err(self.stop(Error::TooBig(why)));
            }
            if total > st.limits.ratio_after && total / st.archive_size.max(1) > st.limits.max_ratio {
                let why = format!("Stopped: it unpacks to over {} times its size, as a zip bomb does.", st.limits.max_ratio);
                drop(st);
                return Err(self.stop(Error::TooBig(why)));
            }
        }
        let Some(file) = self.file.as_mut() else { return Err(io::Error::other("closed")) };
        file.write_all(buf)?;
        self.got += n;
        let mut st = self.state.borrow_mut();
        st.written += n;
        st.progress.done.fetch_add(n, Ordering::Relaxed);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for Sink {
    fn drop(&mut self) {
        // Handed to a decoder (RAR), a sink ends when it's dropped, before
        // the decoder has checked it: it stays partial (removed if the
        // decoder then fails) until the next file starts.
        if self.file.is_some() && self.state.borrow().stopped.is_none() && self.got == self.said {
            self.close();
            let mut st = self.state.borrow_mut();
            st.partial = None;
            st.pending = Some((self.hidden.clone(), self.path.clone()));
        }
    }
}

fn to_system_time(secs: i64) -> Option<SystemTime> {
    if secs >= 0 {
        UNIX_EPOCH.checked_add(Duration::from_secs(secs as u64))
    } else {
        UNIX_EPOCH.checked_sub(Duration::from_secs(secs.unsigned_abs()))
    }
}

pub(crate) fn unix_secs(t: SystemTime) -> Option<i64> {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_secs()).ok(),
        Err(e) => i64::try_from(e.duration().as_secs()).ok().map(|s| -s),
    }
}

/// Seconds since 1970 for a local date and time (as zips keep them).
fn local_to_unix(year: u16, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> Option<i64> {
    // SAFETY: mktime reads and normalises the struct given; it lives
    // through the call.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = i32::from(year) - 1900;
    tm.tm_mon = i32::from(month) - 1;
    tm.tm_mday = i32::from(day);
    tm.tm_hour = i32::from(hour);
    tm.tm_min = i32::from(minute);
    tm.tm_sec = i32::from(second);
    tm.tm_isdst = -1;
    let t = unsafe { libc::mktime(&mut tm) };
    (t != -1).then_some(t)
}

/// `2024-03-09 14:05`, in local time.
// time_t: the libc crate warns that musl's will change; it's 64 bits here.
#[allow(deprecated)]
pub fn format_time(secs: i64) -> String {
    let t = secs as libc::time_t;
    // SAFETY: localtime_r fills the struct given; both live through the call.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return String::new();
    }
    format!("{:04}-{:02}-{:02} {:02}:{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min)
}

// ---------------------------------------------------------------------------
// ZIP

const S_IFMT: u32 = 0o170000;
const S_IFLNK: u32 = 0o120000;

pub(crate) fn zip_err(e: zip::result::ZipError) -> Error {
    use zip::result::ZipError;
    match e {
        ZipError::InvalidPassword => Error::WrongPassword,
        ZipError::UnsupportedArchive(m) if m == ZipError::PASSWORD_REQUIRED => Error::NeedPassword,
        ZipError::UnsupportedArchive(m) => Error::Unsupported(m.to_lowercase()),
        ZipError::CompressionMethodNotSupported(m) => Error::Unsupported(format!("compression method {m:?} isn't supported")),
        ZipError::InvalidArchive(m) => Error::Damaged(m.to_lowercase()),
        ZipError::Io(e) => Error::Damaged(e.to_string()),
        ZipError::FileNotFound => Error::Damaged("an entry is missing".into()),
        other => Error::Damaged(other.to_string()),
    }
}

/// The archive's bytes: one file, or its numbered pieces end to end.
type Source = io::BufReader<volumes::Parts>;

fn source(path: &Path) -> Result<Source, Error> {
    Ok(io::BufReader::new(volumes::Parts::open(path)?))
}

fn zip_open(path: &Path) -> Result<zip::ZipArchive<Source>, Error> {
    zip::ZipArchive::new(source(path)?).map_err(zip_err)
}

fn zip_time(f: &zip::read::ZipFile<'_, Source>) -> Option<i64> {
    use zip::extra_fields::ExtraField;
    // The exact time (UTC) where the zip has it; the DOS one is local, to
    // two seconds, and some zippers get summer time wrong in it.
    for x in f.extra_data_fields() {
        match x {
            ExtraField::ExtendedTimestamp(t) => {
                if let Some(m) = t.mod_time() {
                    return Some(i64::from(m));
                }
            }
            ExtraField::Ntfs(n) if n.mtime() > 0 => return unix_secs(sevenz_rust2::NtTime::new(n.mtime()).into()),
            _ => {}
        }
    }
    let d = f.last_modified()?;
    local_to_unix(d.year(), d.month(), d.day(), d.hour(), d.minute(), d.second())
}

fn zip_list(path: &Path) -> Result<Vec<Entry>, Error> {
    let mut z = zip_open(path)?;
    let mut out = Vec::with_capacity(z.len());
    for i in 0..z.len() {
        let f = z.by_index_raw(i).map_err(zip_err)?;
        let is_link = f.unix_mode().is_some_and(|m| m & S_IFMT == S_IFLNK);
        out.push(Entry {
            name: f.name().map_err(zip_err)?.replace('\\', "/"),
            size: f.size(),
            packed: f.compressed_size(),
            modified: zip_time(&f),
            is_dir: f.is_dir(),
            is_link,
            encrypted: f.encrypted(),
        });
    }
    Ok(out)
}

fn zip_extract(path: &Path, listing: &Listing, wanted: &[bool], password: Option<&str>, out: &mut Out) -> Result<(), Error> {
    let mut z = zip_open(path)?;
    for (i, e) in listing.entries.iter().enumerate() {
        if !wanted[i] {
            continue;
        }
        if out.cancelled() {
            return Err(Error::Cancelled);
        }
        if e.is_dir {
            out.folder(&e.name)?;
            continue;
        }
        let mut f = match (e.encrypted, password) {
            (true, Some(pw)) => z.by_index_decrypt(i, pw.as_bytes()),
            (true, None) => return Err(Error::NeedPassword),
            _ => z.by_index(i),
        }
        .map_err(zip_err)?;
        if e.is_link {
            // A link's target is its contents; a sensible one is short.
            let mut target = String::new();
            (&mut f).take(4096).read_to_string(&mut target).map_err(|_| Error::Damaged("a link's target can't be read".into()))?;
            out.link(&e.name, &target);
            continue;
        }
        let exec = f.unix_mode().is_some_and(|m| m & 0o111 != 0);
        let Some(mut sink) = out.file(&e.name, e.size, e.modified, exec)? else { continue };
        match io::copy(&mut f, &mut sink) {
            Ok(_) if sink.got == sink.said => sink.finish()?,
            Ok(_) => return Err(Error::Damaged("an entry holds less than it says".into())),
            Err(err) => {
                drop(sink);
                let why = out.why(Error::Damaged(err.to_string()));
                // ZipCrypto lets 1 wrong password in 256 through, to fail
                // on the checksum.
                return Err(if e.encrypted && matches!(why, Error::Damaged(_)) { Error::WrongPassword } else { why });
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 7z

pub(crate) fn sz_err(e: sevenz_rust2::Error) -> Error {
    use sevenz_rust2::Error as E;
    match e {
        E::PasswordRequired => Error::NeedPassword,
        E::MaybeBadPassword(_) => Error::WrongPassword,
        E::BadSignature(_) => Error::Unsupported("it isn't a 7z archive".into()),
        E::UnsupportedVersion { .. } => Error::Unsupported("it's from a newer 7-Zip".into()),
        E::UnsupportedCompressionMethod(m) => Error::Unsupported(format!("compression method {m} isn't supported")),
        E::Unsupported(m) => Error::Unsupported(m.into_owned()),
        E::MaxMemLimited { .. } => Error::TooBig("It needs more memory to unpack than is allowed.".into()),
        E::ChecksumVerificationFailed => Error::Damaged("a checksum doesn't match".into()),
        E::FileOpen(e, _) => Error::Io(format!("Couldn't open it: {e}")),
        other => Error::Damaged(other.to_string()),
    }
}

pub(crate) fn sz_password(password: Option<&str>) -> sevenz_rust2::Password {
    password.map_or_else(sevenz_rust2::Password::empty, sevenz_rust2::Password::new)
}

fn sz_list(path: &Path, password: Option<&str>) -> Result<Vec<Entry>, Error> {
    let reader = sevenz_rust2::ArchiveReader::new(source(path)?, sz_password(password)).map_err(|e| match sz_err(e) {
        // Encrypted names that don't decrypt to a list: the wrong password.
        Error::Damaged(_) if password.is_some() => Error::WrongPassword,
        e => e,
    })?;
    let a = reader.archive();
    let encrypted_block: Vec<bool> =
        a.blocks.iter().map(|b| b.coders.iter().any(|c| c.encoder_method_id() == sevenz_rust2::EncoderMethod::ID_AES256_SHA256)).collect();
    Ok(a.files
        .iter()
        .enumerate()
        .filter(|(_, f)| !f.is_anti_item)
        .map(|(i, f)| {
            let block = a.stream_map.file_block_index.get(i).copied().flatten();
            Entry {
                name: f.name.replace('\\', "/"),
                size: f.size,
                packed: f.compressed_size,
                modified: f.has_last_modified_date.then(|| unix_secs(f.last_modified_date.into())).flatten(),
                is_dir: f.is_directory,
                is_link: sz_is_link(f),
                encrypted: block.is_some_and(|b| encrypted_block.get(b).copied().unwrap_or(false)),
            }
        })
        .collect())
}

/// p7zip keeps a Unix mode in the attributes' top half (with 0x8000 set).
fn sz_unix_mode(f: &sevenz_rust2::ArchiveEntry) -> Option<u32> {
    (f.has_windows_attributes && f.windows_attributes & 0x8000 != 0).then_some(f.windows_attributes >> 16)
}

fn sz_is_link(f: &sevenz_rust2::ArchiveEntry) -> bool {
    sz_unix_mode(f).is_some_and(|m| m & S_IFMT == S_IFLNK)
}

/// The memory a block's decoders ask for (LZMA's dictionary can be 4 GB,
/// PPMd's model as big): over 1 GB is refused before decoding starts.
fn sz_memory_check(a: &sevenz_rust2::Archive) -> Result<(), Error> {
    use sevenz_rust2::EncoderMethod as M;
    let le32 = |p: &[u8]| p.get(1..5).map_or(0, |b| u64::from(u32::from_le_bytes([b[0], b[1], b[2], b[3]])));
    for c in a.blocks.iter().flat_map(|b| &b.coders) {
        let p = c.properties();
        let need = match c.encoder_method_id() {
            M::ID_LZMA | M::ID_PPMD => le32(p),
            M::ID_LZMA2 => match p.first().map(|b| b & 0x3F) {
                Some(40..) => u64::from(u32::MAX),
                Some(b) => (2 | u64::from(b & 1)) << (b / 2 + 11),
                None => 0,
            },
            _ => 0,
        };
        if need > GB {
            return Err(Error::TooBig(format!("It needs {} of memory to unpack; more than 1 GB isn't allowed.", human_size(need))));
        }
    }
    Ok(())
}

fn sz_extract(path: &Path, wanted: &[bool], password: Option<&str>, out: &mut Out) -> Result<(), Error> {
    let mut reader = sevenz_rust2::ArchiveReader::new(source(path)?, sz_password(password)).map_err(sz_err)?;
    sz_memory_check(reader.archive())?;
    // The listing leaves anti-items out; entries are matched by address.
    let mut index = HashMap::new();
    let mut n = 0;
    for f in &reader.archive().files {
        if !f.is_anti_item {
            index.insert(f as *const sevenz_rust2::ArchiveEntry as usize, n);
            n += 1;
        }
    }
    let mut failed: Option<Error> = None;
    let result = reader.for_each_entries(|entry, data| {
        // A stop ends a block only; the reader goes on to the next.
        if failed.is_some() {
            return Ok(false);
        }
        let i = index.get(&(entry as *const sevenz_rust2::ArchiveEntry as usize)).copied();
        let wanted = i.is_some_and(|i| wanted.get(i).copied().unwrap_or(false));
        if out.cancelled() {
            failed = Some(Error::Cancelled);
            return Ok(false);
        }
        if !wanted || entry.is_anti_item {
            // The next entry's data follows this one's: read through it.
            io::copy(data, &mut io::sink())?;
            return Ok(true);
        }
        let name = entry.name.replace('\\', "/");
        if entry.is_directory {
            if let Err(e) = out.folder(&name) {
                failed = Some(e);
                return Ok(false);
            }
            return Ok(true);
        }
        if sz_is_link(entry) {
            let mut target = String::new();
            data.take(4096).read_to_string(&mut target)?;
            io::copy(data, &mut io::sink())?;
            out.link(&name, &target);
            return Ok(true);
        }
        let modified = entry.has_last_modified_date.then(|| unix_secs(entry.last_modified_date.into())).flatten();
        let exec = sz_unix_mode(entry).is_some_and(|m| m & 0o111 != 0);
        match out.file(&name, entry.size, modified, exec) {
            Ok(Some(mut sink)) => match io::copy(data, &mut sink) {
                Ok(_) if sink.got == sink.said => {
                    let _ = sink.finish();
                    Ok(true)
                }
                Ok(_) => {
                    failed = Some(Error::Damaged("an entry holds less than it says".into()));
                    Ok(false)
                }
                Err(e) => {
                    drop(sink);
                    if let Some(why) = out.state.borrow_mut().stopped.take() {
                        failed = Some(why);
                        return Ok(false);
                    }
                    Err(e.into())
                }
            },
            Ok(None) => {
                io::copy(data, &mut io::sink())?;
                Ok(true)
            }
            Err(e) => {
                failed = Some(e);
                Ok(false)
            }
        }
    });
    if let Some(e) = failed {
        return Err(e);
    }
    result.map_err(|e| {
        let e = sz_err(e);
        // A bad password shows as data that doesn't check out.
        if password.is_some() && matches!(e, Error::Damaged(_)) {
            Error::WrongPassword
        } else {
            e
        }
    })
}

// ---------------------------------------------------------------------------
// RAR

fn rar_err(e: rars::Error) -> Error {
    use rars::ErrorKind as K;
    if e.to_string().contains("multivolume") {
        return Error::Damaged("a file in it carries on in another part, but it isn't marked as one part of several".into());
    }
    match e.kind() {
        K::PasswordRequired => Error::NeedPassword,
        K::BadPassword => Error::WrongPassword,
        K::UnsupportedFormat => Error::Unsupported("it isn't a RAR archive".into()),
        K::UnsupportedFeature => Error::Unsupported(e.to_string()),
        K::ResourceLimit => Error::TooBig(format!("Stopped: {e}.")),
        K::Cancelled => Error::Cancelled,
        K::Io => Error::Io(format!("Couldn't read it: {e}")),
        _ => Error::Damaged(e.to_string()),
    }
}

fn rar_options(password: Option<&str>) -> rars::ArchiveReadOptions<'_> {
    let mut o = rars::ArchiveReadOptions::with_optional_password(password.map(str::as_bytes));
    // No archive's headers need more than this; past it, it's an attack.
    o = o.with_max_header_count(2_000_000).with_max_header_bytes(512 << 20);
    o
}

fn rar_name(m: &rars::ArchiveMember) -> String {
    let raw = m.decoded_name(None).map(|n| n.into_owned()).unwrap_or_else(|_| m.meta.name.clone());
    String::from_utf8_lossy(&raw).replace('\\', "/")
}

/// One file as a RAR archive (its numbered pieces end to end, if it's cut
/// into `.rar.001`...).
fn rar_open(path: &Path, password: Option<&str>) -> Result<rars::Archive, Error> {
    let read = |password: Option<&str>| {
        if volumes::is_numbered_piece(path) {
            return rars::ArchiveReader::read_reader_with_options(volumes::Parts::open(path)?, rar_options(password)).map_err(rar_err);
        }
        rars::ArchiveReader::read_path_with_options(path, rar_options(password)).map_err(rar_err)
    };
    match read(password) {
        // Older RARs with their names encrypted have nothing to check the
        // password by: a wrong one shows as headers that make no sense.
        Err(Error::Damaged(_)) if password.is_some() && read(None).is_err_and(|e| e == Error::NeedPassword) => Err(Error::WrongPassword),
        other => other,
    }
}

/// Whether the archive says it's one volume of several.
fn rar_is_volume(a: &rars::Archive) -> bool {
    match a {
        rars::Archive::Rar13(a) => a.main.is_volume(),
        rars::Archive::Rar15To40(a) => a.main.is_volume(),
        rars::Archive::Rar50Plus(a) => a.main.is_volume(),
        _ => false,
    }
}

/// Whether another volume follows this one: what its end says, or (older
/// ones don't always) that its last file carries on.
fn rar_has_next(a: &rars::Archive) -> Option<bool> {
    const EARC_NEXT_VOLUME: u16 = 0x0001;
    let split = a.members().last().is_some_and(|m| m.meta.is_split_after);
    let end = match a {
        rars::Archive::Rar13(_) => None,
        rars::Archive::Rar15To40(a) => a.blocks.iter().find_map(|b| match b {
            rars::rar15_40::Block::End(h) => Some(h.flags & EARC_NEXT_VOLUME != 0),
            _ => None,
        }),
        rars::Archive::Rar50Plus(a) => a.blocks.iter().find_map(|b| match b {
            rars::rar50::Block::End(h) => Some(h.has_next_volume()),
            _ => None,
        }),
        _ => None,
    };
    match (split, end) {
        (true, _) => Some(true),
        (false, end) => end,
    }
}

/// More parts than any real set has; past it, something is feeding us files.
const MAX_VOLUMES: u64 = 10_000;

/// The RAR archive `path` is, or every volume of the set it's one of, in
/// order, from whichever of them `path` is: the others are looked for
/// beside it, by name. With them, the size of all the files together.
fn rar_set(path: &Path, password: Option<&str>) -> Result<(Vec<rars::Archive>, u64), Error> {
    let size = |p: &Path| fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    let opened = rar_open(path, password)?;
    if !rar_is_volume(&opened) {
        let total = volumes::Parts::open(path).map(|p| p.len()).unwrap_or(0);
        return Ok((vec![opened], total));
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    let Some(part) = volumes::rar_part(name) else {
        return Err(Error::Unsupported(
            "it's one part of an archive split into several files, but its name doesn't say which (name.part1.rar, name.part2.rar... or name.rar, name.r00...)".into(),
        ));
    };
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let found = volumes::rar_siblings(dir, name, &part);
    let family = opened.family();
    let mut opened = Some(opened);
    let (mut set, mut total) = (Vec::new(), 0u64);
    for index in 0..MAX_VOLUMES {
        let Some(file) = found.get(&index) else {
            return Err(volumes::missing(&part.name_of(index)));
        };
        let a = match opened.take_if(|_| index == part.index) {
            Some(a) => a,
            None => rar_open(file, password)?,
        };
        if a.family() != family || !rar_is_volume(&a) {
            let shown = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            return Err(Error::Damaged(format!("'{shown}' isn't a part of the same archive as '{name}'")));
        }
        total = total.saturating_add(size(file));
        // Where it ends: what the volume says; where it doesn't, with the
        // last part there is.
        let more = rar_has_next(&a).unwrap_or_else(|| found.contains_key(&(index + 1)));
        set.push(a);
        if !more {
            // The one opened comes after the end: not of this set.
            if index < part.index {
                return Err(Error::Damaged(format!("'{name}' comes after the last part of its archive ('{}')", part.name_of(index))));
            }
            return Ok((set, total));
        }
    }
    Err(Error::TooBig(format!("It's split into more than {MAX_VOLUMES} parts; that many aren't opened.")))
}

/// The files in a set of volumes, one for each (not one for every part of
/// a file that runs across volumes).
fn rar_members(set: &[rars::Archive]) -> Result<Vec<rars::ArchiveMember>, Error> {
    match set {
        [one] => Ok(one.members().collect()),
        _ => rars::volume_members(set).map_err(rar_err),
    }
}

fn rar_list(path: &Path, password: Option<&str>) -> Result<Vec<Entry>, Error> {
    let (set, _) = rar_set(path, password)?;
    Ok(rar_members(&set)?
        .iter()
        .map(|m| Entry {
            name: rar_name(m),
            size: m.meta.unpacked_size,
            packed: m.meta.packed_size,
            modified: m.meta.modification_time().and_then(unix_secs),
            is_dir: m.meta.is_directory,
            is_link: m.meta.is_redirection,
            encrypted: m.meta.is_encrypted,
        })
        .collect())
}

fn rar_extract(path: &Path, wanted: &[bool], password: Option<&str>, out: &mut Out) -> Result<(), Error> {
    let (set, size) = rar_set(path, password)?;
    let members = rar_members(&set)?;
    let solid = members.iter().any(|m| m.is_solid());
    let mut options = rar_options(password);
    {
        let mut st = out.state.borrow_mut();
        st.archive_size = size;
        options.max_total_output_bytes = Some(st.limits.max_total);
        // RAR5's dictionary can be 64 GB; this much memory is plenty.
        options.rar50_dictionary_size_limit = Some(GB);
        options.max_reader_workspace_bytes = Some(2 * GB);
    }
    // Cancel reaches into the decoder too, not only into what it writes: a
    // member that decodes for long without output still stops.
    let token = rars::ReadCancellation::new();
    let watch = {
        let (token, progress) = (token.clone(), out.state.borrow().progress.clone());
        let done = Arc::new(AtomicBool::new(false));
        let d = done.clone();
        std::thread::spawn(move || {
            while !d.load(Ordering::Relaxed) {
                if progress.cancel.load(Ordering::Relaxed) {
                    token.cancel();
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        done
    };
    let options = options.with_cancellation(&token);
    let last = wanted.iter().rposition(|w| *w);
    let result = match set.as_slice() {
        [one] => rar_extract_one(one, options, wanted, last, solid, out),
        _ => rar_extract_volumes(&set, &members, options, &token, wanted, last, out),
    };
    watch.store(true, Ordering::Relaxed);
    if let Some(e) = out.state.borrow_mut().stopped.take() {
        return Err(e);
    }
    result
}

fn rar_extract_one(
    a: &rars::Archive,
    options: rars::ArchiveReadOptions<'_>,
    wanted: &[bool],
    last: Option<usize>,
    solid: bool,
    out: &mut Out,
) -> Result<(), Error> {
    let mut i = 0;
    let result = a.extract_with_control(options, |m| {
        let n = i;
        i += 1;
        if last.is_none_or(|l| n > l) {
            // Nothing more wanted: no need to decode the rest.
            return Ok(rars::ExtractionDecision::Stop);
        }
        let want = wanted.get(n).copied().unwrap_or(false);
        if !want {
            // A solid archive's history needs every member decoded.
            return Ok(if solid && !m.meta.is_directory && !m.meta.is_redirection {
                rars::ExtractionDecision::Extract(Box::new(io::sink()))
            } else {
                rars::ExtractionDecision::Skip
            });
        }
        if out.cancelled() {
            out.state.borrow_mut().stopped = Some(Error::Cancelled);
            return Ok(rars::ExtractionDecision::Stop);
        }
        let name = rar_name(m);
        if m.meta.is_directory {
            if let Err(e) = out.folder(&name) {
                out.state.borrow_mut().stopped = Some(e);
                return Ok(rars::ExtractionDecision::Stop);
            }
            return Ok(rars::ExtractionDecision::Skip);
        }
        if m.meta.is_redirection {
            // Links and file copies in RAR5: not made.
            out.skip_link();
            return Ok(rars::ExtractionDecision::Skip);
        }
        let exec = m.meta.attr_source() == rars::AttrSource::Unix && m.meta.file_attr & 0o111 != 0;
        let modified = m.meta.modification_time().and_then(unix_secs);
        match out.file(&name, m.meta.unpacked_size, modified, exec) {
            Ok(Some(sink)) => Ok(rars::ExtractionDecision::Extract(Box::new(sink))),
            Ok(None) if solid => Ok(rars::ExtractionDecision::Extract(Box::new(io::sink()))),
            Ok(None) => Ok(rars::ExtractionDecision::Skip),
            Err(e) => {
                out.state.borrow_mut().stopped = Some(e);
                Ok(rars::ExtractionDecision::Stop)
            }
        }
    });
    result.map(|_| ()).map_err(rar_err)
}

/// A set of volumes. The decoder goes through every file in turn, running
/// on into the next volume where a file does, and asks only where each
/// goes: what isn't wanted goes nowhere, and stopping is by cancelling.
fn rar_extract_volumes(
    set: &[rars::Archive],
    members: &[rars::ArchiveMember],
    options: rars::ArchiveReadOptions<'_>,
    token: &rars::ReadCancellation,
    wanted: &[bool],
    last: Option<usize>,
    out: &mut Out,
) -> Result<(), Error> {
    // Links and file copies (RAR5) aren't made, and aren't asked about.
    for (n, m) in members.iter().enumerate() {
        if m.meta.is_redirection && wanted.get(n).copied().unwrap_or(false) {
            out.skip_link();
        }
    }
    let mut asked = (0..members.len()).filter(|n| !members[*n].meta.is_redirection);
    // Past the last one wanted: stopped by us, with all of it out.
    let mut done = false;
    let nowhere = || -> rars::Result<Box<dyn Write>> { Ok(Box::new(io::sink())) };
    let result = rars::extract_volumes_to_with_options(set, options, |meta| {
        if done || out.state.borrow().stopped.is_some() {
            return nowhere();
        }
        let mut stop = |why: Option<Error>| {
            match why {
                Some(e) => out.state.borrow_mut().stopped = Some(e),
                None => done = true,
            }
            token.cancel();
        };
        let Some(n) = asked.next() else {
            stop(Some(Error::Damaged("its parts don't agree on what's in it".into())));
            return nowhere();
        };
        if last.is_none_or(|l| n > l) {
            stop(None);
            return nowhere();
        }
        let m = &members[n];
        if m.meta.name != meta.name || m.meta.is_directory != meta.is_directory {
            stop(Some(Error::Damaged("its parts don't agree on what's in it".into())));
            return nowhere();
        }
        if !wanted.get(n).copied().unwrap_or(false) {
            return nowhere();
        }
        if out.cancelled() {
            stop(Some(Error::Cancelled));
            return nowhere();
        }
        let name = rar_name(m);
        if m.meta.is_directory {
            if let Err(e) = out.folder(&name) {
                stop(Some(e));
            }
            return nowhere();
        }
        let exec = m.meta.attr_source() == rars::AttrSource::Unix && m.meta.file_attr & 0o111 != 0;
        let modified = m.meta.modification_time().and_then(unix_secs);
        match out.file(&name, m.meta.unpacked_size, modified, exec) {
            Ok(Some(sink)) => Ok(Box::new(sink)),
            Ok(None) => nowhere(),
            Err(e) => {
                stop(Some(e));
                nowhere()
            }
        }
    });
    match result {
        Err(e) if done && e.kind() == rars::ErrorKind::Cancelled => Ok(()),
        other => other.map_err(rar_err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_names_stay_inside() {
        assert_eq!(safe_path("a/b.txt"), Some(PathBuf::from("a/b.txt")));
        assert_eq!(safe_path("./a//b/"), Some(PathBuf::from("a/b")));
        assert_eq!(safe_path("dir\\file.txt"), Some(PathBuf::from("dir/file.txt")));
        assert_eq!(safe_path("a..b/..c"), Some(PathBuf::from("a..b/..c")));
        assert_eq!(safe_path("a/C:/x:y.txt"), Some(PathBuf::from("a/C:/x:y.txt")));
        for bad in ["../x", "a/../../x", "a/..", "/etc/passwd", "\\x", "..\\x", "C:\\x", "c:x", "a\0b", "", "./", "."] {
            assert_eq!(safe_path(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn links_point_inside_or_not_at_all() {
        let at = |s: &str| PathBuf::from(s);
        assert!(link_inside(&at("a/link"), "file"));
        assert!(link_inside(&at("a/link"), "../b/file"));
        assert!(link_inside(&at("a/b/link"), "../../c"));
        assert!(!link_inside(&at("link"), ".."));
        assert!(!link_inside(&at("a/link"), "../.."));
        assert!(!link_inside(&at("a/link"), "/etc"));
        // `..` after a name could climb back through another link.
        assert!(!link_inside(&at("a/link"), "b/../x"));
        assert!(!link_inside(&at("a/link"), ""));
        assert!(!link_inside(&at("a/link"), "C:x"));
        assert!(!link_inside(&at("a/link"), "..\\x"));
    }

    #[test]
    fn programs_are_not_opened_from_inside() {
        for n in ["setup.exe", "run.SH", "a.desktop", "Game.AppImage", "x.deb", "tool.py"] {
            assert!(is_runnable_name(n), "{n}");
        }
        for n in ["photo.jpg", "notes.txt", "film.mkv", "page.pdf", "song.mp3", "inner.zip", "README"] {
            assert!(!is_runnable_name(n), "{n}");
        }
    }

    #[test]
    fn names_and_sizes() {
        assert_eq!(stem("photos.zip"), "photos");
        assert_eq!(stem("a.b.rar"), "a.b");
        assert_eq!(stem(".hidden"), ".hidden");
        assert_eq!(archive_stem("a.b.rar"), "a.b");
        assert_eq!(archive_stem("Film.part02.rar"), "Film");
        assert_eq!(archive_stem("Film.r00"), "Film");
        assert_eq!(archive_stem("Film.7z.001"), "Film");
        assert_eq!(archive_stem("notes.zip"), "notes");
        assert_eq!(human_size(500), "500 bytes");
        assert_eq!(human_size(1536), "1.5 KB");
        let p = Progress::default();
        assert_eq!(p.percent(), 0);
        p.total.store(200, Ordering::Relaxed);
        p.done.store(50, Ordering::Relaxed);
        assert_eq!(p.percent(), 25);
    }

    #[test]
    fn report_summary() {
        let r = Report { files: 3, folders: 1, unsafe_names: vec!["../x".into()], existing: 2, ..Default::default() };
        assert_eq!(r.summary(), "Extracted 3 file(s) and 1 folder(s). Skipped 1 unsafe name(s). Kept 2 already there.");
        assert_eq!(Report { files: 1, ..Default::default() }.summary_into(Some("Photos")), "Extracted 1 file(s) into 'Photos'.");
    }
}
