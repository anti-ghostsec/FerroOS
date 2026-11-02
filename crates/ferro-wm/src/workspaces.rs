//! Desktops (workspaces), as X11's window managers have them (EWMH's
//! `_NET_NUMBER_OF_DESKTOPS` and friends): each window is on one of them,
//! or on all of them, and only the desktop shown has its windows on screen.
//!
//! How many there are is kept in `~/.config/ferro/workspaces.conf`
//! (`count = 4`), so one added or closed stays that way the next session.

use std::fs;
use std::path::PathBuf;

/// A window on every desktop (EWMH's 0xFFFFFFFF).
pub const ALL: u32 = 0xFFFF_FFFF;
/// How many a new user has.
pub const DEFAULT: u32 = 4;
/// The most there may be: each has its numbered button in the taskbar.
pub const MOST: u32 = 9;

fn config_path() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".config").join("ferro").join("workspaces.conf")
    } else {
        PathBuf::from("/etc/ferro/workspaces.conf")
    }
}

/// How many desktops there are.
pub fn load() -> u32 {
    fs::read_to_string(config_path()).map_or(DEFAULT, |text| parse(&text))
}

pub fn parse(text: &str) -> u32 {
    text.lines()
        .filter_map(|l| l.split_once('='))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case("count"))
        .and_then(|(_, v)| v.trim().parse().ok())
        .map_or(DEFAULT, |n: u32| n.clamp(1, MOST))
}

pub fn save(count: u32) {
    let path = config_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(&path, format!("# FerroOS Desktops\ncount = {count}\n"));
}

/// The desktops' names for `_NET_DESKTOP_NAMES`: each ended by a nul.
pub fn names(count: u32) -> Vec<u8> {
    (1..=count).flat_map(|i| format!("Desktop {i}\0").into_bytes()).collect()
}

/// Where a window on `desk` is once desktop `removed` is closed: its own
/// windows go to the one on its left (the first's to the one after it,
/// which becomes the first), and those after it move down one.
pub fn after_removal(desk: u32, removed: u32) -> u32 {
    if desk == ALL || desk < removed {
        desk
    } else if desk == removed {
        removed.saturating_sub(1)
    } else {
        desk - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_how_many() {
        assert_eq!(parse("# FerroOS Desktops\ncount = 6\n"), 6);
        assert_eq!(parse("Count=2"), 2);
        assert_eq!(parse(""), DEFAULT);
        assert_eq!(parse("count = lots"), DEFAULT);
        assert_eq!(parse("count = 0"), 1, "never none");
        assert_eq!(parse("count = 40"), MOST);
    }

    #[test]
    fn names_each_end_with_a_nul() {
        assert_eq!(names(2), b"Desktop 1\0Desktop 2\0");
    }

    #[test]
    fn closing_a_desktop_moves_its_windows_left() {
        // Closing the third of four: its windows to the second, the fourth's down one.
        assert_eq!([0, 1, 2, 3].map(|d| after_removal(d, 2)), [0, 1, 1, 2]);
        // Closing the first: its windows join the second's, which is now the first.
        assert_eq!([0, 1, 2].map(|d| after_removal(d, 0)), [0, 0, 1]);
        assert_eq!(after_removal(ALL, 1), ALL, "on all of them still");
    }
}
