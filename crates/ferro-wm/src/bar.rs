//! The taskbar and the Start menu: where their parts are and what they
//! look like. Pure geometry and pixels (ferro-gfx), like `frame`.

use ferro_gfx::{color, ellipsize, text_width, Bevel, Rect, Surface, GLYPH_H, GLYPH_W};

pub const HEIGHT: i32 = 28;
const START: Rect = Rect::new(2, 4, 54, 22);
const TASKS_X: i32 = 62;
const TASK_W: i32 = 160;
const CLOCK_W: i32 = 74;

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
    Nothing,
}

fn clock_rect(w: i32) -> Rect {
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
    (0..count).find(|i| task_rect(w, count, *i).contains(x, y)).map_or(BarPart::Nothing, BarPart::Task)
}

/// Draws the taskbar, `w` wide.
pub fn draw(s: &mut Surface, w: i32, tasks: &[Task], start_open: bool, clock: &str) {
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
    s.text(c.x + (c.w - text_width(clock)) / 2, c.y + (c.h - GLYPH_H) / 2, clock, color::BLACK);
}

// ---- Start menu ------------------------------------------------------------

const ITEM_H: i32 = 20;
/// The band down the left side with the system's name on it.
const BAND_W: i32 = 22;
pub const MENU_W: i32 = 230;
/// More programs than this get a taller menu than most screens.
pub const MAX_ITEMS: usize = 24;

/// One line of the Start menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item<'a> {
    Entry(&'a str),
    Separator,
}

fn item_height(item: &Item) -> i32 {
    match item {
        Item::Entry(_) => ITEM_H,
        Item::Separator => 8,
    }
}

pub fn menu_height(items: &[Item]) -> i32 {
    items.iter().map(item_height).sum::<i32>() + 6
}

/// Which item (by index) is at `y`, if it's one that can be chosen.
pub fn menu_hit(items: &[Item], x: i32, y: i32) -> Option<usize> {
    if !(BAND_W + 3..MENU_W - 3).contains(&x) {
        return None;
    }
    let mut top = 3;
    for (i, item) in items.iter().enumerate() {
        let h = item_height(item);
        if (top..top + h).contains(&y) {
            return matches!(item, Item::Entry(_)).then_some(i);
        }
        top += h;
    }
    None
}

pub fn draw_menu(s: &mut Surface, items: &[Item], hover: Option<usize>, band: &str) {
    let h = menu_height(items);
    let all = Rect::new(0, 0, MENU_W, h);
    s.fill(all, color::FACE);
    s.bevel(all, Bevel::Window);
    let strip = Rect::new(3, 3, BAND_W, h - 6);
    s.fill(strip, color::SHADOW);
    s.text_vertical(strip.x + (BAND_W - GLYPH_H) / 2, strip.bottom() - 6, band, color::WHITE);
    let mut top = 3;
    for (i, item) in items.iter().enumerate() {
        let ih = item_height(item);
        match item {
            Item::Entry(name) => {
                let r = Rect::new(BAND_W + 3, top, MENU_W - BAND_W - 6, ih);
                let chosen = hover == Some(i);
                if chosen {
                    s.fill(r, color::NAVY);
                }
                let room = ((r.w - 16) / GLYPH_W).max(0) as usize;
                s.text(r.x + 8, r.y + (ih - GLYPH_H) / 2, &ellipsize(name, room), if chosen { color::WHITE } else { color::BLACK });
            }
            Item::Separator => s.etched_hline(BAND_W + 5, top + 3, MENU_W - BAND_W - 10),
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
        // Many windows share the room instead of running under the clock.
        assert!(task_rect(1024, 30, 29).right() <= clock_rect(1024).x);
    }

    #[test]
    fn menu_items_and_separators() {
        let items = [Item::Entry("Steam"), Item::Entry("Terminal"), Item::Separator, Item::Entry("Shut Down...")];
        assert_eq!(menu_height(&items), 3 * ITEM_H + 8 + 6);
        assert_eq!(menu_hit(&items, 100, 5), Some(0));
        assert_eq!(menu_hit(&items, 100, 3 + ITEM_H + 2), Some(1));
        assert_eq!(menu_hit(&items, 100, 3 + 2 * ITEM_H + 3), None);
        assert_eq!(menu_hit(&items, 100, 3 + 2 * ITEM_H + 8 + 2), Some(3));
        assert_eq!(menu_hit(&items, 5, 5), None);
        let mut s = Surface::new(MENU_W as usize, menu_height(&items) as usize);
        draw_menu(&mut s, &items, Some(0), "FerroOS");
        assert_eq!(s.pixels()[5 * MENU_W as usize + 200], color::NAVY);
    }
}
