//! The copying: what's new or changed (by size and time) from one folder
//! to another, and what's no longer there deleted there too (written over
//! first), links and special files left alone.

use std::path::Path;

/// What's being copied now.
#[derive(Debug)]
pub struct Progress {
    pub name: String,
}

/// What a backup did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Done {
    pub copied: u64,
    pub unchanged: u64,
    pub failed: u64,
    pub bytes: u64,
    pub deleted: u64,
}

impl Done {
    pub fn add(&mut self, o: Done) {
        self.copied += o.copied;
        self.unchanged += o.unchanged;
        self.failed += o.failed;
        self.bytes += o.bytes;
        self.deleted += o.deleted;
    }

    pub fn said(&self) -> String {
        let mb = self.bytes as f64 / 1_048_576.0;
        let mut s = format!("{} file(s) copied ({mb:.1} MB), {} already there.", self.copied, self.unchanged);
        if self.deleted > 0 {
            s.push_str(&format!(" {} deleted, as they are here.", self.deleted));
        }
        if self.failed > 0 {
            s.push_str(&format!(" {} couldn't be copied.", self.failed));
        }
        s
    }
}

/// Whether `to` already holds what `from` does: the same size, and not older.
/// (A link there isn't: it's replaced by the file.)
fn current(from: &std::fs::Metadata, to: &Path) -> bool {
    let Ok(t) = std::fs::symlink_metadata(to) else { return false };
    t.is_file() && t.len() == from.len() && matches!((from.modified(), t.modified()), (Ok(a), Ok(b)) if b >= a)
}

/// Copies what's new or changed in `from` into `to`, folder by folder.
///
/// Nothing on the drive is followed: a folder there that's a link (to the
/// home folder, say, on a drive someone prepared) isn't copied into.
pub fn mirror(from: &Path, to: &Path, progress: &impl Fn(Progress)) -> Done {
    let mut done = Done::default();
    if !real_dir(to) {
        done.failed += 1;
        return done;
    }
    let Ok(entries) = std::fs::read_dir(from) else {
        done.failed += 1;
        return done;
    };
    let mut here = std::collections::HashSet::new();
    for e in entries.flatten() {
        here.insert(e.file_name());
        let Ok(kind) = e.file_type() else { continue };
        let dest = to.join(e.file_name());
        if kind.is_dir() {
            done.add(mirror(&e.path(), &dest, progress));
        } else if kind.is_file() {
            let Ok(meta) = e.metadata() else {
                done.failed += 1;
                continue;
            };
            if current(&meta, &dest) {
                done.unchanged += 1;
                continue;
            }
            progress(Progress { name: e.file_name().to_string_lossy().into_owned() });
            // Into a part file first: a pulled-out drive leaves no half copy
            // under the real name.
            let part = to.join(format!(".{}.part", e.file_name().to_string_lossy()));
            let ok = copy_new(&e.path(), &part).is_ok()
                && std::fs::File::options().write(true).open(&part).and_then(|f| f.set_modified(meta.modified()?)).is_ok()
                && std::fs::rename(&part, &dest).is_ok();
            if ok {
                done.copied += 1;
                done.bytes += meta.len();
            } else {
                let _ = std::fs::remove_file(&part);
                done.failed += 1;
            }
        }
    }
    // What was deleted here (or became a link, or something special): gone
    // from the backup too, written over.
    for e in std::fs::read_dir(to).into_iter().flatten().flatten() {
        let name = e.file_name();
        let part = name.to_string_lossy().starts_with('.') && name.to_string_lossy().ends_with(".part");
        let kept = here.contains(&name)
            && std::fs::symlink_metadata(from.join(&name)).is_ok_and(|m| m.is_file() || m.is_dir())
            && e.file_type().is_ok_and(|t| !t.is_symlink());
        if part || !kept {
            if ferro_associations::trash::shred(&e.path()).is_ok() {
                done.deleted += 1;
            } else {
                done.failed += 1;
            }
        }
    }
    done
}

/// `dir` is a folder of its own (made if it isn't there), not a link to one.
pub fn real_dir(dir: &Path) -> bool {
    match std::fs::symlink_metadata(dir) {
        Ok(m) => m.is_dir(),
        Err(_) => std::fs::create_dir(dir).is_ok(),
    }
}

/// A new file at `to` (never through a link left there) with `from`'s bytes.
fn copy_new(from: &Path, to: &Path) -> std::io::Result<()> {
    let _ = std::fs::remove_file(to);
    let mut out = std::fs::File::options().write(true).create_new(true).open(to)?;
    std::io::copy(&mut std::fs::File::open(from)?, &mut out)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_what_changed_and_deletes_what_was_deleted() {
        let base = std::env::temp_dir().join(format!("ferro-backup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (from, to) = (base.join("from"), base.join("to"));
        std::fs::create_dir_all(from.join("sub")).unwrap();
        std::fs::write(from.join("a.txt"), "one").unwrap();
        std::fs::write(from.join("sub/b.txt"), "two").unwrap();
        let first = mirror(&from, &to, &|_| {});
        assert_eq!((first.copied, first.unchanged, first.failed), (2, 0, 0));
        assert_eq!(std::fs::read_to_string(to.join("sub/b.txt")).unwrap(), "two");
        // Again: nothing to do. Then one changed, one deleted here.
        assert_eq!(mirror(&from, &to, &|_| {}).unchanged, 2);
        std::fs::write(from.join("a.txt"), "one, longer").unwrap();
        std::fs::remove_file(from.join("sub/b.txt")).unwrap();
        let third = mirror(&from, &to, &|_| {});
        assert_eq!((third.copied, third.unchanged), (1, 0));
        assert_eq!(std::fs::read_to_string(to.join("a.txt")).unwrap(), "one, longer");
        assert!(!to.join("sub/b.txt").exists(), "what was deleted here is deleted there");
        assert_eq!(third.deleted, 1);
        assert!(!to.join(".a.txt.part").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn links_on_the_drive_are_not_followed() {
        let base = std::env::temp_dir().join(format!("ferro-backup-links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (from, to, home) = (base.join("from"), base.join("to"), base.join("home"));
        std::fs::create_dir_all(from.join("sub")).unwrap();
        std::fs::create_dir_all(&to).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(from.join("sub/b.txt"), "two").unwrap();
        std::fs::write(from.join("a.txt"), "one").unwrap();
        // A prepared drive: a folder and a part file that lead elsewhere.
        std::os::unix::fs::symlink(&home, to.join("sub")).unwrap();
        std::os::unix::fs::symlink(home.join("planted"), to.join(".a.txt.part")).unwrap();
        let done = mirror(&from, &to, &|_| {});
        assert_eq!(std::fs::read_dir(&home).unwrap().count(), 0, "nothing written through the links");
        assert_eq!(std::fs::read_to_string(to.join("a.txt")).unwrap(), "one");
        assert_eq!(done.failed, 1, "the linked folder is refused");
        let _ = std::fs::remove_dir_all(&base);
    }
}
