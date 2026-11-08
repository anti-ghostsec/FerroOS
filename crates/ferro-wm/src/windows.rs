//! Window size and position persistence for FerroOS Heavy.
//!
//! Remembers application window geometry (x, y, width, height) in
//! `~/.config/ferro/windows.conf` so closing and reopening applications
//! restores their exact size and screen placement, like in Windows 7.
//! A program not opened for `FORGET_AFTER_DAYS` is forgotten: the file
//! would otherwise be a list of every program ever used.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// Programs not opened for this long are dropped from the file.
pub const FORGET_AFTER_DAYS: u64 = 30;

/// Today, in days since 1970.
fn today() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() / 86_400)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WindowStore {
    pub geometries: HashMap<String, (i32, i32, i32, i32)>,
    /// The day each was last opened or moved (days since 1970).
    pub used: HashMap<String, u64>,
}

fn config_path() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".config").join("ferro").join("windows.conf")
    } else {
        PathBuf::from("/etc/ferro/windows.conf")
    }
}

impl WindowStore {
    pub fn load() -> Self {
        let path = config_path();
        if let Ok(content) = fs::read_to_string(&path) {
            let mut store = Self::parse(&content);
            if store.forget_older(today(), FORGET_AFTER_DAYS) {
                store.save();
            }
            return store;
        }
        Self::default()
    }

    pub fn parse(text: &str) -> Self {
        let mut geometries = HashMap::new();
        let mut used_days = HashMap::new();
        let mut used = None;
        let mut current_section: Option<String> = None;
        let mut x = None;
        let mut y = None;
        let mut w = None;
        let mut h = None;

        let commit_section = |geos: &mut HashMap<String, (i32, i32, i32, i32)>,
                              sec: &Option<String>,
                              gx: Option<i32>,
                              gy: Option<i32>,
                              gw: Option<i32>,
                              gh: Option<i32>| {
            if let (Some(s), Some(vx), Some(vy), Some(vw), Some(vh)) = (sec, gx, gy, gw, gh) {
                geos.insert(s.clone(), (vx, vy, vw, vh));
            }
        };

        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            if let Some(sec) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                commit_section(&mut geometries, &current_section, x, y, w, h);
                if let (Some(s), Some(d)) = (&current_section, used.take()) {
                    used_days.insert(s.clone(), d);
                }
                current_section = Some(sec.trim().to_lowercase());
                x = None;
                y = None;
                w = None;
                h = None;
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                let k = k.trim().to_lowercase();
                let v = v.trim();
                match k.as_str() {
                    "x" => x = v.parse().ok(),
                    "y" => y = v.parse().ok(),
                    "w" | "width" => w = v.parse().ok(),
                    "h" | "height" => h = v.parse().ok(),
                    "used" => used = v.parse().ok(),
                    _ => {}
                }
            }
        }
        commit_section(&mut geometries, &current_section, x, y, w, h);
        if let (Some(s), Some(d)) = (&current_section, used) {
            used_days.insert(s.clone(), d);
        }
        // From before days were kept: counted from today.
        let now = today();
        for k in geometries.keys() {
            used_days.entry(k.clone()).or_insert(now);
        }
        used_days.retain(|k, _| geometries.contains_key(k));
        Self { geometries, used: used_days }
    }

    /// Drops programs last used more than `days` ago; true if any went.
    pub fn forget_older(&mut self, now: u64, days: u64) -> bool {
        let old: Vec<String> = self.used.iter().filter(|&(_, &d)| now.saturating_sub(d) > days).map(|(k, _)| k.clone()).collect();
        for k in &old {
            self.geometries.remove(k);
            self.used.remove(k);
        }
        !old.is_empty()
    }

    /// Opened again today: kept for another `FORGET_AFTER_DAYS`.
    pub fn touch(&mut self, app_id: &str) {
        let key = app_id.to_lowercase();
        if self.geometries.contains_key(&key) && self.used.insert(key, today()) != Some(today()) {
            self.save();
        }
    }

    pub fn get(&self, app_id: &str) -> Option<(i32, i32, i32, i32)> {
        self.geometries.get(&app_id.to_lowercase()).copied()
    }

    pub fn set(&mut self, app_id: &str, x: i32, y: i32, w: i32, h: i32) {
        if app_id.is_empty() || w < 100 || h < 100 {
            return;
        }
        let key = app_id.to_lowercase();
        self.geometries.insert(key.clone(), (x, y, w, h));
        self.used.insert(key, today());
        self.save();
    }

    pub fn save(&self) {
        let path = config_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let mut lines = Vec::new();
        lines.push("# FerroOS Window Placement Preferences".to_string());
        let mut keys: Vec<&String> = self.geometries.keys().collect();
        keys.sort();
        for k in keys {
            if let Some(&(x, y, w, h)) = self.geometries.get(k) {
                lines.push(format!("\n[{k}]"));
                lines.push(format!("x = {x}"));
                lines.push(format!("y = {y}"));
                lines.push(format!("w = {w}"));
                lines.push(format!("h = {h}"));
                lines.push(format!("used = {}", self.used.get(k).copied().unwrap_or_else(today)));
            }
        }
        let content = lines.join("\n") + "\n";
        let _ = fs::write(&path, content);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_retrieves_window_geometries() {
        let conf = r#"
[ferro-files]
x = 80
y = 60
w = 640
h = 480

[ferro-taskmgr]
x = 120
y = 100
w = 540
h = 420
"#;
        let store = WindowStore::parse(conf);
        assert_eq!(store.get("ferro-files"), Some((80, 60, 640, 480)));
        assert_eq!(store.get("FERRO-TASKMGR"), Some((120, 100, 540, 420)));
        assert_eq!(store.get("unknown"), None);
    }

    #[test]
    fn programs_not_used_for_a_month_are_forgotten() {
        let conf = "[old]\nx = 1\ny = 1\nw = 300\nh = 300\nused = 100\n\n[recent]\nx = 2\ny = 2\nw = 300\nh = 300\nused = 125\n";
        let mut store = WindowStore::parse(conf);
        assert_eq!(store.used.get("old"), Some(&100));
        assert!(store.forget_older(140, FORGET_AFTER_DAYS));
        assert_eq!(store.get("old"), None);
        assert_eq!(store.get("recent"), Some((2, 2, 300, 300)));
        assert!(!store.forget_older(140, FORGET_AFTER_DAYS));
    }

    #[test]
    fn entries_from_before_days_were_kept_start_today() {
        let store = WindowStore::parse("[legacy]\nx = 1\ny = 1\nw = 300\nh = 300\n");
        assert_eq!(store.used.get("legacy"), Some(&today()));
    }
}
