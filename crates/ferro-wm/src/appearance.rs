//! Desktop appearance and theming settings for FerroOS.
//!
//! Loads and saves configuration from `~/.config/ferro/appearance.conf`
//! (falling back to `/etc/ferro/appearance.conf`). Supports custom solid
//! background colors (Classic Teal, Dark Slate, Obsidian Black, Navy, Plum),
//! dark UI mode, and wallpaper paths.

use std::path::PathBuf;

pub const COLOR_CLASSIC_TEAL: u32 = 0x008080;
pub const COLOR_DARK_SLATE: u32 = 0x182026;
pub const COLOR_OBSIDIAN: u32 = 0x000000;
pub const COLOR_RETRO_NAVY: u32 = 0x000080;
pub const COLOR_PLUM: u32 = 0x550055;
pub const COLOR_FOREST_GREEN: u32 = 0x005522;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Appearance {
    pub desktop_color: u32,
    pub dark_mode: bool,
    pub wallpaper: Option<String>,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            desktop_color: COLOR_CLASSIC_TEAL,
            dark_mode: false,
            wallpaper: None,
        }
    }
}

pub fn config_path() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".config").join("ferro").join("appearance.conf")
    } else {
        PathBuf::from("/etc/ferro/appearance.conf")
    }
}

impl Appearance {
    /// Loads settings from `~/.config/ferro/appearance.conf`, falling back to
    /// `/etc/ferro/appearance.conf`, or default classic teal.
    pub fn load() -> Self {
        let user_path = config_path();
        if let Ok(content) = std::fs::read_to_string(&user_path) {
            return Self::parse(&content);
        }
        if let Ok(content) = std::fs::read_to_string("/etc/ferro/appearance.conf") {
            return Self::parse(&content);
        }
        Self::default()
    }

    /// Parses a simple ini-style appearance configuration.
    pub fn parse(text: &str) -> Self {
        let mut app = Self::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                let key = k.trim().to_ascii_lowercase();
                let val = v.trim();
                match key.as_str() {
                    "desktop_color" | "color" | "background" => {
                        if let Some(c) = parse_color(val) {
                            app.desktop_color = c;
                        }
                    }
                    "dark_mode" | "dark" => {
                        app.dark_mode = val.eq_ignore_ascii_case("true") || val == "1" || val.eq_ignore_ascii_case("yes");
                    }
                    "theme" => {
                        app.dark_mode = val.eq_ignore_ascii_case("dark");
                    }
                    "wallpaper" => {
                        if !val.is_empty() {
                            app.wallpaper = Some(val.to_string());
                        } else {
                            app.wallpaper = None;
                        }
                    }
                    _ => {}
                }
            }
        }
        app
    }

    /// Saves appearance to `~/.config/ferro/appearance.conf`.
    #[allow(dead_code)]
    pub fn save(&self) -> std::io::Result<()> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let content = format!(
            "# FerroOS Appearance Configuration\n\
             desktop_color = 0x{:06X}\n\
             theme = {}\n\
             wallpaper = {}\n",
            self.desktop_color,
            if self.dark_mode { "dark" } else { "classic" },
            self.wallpaper.as_deref().unwrap_or("")
        );
        std::fs::write(path, content)
    }
}

/// Parses hex string like `0x008080`, `#008080`, or color name.
pub fn parse_color(s: &str) -> Option<u32> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    match s.to_ascii_lowercase().as_str() {
        "teal" | "classic" => return Some(COLOR_CLASSIC_TEAL),
        "dark" | "slate" | "charcoal" => return Some(COLOR_DARK_SLATE),
        "black" | "obsidian" => return Some(COLOR_OBSIDIAN),
        "navy" | "blue" => return Some(COLOR_RETRO_NAVY),
        "plum" | "purple" | "wine" => return Some(COLOR_PLUM),
        "green" | "forest" => return Some(COLOR_FOREST_GREEN),
        _ => {}
    }
    let hex = s.strip_prefix("0x").or_else(|| s.strip_prefix('#')).unwrap_or(s);
    u32::from_str_radix(hex, 16).ok().map(|c| c & 0x00ffffff)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferro_gfx::color;

    #[test]
    fn parses_color_names_and_hex() {
        assert_eq!(parse_color("teal"), Some(COLOR_CLASSIC_TEAL));
        assert_eq!(parse_color("dark"), Some(COLOR_DARK_SLATE));
        assert_eq!(parse_color("black"), Some(COLOR_OBSIDIAN));
        assert_eq!(parse_color("0x008080"), Some(0x008080));
        assert_eq!(parse_color("#182026"), Some(0x182026));
        assert_eq!(parse_color("000000"), Some(0x000000));
    }

    #[test]
    fn parses_ini_config() {
        let text = "\
# test comment
desktop_color = 0x182026
theme = dark
wallpaper = /path/to/bg.png
";
        let app = Appearance::parse(text);
        assert_eq!(app.desktop_color, 0x182026);
        assert!(app.dark_mode);
        assert_eq!(app.wallpaper.as_deref(), Some("/path/to/bg.png"));
    }

    #[test]
    fn default_is_classic_teal() {
        let app = Appearance::default();
        assert_eq!(app.desktop_color, color::DESKTOP);
        assert!(!app.dark_mode);
        assert_eq!(app.wallpaper, None);
    }
}
