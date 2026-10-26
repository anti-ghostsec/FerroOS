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
    pub rect: Rect,
    pub items: Vec<(&'static str, MenuAction)>,
    pub hover: Option<usize>,
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

pub fn make_menu(x: i32, y: i32, items: Vec<(&'static str, MenuAction)>, w: i32, h: i32) -> ContextMenu {
    let mh = items.len() as i32 * MENU_ITEM_H + 6;
    let mx = x.clamp(2, (w - MENU_W - 4).max(2));
    let my = y.clamp(TOOLBAR_H, (h - mh - 4).max(TOOLBAR_H));
    ContextMenu {
        rect: Rect::new(mx, my, MENU_W, mh),
        items,
        hover: None,
    }
}

pub fn menu_hit(menu: &ContextMenu, x: i32, y: i32) -> Option<(usize, MenuAction)> {
    if !menu.rect.contains(x, y) {
        return None;
    }
    let idx = ((y - menu.rect.y - 3) / MENU_ITEM_H).max(0) as usize;
    menu.items.get(idx).map(|(_, act)| (idx, *act))
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

pub fn cell_rect(w: i32, i: usize, scroll: usize) -> Rect {
    cell(w, i, scroll)
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
    selected_set: Option<&std::collections::HashSet<usize>>,
    scroll: usize,
    menu: Option<&ContextMenu>,
    status: &str,
    prompt: Option<&Prompt>,
    dragging: Option<(usize, i32, i32)>,
    drag_target: Option<usize>,
    rubberband: Option<Rect>,
    renaming: Option<(usize, &str, usize)>,
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
        if drag_target == Some(i) {
            ferro_ui::focus_rect(s, c.inset(1));
        }
        let icon = e.icon.unwrap_or(if e.is_dir { icons::FOLDER } else { icons::FILE });
        s.sprite(c.x + (CELL_W - ICON) / 2, c.y + 2, icon, 2);
        if let Some((_, ren_text, ren_cursor)) = renaming.filter(|(idx, _, _)| *idx == i) {
            let tw = text_width(ren_text);
            let box_w = (tw + 12).clamp(CELL_W, CELL_W * 2);
            let box_x = c.x + (CELL_W - box_w) / 2;
            let box_y = c.y + ICON + 6;
            let box_rect = Rect::new(box_x, box_y, box_w, GLYPH_H + 4);
            s.fill(box_rect, color::WHITE);
            s.hline(box_x, box_y, box_w, color::BLACK);
            s.hline(box_x, box_y + box_rect.h - 1, box_w, color::BLACK);
            s.vline(box_x, box_y, box_rect.h, color::BLACK);
            s.vline(box_x + box_w - 1, box_y, box_rect.h, color::BLACK);
            s.text(box_x + 3, box_y + 2, ren_text, color::BLACK);
            let cx = box_x + 3 + text_width(&ren_text[..ren_cursor]);
            s.vline(cx, box_y + 2, GLYPH_H, color::BLACK);
        } else {
            let label = ellipsize(&e.name, (CELL_W / GLYPH_W - 1) as usize);
            let lw = text_width(&label);
            let (lx, ly) = (c.x + (CELL_W - lw) / 2, c.y + ICON + 8);
            let is_sel = selected == Some(i) || selected_set.is_some_and(|set| set.contains(&i));
            if is_sel {
                s.fill(Rect::new(lx - 2, ly - 2, lw + 4, GLYPH_H + 4), color::NAVY);
                s.text(lx, ly, &label, color::WHITE);
            } else {
                s.text(lx, ly, &label, color::BLACK);
            }
        }
    }
    if let Some(rb) = rubberband {
        if rb.w > 0 && rb.h > 0 {
            s.hline(rb.x, rb.y, rb.w, color::NAVY);
            s.hline(rb.x, rb.bottom() - 1, rb.w, color::NAVY);
            s.vline(rb.x, rb.y, rb.h, color::NAVY);
            s.vline(rb.right() - 1, rb.y, rb.h, color::NAVY);
        }
    }
    s.set_clip(None);

    if let Some((src_idx, mx, my)) = dragging {
        if let Some(src_e) = entries.get(src_idx) {
            let badge_text = format!("Move '{}'", ellipsize(&src_e.name, 16));
            let bw = text_width(&badge_text) + 26;
            let bh = 22;
            let bx = (mx + 12).clamp(2, (w - bw - 2).max(2));
            let by = (my + 12).clamp(TOOLBAR_H, (h - bh - STATUS_H - 2).max(TOOLBAR_H));
            let br = Rect::new(bx, by, bw, bh);
            s.fill(br, color::FACE);
            s.bevel(br, Bevel::Raised);
            let icon = src_e.icon.unwrap_or(if src_e.is_dir { icons::FOLDER } else { icons::FILE });
            s.sprite(bx + 4, by + 3, icon, 1);
            s.text(bx + 22, by + (bh - GLYPH_H) / 2, &badge_text, color::BLACK);
        }
    }

    let status_bar = Rect::new(2, h - STATUS_H + 2, w - 4, STATUS_H - 4);
    s.bevel(status_bar, Bevel::Field);
    let count = format!("{} object(s)", entries.len());
    let line = if status.is_empty() { count } else { format!("{count}   {status}") };
    let room = ((status_bar.w - 10) / GLYPH_W).max(0) as usize;
    s.text(status_bar.x + 5, status_bar.y + (status_bar.h - GLYPH_H) / 2, &ellipsize(&line, room), color::BLACK);

    if let Some(m) = menu {
        let r = m.rect;
        s.fill(r, color::FACE);
        s.bevel(r, Bevel::Window);
        for (i, (label, _)) in m.items.iter().enumerate() {
            let iy = r.y + 3 + i as i32 * MENU_ITEM_H;
            let item_r = Rect::new(r.x + 3, iy, r.w - 6, MENU_ITEM_H);
            if m.hover == Some(i) {
                s.fill(item_r, color::NAVY);
                s.text(r.x + 8, iy + (MENU_ITEM_H - GLYPH_H) / 2, label, color::WHITE);
            } else {
                s.text(r.x + 8, iy + (MENU_ITEM_H - GLYPH_H) / 2, label, color::BLACK);
            }
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
        draw(&mut s, w, h, r"C:\Users\atom", &list, Some(1), None, 0, None, "", None, None, None, None, None);
        let navy = s.pixels().iter().filter(|p| **p == color::NAVY).count();
        assert!(navy > 100, "the selected name is highlighted");
        draw(&mut s, w, h, r"C:\Users\atom", &list, None, None, 0, None, "", None, None, None, None, None);
        assert_eq!(s.pixels().iter().filter(|p| **p == color::NAVY).count(), 0);
    }

    #[test]
    fn context_menu_hit_testing() {
        let menu = make_menu(50, 50, vec![("Open", MenuAction::Open), ("Delete", MenuAction::Delete)], 520, 360);
        assert_eq!(menu_hit(&menu, 60, 55), Some((0, MenuAction::Open)));
        assert_eq!(menu_hit(&menu, 60, 75), Some((1, MenuAction::Delete)));
        assert_eq!(menu_hit(&menu, 250, 55), None);
    }
}
