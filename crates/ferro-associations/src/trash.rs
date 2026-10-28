//! The Recycle Bin: `~/.local/share/Trash`, where the desktop, the file
//! manager and the image viewer put what's deleted, so it can be got back.
//! A name already in there is never overwritten: the newcomer gets
//! `name (2)`, `name (3)`, and so on.
//!
//! Only what's on the bin's own drive goes in. Something on a USB stick or
//! an SD card is never copied onto the main drive to sit in the bin (a
//! private picture would then be on the computer as well): as on Windows 95,
//! it's deleted for good instead, once asked, and overwritten first
//! (`shred`). `can_take` tells which, for the desktop, the file manager and
//! the image viewer alike.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Where the Recycle Bin keeps things.
pub fn dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"));
    home.join(".local/share/Trash")
}

/// How many things are in it (what the desktop's icon shows).
pub fn count() -> usize {
    count_in(&dir())
}

fn count_in(bin: &Path) -> usize {
    fs::read_dir(bin).map(|d| d.flatten().filter(|e| !e.file_name().to_string_lossy().starts_with('.')).count()).unwrap_or(0)
}

/// Whether `path` can go to the Recycle Bin: only from the drive the bin
/// is on (the same filesystem, so it's only renamed in). Anything else (a
/// USB stick, an SD card, another drive) is deleted for good, by `shred`,
/// after asking.
pub fn can_take(path: &Path) -> bool {
    can_take_in(path, &dir())
}

fn can_take_in(path: &Path, bin: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(meta) = fs::symlink_metadata(path) else { return false };
    // The bin may not be made yet: the drive of what's above it, then.
    let bin_dev = bin.ancestors().find_map(|a| fs::metadata(a).ok()).map(|m| m.dev());
    bin_dev == Some(meta.dev())
}

/// Moves `path` into the Recycle Bin; returns where it went. Refuses what's
/// on another drive (see `can_take`): that's never copied over.
pub fn put(path: &Path) -> io::Result<PathBuf> {
    put_in(path, &dir())
}

fn put_in(path: &Path, bin: &Path) -> io::Result<PathBuf> {
    fs::symlink_metadata(path)?;
    if bin.starts_with(path) || path.starts_with(bin) {
        return Err(io::Error::other("that's the Recycle Bin itself"));
    }
    if !can_take_in(path, bin) {
        return Err(io::Error::other("it's on another drive, so it can't go to the Recycle Bin"));
    }
    fs::create_dir_all(bin)?;
    let name = path.file_name().ok_or_else(|| io::Error::other("no name"))?.to_string_lossy().into_owned();
    let to = free_name(bin, &name);
    fs::rename(path, &to)?;
    Ok(to)
}

/// Deletes `path` for good, a folder with all that's in it, each file's
/// contents overwritten (once, with random bytes, written out to the drive)
/// before it's cut to nothing and removed. Links are never followed: only
/// the link goes, never what it points at.
///
/// On a hard disk that's the old contents gone. On flash (USB sticks, SD
/// cards, SSDs) it's best effort only: the drive itself decides where each
/// write lands (wear levelling) and may keep the old blocks until it reuses
/// them, out of reach of anything a program can do. Still far better than
/// a plain delete, which leaves every byte where it was.
pub fn shred(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let dev = fs::symlink_metadata(path)?.dev();
    shred_on(path, dev)
}

fn shred_on(path: &Path, dev: u64) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::symlink_metadata(path)?;
    // Another drive mounted inside the folder: not that one's too.
    if meta.dev() != dev {
        return Err(io::Error::other("another drive is mounted in it"));
    }
    let kind = meta.file_type();
    if kind.is_dir() {
        writable(path, &meta, 0o700);
        for entry in fs::read_dir(path)? {
            shred_on(&entry?.path(), dev)?;
        }
        return fs::remove_dir(path);
    }
    // Written over only if this is its one name: another (a hard link) is
    // the same contents, still wanted there.
    if kind.is_file() && meta.nlink() == 1 {
        let file = match open_for_overwrite(path) {
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                // Read-only: it's still ours to delete, so ours to overwrite.
                writable(path, &meta, 0o600);
                open_for_overwrite(path)?
            }
            other => other?,
        };
        overwrite(&file, meta.len())?;
        file.set_len(0)?;
        file.sync_all()?;
    }
    // A link, a device, a socket: there's only the name.
    fs::remove_file(path)
}

/// Opened for writing, never through a link (if `path` was swapped for one
/// since it was looked at, this fails rather than overwrite what it points
/// at), and never made anew.
fn open_for_overwrite(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new().write(true).custom_flags(libc::O_NOFOLLOW).open(path)
}

fn writable(path: &Path, meta: &fs::Metadata, bits: u32) {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = meta.permissions();
    if perms.mode() & bits != bits {
        perms.set_mode(perms.mode() | bits);
        let _ = fs::set_permissions(path, perms);
    }
}

/// `len` bytes over the start of `file`, a megabyte at a time (a video of
/// several gigabytes needn't be in memory), then out to the drive.
fn overwrite(file: &fs::File, len: u64) -> io::Result<()> {
    use std::io::{Read, Seek, Write};
    let mut file = file;
    let mut random = fs::File::open("/dev/urandom").ok();
    let mut buf = vec![0u8; 1 << 20];
    file.seek(io::SeekFrom::Start(0))?;
    let mut left = len;
    while left > 0 {
        let n = left.min(buf.len() as u64) as usize;
        // Without /dev/urandom, zeros do the same job.
        if let Some(r) = &mut random {
            if r.read_exact(&mut buf[..n]).is_err() {
                random = None;
                buf.fill(0);
            }
        }
        file.write_all(&buf[..n])?;
        left -= n as u64;
    }
    file.sync_all()
}

/// `name`, or `name (2)`, `name (3)`... whichever isn't taken in `dir`,
/// the extension kept at the end.
fn free_name(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if fs::symlink_metadata(&first).is_err() {
        return first;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => name.split_at(i),
        _ => (name, ""),
    };
    (2..).map(|n| dir.join(format!("{stem} ({n}){ext}"))).find(|p| fs::symlink_metadata(p).is_err()).expect("a free name")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_never_overwritten() {
        let d = std::env::temp_dir().join(format!("ferro-trash-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        let (bin, home) = (d.join("bin"), d.join("home"));
        fs::create_dir_all(home.join("folder")).unwrap();
        fs::write(home.join("a.jpg"), "1").unwrap();
        assert_eq!(put_in(&home.join("a.jpg"), &bin).unwrap(), bin.join("a.jpg"));
        fs::write(home.join("a.jpg"), "2").unwrap();
        assert_eq!(put_in(&home.join("a.jpg"), &bin).unwrap(), bin.join("a (2).jpg"));
        assert_eq!(fs::read_to_string(bin.join("a.jpg")).unwrap(), "1", "the first one is still there");
        put_in(&home.join("folder"), &bin).unwrap();
        assert_eq!(count_in(&bin), 3);
        assert!(put_in(&bin, &bin).is_err(), "not the bin into itself");
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn only_from_the_bins_own_drive() {
        let d = std::env::temp_dir().join(format!("ferro-trash-drive-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("a.jpg"), "1").unwrap();
        // The bin not made yet: its drive is still known.
        assert!(can_take_in(&d.join("a.jpg"), &d.join("not/made/bin")));
        // /proc is a drive of its own, as a USB stick would be.
        assert!(!can_take_in(Path::new("/proc/self/status"), &d.join("bin")));
        assert!(put_in(Path::new("/proc/self/status"), &d.join("bin")).is_err(), "never copied over");
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn shred_overwrites_and_never_follows_links() {
        let d = std::env::temp_dir().join(format!("ferro-trash-shred-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("stick/folder/inner")).unwrap();
        fs::write(d.join("elsewhere.txt"), "keep me").unwrap();
        fs::write(d.join("stick/folder/a.txt"), "secret").unwrap();
        fs::write(d.join("stick/folder/inner/b.txt"), "secret too").unwrap();
        std::os::unix::fs::symlink(d.join("elsewhere.txt"), d.join("stick/folder/link")).unwrap();
        std::os::unix::fs::symlink(&d, d.join("stick/folder/inner/up")).unwrap();
        fs::hard_link(d.join("elsewhere.txt"), d.join("stick/folder/hard")).unwrap();
        // Read-only, as copied off a CD.
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(d.join("stick/folder/a.txt"), fs::Permissions::from_mode(0o400)).unwrap();
        shred(&d.join("stick/folder")).unwrap();
        assert!(fs::symlink_metadata(d.join("stick/folder")).is_err());
        assert_eq!(fs::read_to_string(d.join("elsewhere.txt")).unwrap(), "keep me", "what a link points at, or another name for it, is left alone");

        // What's written over is the whole of it, the size kept.
        fs::write(d.join("c.bin"), vec![7u8; 3 << 20]).unwrap();
        let f = open_for_overwrite(&d.join("c.bin")).unwrap();
        overwrite(&f, 3 << 20).unwrap();
        let after = fs::read(d.join("c.bin")).unwrap();
        assert_eq!(after.len(), 3 << 20);
        assert!(after.chunks(1 << 20).all(|c| c.iter().filter(|&&b| b == 7).count() < 1 << 16));
        // A link is never opened to be written over.
        std::os::unix::fs::symlink(d.join("elsewhere.txt"), d.join("l")).unwrap();
        assert!(open_for_overwrite(&d.join("l")).is_err());
        fs::remove_dir_all(&d).unwrap();
    }
}
