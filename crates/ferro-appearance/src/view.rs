//! What the Appearance / Display Properties dialog looks like and where its parts are.
//!
//! A classic Windows 95 Display Properties layout:
//! - Monitor preview showing the current background wallpaper/color and a mini active window.
//! - Wallpaper selector with next/previous buttons and logon screen option.
//! - Color palette presets (Teal, Dark Slate, Obsidian, Navy, Plum, Forest).
//! - Dark UI Mode toggle.
//! - OK, Cancel, Apply buttons.

use ferro_gfx::{color, Bevel, DecodedImage, Rect, Surface, GLYPH_H};
use ferro_ui::button;

pub const W: i32 = 380;
pub const H: i32 = 380;

pub const PRESET_COLORS: [(&str, u32); 6] =
    [("Teal", 0x008080), ("Slate", 0x182026), ("Black", 0x000000), ("Navy", 0x000080), ("Plum", 0x550055), ("Forest", 0x005522)];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WallpaperItem {
    pub name: String,
    pub path: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Color(u32),
    PrevWallpaper,
    NextWallpaper,
    ToggleLogonWallpaper,
    ToggleDark,
    Ok,
    Cancel,
    Apply,
    Nothing,
}

pub struct State {
    pub selected_color: u32,
    pub dark_mode: bool,
    pub applied: bool,
    pub wallpapers: Vec<WallpaperItem>,
    pub selected_wallpaper: usize,
    pub logon_wallpaper: bool,
    pub cached_preview: Option<DecodedImage>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            selected_color: 0x008080,
            dark_mode: false,
            applied: true,
            wallpapers: vec![WallpaperItem { name: "(None)".into(), path: None }],
            selected_wallpaper: 0,
            logon_wallpaper: false,
            cached_preview: None,
        }
    }
}

impl State {
    pub fn reload_preview(&mut self) {
        self.cached_preview = self.wallpapers.get(self.selected_wallpaper).and_then(|w| w.path.as_deref()).and_then(DecodedImage::load_from_file);
    }
}

// Geometry constants
const MONITOR_OUTER: Rect = Rect::new(90, 16, 200, 115);
const MONITOR_SCREEN: Rect = Rect::new(104, 26, 172, 95);

const WP_BOX: Rect = Rect::new(30, 158, 240, 22);
const WP_PREV_BTN: Rect = Rect::new(276, 158, 30, 22);
const WP_NEXT_BTN: Rect = Rect::new(310, 158, 30, 22);
const LOGON_CHECK: Rect = Rect::new(30, 186, 240, 20);

const COLOR_ROW1_Y: i32 = 224;
const COLOR_ROW2_Y: i32 = 250;
const SWATCH_W: i32 = 100;
const SWATCH_H: i32 = 22;

const DARK_CHECK: Rect = Rect::new(30, 296, 200, 20);

const OK_BTN: Rect = Rect::new(110, 340, 75, 24);
const CANCEL_BTN: Rect = Rect::new(195, 340, 75, 24);
const APPLY_BTN: Rect = Rect::new(280, 340, 75, 24);

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
    if WP_PREV_BTN.contains(x, y) {
        return Hit::PrevWallpaper;
    }
    if WP_NEXT_BTN.contains(x, y) {
        return Hit::NextWallpaper;
    }
    if LOGON_CHECK.contains(x, y) {
        return Hit::ToggleLogonWallpaper;
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

    if let Some(img) = &state.cached_preview {
        s.draw_image_scaled(MONITOR_SCREEN, img);
    } else {
        s.fill(MONITOR_SCREEN, state.selected_color);
    }
    s.bevel(MONITOR_SCREEN, Bevel::Sunken);

    // Mini active window on the monitor
    let mini_win = Rect::new(MONITOR_SCREEN.x + 20, MONITOR_SCREEN.y + 15, 120, 60);
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

    // 2. Wallpaper selector
    s.text(30, 142, "Wallpaper / Background Image:", color::BLACK);
    s.fill(WP_BOX, color::WHITE);
    s.bevel(WP_BOX, Bevel::Sunken);
    let wp_name = state.wallpapers.get(state.selected_wallpaper).map_or("(None)", |w| &w.name);
    s.text(WP_BOX.x + 6, WP_BOX.y + (WP_BOX.h - GLYPH_H) / 2, wp_name, color::BLACK);
    button(s, WP_PREV_BTN, "<", false);
    button(s, WP_NEXT_BTN, ">", false);

    // Logon wallpaper checkbox
    let logon_box = Rect::new(LOGON_CHECK.x, LOGON_CHECK.y + 3, 14, 14);
    s.fill(logon_box, color::WHITE);
    s.bevel(logon_box, Bevel::Sunken);
    if state.logon_wallpaper {
        s.text_bold(logon_box.x + 3, logon_box.y + 3, "x", color::BLACK);
    }
    s.text(LOGON_CHECK.x + 20, LOGON_CHECK.y + (LOGON_CHECK.h - GLYPH_H) / 2, "Use as Logon Screen Wallpaper", color::BLACK);

    // 3. Color swatches
    s.text(30, 210, "Desktop Background Color:", color::BLACK);
    for (i, (name, col)) in PRESET_COLORS.iter().enumerate() {
        let r = swatch_rect(i);
        let selected = state.selected_color == *col && state.cached_preview.is_none();
        button(s, r, "", selected);
        let box_r = Rect::new(r.x + 4, r.y + 3, 16, 16);
        s.fill(box_r, *col);
        s.bevel(box_r, Bevel::Sunken);
        s.text(r.x + 26, r.y + (r.h - GLYPH_H) / 2, name, color::BLACK);
        if selected {
            let f = r.inset(2);
            s.hline(f.x, f.y, f.w, color::BLACK);
            s.hline(f.x, f.bottom() - 1, f.w, color::BLACK);
        }
    }

    // 4. Dark mode checkbox
    s.text(30, 282, "Theme Options:", color::BLACK);
    let box_rect = Rect::new(DARK_CHECK.x, DARK_CHECK.y + 3, 14, 14);
    s.fill(box_rect, color::WHITE);
    s.bevel(box_rect, Bevel::Sunken);
    if state.dark_mode {
        s.text_bold(box_rect.x + 3, box_rect.y + 3, "x", color::BLACK);
    }
    s.text(DARK_CHECK.x + 20, DARK_CHECK.y + (DARK_CHECK.h - GLYPH_H) / 2, "Dark Mode UI (Charcoal)", color::BLACK);

    // 5. Buttons
    button(s, OK_BTN, "OK", false);
    button(s, CANCEL_BTN, "Cancel", false);
    button(s, APPLY_BTN, "Apply", false);
}
