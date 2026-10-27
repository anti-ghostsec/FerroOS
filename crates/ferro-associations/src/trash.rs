//! The Recycle Bin: `~/.local/share/Trash`, where the desktop, the file
//! manager and the image viewer put what's deleted, so it can be got back.
//! A name already in there is never overwritten: the newcomer gets
//! `name (2)`, `name (3)`, and so on. Things on another drive (a USB stick)
//! are copied in, then removed from where they were.

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

/// Moves `path` into the Recycle Bin; returns where it went.
pub fn put(path: &Path) -> io::Result<PathBuf> {
    put_in(path, &dir())
}

fn put_in(path: &Path, bin: &Path) -> io::Result<PathBuf> {
    fs::symlink_metadata(path)?;
    if bin.starts_with(path) || path.starts_with(bin) {
        return Err(io::Error::other("that's the Recycle Bin itself"));
    }
    fs::create_dir_all(bin)?;
    let name = path.file_name().ok_or_else(|| io::Error::other("no name"))?.to_string_lossy().into_owned();
    let to = free_name(bin, &name);
    match fs::rename(path, &to) {
        Ok(()) => return Ok(to),
        // Not on the same drive: copy it over, then remove it.
        Err(e) if e.raw_os_error() == Some(18) => {}
        Err(e) => return Err(e),
    }
    if let Err(e) = copy(path, &to) {
        let _ = remove(&to);
        return Err(e);
    }
    remove(path)?;
    Ok(to)
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

fn copy(from: &Path, to: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(from)?;
    if meta.file_type().is_symlink() {
        #[cfg(unix)]
        return std::os::unix::fs::symlink(fs::read_link(from)?, to);
    }
    if meta.is_dir() {
        fs::create_dir(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy(&entry.path(), &to.join(entry.file_name()))?;
        }
        return Ok(());
    }
    fs::copy(from, to).map(drop).or_else(|_| {
        // A drive that won't take the permissions (FAT): just the bytes.
        let mut src = fs::File::open(from)?;
        let mut dst = fs::File::create(to)?;
        io::copy(&mut src, &mut dst).map(drop)
    })
}

fn remove(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path)?.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
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
}
