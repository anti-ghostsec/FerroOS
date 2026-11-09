//! Open as Text: the list handed to the text editor in a file kept in
//! memory (the session's runtime folder, which only its owner can open),
//! never on the disk, and taken away again as soon as the editor has read
//! it. What the editor shows is its own copy; the file itself lasts a
//! moment, not until the next logout.

use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long the editor gets to read it before it's removed anyway.
pub const READ_TIME: Duration = Duration::from_secs(30);
/// How long a window that's closing waits for an editor still starting.
pub const EXIT_WAIT: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(100);
/// How long a program that's read it gets to close it.
const LET_GO: Duration = Duration::from_secs(2);

/// The files handed over and not yet removed.
static PENDING: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Shows `text` in the text editor; or why it can't.
pub fn show(text: &str) -> Result<(), String> {
    let mounts = std::fs::read_to_string("/proc/self/mounts").unwrap_or_default();
    let dir = memory_dir(std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from), &mounts)
        .ok_or_else(|| "There's nowhere in memory to put it (no runtime folder), and it isn't put on the disk.".to_owned())?;
    let file = write_private(&dir, text).map_err(|e| format!("It couldn't be made: {e}"))?;
    ferro_associations::open_in_program(&file);
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).push(file.clone());
    std::thread::spawn(move || {
        remove_once_read(&file, READ_TIME);
        PENDING.lock().unwrap_or_else(|e| e.into_inner()).retain(|p| p != &file);
    });
    Ok(())
}

/// Where the file can go: the session's runtime folder, or else `/tmp` if
/// that's in memory (tmpfs). Never a folder on the disk.
fn memory_dir(runtime: Option<PathBuf>, mounts: &str) -> Option<PathBuf> {
    if let Some(dir) = runtime.filter(|d| d.is_absolute() && d.is_dir()) {
        return Some(dir);
    }
    let tmp = Path::new("/tmp");
    (fs_type(tmp, mounts) == Some("tmpfs")).then(|| tmp.to_path_buf())
}

/// What kind of file system `dir` is on, by the mount table
/// (`/proc/self/mounts`): the one mounted deepest above it.
fn fs_type<'a>(dir: &Path, mounts: &'a str) -> Option<&'a str> {
    let mut best: Option<(usize, &str)> = None;
    for line in mounts.lines() {
        let mut parts = line.split_whitespace();
        let (Some(_), Some(at), Some(kind)) = (parts.next(), parts.next(), parts.next()) else { continue };
        // Spaces in mount points are written \040.
        let at = at.replace("\\040", " ");
        let at = Path::new(&at);
        if dir.starts_with(at) {
            let depth = at.components().count();
            // The later of two at the same place is the one on top.
            if best.is_none_or(|(d, _)| depth >= d) {
                best = Some((depth, kind));
            }
        }
    }
    best.map(|(_, kind)| kind)
}

/// A name no other program can guess beforehand.
fn unguessable() -> String {
    let mut bytes = [0u8; 16];
    let read = std::fs::File::open("/dev/urandom").and_then(|mut f| std::io::Read::read_exact(&mut f, &mut bytes));
    if read.is_err() {
        // Not to be had: the time and the process, which at least never repeat.
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        bytes[..8].copy_from_slice(&(t.as_nanos() as u64).to_le_bytes());
        bytes[8..12].copy_from_slice(&std::process::id().to_le_bytes());
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `text` in a new file in `dir` that only its owner can read (0600),
/// never one already there (nor a link someone left in its place).
fn write_private(dir: &Path, text: &str) -> std::io::Result<PathBuf> {
    let file = dir.join(format!("ferro-info-{}.txt", unguessable()));
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&file)?;
    if let Err(e) = f.write_all(text.as_bytes()) {
        let _ = std::fs::remove_file(&file);
        return Err(e);
    }
    Ok(file)
}

/// Whether something has read `file` since it was written: on a file system
/// in memory (relatime) the first read moves its access time past its
/// change time.
fn been_read(file: &Path) -> bool {
    std::fs::metadata(file).is_ok_and(|m| (m.atime(), m.atime_nsec()) > (m.mtime(), m.mtime_nsec()))
}

/// Whether a program (of ours: others' can't be looked into) has `file`
/// open now.
fn held_open(file: &Path) -> bool {
    let Ok(procs) = std::fs::read_dir("/proc") else { return false };
    procs.flatten().filter(|p| p.file_name().to_str().is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit()))).any(|p| {
        std::fs::read_dir(p.path().join("fd")).into_iter().flatten().flatten().any(|fd| std::fs::read_link(fd.path()).is_ok_and(|t| t == file))
    })
}

/// Waits for `file` to be read and closed again, `wait` at the most; then
/// removes it. Whether it was read.
fn remove_once_read(file: &Path, wait: Duration) -> bool {
    let start = Instant::now();
    let mut seen_open = false;
    let mut read = false;
    while start.elapsed() < wait && file.exists() {
        let open = held_open(file);
        if been_read(file) || (seen_open && !open) {
            // Read: given a moment to let go of it (an editor that keeps it
            // open has its copy all the same).
            let until = (start.elapsed() + LET_GO).min(wait);
            while start.elapsed() < until && held_open(file) {
                std::thread::sleep(POLL);
            }
            read = true;
            break;
        }
        seen_open |= open;
        std::thread::sleep(POLL);
    }
    let _ = std::fs::remove_file(file);
    read
}

/// For a window closing: removes what's left, after giving an editor that
/// hasn't read its file yet `wait` (all of them together) to do it.
pub fn finish(wait: Duration) {
    let left: Vec<PathBuf> = std::mem::take(&mut *PENDING.lock().unwrap_or_else(|e| e.into_inner()));
    let deadline = Instant::now() + wait;
    for file in left {
        remove_once_read(&file, deadline.saturating_duration_since(Instant::now()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOUNTS: &str = "sysfs /sys sysfs rw 0 0\n/dev/sda1 / ext4 rw 0 0\ntmpfs /run/user/1000 tmpfs rw,relatime 0 0\n/dev/sda2 /home ext4 rw 0 0\n";

    #[test]
    fn file_system_by_mount_table() {
        assert_eq!(fs_type(Path::new("/tmp"), MOUNTS), Some("ext4"), "on the root, a disk");
        assert_eq!(fs_type(Path::new("/run/user/1000"), MOUNTS), Some("tmpfs"));
        let with_tmp = format!("{MOUNTS}tmpfs /tmp tmpfs rw 0 0\n");
        assert_eq!(fs_type(Path::new("/tmp"), &with_tmp), Some("tmpfs"));
        assert_eq!(fs_type(Path::new("/tmpx"), &with_tmp), Some("ext4"), "whole names only");
    }

    #[test]
    fn never_a_folder_on_the_disk() {
        let runtime = std::env::temp_dir();
        assert_eq!(memory_dir(Some(runtime.clone()), MOUNTS), Some(runtime), "the runtime folder when there is one");
        assert_eq!(memory_dir(None, MOUNTS), None, "/tmp on the disk: refused");
        assert_eq!(memory_dir(Some(PathBuf::from("relative")), MOUNTS), None);
        assert_eq!(memory_dir(None, "tmpfs /tmp tmpfs rw 0 0\n"), Some(PathBuf::from("/tmp")));
    }

    fn scratch() -> PathBuf {
        let d = std::env::temp_dir().join(format!("ferro-info-text-test-{}-{}", std::process::id(), unguessable()));
        std::fs::create_dir(&d).unwrap();
        d
    }

    #[test]
    fn private_and_unguessable() {
        let d = scratch();
        let (a, b) = (write_private(&d, "one").unwrap(), write_private(&d, "two").unwrap());
        assert_ne!(a, b);
        assert_eq!(std::fs::metadata(&a).unwrap().mode() & 0o777, 0o600);
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "two");
        let name = a.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(name.len(), "ferro-info-.txt".len() + 32, "{name}");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn removed_once_read() {
        let d = scratch();
        let file = write_private(&d, "list").unwrap();
        let reader = file.clone();
        // An editor: starts a moment later, reads it, keeps its copy.
        let editor = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            std::fs::read_to_string(reader).unwrap()
        });
        let start = Instant::now();
        // Where `atime` isn't kept (noatime), the read is seen by the
        // file being open, or not at all: then only the wait removes it.
        let read = remove_once_read(&file, Duration::from_secs(5));
        assert_eq!(editor.join().unwrap(), "list");
        assert!(!file.exists());
        if read {
            assert!(start.elapsed() < Duration::from_secs(4), "removed soon after it's read");
        }
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn removed_anyway_when_never_read() {
        let d = scratch();
        let file = write_private(&d, "list").unwrap();
        let start = Instant::now();
        assert!(!remove_once_read(&file, Duration::from_millis(400)));
        assert!(!file.exists());
        assert!(start.elapsed() >= Duration::from_millis(400));
        std::fs::remove_dir_all(&d).unwrap();
    }
}
