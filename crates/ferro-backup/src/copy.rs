//! The copying: what's new or changed (by size and time) from one folder
//! to another, nothing deleted, links and special files left alone.

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
}

impl Done {
    pub fn add(&mut self, o: Done) {
        self.copied += o.copied;
        self.unchanged += o.unchanged;
        self.failed += o.failed;
        self.bytes += o.bytes;
    }

    pub fn said(&self) -> String {
        let mb = self.bytes as f64 / 1_048_576.0;
        let mut s = format!("{} file(s) copied ({mb:.1} MB), {} already there.", self.copied, self.unchanged);
        if self.failed > 0 {
            s.push_str(&format!(" {} couldn't be copied.", self.failed));
        }
        s
    }
}

/// Whether `to` already holds what `from` does: the same size, and not older.
fn current(from: &std::fs::Metadata, to: &Path) -> bool {
    let Ok(t) = std::fs::metadata(to) else { return false };
    t.len() == from.len() && matches!((from.modified(), t.modified()), (Ok(a), Ok(b)) if b >= a)
}

/// Copies what's new or changed in `from` into `to`, folder by folder.
pub fn mirror(from: &Path, to: &Path, progress: &impl Fn(Progress)) -> Done {
    let mut done = Done::default();
    if std::fs::create_dir_all(to).is_err() {
        done.failed += 1;
        return done;
    }
    let Ok(entries) = std::fs::read_dir(from) else {
        done.failed += 1;
        return done;
    };
    for e in entries.flatten() {
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
            let ok = std::fs::copy(e.path(), &part).is_ok()
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
    done
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_what_changed_and_deletes_nothing() {
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
        assert!(to.join("sub/b.txt").exists(), "a backup keeps what was deleted here");
        assert!(!to.join(".a.txt.part").exists());
        let _ = std::fs::remove_dir_all(&base);
    }
}
