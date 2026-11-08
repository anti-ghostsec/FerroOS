//! Making archives: 7z (the default) or zip, from files and folders.
//!
//! What goes in is each file's name, contents, last-changed time and
//! permission bits (rwx) only: no owner or group, no other times. The
//! archive is written under a hidden name beside where it goes and given
//! its name when it's whole; cancelled or failed, it's removed. Files are
//! read and written a piece at a time, never whole into memory.
//!
//! With a password: 7z uses AES-256 and encrypts the names too; zip uses
//! AES-256 (never the old ZipCrypto), which leaves the names readable.

use crate::{human_size, rename_noreplace, Error, Progress};
use std::fs::{self, File};
use std::io::{self, Read};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::SystemTime;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NewFormat {
    SevenZ,
    Zip,
}

impl NewFormat {
    pub fn ext(self) -> &'static str {
        match self {
            NewFormat::SevenZ => "7z",
            NewFormat::Zip => "zip",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// No compression.
    Store,
    Normal,
    /// Smaller, slower. For 7z, LZMA2 with a 16 MB dictionary: it needs
    /// about 200 MB to make, which a 1 GB machine has.
    Maximum,
}

/// What was put in.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Made {
    pub files: usize,
    pub folders: usize,
    /// Links and devices aren't put in (a link could lead anywhere).
    pub skipped: usize,
    pub bytes: u64,
}

/// One thing to put in.
struct Item {
    name: String,
    path: PathBuf,
    is_dir: bool,
    size: u64,
    modified: Option<SystemTime>,
    mode: u32,
}

/// Everything under `sources`, named from each one's own name down
/// (`Holiday/Photos/a.jpg`), in order. Links aren't followed or put in;
/// `skip` (the archive being made) is left out.
fn gather(sources: &[PathBuf], skip: &Path, made: &mut Made) -> Result<Vec<Item>, Error> {
    fn walk(path: &Path, name: String, skip: &Path, out: &mut Vec<Item>, made: &mut Made) -> Result<(), Error> {
        let m = fs::symlink_metadata(path).map_err(|e| Error::Io(format!("Couldn't read something to add: {e}")))?;
        if path == skip {
            return Ok(());
        }
        let ft = m.file_type();
        let item = |is_dir| Item {
            name: name.clone(),
            path: path.to_path_buf(),
            is_dir,
            size: if is_dir { 0 } else { m.len() },
            modified: m.modified().ok(),
            mode: m.permissions().mode() & 0o777,
        };
        if ft.is_dir() {
            out.push(item(true));
            let mut kids: Vec<_> = fs::read_dir(path).map_err(|e| Error::Io(format!("Couldn't read a folder to add: {e}")))?.flatten().collect();
            kids.sort_by_key(|k| k.file_name());
            for k in kids {
                let kid_name = format!("{name}/{}", k.file_name().to_string_lossy());
                walk(&k.path(), kid_name, skip, out, made)?;
            }
        } else if ft.is_file() {
            out.push(item(false));
        } else {
            made.skipped += 1;
        }
        Ok(())
    }
    let mut out = Vec::new();
    for s in sources {
        let name = s.file_name().map(|n| n.to_string_lossy().into_owned()).ok_or_else(|| Error::Io("Something to add has no name.".into()))?;
        walk(s, name, skip, &mut out, made)?;
    }
    if out.is_empty() {
        return Err(Error::Io("There's nothing to add.".into()));
    }
    Ok(out)
}

/// A file read a piece at a time, opened only when its turn comes (so a
/// thousand files don't hold a thousand open), counting progress and
/// stopping on Cancel.
struct Piecewise {
    path: PathBuf,
    file: Option<File>,
    done: bool,
    progress: Arc<Progress>,
}

impl Read for Piecewise {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.progress.cancel.load(Ordering::Relaxed) {
            // Not `Interrupted`: copying retries those.
            return Err(io::Error::other("cancelled"));
        }
        if self.done {
            return Ok(0);
        }
        if self.file.is_none() {
            self.file = Some(File::open(&self.path)?);
        }
        let n = self.file.as_mut().map_or(Ok(0), |f| f.read(buf))?;
        if n == 0 {
            self.done = true;
            self.file = None;
        }
        self.progress.done.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

/// Makes the archive `dest` from `sources`. `dest` must not be there yet.
pub fn create(
    sources: &[PathBuf],
    dest: &Path,
    format: NewFormat,
    level: Level,
    password: Option<&str>,
    progress: &Arc<Progress>,
) -> Result<Made, Error> {
    let dir = dest.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    if fs::symlink_metadata(dest).is_ok() {
        return Err(Error::Io("There's already something by that name.".into()));
    }
    let hidden =
        (0..).map(|n| dir.join(format!(".ferro-part-{}-{n}", std::process::id()))).find(|p| fs::symlink_metadata(p).is_err()).expect("a free name");
    let mut made = Made::default();
    let items = gather(sources, &hidden, &mut made)?;
    made.files = items.iter().filter(|i| !i.is_dir).count();
    made.folders = items.iter().filter(|i| i.is_dir).count();
    made.bytes = items.iter().map(|i| i.size).sum();
    if let Some(free) = crate::free_space(dir) {
        // Stored, it's as big as what goes in; compressed, less.
        if level == Level::Store && made.bytes > free {
            return Err(Error::TooBig(format!("It needs {}; the drive has {} free.", human_size(made.bytes), human_size(free))));
        }
    }
    progress.total.store(made.bytes, Ordering::Relaxed);
    progress.done.store(0, Ordering::Relaxed);
    let password = password.filter(|p| !p.is_empty());
    let file = {
        use std::os::unix::fs::OpenOptionsExt;
        fs::OpenOptions::new().read(true).write(true).create_new(true).custom_flags(libc::O_NOFOLLOW).mode(0o644).open(&hidden)?
    };
    let result = match format {
        NewFormat::SevenZ => write_7z(file, &items, level, password, progress),
        NewFormat::Zip => write_zip(file, &items, level, password, progress),
    }
    .and_then(|()| rename_noreplace(&hidden, dest).map_err(|_| Error::Io("Something by that name turned up while it was made.".into())));
    if result.is_err() {
        let _ = fs::remove_file(&hidden);
    }
    let cancelled = progress.cancel.load(Ordering::Relaxed);
    match result {
        Err(_) if cancelled => Err(Error::Cancelled),
        Err(e) => Err(e),
        Ok(()) => Ok(made),
    }
}

fn piecewise(i: &Item, progress: &Arc<Progress>) -> Piecewise {
    Piecewise { path: i.path.clone(), file: None, done: false, progress: progress.clone() }
}

fn io_err(e: impl std::fmt::Display) -> Error {
    Error::Io(format!("Couldn't make it: {e}"))
}

fn write_7z(file: File, items: &[Item], level: Level, password: Option<&str>, progress: &Arc<Progress>) -> Result<(), Error> {
    use sevenz_rust2::{encoder_options::Lzma2Options, ArchiveEntry, ArchiveWriter, EncoderConfiguration, EncoderMethod, NtTime, SourceReader};
    let mut w = ArchiveWriter::new(file).map_err(io_err)?;
    let mut methods: Vec<EncoderConfiguration> = Vec::new();
    if let Some(pw) = password {
        methods.push(sevenz_rust2::encoder_options::AesEncoderOptions::new(sevenz_rust2::Password::new(pw)).into());
    }
    methods.push(match level {
        Level::Store => EncoderMethod::COPY.into(),
        Level::Normal => Lzma2Options::from_level(5).into(),
        Level::Maximum => Lzma2Options::from_level(7).into(),
    });
    w.set_content_methods(methods);
    // The names, encrypted too.
    w.set_encrypt_header(password.is_some());
    let entry = |i: &Item| {
        let mut e = if i.is_dir { ArchiveEntry::new_directory(&i.name) } else { ArchiveEntry::new_file(&i.name) };
        if let Some(t) = i.modified.and_then(|t| NtTime::try_from(t).ok()) {
            e.has_last_modified_date = true;
            e.last_modified_date = t;
        }
        // Permission bits, as p7zip keeps them; with the folder bit for folders.
        e.has_windows_attributes = true;
        // (Windows' directory or archive bit, low; the Unix mode, high.)
        let (unix_kind, dos) = if i.is_dir { (0o040000, 0x10) } else { (0o100000, 0x20) };
        e.windows_attributes = 0x8000 | ((unix_kind | i.mode) << 16) | dos;
        e
    };
    // Folders and empty files have no data.
    for i in items.iter().filter(|i| i.is_dir || i.size == 0) {
        let mut e = entry(i);
        e.has_stream = false;
        w.push_archive_entry::<&[u8]>(e, None).map_err(io_err)?;
    }
    // The rest in one solid block, as 7-Zip does.
    let full: Vec<&Item> = items.iter().filter(|i| !i.is_dir && i.size > 0).collect();
    if !full.is_empty() {
        let entries = full.iter().map(|i| entry(i)).collect();
        let readers = full.iter().map(|i| SourceReader::new(piecewise(i, progress))).collect();
        w.push_archive_entries(entries, readers).map_err(io_err)?;
    }
    w.finish().map_err(io_err)?;
    Ok(())
}

fn write_zip(file: File, items: &[Item], level: Level, password: Option<&str>, progress: &Arc<Progress>) -> Result<(), Error> {
    use zip::write::SimpleFileOptions;
    let mut z = zip::ZipWriter::new(file);
    for i in items {
        let mut o = SimpleFileOptions::default().unix_permissions(i.mode).large_file(i.size >= 0xFFFF_FFFF);
        o = match level {
            Level::Store => o.compression_method(zip::CompressionMethod::Stored),
            Level::Normal => o.compression_method(zip::CompressionMethod::Deflated).compression_level(Some(6)),
            Level::Maximum => o.compression_method(zip::CompressionMethod::Deflated).compression_level(Some(9)),
        };
        if let Some(t) = i.modified.and_then(zip_time) {
            o = o.last_modified_time(t);
        }
        if i.is_dir {
            z.add_directory(format!("{}/", i.name), o).map_err(io_err)?;
            continue;
        }
        let o = match password {
            Some(pw) => o.with_aes_encryption(zip::AesMode::Aes256, pw),
            None => o,
        };
        z.start_file(i.name.as_str(), o).map_err(io_err)?;
        io::copy(&mut piecewise(i, progress), &mut z).map_err(io_err)?;
    }
    z.finish().map_err(io_err)?;
    Ok(())
}

/// The local date and time a zip keeps (1980 to 2107).
fn zip_time(t: SystemTime) -> Option<zip::DateTime> {
    let secs = crate::unix_secs(t)?;
    #[allow(deprecated)]
    let tt = secs as libc::time_t;
    // SAFETY: localtime_r fills the struct given; both live through the call.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&tt, &mut tm) }.is_null() {
        return None;
    }
    let y = u16::try_from(tm.tm_year + 1900).ok()?;
    zip::DateTime::from_date_and_time(y, (tm.tm_mon + 1) as u8, tm.tm_mday as u8, tm.tm_hour as u8, tm.tm_min as u8, tm.tm_sec as u8).ok()
}

/// `Holiday` -> `Holiday.7z` in `dir`, or `Holiday (2).7z`... if taken.
pub fn free_archive_name(dir: &Path, stem: &str, format: NewFormat) -> PathBuf {
    let ext = format.ext();
    (1..)
        .map(|n| if n == 1 { dir.join(format!("{stem}.{ext}")) } else { dir.join(format!("{stem} ({n}).{ext}")) })
        .find(|p| fs::symlink_metadata(p).is_err())
        .expect("some name is free")
}

/// What to call a new archive of `sources`: the one thing's name, or the
/// folder they're in.
pub fn suggested_stem(sources: &[PathBuf]) -> String {
    let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned());
    match sources {
        [one] => {
            let n = name(one).unwrap_or_else(|| "Archive".into());
            // `report.txt` -> `report`; a folder keeps its whole name.
            if one.is_dir() {
                n
            } else {
                crate::stem(&n).to_owned()
            }
        }
        _ => sources.first().and_then(|p| p.parent()).and_then(name).unwrap_or_else(|| "Archive".into()),
    }
}
