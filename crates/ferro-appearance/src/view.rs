//! What the Appearance / Display Properties dialog looks like and where its parts are.
//!
//! A classic Windows 95 Display Properties layout:
//! - Tabbed interface: [Background], [Screen Saver] and [Graphics].
//! - Monitor preview showing the current background wallpaper/color or screensaver.
//! - Wallpaper selector with next/previous buttons and logon screen option.
//! - Screensaver selector with Flying Stars, Matrix Rain, Blank, or Custom scripts.
//! - Timeout adjustment and password protection toggle.
//! - Color palette presets (Teal, Dark Slate, Obsidian, Navy, Plum, Forest).
//! - Dark UI Mode toggle.
//! - Graphics: which graphics card draws the desktop, programs, and named
//!   programs (Wine on the second card, say), through ferro-gpu.
//! - OK, Cancel, Apply buttons.

use ferro_gfx::{color, Bevel, DecodedImage, Rect, Surface, GLYPH_H, GLYPH_W};
use ferro_ui::{button, default_button};

pub const W: i32 = 380;
pub const H: i32 = 410;

pub const PRESET_COLORS: [(&str, u32); 6] =
    [("Teal", 0x008080), ("Slate", 0x182026), ("Black", 0x000000), ("Navy", 0x000080), ("Plum", 0x550055), ("Forest", 0x005522)];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WallpaperItem {
    pub name: String,
    pub path: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreensaverItem {
    pub name: String,
    pub mode: String, // "stars", "matrix", "blank", "custom"
    pub exec: Option<String>,
}

/// A graphics card, as `ferro-gpu list` numbers them: gpu0 has the monitor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GpuItem {
    pub pci: String,
    pub driver: String,
    pub monitor: bool,
    pub name: String,
}

/// A program given its own card: its name (or a pattern, wine*) and the card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GpuRule {
    pub program: String,
    pub gpu: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Tab(usize),
    Color(u32),
    PrevWallpaper,
    NextWallpaper,
    BrowseWallpaper,
    BrowseLogonWallpaper,
    ToggleLogonWallpaper,
    ToggleDark,
    ToggleCompositor,
    ToggleSounds,
    PrevScreensaver,
    NextScreensaver,
    BrowseScreensaver,
    DecTimeout,
    IncTimeout,
    ToggleSsLock,
    PreviewScreensaver,
    OpenScreensaversFolder,
    PrevDesktopGpu,
    NextDesktopGpu,
    PrevAppsGpu,
    NextAppsGpu,
    SelectRule(usize),
    AddRule,
    RemoveRule,
    PrevRuleGpu,
    NextRuleGpu,
    Ok,
    Cancel,
    Apply,
    Nothing,
}

pub struct State {
    pub tab: usize, // 0 = Background, 1 = Screen Saver, 2 = Graphics
    pub selected_color: u32,
    pub dark_mode: bool,
    /// Smooth drawing (picom, the compositor): VSync, no tearing, smooth
    /// dragging; off saves its memory.
    pub compositor: bool,
    /// System sounds (startup, errors, questions).
    pub sounds: bool,
    pub applied: bool,
    pub wallpapers: Vec<WallpaperItem>,
    pub selected_wallpaper: usize,
    pub logon_wallpaper: bool,
    pub cached_preview: Option<DecodedImage>,

    pub screensavers: Vec<ScreensaverItem>,
    pub selected_screensaver: usize,
    pub screensaver_timeout: u32, // minutes (1..=60)
    pub screensaver_lock: bool,
    /// Which of `controls` has the keyboard, if any (Tab moves it).
    pub focus: Option<usize>,

    pub gpus: Vec<GpuItem>,
    /// The desktop's card: None is automatic (the monitor's), else gpuN.
    pub gpu_desktop: Option<usize>,
    /// What the desktop's card was when the dialog opened (changing it asks
    /// ferro-gpu, as root, only when it was changed).
    pub gpu_desktop_saved: Option<usize>,
    /// Every program's card, unless a rule names it.
    pub gpu_apps: usize,
    pub gpu_rules: Vec<GpuRule>,
    pub gpu_rule_selected: Option<usize>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            tab: 0,
            selected_color: 0x008080,
            dark_mode: false,
            compositor: false,
            sounds: true,
            applied: true,
            wallpapers: vec![WallpaperItem { name: "(None)".into(), path: None }],
            selected_wallpaper: 0,
            logon_wallpaper: false,
            cached_preview: None,

            screensavers: vec![
                ScreensaverItem { name: "(None)".into(), mode: "none".into(), exec: None },
                ScreensaverItem { name: "Flying Stars".into(), mode: "stars".into(), exec: None },
                ScreensaverItem { name: "Phosphor Matrix".into(), mode: "matrix".into(), exec: None },
                ScreensaverItem { name: "Blank Screen".into(), mode: "blank".into(), exec: None },
            ],
            selected_screensaver: 1, // default to Flying Stars
            screensaver_timeout: 5,
            screensaver_lock: false,
            focus: None,

            gpus: Vec::new(),
            gpu_desktop: None,
            gpu_desktop_saved: None,
            gpu_apps: 0,
            gpu_rules: Vec::new(),
            gpu_rule_selected: None,
        }
    }
}

impl State {
    pub fn reload_preview(&mut self) {
        self.cached_preview = self.wallpapers.get(self.selected_wallpaper).and_then(|w| w.path.as_deref()).and_then(DecodedImage::load_from_file);
    }
}

// Geometry constants
pub const TAB_BG_RECT: Rect = Rect::new(20, 6, 95, 20);
pub const TAB_SS_RECT: Rect = Rect::new(118, 6, 100, 20);
pub const TAB_GPU_RECT: Rect = Rect::new(226, 6, 80, 20);

const MONITOR_OUTER: Rect = Rect::new(90, 32, 200, 115);
const MONITOR_SCREEN: Rect = Rect::new(104, 42, 172, 95);

// Background Tab Geometry
const WP_BOX: Rect = Rect::new(30, 174, 175, 22);
const WP_PREV_BTN: Rect = Rect::new(210, 174, 25, 22);
const WP_NEXT_BTN: Rect = Rect::new(238, 174, 25, 22);
const WP_BROWSE_BTN: Rect = Rect::new(268, 174, 82, 22);
const LOGON_CHECK: Rect = Rect::new(30, 202, 235, 20);
const LOGON_BROWSE_BTN: Rect = Rect::new(268, 202, 82, 22);

const COLOR_ROW1_Y: i32 = 240;
const COLOR_ROW2_Y: i32 = 266;
const SWATCH_W: i32 = 100;
const SWATCH_H: i32 = 22;

const DARK_CHECK: Rect = Rect::new(30, 312, 200, 20);
const COMPOSITOR_CHECK: Rect = Rect::new(30, 334, 320, 20);
const SOUNDS_CHECK: Rect = Rect::new(240, 312, 125, 20);

// Screensaver Tab Geometry
const SS_BOX: Rect = Rect::new(30, 174, 175, 22);
const SS_PREV_BTN: Rect = Rect::new(210, 174, 25, 22);
const SS_NEXT_BTN: Rect = Rect::new(238, 174, 25, 22);
const SS_BROWSE_BTN: Rect = Rect::new(268, 174, 82, 22);

const SS_TIMEOUT_DEC: Rect = Rect::new(110, 215, 24, 22);
const SS_TIMEOUT_INC: Rect = Rect::new(174, 215, 24, 22);
const SS_LOCK_CHECK: Rect = Rect::new(30, 252, 260, 20);

const SS_PREVIEW_BTN: Rect = Rect::new(30, 288, 90, 24);
const SS_FOLDER_BTN: Rect = Rect::new(130, 288, 160, 24);

// Graphics Tab Geometry
const GPU_LIST: Rect = Rect::new(30, 48, 320, 50);
const GPU_DESKTOP_BOX: Rect = Rect::new(110, 108, 180, 22);
const GPU_DESKTOP_PREV: Rect = Rect::new(295, 108, 25, 22);
const GPU_DESKTOP_NEXT: Rect = Rect::new(323, 108, 25, 22);
const GPU_APPS_BOX: Rect = Rect::new(110, 136, 180, 22);
const GPU_APPS_PREV: Rect = Rect::new(295, 136, 25, 22);
const GPU_APPS_NEXT: Rect = Rect::new(323, 136, 25, 22);
const GPU_RULES: Rect = Rect::new(30, 184, 320, 86);
const GPU_RULE_ROW_H: i32 = 14;
const GPU_ADD_BTN: Rect = Rect::new(30, 276, 75, 22);
const GPU_REMOVE_BTN: Rect = Rect::new(110, 276, 75, 22);
const GPU_RULE_PREV: Rect = Rect::new(295, 276, 25, 22);
const GPU_RULE_NEXT: Rect = Rect::new(323, 276, 25, 22);

// Bottom buttons
const OK_BTN: Rect = Rect::new(110, 370, 75, 24);
const CANCEL_BTN: Rect = Rect::new(195, 370, 75, 24);
const APPLY_BTN: Rect = Rect::new(280, 370, 75, 24);

/// What can be clicked on the tab shown, in Tab order.
pub fn controls(state: &State) -> Vec<Rect> {
    let mut c = vec![TAB_BG_RECT, TAB_SS_RECT, TAB_GPU_RECT];
    if state.tab == 2 {
        if state.gpus.len() > 1 {
            c.extend([GPU_DESKTOP_PREV, GPU_DESKTOP_NEXT, GPU_APPS_PREV, GPU_APPS_NEXT]);
            c.extend((0..state.gpu_rules.len().min(rule_rows())).map(rule_rect));
            c.extend([GPU_ADD_BTN, GPU_REMOVE_BTN, GPU_RULE_PREV, GPU_RULE_NEXT]);
        }
    } else if state.tab == 0 {
        c.extend([WP_PREV_BTN, WP_NEXT_BTN, WP_BROWSE_BTN, LOGON_CHECK, LOGON_BROWSE_BTN]);
        c.extend((0..PRESET_COLORS.len()).map(swatch_rect));
        c.extend([DARK_CHECK, SOUNDS_CHECK, COMPOSITOR_CHECK]);
    } else {
        c.extend([SS_PREV_BTN, SS_NEXT_BTN, SS_BROWSE_BTN, SS_TIMEOUT_DEC, SS_TIMEOUT_INC, SS_LOCK_CHECK, SS_PREVIEW_BTN, SS_FOLDER_BTN]);
    }
    c.extend([OK_BTN, CANCEL_BTN, APPLY_BTN]);
    c
}

fn rule_rows() -> usize {
    ((GPU_RULES.h - 4) / GPU_RULE_ROW_H) as usize
}

fn rule_rect(idx: usize) -> Rect {
    Rect::new(GPU_RULES.x + 2, GPU_RULES.y + 2 + idx as i32 * GPU_RULE_ROW_H, GPU_RULES.w - 4, GPU_RULE_ROW_H)
}

/// A card as the dialog names it: gpu0 (the monitor's), gpu1...
pub fn gpu_label(i: usize) -> String {
    if i == 0 {
        "gpu0 (monitor)".to_string()
    } else {
        format!("gpu{i}")
    }
}

/// `text` cut to fit `w` pixels, ending in "..." when it was cut.
fn fit(text: &str, w: i32) -> String {
    let max = (w / GLYPH_W).max(0) as usize;
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let mut t: String = text.chars().take(max.saturating_sub(3)).collect();
        t.push_str("...");
        t
    }
}

fn swatch_rect(idx: usize) -> Rect {
    let col = idx % 3;
    let row = idx / 3;
    let x = 30 + col as i32 * (SWATCH_W + 15);
    let y = if row == 0 { COLOR_ROW1_Y } else { COLOR_ROW2_Y };
    Rect::new(x, y, SWATCH_W, SWATCH_H)
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
    if TAB_BG_RECT.contains(x, y) {
        return Hit::Tab(0);
    }
    if TAB_SS_RECT.contains(x, y) {
        return Hit::Tab(1);
    }
    if TAB_GPU_RECT.contains(x, y) {
        return Hit::Tab(2);
    }

    if state.tab == 2 {
        if state.gpus.len() < 2 {
            return Hit::Nothing;
        }
        let hits = [
            (GPU_DESKTOP_PREV, Hit::PrevDesktopGpu),
            (GPU_DESKTOP_NEXT, Hit::NextDesktopGpu),
            (GPU_APPS_PREV, Hit::PrevAppsGpu),
            (GPU_APPS_NEXT, Hit::NextAppsGpu),
            (GPU_ADD_BTN, Hit::AddRule),
            (GPU_REMOVE_BTN, Hit::RemoveRule),
            (GPU_RULE_PREV, Hit::PrevRuleGpu),
            (GPU_RULE_NEXT, Hit::NextRuleGpu),
        ];
        if let Some((_, h)) = hits.iter().find(|(r, _)| r.contains(x, y)) {
            return *h;
        }
        if let Some(i) = (0..state.gpu_rules.len().min(rule_rows())).find(|&i| rule_rect(i).contains(x, y)) {
            return Hit::SelectRule(i);
        }
        return Hit::Nothing;
    }

    if state.tab == 0 {
        if WP_PREV_BTN.contains(x, y) {
            return Hit::PrevWallpaper;
        }
        if WP_NEXT_BTN.contains(x, y) {
            return Hit::NextWallpaper;
        }
        if WP_BROWSE_BTN.contains(x, y) {
            return Hit::BrowseWallpaper;
        }
        if LOGON_BROWSE_BTN.contains(x, y) {
            return Hit::BrowseLogonWallpaper;
        }
        if LOGON_CHECK.contains(x, y) {
            return Hit::ToggleLogonWallpaper;
        }
        if DARK_CHECK.contains(x, y) {
            return Hit::ToggleDark;
        }
        if COMPOSITOR_CHECK.contains(x, y) {
            return Hit::ToggleCompositor;
        }
        if SOUNDS_CHECK.contains(x, y) {
            return Hit::ToggleSounds;
        }
        for (i, (_, c)) in PRESET_COLORS.iter().enumerate() {
            if swatch_rect(i).contains(x, y) {
                return Hit::Color(*c);
            }
        }
    } else {
        if SS_PREV_BTN.contains(x, y) {
            return Hit::PrevScreensaver;
        }
        if SS_NEXT_BTN.contains(x, y) {
            return Hit::NextScreensaver;
        }
        if SS_BROWSE_BTN.contains(x, y) {
            return Hit::BrowseScreensaver;
        }
        if SS_TIMEOUT_DEC.contains(x, y) {
            return Hit::DecTimeout;
        }
        if SS_TIMEOUT_INC.contains(x, y) {
            return Hit::IncTimeout;
        }
        if SS_LOCK_CHECK.contains(x, y) {
            return Hit::ToggleSsLock;
        }
        if SS_PREVIEW_BTN.contains(x, y) {
            return Hit::PreviewScreensaver;
        }
        if SS_FOLDER_BTN.contains(x, y) {
            return Hit::OpenScreensaversFolder;
        }
    }

    Hit::Nothing
}

pub fn draw(s: &mut Surface, w: i32, h: i32, state: &State) {
    s.fill(Rect::new(0, 0, w, h), color::FACE);

    // Tab Bar
    s.fill(TAB_BG_RECT, color::FACE);
    s.bevel(TAB_BG_RECT, if state.tab == 0 { Bevel::Raised } else { Bevel::Sunken });
    s.text(TAB_BG_RECT.x + 10, TAB_BG_RECT.y + 4, "Background", color::BLACK);

    s.fill(TAB_SS_RECT, color::FACE);
    s.bevel(TAB_SS_RECT, if state.tab == 1 { Bevel::Raised } else { Bevel::Sunken });
    s.text(TAB_SS_RECT.x + 8, TAB_SS_RECT.y + 4, "Screen Saver", color::BLACK);

    s.fill(TAB_GPU_RECT, color::FACE);
    s.bevel(TAB_GPU_RECT, if state.tab == 2 { Bevel::Raised } else { Bevel::Sunken });
    s.text(TAB_GPU_RECT.x + 8, TAB_GPU_RECT.y + 4, "Graphics", color::BLACK);

    if state.tab == 2 {
        draw_graphics(s, state);
        draw_bottom(s, state);
        return;
    }

    // Mini monitor preview (shared)
    s.fill(MONITOR_OUTER, color::SHADOW);
    s.bevel(MONITOR_OUTER, Bevel::Raised);

    if state.tab == 0 {
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

        // Wallpaper selector
        s.text(30, 158, "Wallpaper / Background Image:", color::BLACK);
        s.fill(WP_BOX, color::WHITE);
        s.bevel(WP_BOX, Bevel::Sunken);
        let wp_name = state.wallpapers.get(state.selected_wallpaper).map_or("(None)", |w| &w.name);
        s.text(WP_BOX.x + 6, WP_BOX.y + (WP_BOX.h - GLYPH_H) / 2, wp_name, color::BLACK);
        button(s, WP_PREV_BTN, "<", true);
        button(s, WP_NEXT_BTN, ">", true);
        button(s, WP_BROWSE_BTN, "Browse...", true);

        // Logon wallpaper checkbox
        let logon_box = Rect::new(LOGON_CHECK.x, LOGON_CHECK.y + 3, 14, 14);
        s.fill(logon_box, color::WHITE);
        s.bevel(logon_box, Bevel::Sunken);
        if state.logon_wallpaper {
            s.text_bold(logon_box.x + 3, logon_box.y + 3, "x", color::BLACK);
        }
        s.text(LOGON_CHECK.x + 20, LOGON_CHECK.y + (LOGON_CHECK.h - GLYPH_H) / 2, "Use for the Logon screen", color::BLACK);
        button(s, LOGON_BROWSE_BTN, "Logon...", true);

        // Color swatches
        s.text(30, 226, "Desktop Background Color:", color::BLACK);
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

        // Dark mode checkbox
        s.text(30, 298, "Theme Options:", color::BLACK);
        let box_rect = Rect::new(DARK_CHECK.x, DARK_CHECK.y + 3, 14, 14);
        s.fill(box_rect, color::WHITE);
        s.bevel(box_rect, Bevel::Sunken);
        if state.dark_mode {
            s.text_bold(box_rect.x + 3, box_rect.y + 3, "x", color::BLACK);
        }
        s.text(DARK_CHECK.x + 20, DARK_CHECK.y + (DARK_CHECK.h - GLYPH_H) / 2, "Dark Mode UI (Charcoal)", color::BLACK);
        let box_rect = Rect::new(COMPOSITOR_CHECK.x, COMPOSITOR_CHECK.y + 3, 14, 14);
        s.fill(box_rect, color::WHITE);
        s.bevel(box_rect, Bevel::Sunken);
        if state.compositor {
            s.text_bold(box_rect.x + 3, box_rect.y + 3, "x", color::BLACK);
        }
        s.text(COMPOSITOR_CHECK.x + 20, COMPOSITOR_CHECK.y + (COMPOSITOR_CHECK.h - GLYPH_H) / 2, "Smooth drawing (about 60 MB)", color::BLACK);
        let box_rect = Rect::new(SOUNDS_CHECK.x, SOUNDS_CHECK.y + 3, 14, 14);
        s.fill(box_rect, color::WHITE);
        s.bevel(box_rect, Bevel::Sunken);
        if state.sounds {
            s.text_bold(box_rect.x + 3, box_rect.y + 3, "x", color::BLACK);
        }
        s.text(SOUNDS_CHECK.x + 20, SOUNDS_CHECK.y + (SOUNDS_CHECK.h - GLYPH_H) / 2, "System sounds", color::BLACK);
    } else {
        // Screensaver tab monitor preview
        s.fill(MONITOR_SCREEN, color::BLACK);
        s.bevel(MONITOR_SCREEN, Bevel::Sunken);

        let ss_item = state.screensavers.get(state.selected_screensaver);
        let ss_name = ss_item.map_or("(None)", |s| &s.name);

        if ss_item.is_some_and(|s| s.mode == "stars") {
            // Little star dots in monitor preview
            s.text(MONITOR_SCREEN.x + 30, MONITOR_SCREEN.y + 20, "*", color::WHITE);
            s.text(MONITOR_SCREEN.x + 110, MONITOR_SCREEN.y + 35, "*", color::LIGHT);
            s.text(MONITOR_SCREEN.x + 70, MONITOR_SCREEN.y + 60, "*", color::WHITE);
            s.text(MONITOR_SCREEN.x + 140, MONITOR_SCREEN.y + 70, ".", color::LIGHT);
        } else if ss_item.is_some_and(|s| s.mode == "matrix") {
            s.text(MONITOR_SCREEN.x + 40, MONITOR_SCREEN.y + 25, "1 0 1", 0x00FF33);
            s.text(MONITOR_SCREEN.x + 70, MONITOR_SCREEN.y + 45, "0 1 0", 0x00AA22);
            s.text(MONITOR_SCREEN.x + 100, MONITOR_SCREEN.y + 65, "1 1 0", 0x005511);
        } else {
            let cx = MONITOR_SCREEN.x + (MONITOR_SCREEN.w - (ss_name.len() as i32 * GLYPH_W)) / 2;
            s.text(cx, MONITOR_SCREEN.y + 40, ss_name, color::SHADOW);
        }

        // Screensaver selector
        s.text(30, 158, "Screen Saver:", color::BLACK);
        s.fill(SS_BOX, color::WHITE);
        s.bevel(SS_BOX, Bevel::Sunken);
        s.text(SS_BOX.x + 6, SS_BOX.y + (SS_BOX.h - GLYPH_H) / 2, ss_name, color::BLACK);
        button(s, SS_PREV_BTN, "<", true);
        button(s, SS_NEXT_BTN, ">", true);
        button(s, SS_BROWSE_BTN, "Browse...", true);

        // Timeout adjustment
        s.text(30, 220, "Wait:", color::BLACK);
        button(s, SS_TIMEOUT_DEC, "<", true);

        let min_str = format!("{} min", state.screensaver_timeout);
        let mx = 138 + (32 - min_str.len() as i32 * GLYPH_W) / 2;
        s.text(mx, 220, &min_str, color::NAVY);

        button(s, SS_TIMEOUT_INC, ">", true);

        // Password protection checkbox
        let lock_box = Rect::new(SS_LOCK_CHECK.x, SS_LOCK_CHECK.y + 3, 14, 14);
        s.fill(lock_box, color::WHITE);
        s.bevel(lock_box, Bevel::Sunken);
        if state.screensaver_lock {
            s.text_bold(lock_box.x + 3, lock_box.y + 3, "x", color::BLACK);
        }
        s.text(SS_LOCK_CHECK.x + 20, SS_LOCK_CHECK.y + (SS_LOCK_CHECK.h - GLYPH_H) / 2, "Password protect on resume", color::BLACK);

        // Action buttons
        button(s, SS_PREVIEW_BTN, "Preview", true);
        button(s, SS_FOLDER_BTN, "Custom Screensavers...", true);
    }

    draw_bottom(s, state);
}

fn draw_graphics(s: &mut Surface, state: &State) {
    s.text(30, 34, "Graphics cards:", color::BLACK);
    s.fill(GPU_LIST, color::WHITE);
    s.bevel(GPU_LIST, Bevel::Sunken);
    for (i, g) in state.gpus.iter().take(3).enumerate() {
        let line = format!("gpu{i}  {}  ({}{})", g.name, g.driver, if g.monitor { ", monitor" } else { "" });
        s.text(GPU_LIST.x + 6, GPU_LIST.y + 6 + i as i32 * 14, &fit(&line, GPU_LIST.w - 12), color::BLACK);
    }
    if state.gpus.len() < 2 {
        let msg = if state.gpus.is_empty() { "No graphics cards were found." } else { "This computer has one graphics card:" };
        s.text(30, 112, msg, color::BLACK);
        s.text(30, 128, "everything is drawn on it.", color::BLACK);
        return;
    }

    let row = |s: &mut Surface, label: &str, value: &str, bx: Rect, prev: Rect, next: Rect| {
        s.text(30, bx.y + (bx.h - GLYPH_H) / 2, label, color::BLACK);
        s.fill(bx, color::WHITE);
        s.bevel(bx, Bevel::Sunken);
        s.text(bx.x + 6, bx.y + (bx.h - GLYPH_H) / 2, value, color::BLACK);
        button(s, prev, "<", true);
        button(s, next, ">", true);
    };
    let desktop = state.gpu_desktop.map_or_else(|| "Automatic (gpu0)".to_string(), gpu_label);
    row(s, "Desktop:", &desktop, GPU_DESKTOP_BOX, GPU_DESKTOP_PREV, GPU_DESKTOP_NEXT);
    row(s, "Programs:", &gpu_label(state.gpu_apps), GPU_APPS_BOX, GPU_APPS_PREV, GPU_APPS_NEXT);

    s.text(30, 170, "Programs on a card of their own:", color::BLACK);
    s.fill(GPU_RULES, color::WHITE);
    s.bevel(GPU_RULES, Bevel::Sunken);
    for (i, r) in state.gpu_rules.iter().take(rule_rows()).enumerate() {
        let rr = rule_rect(i);
        let selected = state.gpu_rule_selected == Some(i);
        let fg = if selected { color::WHITE } else { color::BLACK };
        if selected {
            s.fill(rr, color::NAVY);
        }
        s.text(rr.x + 4, rr.y + (rr.h - GLYPH_H) / 2, &fit(&r.program, 200), fg);
        s.text(rr.x + 210, rr.y + (rr.h - GLYPH_H) / 2, &format!("gpu{}", r.gpu), fg);
    }
    if state.gpu_rules.is_empty() {
        s.text(GPU_RULES.x + 6, GPU_RULES.y + 8, "(none: Add... wine, steam...)", color::SHADOW);
    }
    button(s, GPU_ADD_BTN, "Add...", true);
    button(s, GPU_REMOVE_BTN, "Remove", state.gpu_rule_selected.is_some());
    s.text(205, GPU_RULE_PREV.y + (GPU_RULE_PREV.h - GLYPH_H) / 2, "Card:", color::BLACK);
    button(s, GPU_RULE_PREV, "<", state.gpu_rule_selected.is_some());
    button(s, GPU_RULE_NEXT, ">", state.gpu_rule_selected.is_some());

    let note =
        if state.gpu_desktop != state.gpu_desktop_saved { "The desktop moves at the next log on." } else { "Programs switch at their next start." };
    s.text(30, 314, note, color::NAVY);
    // nouveau can't draw 3D on newer NVIDIA cards without their firmware:
    // Mesa then draws on the processor, slowly, and says nothing.
    if let Some(i) = state.gpus.iter().skip(1).position(|g| g.driver == "nouveau") {
        s.text(30, 332, &format!("gpu{} has nouveau: 3D may fall back", i + 1), color::RED);
        s.text(30, 346, "to the CPU. NVIDIA's own driver won't.", color::RED);
    }
}

fn draw_bottom(s: &mut Surface, state: &State) {
    default_button(s, OK_BTN, "OK", true);
    button(s, CANCEL_BTN, "Cancel", true);
    button(s, APPLY_BTN, "Apply", true);

    // OK, the default button, has its dotted outline already.
    if let Some(r) = state.focus.and_then(|i| controls(state).get(i).copied()).filter(|&r| r != OK_BTN) {
        ferro_ui::focus_rect(s, r.inset(2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_testing_tabs_and_buttons() {
        let state = State::default();
        assert_eq!(hit(OK_BTN.x + 2, OK_BTN.y + 2, &state), Hit::Ok);
        assert_eq!(hit(CANCEL_BTN.x + 2, CANCEL_BTN.y + 2, &state), Hit::Cancel);
        assert_eq!(hit(APPLY_BTN.x + 2, APPLY_BTN.y + 2, &state), Hit::Apply);
        assert_eq!(hit(TAB_BG_RECT.x + 2, TAB_BG_RECT.y + 2, &state), Hit::Tab(0));
        assert_eq!(hit(TAB_SS_RECT.x + 2, TAB_SS_RECT.y + 2, &state), Hit::Tab(1));
        assert_eq!(hit(TAB_GPU_RECT.x + 2, TAB_GPU_RECT.y + 2, &state), Hit::Tab(2));
    }

    fn two_cards() -> State {
        let card = |pci: &str, monitor| GpuItem { pci: pci.into(), driver: "amdgpu".into(), monitor, name: "Card".into() };
        State {
            tab: 2,
            gpus: vec![card("0000:0a:00.0", true), card("0000:03:00.0", false)],
            gpu_rules: vec![GpuRule { program: "wine*".into(), gpu: 1 }],
            ..State::default()
        }
    }

    #[test]
    fn graphics_tab_hits() {
        let state = two_cards();
        assert_eq!(hit(GPU_DESKTOP_NEXT.x + 2, GPU_DESKTOP_NEXT.y + 2, &state), Hit::NextDesktopGpu);
        assert_eq!(hit(GPU_APPS_PREV.x + 2, GPU_APPS_PREV.y + 2, &state), Hit::PrevAppsGpu);
        assert_eq!(hit(rule_rect(0).x + 2, rule_rect(0).y + 2, &state), Hit::SelectRule(0));
        // Below the one rule there's nothing to pick.
        assert_eq!(hit(rule_rect(1).x + 2, rule_rect(1).y + 2, &state), Hit::Nothing);
        assert_eq!(hit(GPU_ADD_BTN.x + 2, GPU_ADD_BTN.y + 2, &state), Hit::AddRule);
        // With one card there's nothing to choose.
        let one = State { gpus: vec![state.gpus[0].clone()], ..two_cards() };
        assert_eq!(hit(GPU_ADD_BTN.x + 2, GPU_ADD_BTN.y + 2, &one), Hit::Nothing);
    }

    #[test]
    fn graphics_tab_draws() {
        let mut s = Surface::new(W as usize, H as usize);
        draw(&mut s, W, H, &two_cards());
        assert_eq!(controls(&two_cards()).len(), 3 + 4 + 1 + 4 + 3);
    }

    #[test]
    fn long_names_fit() {
        assert_eq!(fit("short", 80), "short");
        assert_eq!(fit("NVIDIA Corporation TU116", 80), "NVIDIA ...");
    }
}
