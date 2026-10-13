//! Retro Windows 95 File Types / Associations view and hit-testing.

use crate::config::{default_associations, AssociationEntry, PRESET_APPS};
use ferro_gfx::{color, icons, Bevel, Rect, Surface, GLYPH_H, GLYPH_W};

pub const W: i32 = 420;
pub const H: i32 = 340;

pub const LIST_RECT: Rect = Rect::new(20, 42, 380, 150);
pub const ROW_H: i32 = 18;
pub const VISIBLE_ROWS: usize = (LIST_RECT.h / ROW_H) as usize;

pub const PRESET_BTN: Rect = Rect::new(280, 240, 120, 22);
pub const RESTORE_BTN: Rect = Rect::new(20, 268, 125, 22);

pub const OK_BTN: Rect = Rect::new(170, 305, 75, 24);
pub const CANCEL_BTN: Rect = Rect::new(250, 305, 75, 24);
pub const APPLY_BTN: Rect = Rect::new(330, 305, 75, 24);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Select(usize),
    #[allow(dead_code)]
    ScrollUp,
    #[allow(dead_code)]
    ScrollDown,
    CyclePreset,
    RestoreDefaults,
    Ok,
    Cancel,
    Apply,
    Nothing,
}

pub struct State {
    pub entries: Vec<AssociationEntry>,
    pub selected: usize,
    pub scroll: usize,
    pub applied: bool,
}

impl Default for State {
    fn default() -> Self {
        Self { entries: default_associations(), selected: 0, scroll: 0, applied: true }
    }
}

pub fn hit(x: i32, y: i32, state: &State) -> Hit {
    if OK_BTN.contains(x, y) {
        return Hit::Ok;
    }
    if CANCEL_BTN.contains(x, y) {
        return Hit::Cancel;
    }
    if APPLY_BTN.contains(x, y) {
        return Hit::Apply;
    }
    if PRESET_BTN.contains(x, y) {
        return Hit::CyclePreset;
    }
    if RESTORE_BTN.contains(x, y) {
        return Hit::RestoreDefaults;
    }

    if LIST_RECT.contains(x, y) {
        let row = ((y - LIST_RECT.y) / ROW_H) as usize;
        let idx = state.scroll + row;
        if idx < state.entries.len() {
            return Hit::Select(idx);
        }
    }

    Hit::Nothing
}

pub fn draw(s: &mut Surface, w: i32, h: i32, state: &State) {
    s.fill(Rect::new(0, 0, w, h), color::FACE);

    // Tab Header
    let tab = Rect::new(10, 6, 80, 20);
    s.fill(tab, color::FACE);
    s.bevel(tab, Bevel::Raised);
    s.text(16, 9, "File Types", color::BLACK);

    // List header label
    s.text(20, 28, "Registered file types:", color::BLACK);

    // List Box
    s.fill(LIST_RECT, color::WHITE);
    s.bevel(LIST_RECT, Bevel::Sunken);

    for (i, entry) in state.entries.iter().skip(state.scroll).take(VISIBLE_ROWS).enumerate() {
        let abs_idx = state.scroll + i;
        let ry = LIST_RECT.y + 2 + i as i32 * ROW_H;
        let row_rect = Rect::new(LIST_RECT.x + 2, ry, LIST_RECT.w - 4, ROW_H);

        let is_sel = abs_idx == state.selected;
        if is_sel {
            s.fill(row_rect, color::NAVY);
        }

        let icon = if entry.ext == "png" || entry.ext == "jpg" || entry.ext == "bmp" {
            icons::FILE
        } else if entry.ext == "mp4" || entry.ext == "mp3" {
            icons::DRIVE
        } else {
            icons::FILE
        };
        s.sprite(row_rect.x + 2, ry + 1, icon, 1);

        let text_color = if is_sel { color::WHITE } else { color::BLACK };
        let ext_str = format!(".{}", entry.ext);
        s.text_bold(row_rect.x + 22, ry + 3, &ext_str, text_color);

        let desc_x = row_rect.x + 80;
        s.text(desc_x, ry + 3, &entry.desc, text_color);
    }

    // Details Group Box
    let det_grp = Rect::new(10, 200, w - 20, 95);
    s.bevel(det_grp, Bevel::Sunken);
    let det_title = Rect::new(18, 198, 105, GLYPH_H + 2);
    s.fill(det_title, color::FACE);
    s.text(20, 199, "File type details", color::BLACK);

    if let Some(entry) = state.entries.get(state.selected) {
        let ext_lbl = format!("Extension: .{}", entry.ext);
        s.text(20, 216, &ext_lbl, color::BLACK);

        let cmd_lbl = format!("Opens with: {}", entry.command);
        let max_chars = ((w - 50) / GLYPH_W) as usize;
        let trunc_cmd: String = cmd_lbl.chars().take(max_chars).collect();
        s.text(20, 234, &trunc_cmd, color::NAVY);
    }

    // Preset button
    s.button(PRESET_BTN, false);
    s.text(PRESET_BTN.x + 8, PRESET_BTN.y + 5, "Choose App...", color::BLACK);

    // Restore defaults button
    s.button(RESTORE_BTN, false);
    s.text(RESTORE_BTN.x + 8, RESTORE_BTN.y + 5, "Restore Defaults", color::BLACK);

    // Bottom action buttons
    s.button(OK_BTN, false);
    s.text(OK_BTN.x + (OK_BTN.w - 2 * GLYPH_W) / 2, OK_BTN.y + 6, "OK", color::BLACK);

    s.button(CANCEL_BTN, false);
    s.text(CANCEL_BTN.x + (CANCEL_BTN.w - 6 * GLYPH_W) / 2, CANCEL_BTN.y + 6, "Cancel", color::BLACK);

    s.button(APPLY_BTN, false);
    s.text(APPLY_BTN.x + (APPLY_BTN.w - 5 * GLYPH_W) / 2, APPLY_BTN.y + 6, "Apply", color::BLACK);
}

pub fn cycle_preset(cmd: &str) -> String {
    for (i, (_, app_cmd)) in PRESET_APPS.iter().enumerate() {
        if *app_cmd == cmd {
            let next_idx = (i + 1) % PRESET_APPS.len();
            return PRESET_APPS[next_idx].1.to_string();
        }
    }
    PRESET_APPS[0].1.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_test_buttons() {
        let state = State::default();
        assert_eq!(hit(OK_BTN.x + 2, OK_BTN.y + 2, &state), Hit::Ok);
        assert_eq!(hit(CANCEL_BTN.x + 2, CANCEL_BTN.y + 2, &state), Hit::Cancel);
        assert_eq!(hit(APPLY_BTN.x + 2, APPLY_BTN.y + 2, &state), Hit::Apply);
        assert_eq!(hit(PRESET_BTN.x + 2, PRESET_BTN.y + 2, &state), Hit::CyclePreset);
        assert_eq!(hit(RESTORE_BTN.x + 2, RESTORE_BTN.y + 2, &state), Hit::RestoreDefaults);
    }

    #[test]
    fn cycle_preset_advances() {
        let first = PRESET_APPS[0].1;
        let second = cycle_preset(first);
        assert_ne!(first, second);
        assert_eq!(second, PRESET_APPS[1].1);
    }
}
