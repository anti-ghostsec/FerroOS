//! What the file manager does to files: copy, move, delete, unpack. Plain
//! std (and `unzip`/`tar` for archives), so it's tested without a screen.
//! Every operation answers with a line for the status bar.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

const BUFFER_SIZE: usize = 128 * 1024;

/// Calculates total size of all files in a tree.
pub fn tree_size(path: &Path) -> u64 {
    if let Ok(meta) = fs::symlink_metadata(path) {
        if meta.is_file() {
            return meta.len();
        } else if meta.is_dir() {
            let mut total = 0;
            if let Ok(entries) = fs::read_dir(path) {
                for e in entries.flatten() {
                    total += tree_size(&e.path());
                }
            }
            return total;
        }
    }
    0
}

/// A free name for `name` in `dir`, as Windows picks it: `x` if it's free,
/// else `x - Copy`, `x - Copy (2)`, ... (the extension kept at the end).
pub fn free_name(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if fs::symlink_metadata(&first).is_err() {
        return first;
    }
    let (stem, ext) = split_ext(name);
    (1..)
        .map(|n| if n == 1 { format!("{stem} - Copy{ext}") } else { format!("{stem} - Copy ({n}){ext}") })
        .map(|candidate| dir.join(candidate))
        .find(|p| fs::symlink_metadata(p).is_err())
        .expect("some name is free")
}

/// `report.txt` -> (`report`, `.txt`); folders and dotless names keep it all.
fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => name.split_at(i),
        _ => (name, ""),
    }
}

/// One paste (copy or move) of several things into one folder: one
/// progress figure for all of it, and a list of what couldn't be copied,
/// so nothing is skipped without being said.
struct Transfer<'a> {
    copied: u64,
    total: u64,
    progress: &'a mut dyn FnMut(usize, u64, u64, &str),
    /// What couldn't be copied, and why.
    failed: Vec<(String, String)>,
    /// Names changed to ones the drive can hold (FAT and exFAT refuse
    /// `: ? * " < > | \` and trailing dots and spaces).
    renamed: usize,
}

impl Transfer<'_> {
    fn report(&mut self, what: &str) {
        let pct = (self.copied * 100).checked_div(self.total).map(|p| p.min(100) as usize).unwrap_or(100);
        (self.progress)(pct, self.copied, self.total, what);
    }

    /// Copies `from` (all of it) to `to`, going on past what fails.
    fn copy(&mut self, from: &Path, to: &Path) {
        let name = from.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let meta = match fs::symlink_metadata(from) {
            Ok(m) => m,
            Err(e) => return self.failed.push((name, e.to_string())),
        };
        if meta.file_type().is_symlink() {
            return self.copy_link(from, to, &name);
        }
        if meta.is_dir() {
            if let Err(e) = self.create(to, |p| fs::create_dir(p)) {
                return self.failed.push((name, reason(&e)));
            }
            match fs::read_dir(from) {
                Ok(entries) => {
                    for entry in entries {
                        match entry {
                            Ok(entry) => self.copy(&entry.path(), &to.join(entry.file_name())),
                            Err(e) => self.failed.push((name.clone(), e.to_string())),
                        }
                    }
                }
                Err(e) => self.failed.push((name, reason(&e))),
            }
            let _ = fs::set_permissions(to, meta.permissions());
            return;
        }
        if let Err(e) = self.copy_file(from, to, &name, &meta) {
            self.failed.push((name, reason(&e)));
        }
    }

    /// Makes `to` with `make`; when the drive refuses the name, with a
    /// name it takes. Returns where it went.
    fn create(&mut self, to: &Path, make: impl Fn(&Path) -> io::Result<()>) -> io::Result<PathBuf> {
        match make(to) {
            Ok(()) => Ok(to.to_path_buf()),
            Err(e) if e.raw_os_error() == Some(22) => {
                let fixed = to.with_file_name(drive_safe_name(&to.file_name().unwrap_or_default().to_string_lossy()));
                make(&fixed)?;
                self.renamed += 1;
                Ok(fixed)
            }
            Err(e) => Err(e),
        }
    }

    fn copy_file(&mut self, from: &Path, to: &Path, name: &str, meta: &fs::Metadata) -> io::Result<()> {
        let mut src = fs::File::open(from)?;
        let open = |p: &Path| fs::OpenOptions::new().write(true).create_new(true).open(p).map(drop);
        let to = self.create(to, open)?;
        let mut dst = fs::OpenOptions::new().write(true).open(&to)?;
        let mut buf = vec![0u8; BUFFER_SIZE];
        let (mut since_report, mut since_sync) = (0u64, 0u64);
        let result = (|| {
            loop {
                let n = src.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                dst.write_all(&buf[..n])?;
                self.copied += n as u64;
                since_report += n as u64;
                since_sync += n as u64;
                // Written out as it goes, so the figure is what has really
                // reached the drive, not what waits in memory (a USB stick
                // takes far longer than the copy into memory).
                if since_sync >= SYNC_EVERY {
                    since_sync = 0;
                    dst.sync_data()?;
                }
                if since_report >= 256 * 1024 {
                    since_report = 0;
                    self.report(name);
                }
            }
            if meta.len() >= SYNC_EVERY {
                self.report(&format!("{name}, writing it to the drive"));
                dst.sync_data()?;
            }
            io::Result::Ok(())
        })();
        if let Err(e) = result {
            // No half file left behind.
            drop(dst);
            let _ = fs::remove_file(&to);
            return Err(e);
        }
        let _ = dst.set_permissions(meta.permissions());
        self.report(name);
        Ok(())
    }

    /// A link is made again as a link; where the drive can't hold links
    /// (FAT, exFAT), the file it points to is copied in its place.
    fn copy_link(&mut self, from: &Path, to: &Path, name: &str) {
        let target = match fs::read_link(from) {
            Ok(t) => t,
            Err(e) => return self.failed.push((name.to_owned(), e.to_string())),
        };
        #[cfg(unix)]
        if std::os::unix::fs::symlink(&target, to).is_ok() {
            return;
        }
        match fs::metadata(from) {
            Ok(m) if m.is_file() => {
                if let Err(e) = self.copy_file(from, to, name, &m) {
                    self.failed.push((name.to_owned(), reason(&e)));
                }
            }
            _ => self.failed.push((name.to_owned(), "a link this drive can't hold".into())),
        }
    }
}

/// Written out to the drive every this many bytes.
const SYNC_EVERY: u64 = 32 << 20;

/// A name FAT and exFAT drives accept: their forbidden characters become
/// `_`, and trailing dots and spaces go.
pub fn drive_safe_name(name: &str) -> String {
    let clean: String = name.chars().map(|c| if ":?*\"<>|\\".contains(c) || (c as u32) < 32 { '_' } else { c }).collect();
    let trimmed = clean.trim_end_matches(['.', ' ']);
    if trimmed.is_empty() {
        "_".into()
    } else {
        trimmed.to_owned()
    }
}

/// Why something failed, in a few words.
fn reason(e: &io::Error) -> String {
    match e.raw_os_error() {
        Some(13 | 1) => "no permission".into(),
        Some(28) => "the drive is full".into(),
        Some(30) => "the drive is read-only".into(),
        Some(22) => "a name this drive can't hold".into(),
        Some(36) => "the name is too long for this drive".into(),
        Some(27) => "too big for this drive (FAT holds files up to 4 GB)".into(),
        _ => e.to_string(),
    }
}

/// Copies a file, a folder (all of it) or a link to `to`.
#[allow(dead_code)]
pub fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    let mut nothing = |_: usize, _: u64, _: u64, _: &str| {};
    let mut t = Transfer { copied: 0, total: 0, progress: &mut nothing, failed: Vec::new(), renamed: 0 };
    t.copy(from, to);
    match t.failed.first() {
        None => Ok(()),
        Some((name, why)) => Err(io::Error::other(format!("{name}: {why}"))),
    }
}

/// Whether `to` is `from` itself or inside it: a folder can't go into itself.
fn into_itself(from: &Path, to_dir: &Path) -> bool {
    let (Ok(a), Ok(b)) = (fs::canonicalize(from), fs::canonicalize(to_dir)) else { return false };
    b.starts_with(a)
}

/// Pastes `from` into `dir`: a copy, or a move when `cut`. A name already
/// there isn't overwritten; the new one gets a free name.
#[allow(dead_code)]
pub fn paste(from: &Path, dir: &Path, cut: bool) -> Result<String, String> {
    paste_all(&[from.to_path_buf()], dir, cut, |_, _, _, _| {})
}

/// Pastes everything in `froms` into `dir` (moves them when `cut`), with
/// one progress figure for all of it: `progress(percent, bytes done,
/// bytes in all, what's being copied)`. What can't be copied is skipped and
/// listed in the error at the end; nothing is lost on a move, since an
/// original is removed only once all of it has been copied.
pub fn paste_all<F>(froms: &[PathBuf], dir: &Path, cut: bool, mut progress: F) -> Result<String, String>
where
    F: FnMut(usize, u64, u64, &str),
{
    let verb = if cut { "Moved" } else { "Copied" };
    let mut problems: Vec<String> = Vec::new();
    // First what needs no copying: moves on the same drive are renames.
    let mut to_copy: Vec<(PathBuf, PathBuf, String)> = Vec::new();
    let mut done = 0usize;
    // The last one done, and the name it got there (for "as 'x - Copy'").
    let mut last: Option<(String, String)> = None;
    for from in froms {
        let Some(name) = from.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
        if fs::symlink_metadata(from).is_err() {
            problems.push(format!("'{name}' isn't there any more"));
            continue;
        }
        if fs::symlink_metadata(from).is_ok_and(|m| m.is_dir()) && into_itself(from, dir) {
            problems.push(format!("'{name}' can't go inside itself"));
            continue;
        }
        if cut && from.parent() == Some(dir) {
            done += 1;
            last = Some((name.clone(), name));
            continue;
        }
        let to = free_name(dir, &name);
        if cut {
            match fs::rename(from, &to) {
                Ok(()) => {
                    done += 1;
                    last = Some((name, to.file_name().unwrap_or_default().to_string_lossy().into_owned()));
                    continue;
                }
                // Another drive: copied over, then the original removed.
                Err(e) if e.raw_os_error() == Some(EXDEV) => {}
                Err(e) => {
                    problems.push(explain("move", &name, &e));
                    continue;
                }
            }
        }
        to_copy.push((from.clone(), to, name));
    }
    let total = to_copy.iter().map(|(from, _, _)| tree_size(from)).sum();
    let mut t = Transfer { copied: 0, total, progress: &mut progress, failed: Vec::new(), renamed: 0 };
    let mut kept = 0usize;
    for (from, to, name) in &to_copy {
        let failed_before = t.failed.len();
        t.copy(from, to);
        let whole = t.failed.len() == failed_before;
        if whole {
            done += 1;
            last = Some((name.clone(), to.file_name().unwrap_or_default().to_string_lossy().into_owned()));
        }
        if cut {
            if whole {
                if let Err(e) = remove(from) {
                    let name = from.file_name().unwrap_or_default().to_string_lossy();
                    problems.push(explain("remove the original of", &name, &e));
                }
            } else {
                kept += 1;
            }
        }
    }
    let mut said = match (done, &last) {
        (1, Some((name, shown))) if name == shown => format!("{verb} '{name}'."),
        (1, Some((name, shown))) => format!("{verb} '{name}' as '{shown}'."),
        _ => format!("{verb} {done} item(s)."),
    };
    if t.renamed > 0 {
        said.push_str(&format!(" {} name(s) changed to ones this drive can hold.", t.renamed));
    }
    if t.failed.is_empty() && problems.is_empty() {
        return Ok(said);
    }
    if !t.failed.is_empty() {
        let shown: Vec<String> = t.failed.iter().take(3).map(|(n, why)| format!("'{n}' ({why})")).collect();
        let more = t.failed.len().saturating_sub(3);
        said.push_str(&format!(" {} file(s) couldn't be copied: {}", t.failed.len(), shown.join(", ")));
        if more > 0 {
            said.push_str(&format!(" and {more} more"));
        }
        said.push('.');
    }
    if kept > 0 {
        said.push_str(&format!(" The original of {kept} item(s) was kept, since not all of it could be copied."));
    }
    for p in problems.iter().take(3) {
        said.push_str(&format!(" {p}."));
    }
    Err(said)
}

/// EXDEV: "not on the same drive", which rename can't do.
const EXDEV: i32 = 18;

#[cfg(unix)]
fn make_tree_writable(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if let Ok(meta) = fs::symlink_metadata(&p) {
                if meta.is_dir() {
                    let mut perms = meta.permissions();
                    let mode = perms.mode();
                    if mode & 0o700 != 0o700 {
                        perms.set_mode(mode | 0o700);
                        let _ = fs::set_permissions(&p, perms);
                    }
                    make_tree_writable(&p);
                } else if !meta.file_type().is_symlink() {
                    let mut perms = meta.permissions();
                    let mode = perms.mode();
                    if mode & 0o200 == 0 {
                        perms.set_mode(mode | 0o600);
                        let _ = fs::set_permissions(&p, perms);
                    }
                }
            }
        }
    }
    if let Ok(meta) = fs::metadata(dir) {
        let mut perms = meta.permissions();
        let mode = perms.mode();
        if mode & 0o700 != 0o700 {
            perms.set_mode(mode | 0o700);
            let _ = fs::set_permissions(dir, perms);
        }
    }
}

/// Deletes a file, a link, or a folder with all that's in it.
pub fn remove(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.is_dir() {
        if let Err(e) = fs::remove_dir_all(path) {
            #[cfg(unix)]
            {
                make_tree_writable(path);
                if fs::remove_dir_all(path).is_ok() {
                    return Ok(());
                }
                // Fallback to rm -rf for stubborn external filesystems/drives
                let status = std::process::Command::new("rm").arg("-rf").arg("--").arg(path).status();
                if let Ok(st) = status {
                    if st.success() && !path.exists() {
                        return Ok(());
                    }
                }
            }
            return Err(e);
        }
        Ok(())
    } else if let Err(e) = fs::remove_file(path) {
        #[cfg(unix)]
        {
            if let Ok(meta) = fs::symlink_metadata(path) {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = meta.permissions();
                perms.set_mode(perms.mode() | 0o600);
                let _ = fs::set_permissions(path, perms);
                if fs::remove_file(path).is_ok() {
                    return Ok(());
                }
            }
        }
        Err(e)
    } else {
        Ok(())
    }
}

/// Deletes for good. What's off the main drive (a USB stick) is written
/// over first, as a Delete there can't go to the Recycle Bin.
pub fn delete(path: &Path) -> Result<String, String> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let done = if ferro_associations::trash::can_take(path) { remove(path) } else { ferro_associations::trash::shred(path) };
    done.map(|()| format!("Deleted '{name}'.")).map_err(|e| explain("delete", &name, &e))
}

/// Moves things into the Recycle Bin (never deletes them for good).
pub fn to_recycle_bin(paths: &[PathBuf]) -> Result<String, String> {
    let mut failed = Vec::new();
    for p in paths {
        if let Err(e) = ferro_associations::trash::put(p) {
            failed.push(explain("move to the Recycle Bin", &p.file_name().unwrap_or_default().to_string_lossy(), &e));
        }
    }
    let moved = paths.len() - failed.len();
    match failed.first() {
        None => Ok(format!("Moved {moved} item(s) to the Recycle Bin.")),
        Some(first) => Err(format!("Moved {moved} item(s) to the Recycle Bin. {first}")),
    }
}

/// Delete, said Yes to with some on a removable drive: those deleted for
/// good (written over first), the rest into the Recycle Bin.
pub fn recycle_or_delete(paths: &[PathBuf]) -> Result<String, String> {
    let mut said = String::new();
    for p in paths {
        said = if ferro_associations::trash::can_take(p) { to_recycle_bin(std::slice::from_ref(p))? } else { delete(p)? };
    }
    Ok(if paths.len() == 1 { said } else { format!("Deleted {} items.", paths.len()) })
}

/// Renames `path` to `new_name` in the same folder.
pub fn rename(path: &Path, new_name: &str) -> Result<String, String> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let new_name = new_name.trim();
    if new_name.is_empty() || new_name == "." || new_name == ".." || new_name.contains('/') || new_name.contains('\0') {
        return Err("A name can't be empty or have a / in it.".into());
    }
    if new_name == name {
        return Ok(String::new());
    }
    let to = path.with_file_name(new_name);
    if fs::symlink_metadata(&to).is_ok() {
        return Err(format!("There's already something called '{new_name}' here."));
    }
    fs::rename(path, &to).map(|()| format!("Renamed to '{new_name}'.")).map_err(|e| explain("rename", &name, &e))
}

/// The kinds of archive Extract Here unpacks.
pub fn is_archive(name: &str) -> bool {
    let n = name.to_lowercase();
    [".zip", ".tar", ".tar.gz", ".tgz", ".tar.xz", ".txz", ".tar.bz2", ".tbz2", ".tar.zst"].iter().any(|e| n.ends_with(e))
}

/// Programs to install rather than archives to open: Debian packages and
/// tarballs (`ferro-installer` asks whether a tarball is a program).
pub fn is_installable(name: &str) -> bool {
    let n = name.to_lowercase();
    n.ends_with(".deb") || (is_archive(&n) && !n.ends_with(".zip"))
}

/// The archive's name without its extension: `photos.tar.gz` -> `photos`.
fn archive_stem(name: &str) -> &str {
    let n = name.to_lowercase();
    for e in [".tar.gz", ".tar.xz", ".tar.bz2", ".tar.zst", ".tgz", ".txz", ".tbz2", ".zip", ".tar"] {
        if n.ends_with(e) {
            return &name[..name.len() - e.len()];
        }
    }
    name
}

/// Unpacks an archive next to itself, the way people expect: one folder
/// inside (as most archives have) comes out as that folder, not a folder
/// inside a folder of the same name; loose files come out together in a
/// folder named after the archive. Nothing already there is overwritten.
pub fn extract_here(archive: &Path) -> Result<(String, PathBuf), String> {
    let name = archive.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let dir = archive.parent().ok_or("No folder to unpack into.")?;
    // Unpacked first into a hidden folder beside it, on the same drive, so
    // what comes out is then only renamed into place.
    let work = (0..).map(|n| dir.join(format!(".ferro-extract-{}-{n}", std::process::id()))).find(|p| !p.exists()).expect("a free name");
    fs::create_dir(&work).map_err(|e| explain("unpack", &name, &e))?;
    let result = unpack(archive, &work).and_then(|()| {
        let inside: Vec<_> = fs::read_dir(&work).map_err(|e| e.to_string())?.flatten().collect();
        let (from, want) = match inside.as_slice() {
            [] => return Err(format!("'{name}' is empty.")),
            [one] if one.path().is_dir() => (one.path(), one.file_name().to_string_lossy().into_owned()),
            _ => (work.clone(), archive_stem(&name).to_owned()),
        };
        let to = free_name(dir, &want);
        fs::rename(&from, &to).map_err(|e| explain("unpack", &name, &e))?;
        let shown = to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        Ok((format!("Unpacked '{name}' into '{shown}'."), to))
    });
    let _ = fs::remove_dir_all(&work);
    result
}

fn unpack(archive: &Path, into: &Path) -> Result<(), String> {
    let name = archive.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let mut cmd = if name.ends_with(".zip") {
        let mut c = Command::new("unzip");
        // -qq: quiet; -n: never overwrite (nothing's there anyway).
        c.arg("-qq").arg("-n").arg(archive).arg("-d").arg(into);
        c
    } else {
        let mut c = Command::new("tar");
        // No owners or device files from someone else's archive.
        c.arg("-xf").arg(archive).arg("-C").arg(into).arg("--no-same-owner").arg("--no-same-permissions");
        c
    };
    let out = cmd.output().map_err(|e| format!("Couldn't unpack: {e}"))?;
    // unzip says 1 for warnings (such as odd names) with the files out.
    if out.status.success() || (name.ends_with(".zip") && out.status.code() == Some(1)) {
        return Ok(());
    }
    let why = String::from_utf8_lossy(&out.stderr);
    let why = why.lines().find(|l| !l.trim().is_empty()).unwrap_or("it may be damaged");
    Err(format!("Couldn't unpack it: {}", why.trim()))
}

/// An error, said so people know what to do about it.
pub fn explain(verb: &str, name: &str, e: &io::Error) -> String {
    match e.raw_os_error() {
        // EROFS
        Some(30) => format!(
            "Can't {verb} '{name}': the drive is read-only. A Windows drive is when Windows wasn't fully shut down: turn off Fast Startup there."
        ),
        // EACCES, EPERM
        Some(13 | 1) => format!("Can't {verb} '{name}': you don't have permission here."),
        // ENOSPC
        Some(28) => format!("Can't {verb} '{name}': the drive is full."),
        // ENOTEMPTY
        Some(39) => format!("Can't {verb} '{name}': folder is in use by another program or drive is busy."),
        _ => format!("Can't {verb} '{name}': {e}."),
    }
}

/// Size in the units people read: 512 bytes, 1.4 KB, 3.0 MB...
#[allow(dead_code)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_progress_for_a_whole_paste() {
        let d = std::env::temp_dir().join(format!("ferro-paste-all-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        let (src, dst) = (d.join("src"), d.join("dst"));
        fs::create_dir_all(src.join("folder/deep/er")).unwrap();
        fs::create_dir_all(&dst).unwrap();
        for (f, n) in [("a.bin", 300_000), ("b.bin", 200_000), ("folder/c.bin", 100_000), ("folder/deep/er/d.bin", 50_000)] {
            fs::write(src.join(f), vec![7u8; n]).unwrap();
        }
        let froms = vec![src.join("a.bin"), src.join("b.bin"), src.join("folder"), src.join("gone.bin")];
        let mut last = (0, 0, 0);
        let said = paste_all(&froms, &dst, false, |pct, done, total, _| last = (pct, done, total));
        // Everything there was got copied, all the way down, under one total.
        assert_eq!(last, (100, 650_000, 650_000));
        assert_eq!(fs::read(dst.join("folder/deep/er/d.bin")).unwrap().len(), 50_000);
        // And what wasn't there is said, not skipped in silence.
        let err = said.unwrap_err();
        assert!(err.contains("Copied 3 item(s)") && err.contains("gone.bin"), "{err}");
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn names_a_usb_stick_can_hold() {
        assert_eq!(drive_safe_name("What? Why: \"now\"*.txt"), "What_ Why_ _now__.txt");
        assert_eq!(drive_safe_name("trailing dots... "), "trailing dots");
        assert_eq!(drive_safe_name("..."), "_");
        assert_eq!(drive_safe_name("plain.mp4"), "plain.mp4");
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ferro-files-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn free_names_follow_windows() {
        let d = scratch("names");
        assert_eq!(free_name(&d, "a.txt"), d.join("a.txt"));
        fs::write(d.join("a.txt"), "").unwrap();
        assert_eq!(free_name(&d, "a.txt"), d.join("a - Copy.txt"));
        fs::write(d.join("a - Copy.txt"), "").unwrap();
        assert_eq!(free_name(&d, "a.txt"), d.join("a - Copy (2).txt"));
        fs::create_dir(d.join("Pics")).unwrap();
        assert_eq!(free_name(&d, "Pics"), d.join("Pics - Copy"));
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn copy_move_rename_delete() {
        let d = scratch("ops");
        let (src, dst) = (d.join("src"), d.join("dst"));
        fs::create_dir_all(src.join("folder/inner")).unwrap();
        fs::write(src.join("folder/inner/f.txt"), "hello").unwrap();
        fs::create_dir(&dst).unwrap();

        assert!(paste(&src.join("folder"), &dst, false).is_ok());
        assert_eq!(fs::read_to_string(dst.join("folder/inner/f.txt")).unwrap(), "hello");
        // Again: a second copy beside it, nothing overwritten.
        assert!(paste(&src.join("folder"), &dst, false).unwrap().contains("folder - Copy"));
        // Into itself: refused.
        assert!(paste(&src.join("folder"), &src.join("folder/inner"), false).is_err());

        assert!(paste(&src.join("folder"), &dst, true).is_ok());
        assert!(!src.join("folder").exists());
        assert!(dst.join("folder - Copy (2)/inner/f.txt").exists());

        assert!(rename(&dst.join("folder"), "renamed").is_ok());
        assert!(dst.join("renamed").is_dir());
        assert!(rename(&dst.join("renamed"), "folder - Copy").is_err(), "taken");
        assert!(rename(&dst.join("renamed"), "a/b").is_err());

        assert!(delete(&dst.join("renamed")).is_ok());
        assert!(!dst.join("renamed").exists());
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn extract_does_not_nest() {
        if Command::new("zip").arg("-v").output().is_err() || Command::new("unzip").arg("-v").output().is_err() {
            return;
        }
        let d = scratch("zip");
        // One folder inside: it comes out as that folder.
        fs::create_dir_all(d.join("build/Game/data")).unwrap();
        fs::write(d.join("build/Game/data/x"), "1").unwrap();
        fs::write(d.join("build/Game/readme"), "2").unwrap();
        // Loose files: together, in a folder named after the archive.
        fs::write(d.join("build/a.txt"), "3").unwrap();
        let zip = |out: &str, what: &[&str]| {
            assert!(Command::new("zip").arg("-qr").arg(d.join(out)).args(what).current_dir(d.join("build")).status().unwrap().success());
        };
        zip("Game.zip", &["Game"]);
        zip("loose.zip", &["a.txt", "Game/readme"]);

        let (_, out) = extract_here(&d.join("Game.zip")).unwrap();
        assert_eq!(out, d.join("Game"));
        assert!(d.join("Game/data/x").exists() && !d.join("Game/Game").exists());
        // Again: beside it, not on top of it.
        let (_, out) = extract_here(&d.join("Game.zip")).unwrap();
        assert_eq!(out, d.join("Game - Copy"));

        let (_, out) = extract_here(&d.join("loose.zip")).unwrap();
        assert_eq!(out, d.join("loose"));
        assert!(d.join("loose/a.txt").exists() && d.join("loose/Game/readme").exists());
        // No working folder left behind.
        assert!(fs::read_dir(&d).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().starts_with(".ferro-extract")));
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn archive_names() {
        assert!(is_archive("A.ZIP") && is_archive("x.tar.gz") && !is_archive("x.gz"));
        assert_eq!(archive_stem("photos.tar.gz"), "photos");
        assert_eq!(archive_stem("Game.zip"), "Game");
        assert_eq!(human_size(512), "512 bytes");
        assert_eq!(human_size(1536), "1.5 KB");
    }
}
