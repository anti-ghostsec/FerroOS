//! The taskbar and the Start menu: where their parts are and what they
//! look like. Pure geometry and pixels (ferro-gfx), like `frame`.

use ferro_gfx::icons::{self, Icon};
use ferro_gfx::{color, ellipsize, text_width, Bevel, Rect, Surface, GLYPH_H, GLYPH_W};
use std::fs;
use std::path::PathBuf;

pub const HEIGHT: i32 = 40;
const START: Rect = Rect::new(4, 4, 62, HEIGHT - 8);
const TASKS_X: i32 = 70;
const TASK_W: i32 = 160;
pub const CLOCK_W: i32 = 215;
pub const TRAY_ICON_W: i32 = 18;

/// One running program's button.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task<'a> {
    pub title: &'a str,
    /// The window with the keyboard: its button looks pushed in.
    pub active: bool,
    /// When demanding attention / urgent, flashes between normal and orange.
    pub flashing: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinnedApp {
    pub name: String,
    pub exec: String,
    pub icon: Icon,
}

fn pinned_config_path() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".config").join("ferro").join("pinned.conf")
    } else {
        PathBuf::from("/etc/ferro/pinned.conf")
    }
}

/// The folder a pin opens, when it's a pinned folder (`ferro-files 'path'`).
pub fn pinned_folder(exec: &str) -> Option<PathBuf> {
    let rest = exec.strip_prefix("ferro-files ")?.trim();
    let path = rest.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')).map_or_else(|| rest.to_string(), |q| q.replace("'\\''", "'"));
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// A pin's icon, by what it starts. Folders first: a folder's path may
/// well contain "term" or "cmd".
pub fn pinned_icon(exec: &str) -> Icon {
    if exec.starts_with("ferro-files") {
        icons::FOLDER
    } else if exec.starts_with("xdg-open") {
        icons::FILE
    } else if exec.contains("cmd") || exec.contains("term") {
        icons::TERMINAL
    } else if exec.contains("network") {
        icons::NETWORK
    } else if exec.contains("taskmgr") {
        icons::TASKMGR
    } else {
        icons::COMPUTER
    }
}

pub fn default_pinned() -> Vec<PinnedApp> {
    vec![]
}

pub fn load_pinned() -> Vec<PinnedApp> {
    let path = pinned_config_path();
    if let Ok(content) = fs::read_to_string(&path) {
        let mut apps = Vec::new();
        for line in content.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((name, exec)) = line.split_once('=') {
                let name = name.trim().to_string();
                let exec = exec.trim().to_string();
                let icon = pinned_icon(&exec);
                apps.push(PinnedApp { name, exec, icon });
            }
        }
        return apps;
    }
    default_pinned()
}

pub fn save_pinned(apps: &[PinnedApp]) {
    let path = pinned_config_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut lines = vec!["# FerroOS Pinned Applications".to_string()];
    for app in apps {
        lines.push(format!("{} = {}", app.name, app.exec));
    }
    let _ = fs::write(&path, lines.join("\n") + "\n");
}

pub const PINNED_W: i32 = 28;

pub fn pinned_rect(i: usize) -> Rect {
    Rect::new(START.right() + 4 + i as i32 * (PINNED_W + 4), 4, PINNED_W, HEIGHT - 8)
}

pub fn tasks_x(pinned_count: usize) -> i32 {
    if pinned_count == 0 {
        TASKS_X
    } else {
        START.right() + 6 + pinned_count as i32 * (PINNED_W + 4)
    }
}

/// What a click on the taskbar lands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarPart {
    Start,
    Pinned(usize),
    Task(usize),
    TrayIcon(usize),
    TrayClock,
    TrayRam,
    Tray,
    Nothing,
}

pub fn clock_rect(w: i32) -> Rect {
    Rect::new(w - CLOCK_W - 4, 4, CLOCK_W, HEIGHT - 8)
}

pub fn tray_icon_rect(w: i32, i: usize) -> Rect {
    let c = clock_rect(w);
    let ix = c.x + 4 + i as i32 * TRAY_ICON_W;
    Rect::new(ix, 4, TRAY_ICON_W, HEIGHT - 8)
}

pub fn tray_clock_rect(w: i32) -> Rect {
    let c = clock_rect(w);
    Rect::new(c.x + 118, 4, c.w.saturating_sub(118), HEIGHT - 8)
}

pub fn tray_ram_rect(w: i32) -> Rect {
    let c = clock_rect(w);
    Rect::new(c.x + 4 + 2 * TRAY_ICON_W, 4, 118 - 4 - 2 * TRAY_ICON_W, HEIGHT - 8)
}

/// Each button's width when `count` of them share the taskbar.
fn task_width(w: i32, count: usize, pinned_count: usize) -> i32 {
    let tx = tasks_x(pinned_count);
    let room = clock_rect(w).x - 4 - tx;
    if count == 0 {
        return TASK_W;
    }
    (room / count as i32).clamp(1, TASK_W)
}

fn task_rect(w: i32, count: usize, i: usize, pinned_count: usize) -> Rect {
    let tx = tasks_x(pinned_count);
    let tw = task_width(w, count, pinned_count);
    Rect::new(tx + i as i32 * tw, 4, tw - 3, HEIGHT - 8)
}

pub fn hit(w: i32, count: usize, pinned_count: usize, x: i32, y: i32) -> BarPart {
    if START.contains(x, y) {
        return BarPart::Start;
    }
    for i in 0..pinned_count {
        if pinned_rect(i).contains(x, y) {
            return BarPart::Pinned(i);
        }
    }
    let c = clock_rect(w);
    if c.contains(x, y) {
        for i in 0..2 {
            let ix = c.x + 4 + i as i32 * TRAY_ICON_W;
            if x >= ix && x < ix + TRAY_ICON_W {
                return BarPart::TrayIcon(i);
            }
        }
        if x >= c.x + 118 {
            return BarPart::TrayClock;
        }
        if x >= c.x + 4 + 2 * TRAY_ICON_W {
            return BarPart::TrayRam;
        }
        return BarPart::Tray;
    }
    (0..count).find(|i| task_rect(w, count, *i, pinned_count).contains(x, y)).map_or(BarPart::Nothing, BarPart::Task)
}

/// Draws the taskbar, `w` wide.
#[allow(clippy::too_many_arguments)]
pub fn draw(
    s: &mut Surface,
    w: i32,
    tasks: &[Task],
    pinned: &[PinnedApp],
    start_open: bool,
    tray: &str,
    switches: Option<&ferro_sandbox::store::Switches>,
    in_use: (bool, bool),
    tooltip: Option<(&str, i32)>,
) {
    s.fill(Rect::new(0, 0, w, HEIGHT), color::FACE);
    s.hline(0, 0, w, color::FACE);
    s.hline(0, 1, w, color::WHITE);
    s.button(START, start_open);
    let nudge = i32::from(start_open);
    s.text_bold(START.x + (START.w - text_width("Start")) / 2 + nudge, START.y + (START.h - GLYPH_H) / 2 + nudge, "Start", color::BLACK);

    // Pinned shortcuts / Quick Launch
    for (i, p) in pinned.iter().enumerate() {
        let r = pinned_rect(i);
        s.button(r, false);
        s.sprite(r.x + (r.w - 16) / 2, r.y + (r.h - 16) / 2, p.icon, 1);
    }

    for (i, t) in tasks.iter().enumerate() {
        let r = task_rect(w, tasks.len(), i, pinned.len());
        if t.flashing && !t.active {
            s.button(r, true);
            s.fill(r.inset(2), 0x0080FF); // Windows 95 orange/amber flash
            let room = ((r.w - 10) / GLYPH_W).max(0) as usize;
            let text = ellipsize(t.title, room);
            s.text_bold(r.x + 6, r.y + (r.h - GLYPH_H) / 2 + 1, &text, color::WHITE);
        } else {
            s.button(r, t.active);
            let room = ((r.w - 10) / GLYPH_W).max(0) as usize;
            let text = ellipsize(t.title, room);
            let nudge = i32::from(t.active);
            if t.active {
                s.text_bold(r.x + 5 + nudge, r.y + (r.h - GLYPH_H) / 2 + nudge, &text, color::BLACK);
            } else {
                s.text(r.x + 5, r.y + (r.h - GLYPH_H) / 2, &text, color::BLACK);
            }
        }
    }
    let c = clock_rect(w);
    s.bevel(c, Bevel::Field);
    let sw = switches.copied().unwrap_or_default();
    let icons = [(icons::NETWORK, sw.network), (icons::SPEAKER, true)];
    for (i, (icon, on)) in icons.into_iter().enumerate() {
        let ix = c.x + 4 + i as i32 * TRAY_ICON_W;
        let iy = c.y + (c.h - 16) / 2;
        s.sprite(ix, iy, icon, 1);
        if !on {
            for d in 0..2 {
                s.line(ix + 1 + d, iy + 14, ix + 14 + d, iy + 1, color::RED);
            }
        }
    }
    // Subtle separator after tray icons
    let sep1_x = c.x + 4 + 2 * TRAY_ICON_W + 2;
    s.vline(sep1_x, c.y + 4, c.h - 8, color::SHADOW);
    s.vline(sep1_x + 1, c.y + 4, c.h - 8, color::WHITE);

    if let Some((mem, clk)) = tray.split_once("  ") {
        let mem_x = sep1_x + 6;
        if in_use.0 || in_use.1 {
            // In use: the microphone and the camera, on red, where the
            // memory is (hovering says which program).
            let lights = [(in_use.0, icons::MICROPHONE), (in_use.1, icons::CAMERA)];
            for (k, (_, icon)) in lights.iter().filter(|l| l.0).enumerate() {
                let ix = mem_x + k as i32 * (TRAY_ICON_W + 2);
                s.fill(Rect::new(ix - 1, c.y + 3, TRAY_ICON_W, c.h - 6), color::RED);
                s.sprite(ix, c.y + (c.h - 16) / 2, icon, 1);
            }
        } else {
            s.text(mem_x, c.y + (c.h - GLYPH_H) / 2, mem, color::BLACK);
        }

        let sep2_x = c.x + 118;
        s.vline(sep2_x, c.y + 4, c.h - 8, color::SHADOW);
        s.vline(sep2_x + 1, c.y + 4, c.h - 8, color::WHITE);

        let clk_w = text_width(clk);
        let clk_room = c.right() - sep2_x - 4;
        let clk_offset = ((clk_room - clk_w) / 2).max(4);
        let draw_x = sep2_x + clk_offset;
        s.text(draw_x, c.y + (c.h - GLYPH_H) / 2, clk, color::BLACK);
    } else {
        let text_x = sep1_x + 6;
        let text_w = c.w - (sep1_x - c.x) - 8;
        let offset = ((text_w - text_width(tray)) / 2).max(2);
        let draw_x = (text_x + offset).min(c.right() - text_width(tray) - 4);
        s.text(draw_x, c.y + (c.h - GLYPH_H) / 2, tray, color::BLACK);
    }

    if let Some((tip, tx)) = tooltip {
        let tw = text_width(tip);
        let tip_w = tw + 8;
        let tip_h = GLYPH_H + 6;
        let tip_x = (tx - tip_w / 2).clamp(2, w - tip_w - 4);
        let tip_y = 2;
        let r = Rect::new(tip_x, tip_y, tip_w, tip_h);
        s.fill(r, 0xFFFFE1);
        s.hline(r.x, r.y, r.w, color::BLACK);
        s.hline(r.x, r.bottom() - 1, r.w, color::BLACK);
        s.vline(r.x, r.y, r.h, color::BLACK);
        s.vline(r.right() - 1, r.y, r.h, color::BLACK);
        s.text(r.x + 4, r.y + 3, tip, color::BLACK);
    }
}

// ---- Start menu & Context menus ---------------------------------------------

pub const START_ITEM_H: i32 = 34;
pub const CONTEXT_ITEM_H: i32 = 22;
pub const MENU_SEP_H: i32 = 8;
pub const MENU_BANNER_W: i32 = 21;
pub const START_MENU_W: i32 = 196;
pub const CONTEXT_MENU_W: i32 = 180;
#[allow(dead_code)]
pub const MAX_ITEMS: usize = 32;

/// One line of the Start menu or context menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryItem<'a> {
    pub label: &'a str,
    pub icon: Option<Icon>,
    pub submenu: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item<'a> {
    Entry(EntryItem<'a>),
    Separator,
}

impl<'a> Item<'a> {
    pub fn entry(label: &'a str, icon: Option<Icon>, submenu: bool) -> Self {
        Item::Entry(EntryItem { label, icon, submenu })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MenuLayout {
    pub width: i32,
    pub height: i32,
    pub cols: usize,
    pub items_per_col: usize,
    pub col_width: i32,
}

pub fn menu_layout(items: &[Item], with_band: bool, max_h: Option<i32>) -> MenuLayout {
    let (band_w, menu_w) = if with_band { (MENU_BANNER_W, START_MENU_W) } else { (0, CONTEXT_MENU_W) };
    let single_h = items.iter().map(|item| item_height(item, with_band)).sum::<i32>() + 6;
    if with_band || max_h.is_none() || single_h <= max_h.unwrap() {
        return MenuLayout { width: menu_w, height: single_h, cols: 1, items_per_col: items.len().max(1), col_width: menu_w - band_w };
    }
    let cap = max_h.unwrap().max(120);
    let items_per_col = ((cap - 6) / CONTEXT_ITEM_H).max(4) as usize;
    let cols = items.len().div_ceil(items_per_col);
    let width = (cols as i32 * CONTEXT_MENU_W).max(CONTEXT_MENU_W);
    let height = (items_per_col as i32 * CONTEXT_ITEM_H + 6).min(cap);
    MenuLayout { width, height, cols, items_per_col, col_width: CONTEXT_MENU_W }
}

pub fn item_height(item: &Item, with_band: bool) -> i32 {
    match item {
        Item::Entry(_) => {
            if with_band {
                START_ITEM_H
            } else {
                CONTEXT_ITEM_H
            }
        }
        Item::Separator => MENU_SEP_H,
    }
}

#[allow(dead_code)]
pub fn menu_height(items: &[Item], with_band: bool) -> i32 {
    items.iter().map(|item| item_height(item, with_band)).sum::<i32>() + 6
}

/// Which item (by index) is at `(x, y)`, if it's one that can be chosen.
#[allow(dead_code)]
pub fn menu_hit(items: &[Item], x: i32, y: i32, with_band: bool) -> Option<usize> {
    let (band_w, menu_w) = if with_band { (MENU_BANNER_W, START_MENU_W) } else { (0, CONTEXT_MENU_W) };
    if !(band_w + 3..menu_w - 3).contains(&x) {
        return None;
    }
    let mut top = 3;
    for (i, item) in items.iter().enumerate() {
        let h = item_height(item, with_band);
        if (top..top + h).contains(&y) {
            return matches!(item, Item::Entry(_)).then_some(i);
        }
        top += h;
    }
    None
}

pub fn menu_hit_with_layout(items: &[Item], x: i32, y: i32, with_band: bool, layout: &MenuLayout) -> Option<usize> {
    if layout.cols > 1 {
        if !(3..layout.width - 3).contains(&x) || !(3..layout.height - 3).contains(&y) {
            return None;
        }
        let col = (x - 3) / layout.col_width;
        let row = (y - 3) / CONTEXT_ITEM_H;
        if col < 0 || col >= layout.cols as i32 || row < 0 || row >= layout.items_per_col as i32 {
            return None;
        }
        let idx = col as usize * layout.items_per_col + row as usize;
        if idx < items.len() && matches!(items[idx], Item::Entry(_)) {
            return Some(idx);
        }
        return None;
    }
    menu_hit(items, x, y, with_band)
}

/// The middle of item `idx`, in menu coordinates: where a click on it
/// lands (the keyboard "clicks" there too). `None` for separators.
pub fn menu_item_center(items: &[Item], idx: usize, with_band: bool, layout: &MenuLayout) -> Option<(i32, i32)> {
    if !matches!(items.get(idx)?, Item::Entry(_)) {
        return None;
    }
    if layout.cols > 1 {
        let (col, row) = ((idx / layout.items_per_col) as i32, (idx % layout.items_per_col) as i32);
        return Some((3 + col * layout.col_width + layout.col_width / 2, 3 + row * CONTEXT_ITEM_H + CONTEXT_ITEM_H / 2));
    }
    let (band_w, menu_w) = if with_band { (MENU_BANNER_W, START_MENU_W) } else { (0, CONTEXT_MENU_W) };
    let top = 3 + items[..idx].iter().map(|item| item_height(item, with_band)).sum::<i32>();
    Some(((band_w + menu_w) / 2, top + item_height(&items[idx], with_band) / 2))
}

/// The next selectable item from `from` going `down` (or up), wrapping
/// round; the first (or last) when nothing is selected yet.
pub fn menu_step(items: &[Item], from: Option<usize>, down: bool) -> Option<usize> {
    let n = items.len();
    let entries = |i: &usize| matches!(items[*i], Item::Entry(_));
    match from {
        None if down => (0..n).find(entries),
        None => (0..n).rev().find(entries),
        Some(f) => (1..=n).map(|k| if down { (f + k) % n } else { (f + n - k) % n }).find(entries),
    }
}

pub fn draw_menu_with_layout(s: &mut Surface, items: &[Item], hover: Option<usize>, with_band: bool, layout: &MenuLayout) {
    let (band_w, _) = if with_band { (MENU_BANNER_W, START_MENU_W) } else { (0, CONTEXT_MENU_W) };
    let all = Rect::new(0, 0, layout.width, layout.height);
    s.fill(all, color::FACE);
    s.bevel(all, Bevel::Window);

    if with_band {
        let banner = Rect::new(3, 3, MENU_BANNER_W, layout.height - 6);
        s.fill(banner, color::SHADOW);
        let bottom = banner.bottom() - 6;
        let used = s.text_vertical(banner.x + 7, bottom, "Ferro", color::WHITE);
        s.text_vertical(banner.x + 8, bottom, "Ferro", color::WHITE);
        s.text_vertical(banner.x + 7, bottom - used, "OS", color::FACE);
    }

    if layout.cols > 1 {
        for c in 1..layout.cols {
            let vx = c as i32 * layout.col_width;
            s.etched_vline(vx, 3, layout.height - 6);
        }
        for (i, item) in items.iter().enumerate() {
            let col = i / layout.items_per_col;
            let row = i % layout.items_per_col;
            let ix = col as i32 * layout.col_width + 3;
            let iy = 3 + row as i32 * CONTEXT_ITEM_H;
            let iw = layout.col_width - 6;
            let ih = CONTEXT_ITEM_H;
            match item {
                Item::Entry(e) => {
                    let r = Rect::new(ix, iy, iw, ih);
                    let chosen = hover == Some(i);
                    if chosen {
                        s.fill(r, color::NAVY);
                    }
                    if let Some(icon) = e.icon {
                        s.sprite(r.x + 4, r.y + (ih - 16) / 2, icon, 1);
                    }
                    let text_x = if e.icon.is_some() { r.x + 24 } else { r.x + 8 };
                    let col = if chosen { color::WHITE } else { color::BLACK };
                    let arrow_reserve = if e.submenu { 18 } else { 4 };
                    let room = ((r.right() - text_x - arrow_reserve) / GLYPH_W).max(0) as usize;
                    s.text(text_x, r.y + (ih - GLYPH_H) / 2, &ellipsize(e.label, room), col);
                    if e.submenu {
                        s.sprite_mono(r.right() - 14, r.y + (ih - 7) / 2, icons::GLYPH_SUBMENU, col);
                    }
                }
                Item::Separator => {
                    let sy = iy + ih / 2 - 1;
                    s.etched_hline(ix + 1, sy, iw - 2);
                }
            }
        }
        return;
    }

    let mut top = 3;
    for (i, item) in items.iter().enumerate() {
        let ih = item_height(item, with_band);
        match item {
            Item::Entry(e) => {
                let r = Rect::new(band_w + 3, top, layout.width - band_w - 6, ih);
                let chosen = hover == Some(i);
                if chosen {
                    s.fill(r, color::NAVY);
                }
                if let Some(icon) = e.icon {
                    if with_band {
                        s.sprite(r.x + 4, r.y + (ih - 16 * 2) / 2, icon, 2);
                    } else {
                        s.sprite(r.x + 4, r.y + (ih - 16) / 2, icon, 1);
                    }
                }
                let text_x = if e.icon.is_some() {
                    if with_band {
                        r.x + 42
                    } else {
                        r.x + 24
                    }
                } else if with_band {
                    r.x + 12
                } else {
                    r.x + 8
                };
                let col = if chosen { color::WHITE } else { color::BLACK };
                let arrow_reserve = if e.submenu { 18 } else { 4 };
                let room = ((r.right() - text_x - arrow_reserve) / GLYPH_W).max(0) as usize;
                s.text(text_x, r.y + (ih - GLYPH_H) / 2, &ellipsize(e.label, room), col);

                if e.submenu {
                    s.sprite_mono(r.right() - 14, r.y + (ih - 7) / 2, icons::GLYPH_SUBMENU, col);
                }
            }
            Item::Separator => {
                let sy = top + ih / 2 - 1;
                s.etched_hline(band_w + 4, sy, layout.width - band_w - 8);
            }
        }
        top += ih;
    }
}

#[allow(dead_code)]
pub fn draw_menu(s: &mut Surface, items: &[Item], hover: Option<usize>, with_band: bool) {
    let layout = menu_layout(items, with_band, None);
    draw_menu_with_layout(s, items, hover, with_band, &layout);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_clicks_land_on_their_items() {
        let items =
            [Item::entry("Programs", None, true), Item::entry("Documents", None, false), Item::Separator, Item::entry("Shut Down", None, false)];
        for band in [true, false] {
            let layout = menu_layout(&items, band, None);
            for i in [0, 1, 3] {
                let (x, y) = menu_item_center(&items, i, band, &layout).unwrap();
                assert_eq!(menu_hit_with_layout(&items, x, y, band, &layout), Some(i), "item {i}, band {band}");
            }
            assert_eq!(menu_item_center(&items, 2, band, &layout), None);
        }
        // Up and down skip separators and wrap round.
        assert_eq!(menu_step(&items, None, true), Some(0));
        assert_eq!(menu_step(&items, Some(1), true), Some(3));
        assert_eq!(menu_step(&items, Some(3), true), Some(0));
        assert_eq!(menu_step(&items, Some(0), false), Some(3));
        assert_eq!(menu_step(&items, None, false), Some(3));
    }

    #[test]
    fn finds_start_and_task_buttons() {
        assert_eq!(hit(1024, 2, 0, 20, 20), BarPart::Start);
        assert_eq!(hit(1024, 2, 0, 75, 20), BarPart::Task(0));
        assert_eq!(hit(1024, 2, 0, 75 + TASK_W, 20), BarPart::Task(1));
        assert_eq!(hit(1024, 2, 0, 75 + 2 * TASK_W, 20), BarPart::Nothing);
        assert_eq!(hit(1024, 0, 0, 75, 20), BarPart::Nothing);
        assert_eq!(hit(1024, 2, 0, 1000, 20), BarPart::TrayClock);
        let c = clock_rect(1024);
        assert_eq!(hit(1024, 2, 0, c.x + 5, 20), BarPart::TrayIcon(0));
        assert_eq!(hit(1024, 2, 0, c.x + 5 + TRAY_ICON_W, 20), BarPart::TrayIcon(1));
        assert_eq!(hit(1024, 2, 0, c.x + 5 + 2 * TRAY_ICON_W, 20), BarPart::TrayRam);
        assert_eq!(hit(1024, 2, 0, c.x + 120, 20), BarPart::TrayClock);
        assert!(task_rect(1024, 30, 29, 0).right() <= clock_rect(1024).x);

        // Pinned app hit test
        assert_eq!(hit(1024, 2, 2, pinned_rect(0).x + 2, 20), BarPart::Pinned(0));
        assert_eq!(hit(1024, 2, 2, pinned_rect(1).x + 2, 20), BarPart::Pinned(1));
    }

    #[test]
    fn menu_items_and_separators() {
        let items = [
            Item::entry("Programs", Some(icons::FOLDER), true),
            Item::entry("Documents", Some(icons::FILE), false),
            Item::Separator,
            Item::entry("Shut Down...", Some(icons::COMPUTER), false),
        ];
        assert_eq!(menu_height(&items, true), 3 * START_ITEM_H + MENU_SEP_H + 6);
        assert_eq!(menu_hit(&items, 100, 5, true), Some(0));
        assert_eq!(menu_hit(&items, 100, 3 + START_ITEM_H + 2, true), Some(1));
        assert_eq!(menu_hit(&items, 100, 3 + 2 * START_ITEM_H + 3, true), None);
        assert_eq!(menu_hit(&items, 100, 3 + 2 * START_ITEM_H + MENU_SEP_H + 2, true), Some(3));
        assert_eq!(menu_hit(&items, 5, 5, true), None);
        let mut s = Surface::new(START_MENU_W as usize, menu_height(&items, true) as usize);
        draw_menu(&mut s, &items, Some(0), true);
        assert_eq!(s.pixels()[5 * START_MENU_W as usize + 150], color::NAVY);
    }

    #[test]
    fn context_menu_without_band() {
        let items = [Item::entry("New Folder", Some(icons::FOLDER), false), Item::entry("Open in Terminal", Some(icons::TERMINAL), false)];
        assert_eq!(menu_hit(&items, 50, 8, false), Some(0));
        let mut s = Surface::new(CONTEXT_MENU_W as usize, menu_height(&items, false) as usize);
        draw_menu(&mut s, &items, None, false);
        assert_eq!(s.pixels()[5 * CONTEXT_MENU_W as usize + 5], color::FACE);
    }

    #[test]
    fn tray_clock_text_fits_inside_screen_and_clock_box() {
        let w = 1024;
        let c = clock_rect(w);
        assert!(c.right() <= w);
        let sample_text = "16384 MB  12:05 PM";
        let text_w = c.w - (4 + 2 * TRAY_ICON_W + 4) - 4;
        let offset = ((text_w - text_width(sample_text)) / 2).max(2);
        let draw_x = (c.x + 4 + 2 * TRAY_ICON_W + 4 + offset).min(c.right() - text_width(sample_text) - 4);
        let text_right = draw_x + text_width(sample_text);
        assert!(text_right <= c.right() - 4, "Tray text must not exceed clock right border");
        assert!(text_right < w, "Tray text must not be cut off by the screen edge");
    }
}
