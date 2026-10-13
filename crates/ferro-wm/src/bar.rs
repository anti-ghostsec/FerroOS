//! The taskbar and the Start menu: where their parts are and what they
//! look like. Pure geometry and pixels (ferro-gfx), like `frame`.

use ferro_gfx::icons::{self, Icon};
use ferro_gfx::{color, ellipsize, text_width, Bevel, Rect, Surface, GLYPH_H, GLYPH_W};

pub const HEIGHT: i32 = 28;
const START: Rect = Rect::new(2, 4, 54, 22);
const TASKS_X: i32 = 62;
const TASK_W: i32 = 160;
pub const CLOCK_W: i32 = 190;
pub const TRAY_ICON_W: i32 = 18;

/// One running program's button.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task<'a> {
    pub title: &'a str,
    /// The window with the keyboard: its button looks pushed in.
    pub active: bool,
}

/// What a click on the taskbar lands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarPart {
    Start,
    Task(usize),
    TrayIcon(usize),
    Tray,
    Nothing,
}

pub fn clock_rect(w: i32) -> Rect {
    Rect::new(w - CLOCK_W - 2, 4, CLOCK_W, HEIGHT - 6)
}

/// Each button's width when `count` of them share the taskbar.
fn task_width(w: i32, count: usize) -> i32 {
    let room = clock_rect(w).x - 4 - TASKS_X;
    if count == 0 {
        return TASK_W;
    }
    (room / count as i32).clamp(24, TASK_W)
}

fn task_rect(w: i32, count: usize, i: usize) -> Rect {
    let tw = task_width(w, count);
    Rect::new(TASKS_X + i as i32 * tw, 4, tw - 3, HEIGHT - 6)
}

pub fn hit(w: i32, count: usize, x: i32, y: i32) -> BarPart {
    if START.contains(x, y) {
        return BarPart::Start;
    }
    let c = clock_rect(w);
    if c.contains(x, y) {
        for i in 0..3 {
            let ix = c.x + 4 + i as i32 * TRAY_ICON_W;
            if x >= ix && x < ix + 16 {
                return BarPart::TrayIcon(i);
            }
        }
        return BarPart::Tray;
    }
    (0..count).find(|i| task_rect(w, count, *i).contains(x, y)).map_or(BarPart::Nothing, BarPart::Task)
}

/// Draws the taskbar, `w` wide.
pub fn draw(s: &mut Surface, w: i32, tasks: &[Task], start_open: bool, tray: &str, switches: Option<&ferro_sandbox::store::Switches>) {
    s.fill(Rect::new(0, 0, w, HEIGHT), color::FACE);
    s.hline(0, 0, w, color::FACE);
    s.hline(0, 1, w, color::WHITE);
    s.button(START, start_open);
    let nudge = i32::from(start_open);
    s.text_bold(START.x + (START.w - text_width("Start")) / 2 + nudge, START.y + (START.h - GLYPH_H) / 2 + nudge, "Start", color::BLACK);
    for (i, t) in tasks.iter().enumerate() {
        let r = task_rect(w, tasks.len(), i);
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
    let c = clock_rect(w);
    s.bevel(c, Bevel::Field);
    let sw = switches.copied().unwrap_or_default();
    let icons = [(icons::NETWORK, sw.network), (icons::MICROPHONE, sw.microphone), (icons::CAMERA, sw.camera)];
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
    let text_x = c.x + 4 + 3 * TRAY_ICON_W + 2;
    let text_w = c.w - (4 + 3 * TRAY_ICON_W + 2);
    let offset = ((text_w - text_width(tray)) / 2).max(2);
    s.text(text_x + offset, c.y + (c.h - GLYPH_H) / 2, tray, color::BLACK);
}

// ---- Start menu & Context menus ---------------------------------------------

pub const START_ITEM_H: i32 = 34;
pub const CONTEXT_ITEM_H: i32 = 22;
pub const MENU_SEP_H: i32 = 8;
pub const MENU_BANNER_W: i32 = 21;
pub const START_MENU_W: i32 = 180;
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

pub fn menu_height(items: &[Item], with_band: bool) -> i32 {
    items.iter().map(|item| item_height(item, with_band)).sum::<i32>() + 6
}

/// Which item (by index) is at `(x, y)`, if it's one that can be chosen.
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

pub fn draw_menu(s: &mut Surface, items: &[Item], hover: Option<usize>, with_band: bool) {
    let (band_w, menu_w) = if with_band { (MENU_BANNER_W, START_MENU_W) } else { (0, CONTEXT_MENU_W) };
    let h = menu_height(items, with_band);
    let all = Rect::new(0, 0, menu_w, h);
    s.fill(all, color::FACE);
    s.bevel(all, Bevel::Window);

    if with_band {
        let banner = Rect::new(3, 3, MENU_BANNER_W, h - 6);
        s.fill(banner, color::SHADOW);
        let bottom = banner.bottom() - 6;
        let used = s.text_vertical(banner.x + 7, bottom, "Ferro", color::WHITE);
        s.text_vertical(banner.x + 8, bottom, "Ferro", color::WHITE);
        s.text_vertical(banner.x + 7, bottom - used, "OS", color::FACE);
    }

    let mut top = 3;
    for (i, item) in items.iter().enumerate() {
        let ih = item_height(item, with_band);
        match item {
            Item::Entry(e) => {
                let r = Rect::new(band_w + 3, top, menu_w - band_w - 6, ih);
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
                s.etched_hline(band_w + 4, sy, menu_w - band_w - 8);
            }
        }
        top += ih;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_start_and_task_buttons() {
        assert_eq!(hit(1024, 2, 20, 14), BarPart::Start);
        assert_eq!(hit(1024, 2, 70, 14), BarPart::Task(0));
        assert_eq!(hit(1024, 2, 70 + TASK_W, 14), BarPart::Task(1));
        assert_eq!(hit(1024, 2, 70 + 2 * TASK_W, 14), BarPart::Nothing);
        assert_eq!(hit(1024, 0, 70, 14), BarPart::Nothing);
        assert_eq!(hit(1024, 2, 1000, 14), BarPart::Tray);
        let c = clock_rect(1024);
        assert_eq!(hit(1024, 2, c.x + 5, 14), BarPart::TrayIcon(0));
        assert_eq!(hit(1024, 2, c.x + 5 + TRAY_ICON_W, 14), BarPart::TrayIcon(1));
        assert_eq!(hit(1024, 2, c.x + 5 + 2 * TRAY_ICON_W, 14), BarPart::TrayIcon(2));
        assert!(task_rect(1024, 30, 29).right() <= clock_rect(1024).x);
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
}
