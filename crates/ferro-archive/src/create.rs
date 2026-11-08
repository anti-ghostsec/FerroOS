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

/// A 7z writer set for `level` and `password` (names encrypted too).
fn sz_writer(file: File, level: Level, password: Option<&str>) -> Result<sevenz_rust2::ArchiveWriter<File>, Error> {
    use sevenz_rust2::{encoder_options::Lzma2Options, ArchiveWriter, EncoderConfiguration, EncoderMethod};
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
    Ok(w)
}

fn write_7z(file: File, items: &[Item], level: Level, password: Option<&str>, progress: &Arc<Progress>) -> Result<(), Error> {
    let mut w = sz_writer(file, level, password)?;
    sz_items(&mut w, items, progress)?;
    w.finish().map_err(io_err)?;
    Ok(())
}

/// Puts `items` in a 7z being written: folders and empty files, then the
/// rest in one solid block.
fn sz_items(w: &mut sevenz_rust2::ArchiveWriter<File>, items: &[Item], progress: &Arc<Progress>) -> Result<(), Error> {
    use sevenz_rust2::{ArchiveEntry, NtTime, SourceReader};
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
    Ok(())
}

fn write_zip(file: File, items: &[Item], level: Level, password: Option<&str>, progress: &Arc<Progress>) -> Result<(), Error> {
    let mut z = zip::ZipWriter::new(file);
    zip_items(&mut z, items, level, password, progress)?;
    z.finish().map_err(io_err)?;
    Ok(())
}

fn zip_items(z: &mut zip::ZipWriter<File>, items: &[Item], level: Level, password: Option<&str>, progress: &Arc<Progress>) -> Result<(), Error> {
    use zip::write::SimpleFileOptions;
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
        io::copy(&mut piecewise(i, progress), z).map_err(io_err)?;
    }
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

// ---------------------------------------------------------------------------
// Adding to an archive

/// Adds `sources` to the 7z or zip `archive`. It's never changed in place: a
/// new archive is written beside it (under a hidden name), the old one's
/// contents copied in (zip: as they are, still compressed and encrypted as
/// they were; 7z: decoded and encoded again, streamed, never on disk), the
/// new items added, and only then renamed over it. Something already in
/// the archive by the same name is replaced. A RAR is never changed.
///
/// `password`: a 7z's (the whole new archive is encrypted with it, names
/// too); for a zip, what the new items are encrypted with (AES-256).
pub fn add_to(archive: &Path, sources: &[PathBuf], level: Level, password: Option<&str>, progress: &Arc<Progress>) -> Result<Made, Error> {
    let format = match crate::detect(archive) {
        Some(crate::Format::Zip) => NewFormat::Zip,
        Some(crate::Format::SevenZ) => NewFormat::SevenZ,
        _ => return Err(Error::Unsupported("only 7z and zip archives have files added; RAR archives are never changed".into())),
    };
    let dir = archive.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let hidden =
        (0..).map(|n| dir.join(format!(".ferro-part-{}-{n}", std::process::id()))).find(|p| fs::symlink_metadata(p).is_err()).expect("a free name");
    let mut made = Made::default();
    // The archive itself is never put in itself.
    let items = gather(sources, archive, &mut made)?;
    made.files = items.iter().filter(|i| !i.is_dir).count();
    made.folders = items.iter().filter(|i| i.is_dir).count();
    made.bytes = items.iter().map(|i| i.size).sum();
    let old_size = fs::metadata(archive).map(|m| m.len()).unwrap_or(0);
    if let Some(free) = crate::free_space(dir) {
        if old_size.saturating_add(if level == Level::Store { made.bytes } else { 0 }) > free {
            return Err(Error::TooBig(format!(
                "Adding needs room for a new copy of the archive ({}); the drive has {} free.",
                human_size(old_size),
                human_size(free)
            )));
        }
    }
    let password = password.filter(|p| !p.is_empty());
    let replaced: std::collections::HashSet<String> = items.iter().map(|i| i.name.clone()).collect();
    progress.done.store(0, Ordering::Relaxed);
    let file = {
        use std::os::unix::fs::OpenOptionsExt;
        fs::OpenOptions::new().read(true).write(true).create_new(true).custom_flags(libc::O_NOFOLLOW).mode(0o644).open(&hidden)?
    };
    let result = match format {
        NewFormat::Zip => rewrite_zip(archive, file, &items, &replaced, level, password, progress),
        NewFormat::SevenZ => rewrite_7z(archive, file, &items, &replaced, level, password, progress),
    }
    // Same folder: a plain rename, over the old one, all at once.
    .and_then(|()| fs::rename(&hidden, archive).map_err(Error::from));
    if result.is_err() {
        let _ = fs::remove_file(&hidden);
    }
    match result {
        Err(_) if progress.cancel.load(Ordering::Relaxed) => Err(Error::Cancelled),
        Err(e) => Err(e),
        Ok(()) => Ok(made),
    }
}

/// An archive entry's name as items are named (no trailing `/`).
fn plain(name: &str) -> String {
    name.replace('\\', "/").trim_end_matches('/').to_owned()
}

fn rewrite_zip(
    archive: &Path,
    file: File,
    items: &[Item],
    replaced: &std::collections::HashSet<String>,
    level: Level,
    password: Option<&str>,
    progress: &Arc<Progress>,
) -> Result<(), Error> {
    let old = File::open(archive).map_err(io_err)?;
    let mut old = zip::ZipArchive::new(io::BufReader::new(old)).map_err(crate::zip_err)?;
    let total: u64 =
        (0..old.len()).filter_map(|i| old.by_index_raw(i).ok().map(|f| f.compressed_size())).sum::<u64>() + items.iter().map(|i| i.size).sum::<u64>();
    progress.total.store(total, Ordering::Relaxed);
    let mut z = zip::ZipWriter::new(file);
    for i in 0..old.len() {
        if progress.cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let f = old.by_index_raw(i).map_err(crate::zip_err)?;
        let name = f.name().map_err(crate::zip_err)?;
        if replaced.contains(&plain(&name)) {
            continue;
        }
        let n = f.compressed_size();
        // As it is: still compressed, still encrypted, never decoded.
        z.raw_copy_file(f).map_err(io_err)?;
        progress.done.fetch_add(n, Ordering::Relaxed);
    }
    zip_items(&mut z, items, level, password, progress)?;
    z.finish().map_err(io_err)?;
    Ok(())
}

/// A piece of the old 7z's contents, handed from the thread decoding it to
/// the writer.
enum Piece {
    Data(Vec<u8>),
    /// The end of one entry's contents.
    End,
    Failed(Error),
}

/// One old entry's contents, as the decoding thread hands them over.
struct FromOld {
    rx: std::rc::Rc<std::sync::mpsc::Receiver<Piece>>,
    buf: Vec<u8>,
    at: usize,
    done: bool,
    progress: Arc<Progress>,
    failed: std::rc::Rc<std::cell::RefCell<Option<Error>>>,
}

impl Read for FromOld {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.progress.cancel.load(Ordering::Relaxed) {
            return Err(io::Error::other("cancelled"));
        }
        while !self.done && self.at == self.buf.len() {
            match self.rx.recv() {
                Ok(Piece::Data(d)) => {
                    self.buf = d;
                    self.at = 0;
                }
                Ok(Piece::End) => self.done = true,
                Ok(Piece::Failed(e)) => {
                    let msg = e.to_string();
                    *self.failed.borrow_mut() = Some(e);
                    return Err(io::Error::other(msg));
                }
                Err(_) => return Err(io::Error::other("the old archive stopped")),
            }
        }
        if self.done {
            return Ok(0);
        }
        let n = out.len().min(self.buf.len() - self.at);
        out[..n].copy_from_slice(&self.buf[self.at..self.at + n]);
        self.at += n;
        self.progress.done.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

fn rewrite_7z(
    archive: &Path,
    file: File,
    items: &[Item],
    replaced: &std::collections::HashSet<String>,
    level: Level,
    password: Option<&str>,
    progress: &Arc<Progress>,
) -> Result<(), Error> {
    use sevenz_rust2::SourceReader;
    let old = File::open(archive).map_err(io_err)?;
    let pw = crate::sz_password(password);
    let mut reader = sevenz_rust2::ArchiveReader::new(io::BufReader::new(old), pw).map_err(|e| match crate::sz_err(e) {
        Error::Damaged(_) if password.is_some() => Error::WrongPassword,
        e => e,
    })?;
    // Only the time changed goes on, as in a new archive.
    let files: Vec<_> = reader
        .archive()
        .files
        .iter()
        .map(|f| {
            let mut f = f.clone();
            f.has_creation_date = false;
            f.has_access_date = false;
            f
        })
        .collect();
    let keep: Vec<bool> = files.iter().map(|f| !f.is_anti_item && !replaced.contains(&plain(&f.name))).collect();
    // The decoder hands entries over in the archive's order.
    let streamed: Vec<usize> = (0..files.len()).filter(|&i| keep[i] && files[i].has_stream && files[i].size > 0).collect();
    let total = streamed.iter().map(|&i| files[i].size).sum::<u64>() + items.iter().map(|i| i.size).sum::<u64>();
    progress.total.store(total, Ordering::Relaxed);
    let mut w = sz_writer(file, level, password)?;
    // Folders and empty files from the old archive: no contents to copy.
    for (i, f) in files.iter().enumerate() {
        if keep[i] && !(f.has_stream && f.size > 0) {
            let mut e = f.clone();
            e.has_stream = false;
            e.size = 0;
            e.compressed_size = 0;
            w.push_archive_entry::<&[u8]>(e, None).map_err(io_err)?;
        }
    }
    let failed = std::rc::Rc::new(std::cell::RefCell::new(None));
    let mut decoder = None;
    if !streamed.is_empty() {
        let index: std::collections::HashMap<usize, usize> =
            reader.archive().files.iter().enumerate().map(|(i, f)| (f as *const sevenz_rust2::ArchiveEntry as usize, i)).collect();
        let wanted: Vec<bool> = (0..files.len()).map(|i| streamed.binary_search(&i).is_ok()).collect();
        // A few pieces in flight at most: memory stays small, whatever the size.
        let (tx, rx) = std::sync::mpsc::sync_channel::<Piece>(8);
        let cancel = progress.clone();
        decoder = Some(std::thread::spawn(move || {
            let r = reader.for_each_entries(|entry, data| {
                let i = index.get(&(entry as *const sevenz_rust2::ArchiveEntry as usize)).copied();
                if !i.is_some_and(|i| wanted[i]) {
                    io::copy(data, &mut io::sink())?;
                    return Ok(true);
                }
                let mut buf = vec![0u8; 64 * 1024];
                loop {
                    if cancel.cancel.load(Ordering::Relaxed) {
                        return Ok(false);
                    }
                    let n = data.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    if tx.send(Piece::Data(buf[..n].to_vec())).is_err() {
                        return Ok(false);
                    }
                }
                Ok(tx.send(Piece::End).is_ok())
            });
            if let Err(e) = r {
                let _ = tx.send(Piece::Failed(crate::sz_err(e)));
            }
        }));
        let rx = std::rc::Rc::new(rx);
        let entries: Vec<_> = streamed.iter().map(|&i| files[i].clone()).collect();
        let readers: Vec<_> = streamed
            .iter()
            .map(|_| {
                SourceReader::new(FromOld { rx: rx.clone(), buf: Vec::new(), at: 0, done: false, progress: progress.clone(), failed: failed.clone() })
            })
            .collect();
        let pushed = w.push_archive_entries(entries, readers).map(|_| ());
        drop(rx);
        if let Err(e) = pushed {
            if let Some(d) = decoder.take() {
                let _ = d.join();
            }
            return Err(failed.borrow_mut().take().unwrap_or_else(|| io_err(e)));
        }
    }
    if let Some(d) = decoder {
        let _ = d.join();
    }
    if let Some(e) = failed.borrow_mut().take() {
        return Err(e);
    }
    sz_items(&mut w, items, progress)?;
    w.finish().map_err(io_err)?;
    Ok(())
}
