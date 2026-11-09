//! Find: what's in a folder and the folders in it whose names, or tags
//! (kept inside the files: see ferro_meta::tags), have the words typed.
//! `tag:anime` looks at the tags alone; quotes keep words together
//! (`tag:"ice hockey"`). Everything typed has to be there.
//!
//! Nothing is written down: no index, no cache on the disk. Each Find looks
//! afresh, and what it finds lives only in the window showing it.

use crate::view::Entry;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The most found at once (a Find of "e" over a whole drive stops there).
pub const MOST: usize = 5000;

/// One thing typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Term {
    /// In lower case.
    pub word: String,
    /// `tag:`: only in the tags.
    pub tag_only: bool,
}

/// What was typed, as terms.
pub fn parse(typed: &str) -> Vec<Term> {
    let mut out = Vec::new();
    let mut chars = typed.chars().peekable();
    loop {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        if chars.peek().is_none() {
            return out;
        }
        let mut word = String::new();
        let mut quoted = false;
        while let Some(&c) = chars.peek() {
            if c == '"' {
                quoted = !quoted;
            } else if c.is_whitespace() && !quoted {
                break;
            } else {
                word.push(c);
            }
            chars.next();
        }
        let lower = word.to_lowercase();
        let (word, tag_only) = match lower.strip_prefix("tag:").or_else(|| lower.strip_prefix("tags:")) {
            Some(rest) => (rest.trim().to_owned(), true),
            None => (lower, false),
        };
        if !word.is_empty() {
            out.push(Term { word, tag_only });
        }
    }
}

/// Whether a file called `name`, with the tags `tags` gives (asked only if
/// they're needed), has everything in `terms`.
pub fn matches(terms: &[Term], name: &str, tags: impl FnOnce() -> Vec<String>) -> bool {
    if terms.is_empty() {
        return false;
    }
    let name = name.to_lowercase();
    let by_name = |t: &Term| !t.tag_only && name.contains(&t.word);
    if terms.iter().all(by_name) {
        return true;
    }
    let tags: Vec<String> = tags().iter().map(|t| t.to_lowercase()).collect();
    terms.iter().all(|t| by_name(t) || tags.iter().any(|tag| tag.contains(&t.word)))
}

/// Folders a Find never goes into: the system's own, which aren't files.
fn skipped(dir: &Path) -> bool {
    ["/proc", "/sys", "/dev", "/run"].iter().any(|p| dir == Path::new(p))
}

/// What's in `base` and below it that `terms` finds, as entries of `base`
/// (named by where they are under it: `Pictures\cat.jpg`). Hidden ones only
/// with `hidden`; links to folders aren't followed. Stops early, with what
/// it has, once `gen` isn't `at` any more (another Find, another folder).
pub fn find(base: &Path, terms: &[Term], hidden: bool, gen: &AtomicU64, at: u64) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut todo: Vec<PathBuf> = vec![PathBuf::new()];
    while let Some(rel) = todo.pop() {
        if gen.load(Ordering::Relaxed) != at || out.len() >= MOST {
            break;
        }
        let dir = base.join(&rel);
        let Ok(read) = std::fs::read_dir(&dir) else { continue };
        let mut subdirs = Vec::new();
        for e in read.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !hidden && name.starts_with('.') {
                continue;
            }
            let Ok(kind) = e.file_type() else { continue };
            let is_dir = kind.is_dir() || (kind.is_symlink() && e.path().is_dir());
            let rel_path = rel.join(&name);
            if kind.is_dir() && !skipped(&e.path()) {
                subdirs.push(rel_path.clone());
            }
            let path = e.path();
            let tags = || if is_dir { Vec::new() } else { ferro_meta::tags::read_tags(&path) };
            if matches(terms, &name, tags) {
                let meta = e.metadata().ok();
                let size = if is_dir { 0 } else { meta.as_ref().map_or(0, std::fs::Metadata::len) };
                let modified = meta
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_secs() as i64);
                let real = rel_path.to_string_lossy().into_owned();
                out.push(Entry { name: real.replace('/', "\\"), real, is_dir, icon: None, size, modified });
                if out.len() >= MOST {
                    break;
                }
            }
        }
        // Gone through in name order, nearer ones first.
        subdirs.sort();
        todo.extend(subdirs.into_iter().rev());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_was_typed() {
        assert_eq!(
            parse("  Holiday tag:Anime  tag:\"ice hockey\" \"two words\" tag: "),
            vec![
                Term { word: "holiday".into(), tag_only: false },
                Term { word: "anime".into(), tag_only: true },
                Term { word: "ice hockey".into(), tag_only: true },
                Term { word: "two words".into(), tag_only: false },
            ]
        );
        assert!(parse("   ").is_empty());
    }

    #[test]
    fn names_and_tags() {
        let tags = || vec!["Anime".to_owned(), "Comedy".to_owned()];
        assert!(matches(&parse("anime"), "show.mkv", tags), "by a tag");
        assert!(matches(&parse("SHOW"), "show.mkv", || panic!("tags not needed")), "by the name alone");
        assert!(matches(&parse("show comedy"), "show.mkv", tags), "the name and a tag");
        assert!(!matches(&parse("tag:show"), "show.mkv", tags), "tag: means only tags");
        assert!(matches(&parse("tag:com"), "x.mkv", tags), "part of a tag");
        assert!(!matches(&parse("anime sports"), "x.mkv", tags), "all of it has to be there");
        assert!(!matches(&[], "x", tags));
    }

    #[test]
    fn looks_through_folders_below() {
        let base = std::env::temp_dir().join(format!("ferro-files-find-{}", std::process::id()));
        std::fs::create_dir_all(base.join("Shows/Old")).unwrap();
        std::fs::create_dir_all(base.join(".hidden")).unwrap();
        for f in ["anime list.txt", "Shows/Anime Night.txt", "Shows/Old/anime.txt", ".hidden/anime.txt", "other.txt"] {
            std::fs::write(base.join(f), "x").unwrap();
        }
        let gen = AtomicU64::new(1);
        let names = |hidden| -> Vec<String> {
            let mut v: Vec<String> = find(&base, &parse("anime"), hidden, &gen, 1).into_iter().map(|e| e.name).collect();
            v.sort();
            v
        };
        assert_eq!(names(false), ["Shows\\Anime Night.txt", "Shows\\Old\\anime.txt", "anime list.txt"]);
        assert_eq!(names(true).len(), 4);
        let found = find(&base, &parse("old"), false, &gen, 1);
        assert_eq!(found.len(), 1);
        assert!(found[0].is_dir && found[0].real == "Shows/Old");
        assert!(find(&base, &parse("anime"), false, &gen, 2).is_empty(), "stopped: another Find started");
        std::fs::remove_dir_all(&base).unwrap();
    }
}
