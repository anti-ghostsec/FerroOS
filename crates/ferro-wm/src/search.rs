//! Typing in the Start menu: programs, settings and files whose names have
//! what was typed in them, the best first (names that start with it, then
//! words that do, then anywhere in the name).

use std::path::{Path, PathBuf};

/// The parts of the Control Panel, by the names people look for them by.
pub const SETTINGS: &[(&str, &str)] = &[
    ("Control Panel", "ferro-settings"),
    ("Network & Privacy", "ferro-network"),
    ("Wi-Fi", "ferro-network"),
    ("VPN", "ferro-network"),
    ("Camera and Microphone", "ferro-network"),
    ("Appearance & Display", "ferro-appearance"),
    ("Wallpaper", "ferro-appearance"),
    ("Sound", "ferro-volume"),
    ("Date & Time", "ferro-datetime"),
    ("Default Programs", "ferro-associations"),
    ("Add Programs", "ferro-terminal -e ferro-heavy menu"),
    ("Updates", "ferro-terminal -e ferro-heavy update"),
    ("Task Manager", "ferro-taskmgr"),
    ("Snipping Tool", "ferro-snip"),
    ("Notifications", "ferro-notifications"),
    ("Clipboard History", "ferro-clipboard --pick"),
];

/// How well `name` matches `query` (already lowercase): 0 best, None not.
pub fn rank(name: &str, query: &str) -> Option<u8> {
    let name = name.to_lowercase();
    if query.is_empty() {
        return None;
    }
    if name.starts_with(query) {
        Some(0)
    } else if name.split(|c: char| !c.is_alphanumeric()).any(|w| w.starts_with(query)) {
        Some(1)
    } else if name.contains(query) {
        Some(2)
    } else {
        None
    }
}

/// Files and folders under `home` whose names match `query`: the best
/// `max`, nearest first. Hidden ones, and folders programs fill (caches,
/// build output), aren't looked in; nor more than a few thousand entries,
/// so typing stays quick.
pub fn files(home: &Path, query: &str, max: usize) -> Vec<(String, PathBuf, bool)> {
    const SKIP: &[&str] = &["node_modules", "target", "__pycache__", "snap"];
    let mut found: Vec<(u8, usize, String, PathBuf, bool)> = Vec::new();
    let mut queue = std::collections::VecDeque::from([(home.to_path_buf(), 0usize)]);
    let mut seen = 0usize;
    while let Some((dir, depth)) = queue.pop_front() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            seen += 1;
            if seen > 6000 {
                queue.clear();
                break;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
            if let Some(r) = rank(&name, query) {
                found.push((r, depth, name.clone(), entry.path(), is_dir));
            }
            if is_dir && depth < 5 && !SKIP.contains(&name.as_str()) {
                queue.push_back((entry.path(), depth + 1));
            }
        }
    }
    found.sort_by(|a, b| (a.0, a.1, &a.2).cmp(&(b.0, b.1, &b.2)));
    found.into_iter().take(max).map(|(_, _, n, p, d)| (n, p, d)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_starts_then_words_then_anywhere() {
        assert_eq!(rank("Task Manager", "task"), Some(0));
        assert_eq!(rank("Task Manager", "man"), Some(1));
        assert_eq!(rank("Snipping Tool", "ping"), Some(2));
        assert_eq!(rank("Sound", "vpn"), None);
        assert_eq!(rank("Sound", ""), None);
    }

    #[test]
    fn finds_files_nearest_first_and_not_hidden() {
        let home = std::env::temp_dir().join(format!("ferro-search-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("Documents/old")).unwrap();
        std::fs::create_dir_all(home.join(".cache")).unwrap();
        std::fs::write(home.join("Documents/old/report.txt"), "").unwrap();
        std::fs::write(home.join("report.pdf"), "").unwrap();
        std::fs::write(home.join(".cache/report.tmp"), "").unwrap();
        let names: Vec<String> = files(&home, "report", 10).into_iter().map(|f| f.0).collect();
        assert_eq!(names, ["report.pdf", "report.txt"]);
        let dirs = files(&home, "old", 10);
        assert!(dirs.iter().any(|f| f.0 == "old" && f.2));
        let _ = std::fs::remove_dir_all(&home);
    }
}
