//! What the Appearance / Display Properties dialog looks like and where its parts are.
//!
//! A classic Windows 95 Display Properties layout:
//! - Monitor preview showing the current background color and a mini active window.
//! - Color palette presets (Teal, Dark Slate, Obsidian, Navy, Plum, Forest).
//! - Dark UI Mode toggle.
//! - OK, Cancel, Apply buttons.

use ferro_gfx::{color, Bevel, Rect, Surface, GLYPH_H};
use ferro_ui::button;

pub const W: i32 = 380;
pub const H: i32 = 330;

pub const PRESET_COLORS: [(&str, u32); 6] =
    [("Teal", 0x008080), ("Slate", 0x182026), ("Black", 0x000000), ("Navy", 0x000080), ("Plum", 0x550055), ("Forest", 0x005522)];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Color(u32),
    ToggleDark,
    Ok,
    Cancel,
    Apply,
    Nothing,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub selected_color: u32,
    pub dark_mode: bool,
    pub applied: bool,
}

impl Default for State {
    fn default() -> Self {
        Self { selected_color: 0x008080, dark_mode: false, applied: true }
    }
}

// Geometry constants
const MONITOR_OUTER: Rect = Rect::new(90, 20, 200, 120);
const MONITOR_SCREEN: Rect = Rect::new(104, 30, 172, 98);
const COLOR_ROW1_Y: i32 = 160;
const COLOR_ROW2_Y: i32 = 192;
const SWATCH_W: i32 = 100;
const SWATCH_H: i32 = 24;

const DARK_CHECK: Rect = Rect::new(30, 240, 160, 22);

const OK_BTN: Rect = Rect::new(110, 290, 75, 24);
const CANCEL_BTN: Rect = Rect::new(195, 290, 75, 24);
const APPLY_BTN: Rect = Rect::new(280, 290, 75, 24);

fn swatch_rect(idx: usize) -> Rect {
    let col = idx % 3;
    let row = idx / 3;
    let x = 30 + col as i32 * (SWATCH_W + 15);
    let y = if row == 0 { COLOR_ROW1_Y } else { COLOR_ROW2_Y };
    Rect::new(x, y, SWATCH_W, SWATCH_H)
}

pub fn hit(x: i32, y: i32) -> Hit {
    if OK_BTN.contains(x, y) {
        return Hit::Ok;
    }
    if CANCEL_BTN.contains(x, y) {
        return Hit::Cancel;
    }
    if APPLY_BTN.contains(x, y) {
        return Hit::Apply;
    }
    if DARK_CHECK.contains(x, y) {
        return Hit::ToggleDark;
    }
    for (i, (_, c)) in PRESET_COLORS.iter().enumerate() {
        if swatch_rect(i).contains(x, y) {
            return Hit::Color(*c);
        }
    }
    Hit::Nothing
}

pub fn draw(s: &mut Surface, w: i32, h: i32, state: &State) {
    s.fill(Rect::new(0, 0, w, h), color::FACE);

    // 1. Draw mini monitor preview
    s.fill(MONITOR_OUTER, color::SHADOW);
    s.bevel(MONITOR_OUTER, Bevel::Raised);
    s.fill(MONITOR_SCREEN, state.selected_color);
    s.bevel(MONITOR_SCREEN, Bevel::Sunken);

    // Mini active window on the monitor
    let mini_win = Rect::new(MONITOR_SCREEN.x + 20, MONITOR_SCREEN.y + 15, 120, 65);
    let win_face = if state.dark_mode { 0x282828 } else { color::FACE };
    s.fill(mini_win, win_face);
    s.bevel(mini_win, Bevel::Window);

    let mini_title = Rect::new(mini_win.x + 2, mini_win.y + 2, mini_win.w - 4, 12);
    let title_bg = if state.dark_mode { 0x1E3A5F } else { color::NAVY };
    s.fill(mini_title, title_bg);
    s.text(mini_title.x + 4, mini_title.y + 2, "Display Demo", color::WHITE);

    let mini_client = Rect::new(mini_win.x + 6, mini_win.y + 18, mini_win.w - 12, mini_win.h - 24);
    s.fill(mini_client, if state.dark_mode { 0x1E1E1E } else { color::WHITE });
    s.bevel(mini_client, Bevel::Sunken);

    // 2. Color swatches
    s.text(30, 145, "Desktop Background Color:", color::BLACK);
    for (i, (name, col)) in PRESET_COLORS.iter().enumerate() {
        let r = swatch_rect(i);
        let selected = state.selected_color == *col;
        button(s, r, "", selected);
        // Color box indicator inside button
        let box_r = Rect::new(r.x + 4, r.y + 4, 16, 16);
        s.fill(box_r, *col);
        s.bevel(box_r, Bevel::Sunken);
        s.text(r.x + 26, r.y + (r.h - GLYPH_H) / 2, name, color::BLACK);
        if selected {
            // Draw focus dotted box
            let f = r.inset(2);
            s.hline(f.x, f.y, f.w, color::BLACK);
            s.hline(f.x, f.bottom() - 1, f.w, color::BLACK);
        }
    }

    // 3. Dark mode checkbox
    s.text(30, 225, "Theme Options:", color::BLACK);
    let box_rect = Rect::new(DARK_CHECK.x, DARK_CHECK.y + 3, 14, 14);
    s.fill(box_rect, color::WHITE);
    s.bevel(box_rect, Bevel::Sunken);
    if state.dark_mode {
        s.text_bold(box_rect.x + 3, box_rect.y + 3, "x", color::BLACK);
    }
    s.text(DARK_CHECK.x + 20, DARK_CHECK.y + (DARK_CHECK.h - GLYPH_H) / 2, "Dark Mode UI (Charcoal)", color::BLACK);

    // 4. Buttons
    button(s, OK_BTN, "OK", false);
    button(s, CANCEL_BTN, "Cancel", false);
    button(s, APPLY_BTN, "Apply", false);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_testing_identifies_buttons() {
        assert_eq!(hit(OK_BTN.x + 5, OK_BTN.y + 5), Hit::Ok);
        assert_eq!(hit(CANCEL_BTN.x + 5, CANCEL_BTN.y + 5), Hit::Cancel);
        assert_eq!(hit(APPLY_BTN.x + 5, APPLY_BTN.y + 5), Hit::Apply);
        assert_eq!(hit(DARK_CHECK.x + 5, DARK_CHECK.y + 5), Hit::ToggleDark);
        assert_eq!(hit(swatch_rect(0).x + 5, swatch_rect(0).y + 5), Hit::Color(0x008080));
        assert_eq!(hit(swatch_rect(1).x + 5, swatch_rect(1).y + 5), Hit::Color(0x182026));
        assert_eq!(hit(0, 0), Hit::Nothing);
    }

    #[test]
    fn draws_display_dialog() {
        let mut s = Surface::new(W as usize, H as usize);
        let state = State::default();
        draw(&mut s, W, H, &state);
        let pixels = s.pixels();
        assert!(pixels.contains(&0x008080));
        assert!(pixels.contains(&color::FACE));
        assert!(pixels.contains(&color::NAVY));
    }
}
