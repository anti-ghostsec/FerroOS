//! Window size and position persistence for FerroOS Heavy.
//!
//! Remembers application window geometry (x, y, width, height) in
//! `~/.config/ferro/windows.conf` so closing and reopening applications
//! restores their exact size and screen placement, like in Windows 7.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WindowStore {
    pub geometries: HashMap<String, (i32, i32, i32, i32)>,
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
            return Self::parse(&content);
        }
        Self::default()
    }

    pub fn parse(text: &str) -> Self {
        let mut geometries = HashMap::new();
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
                    _ => {}
                }
            }
        }
        commit_section(&mut geometries, &current_section, x, y, w, h);
        Self { geometries }
    }

    pub fn get(&self, app_id: &str) -> Option<(i32, i32, i32, i32)> {
        self.geometries.get(&app_id.to_lowercase()).copied()
    }

    pub fn set(&mut self, app_id: &str, x: i32, y: i32, w: i32, h: i32) {
        if app_id.is_empty() || w < 100 || h < 100 {
            return;
        }
        let key = app_id.to_lowercase();
        self.geometries.insert(key, (x, y, w, h));
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
}
