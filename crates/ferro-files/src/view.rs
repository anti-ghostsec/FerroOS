//! What a folder window looks like and where its parts are: a toolbar with
//! Up and the address, the icons, a status bar. Pure geometry and pixels
//! (ferro-gfx), so it's the same in tests as on screen.

use ferro_gfx::{color, ellipsize, icons, text_width, Bevel, Rect, Surface, GLYPH_H, GLYPH_W};

/// One thing in the folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The name shown (`Users` for `home` at the top of `C:`).
    pub name: String,
    /// The name on disk.
    pub real: String,
    pub is_dir: bool,
    pub icon: Option<icons::Icon>,
}

const TOOLBAR_H: i32 = 28;
const STATUS_H: i32 = 20;
const UP: Rect = Rect::new(4, 3, 36, 22);
const CELL_W: i32 = 88;
const CELL_H: i32 = 68;
const ICON: i32 = 32;

/// What a click lands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Up,
    Entry(usize),
    Nothing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    Open,
    ExtractHere,
    Cut,
    Copy,
    Paste,
    Rename,
    CreateShortcut,
    Delete,
    Properties,
    NewFolder,
    NewDocument,
    Terminal,
    Refresh,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextMenu {
    pub x: i32,
    pub y: i32,
    pub items: Vec<(&'static str, MenuAction)>,
}

/// A box over the window: a question (Delete this? What to rename it to?),
/// or with `notice`, something that went wrong, with just OK. With
/// `input`, a box to type in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prompt {
    pub message: String,
    pub input: Option<String>,
    pub notice: bool,
}

const PROMPT_W: i32 = 380;
const PROMPT_H: i32 = 140;
const PROMPT_LINES: usize = 4;

/// `text` in lines at most `room` characters long, broken between words.
pub fn wrap(text: &str, room: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(l) if l.chars().count() + 1 + word.chars().count() <= room => {
                l.push(' ');
                l.push_str(word);
            }
            _ => lines.push(word.to_owned()),
        }
    }
    lines
}

fn prompt_rect(w: i32, h: i32) -> Rect {
    Rect::new((w - PROMPT_W) / 2, (h - PROMPT_H) / 2, PROMPT_W, PROMPT_H)
}

/// The prompt's OK and Cancel buttons.
pub fn prompt_buttons(w: i32, h: i32) -> (Rect, Rect) {
    let r = prompt_rect(w, h);
    (Rect::new(r.right() - 170, r.bottom() - 32, 75, 24), Rect::new(r.right() - 87, r.bottom() - 32, 75, 24))
}

const MENU_ITEM_H: i32 = 20;
pub const MENU_W: i32 = 140;

pub fn menu_rect(menu: &ContextMenu) -> Rect {
    let h = menu.items.len() as i32 * MENU_ITEM_H + 6;
    Rect::new(menu.x, menu.y, MENU_W, h)
}

pub fn menu_hit(menu: &ContextMenu, x: i32, y: i32) -> Option<MenuAction> {
    let r = menu_rect(menu);
    if !r.contains(x, y) {
        return None;
    }
    let idx = ((y - r.y - 3) / MENU_ITEM_H).max(0) as usize;
    menu.items.get(idx).map(|(_, act)| *act)
}

/// The white area the icons are in.
fn pane(w: i32, h: i32) -> Rect {
    Rect::new(2, TOOLBAR_H, w - 4, (h - TOOLBAR_H - STATUS_H).max(0))
}

/// Icons per row in a window `w` wide.
pub fn columns(w: i32) -> usize {
    ((pane(w, 0).w - 8) / CELL_W).max(1) as usize
}

/// Rows of icons a window `h` high shows.
pub fn visible_rows(h: i32) -> usize {
    (pane(0, h).h / CELL_H).max(1) as usize
}

/// How many rows of icons can be scrolled off the top.
pub fn max_scroll(w: i32, h: i32, count: usize) -> usize {
    count.div_ceil(columns(w)).saturating_sub(visible_rows(h))
}

fn cell(w: i32, i: usize, scroll: usize) -> Rect {
    let cols = columns(w);
    let (col, row) = ((i % cols) as i32, (i / cols) as i32 - scroll as i32);
    Rect::new(pane(w, 0).x + 4 + col * CELL_W, TOOLBAR_H + 4 + row * CELL_H, CELL_W, CELL_H)
}

pub fn hit(w: i32, h: i32, count: usize, scroll: usize, x: i32, y: i32) -> Hit {
    if UP.contains(x, y) {
        return Hit::Up;
    }
    if !pane(w, h).contains(x, y) {
        return Hit::Nothing;
    }
    (0..count).find(|i| cell(w, *i, scroll).contains(x, y)).map_or(Hit::Nothing, Hit::Entry)
}

/// Draws the whole window: `path` is the folder as people read it
/// (`C:\Users\you`).
#[allow(clippy::too_many_arguments)]
pub fn draw(
    s: &mut Surface,
    w: i32,
    h: i32,
    path: &str,
    entries: &[Entry],
    selected: Option<usize>,
    scroll: usize,
    menu: Option<&ContextMenu>,
    status: &str,
    prompt: Option<&Prompt>,
) {
    s.fill(Rect::new(0, 0, w, h), color::FACE);

    s.button(UP, false);
    s.text(UP.x + (UP.w - text_width("Up")) / 2, UP.y + (UP.h - GLYPH_H) / 2, "Up", color::BLACK);
    let address = Rect::new(UP.right() + 6, 3, (w - UP.right() - 10).max(0), 22);
    s.fill(address, color::WHITE);
    s.bevel(address, Bevel::Sunken);
    let room = ((address.w - 12) / GLYPH_W).max(0) as usize;
    s.text(address.x + 6, address.y + (address.h - GLYPH_H) / 2, &ellipsize(path, room), color::BLACK);

    let pane = pane(w, h);
    s.fill(pane, color::WHITE);
    s.bevel(pane, Bevel::Sunken);
    s.set_clip(Some(pane.inset(2)));
    for (i, e) in entries.iter().enumerate() {
        let c = cell(w, i, scroll);
        if c.bottom() <= pane.y || c.y >= pane.bottom() {
            continue;
        }
        let icon = e.icon.unwrap_or(if e.is_dir { icons::FOLDER } else { icons::FILE });
        s.sprite(c.x + (CELL_W - ICON) / 2, c.y + 2, icon, 2);
        let label = ellipsize(&e.name, (CELL_W / GLYPH_W - 1) as usize);
        let lw = text_width(&label);
        let (lx, ly) = (c.x + (CELL_W - lw) / 2, c.y + ICON + 8);
        if selected == Some(i) {
            s.fill(Rect::new(lx - 2, ly - 2, lw + 4, GLYPH_H + 4), color::NAVY);
            s.text(lx, ly, &label, color::WHITE);
        } else {
            s.text(lx, ly, &label, color::BLACK);
        }
    }
    s.set_clip(None);

    let status_bar = Rect::new(2, h - STATUS_H + 2, w - 4, STATUS_H - 4);
    s.bevel(status_bar, Bevel::Field);
    let count = format!("{} object(s)", entries.len());
    let line = if status.is_empty() { count } else { format!("{count}   {status}") };
    let room = ((status_bar.w - 10) / GLYPH_W).max(0) as usize;
    s.text(status_bar.x + 5, status_bar.y + (status_bar.h - GLYPH_H) / 2, &ellipsize(&line, room), color::BLACK);

    if let Some(m) = menu {
        let mr = menu_rect(m);
        let mx = mr.x.clamp(2, (w - mr.w - 4).max(2));
        let my = mr.y.clamp(TOOLBAR_H, (h - mr.h - 4).max(TOOLBAR_H));
        let r = Rect::new(mx, my, mr.w, mr.h);
        s.fill(r, color::FACE);
        s.bevel(r, Bevel::Window);
        for (i, (label, _)) in m.items.iter().enumerate() {
            let iy = r.y + 3 + i as i32 * MENU_ITEM_H;
            s.text(r.x + 8, iy + (MENU_ITEM_H - GLYPH_H) / 2, label, color::BLACK);
        }
    }

    if let Some(p) = prompt {
        let r = prompt_rect(w, h);
        s.fill(r, color::FACE);
        s.bevel(r, Bevel::Window);
        let room = ((r.w - 24) / GLYPH_W).max(1) as usize;
        let lines = wrap(&p.message, room);
        let shown = if p.input.is_some() { 1 } else { PROMPT_LINES };
        for (i, line) in lines.iter().take(shown).enumerate() {
            s.text(r.x + 12, r.y + 12 + i as i32 * (GLYPH_H + 4), &ellipsize(line, room), color::BLACK);
        }
        if let Some(text) = &p.input {
            ferro_ui::text_field(s, Rect::new(r.x + 12, r.y + 40, r.w - 24, 22), text, true);
        }
        let (ok, cancel) = prompt_buttons(w, h);
        if p.notice {
            ferro_ui::button(s, cancel, "OK", true);
        } else {
            ferro_ui::button(s, ok, if p.input.is_some() { "OK" } else { "Yes" }, true);
            ferro_ui::button(s, cancel, if p.input.is_some() { "Cancel" } else { "No" }, true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(n: usize) -> Vec<Entry> {
        (0..n).map(|i| Entry { name: format!("file{i}"), real: format!("file{i}"), is_dir: i == 0, icon: None }).collect()
    }

    #[test]
    fn icons_flow_into_rows() {
        assert_eq!(columns(520), 5);
        assert_eq!(columns(60), 1);
        assert_eq!(hit(520, 360, 7, 0, 20, 10), Hit::Up);
        assert_eq!(hit(520, 360, 7, 0, 30, 50), Hit::Entry(0));
        assert_eq!(hit(520, 360, 7, 0, 30 + CELL_W, 50), Hit::Entry(1));
        assert_eq!(hit(520, 360, 7, 0, 30, 50 + CELL_H), Hit::Entry(5));
        // Past the last icon, and on the status bar.
        assert_eq!(hit(520, 360, 7, 0, 30 + 3 * CELL_W, 50 + CELL_H), Hit::Nothing);
        assert_eq!(hit(520, 360, 7, 0, 30, 355), Hit::Nothing);
    }

    #[test]
    fn scrolling_moves_rows() {
        // 520 x 360 shows four rows of five.
        assert_eq!(max_scroll(520, 360, 20), 0);
        assert_eq!(max_scroll(520, 360, 31), 3);
        assert_eq!(hit(520, 360, 31, 1, 30, 50), Hit::Entry(5));
    }

    #[test]
    fn draws_the_selection() {
        let (w, h) = (520, 360);
        let mut s = Surface::new(w as usize, h as usize);
        let list = entries(3);
        draw(&mut s, w, h, r"C:\Users\atom", &list, Some(1), 0, None, "", None);
        let navy = s.pixels().iter().filter(|p| **p == color::NAVY).count();
        assert!(navy > 100, "the selected name is highlighted");
        draw(&mut s, w, h, r"C:\Users\atom", &list, None, 0, None, "", None);
        assert_eq!(s.pixels().iter().filter(|p| **p == color::NAVY).count(), 0);
    }

    #[test]
    fn context_menu_hit_testing() {
        let menu = ContextMenu { x: 50, y: 50, items: vec![("Open", MenuAction::Open), ("Delete", MenuAction::Delete)] };
        assert_eq!(menu_hit(&menu, 60, 55), Some(MenuAction::Open));
        assert_eq!(menu_hit(&menu, 60, 75), Some(MenuAction::Delete));
        assert_eq!(menu_hit(&menu, 250, 55), None);
    }
}
