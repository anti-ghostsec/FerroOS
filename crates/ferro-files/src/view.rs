//! What a folder window looks like and where its parts are: a toolbar with
//! Up and the address, the icons, a status bar. Pure geometry and pixels
//! (ferro-gfx), so it's the same in tests as on screen.

use ferro_gfx::{color, ellipsize, icons, text_width, Bevel, Rect, Scrollbar, Surface, GLYPH_H, GLYPH_W, SCROLLBAR_W};

/// One thing in the folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The name shown (`Users` for `home` at the top of `C:`).
    pub name: String,
    /// The name on disk.
    pub real: String,
    pub is_dir: bool,
    pub icon: Option<icons::Icon>,
    pub size: u64,
    /// When it was last changed (seconds since 1970; 0 if not known).
    pub modified: i64,
}

const TOOLBAR_H: i32 = 28;
const STATUS_H: i32 = 20;
const UP: Rect = Rect::new(4, 3, 36, 22);
/// The places down the left (Desktop, Documents, the drives...), in a
/// window at least `SIDE_MIN` wide.
const SIDE_W: i32 = 160;
const SIDE_MIN: i32 = 520;
const SIDE_ROW_H: i32 = 22;

/// One of the places down the left.
pub struct Place {
    pub label: String,
    pub path: std::path::PathBuf,
    pub icon: icons::Icon,
}

fn side_w(w: i32) -> i32 {
    if w >= SIDE_MIN {
        SIDE_W
    } else {
        0
    }
}

/// The list of places, in a window `w` x `h` (empty when it's too narrow).
pub fn side_rect(w: i32, h: i32) -> Rect {
    Rect::new(2, TOOLBAR_H, (side_w(w) - 4).max(0), (h - TOOLBAR_H - STATUS_H).max(0))
}

/// Which place is at `(x, y)`.
pub fn side_hit(w: i32, h: i32, count: usize, x: i32, y: i32) -> Option<usize> {
    let r = side_rect(w, h);
    if !r.contains(x, y) || y < r.y + 4 {
        return None;
    }
    let i = ((y - r.y - 4) / SIDE_ROW_H) as usize;
    (i < count).then_some(i)
}

/// The places, `current` (the folder shown) marked.
pub fn draw_side(s: &mut Surface, w: i32, h: i32, places: &[Place], current: &std::path::Path, target: Option<usize>) {
    let r = side_rect(w, h);
    if r.w <= 0 {
        return;
    }
    s.fill(r, color::WHITE);
    s.bevel(r, Bevel::Sunken);
    let room = ((r.w - 32) / GLYPH_W).max(1) as usize;
    for (i, p) in places.iter().enumerate() {
        let y = r.y + 4 + i as i32 * SIDE_ROW_H;
        if y + SIDE_ROW_H > r.bottom() {
            break;
        }
        let here = p.path == current || target == Some(i);
        s.sprite(r.x + 6, y + (SIDE_ROW_H - 16) / 2, p.icon, 1);
        let label = ellipsize(&p.label, room);
        let (lx, ly) = (r.x + 26, y + (SIDE_ROW_H - GLYPH_H) / 2);
        if here {
            s.fill(Rect::new(lx - 2, ly - 2, text_width(&label) + 4, GLYPH_H + 4), color::NAVY);
            s.text(lx, ly, &label, color::WHITE);
        } else {
            s.text(lx, ly, &label, color::BLACK);
        }
    }
}

/// Where the folder's address is, which can be typed in.
pub fn address_rect(w: i32) -> Rect {
    Rect::new(UP.right() + 6, 3, (w - UP.right() - 10).max(0), 22)
}

/// The icon sizes (View in the menu, Ctrl and the wheel): a name and how many
/// times the 16-pixel icons are drawn.
pub const SIZES: [(&str, i32); 5] = [("Small icons", 1), ("Medium icons", 2), ("Large icons", 4), ("Extra large icons", 6), ("Details", 1)];
/// SIZES' last: a row for each, with its date, type and size in columns.
pub const DETAILS: usize = 4;
const ROW_H: i32 = 20;
const HEADER_H: i32 = 20;

/// The details view's columns, as Windows has them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Col {
    Name,
    Modified,
    Type,
    Size,
}

impl Col {
    /// How it's kept in files.conf.
    pub fn key(self) -> &'static str {
        match self {
            Col::Name => "name",
            Col::Modified => "modified",
            Col::Type => "type",
            Col::Size => "size",
        }
    }

    pub fn from_key(key: &str) -> Option<Col> {
        COLS.iter().map(|c| c.0).find(|c| c.key() == key)
    }

    /// Which way a fresh sort by it goes: names and types A to Z, dates
    /// and sizes newest and biggest first, as Windows has them.
    pub fn starts_up(self) -> bool {
        !matches!(self, Col::Modified | Col::Size)
    }
}

/// Sort by, in the menu: the columns, in the order the headings show them.
pub const SORT_COLS: [(Col, &str); 4] = [(Col::Name, "Name"), (Col::Modified, "Date modified"), (Col::Type, "Type"), (Col::Size, "Size")];

const COLS: [(Col, &str, i32); 4] =
    [(Col::Name, "Name", 0), (Col::Modified, "Date modified", 150), (Col::Type, "Type", 130), (Col::Size, "Size", 90)];

pub fn details() -> bool {
    icon_size() == DETAILS
}

/// Each column's left edge and width, in a window `w` wide (Name takes
/// what the others leave).
fn col_spans(w: i32) -> [(Col, &'static str, i32, i32); 4] {
    let c = cell(w, 0, 0);
    let rest: i32 = COLS.iter().map(|c| c.2).sum();
    // Narrow: the other columns give way, so all four still show.
    let name_w = (c.w - rest).max(c.w * 2 / 5);
    let squeeze = |width: i32| if c.w - rest >= name_w { width } else { (width * (c.w - name_w) / rest).max(40) };
    let mut x = c.x;
    COLS.map(|(col, name, width)| {
        let width = if width == 0 { name_w } else { squeeze(width) };
        let span = (col, name, x, width);
        x += width;
        span
    })
}

/// The column heading at `(x, y)`, in the details view.
pub fn header_hit(w: i32, x: i32, y: i32) -> Option<Col> {
    if !details() || !(TOOLBAR_H + 2..TOOLBAR_H + 2 + HEADER_H).contains(&y) {
        return None;
    }
    col_spans(w).iter().find(|s| (s.2..s.2 + s.3).contains(&x)).map(|s| s.0)
}

/// What kind of file it is, as the Type column says it.
pub fn kind_name(e: &Entry) -> String {
    if e.is_dir {
        return "File folder".into();
    }
    match std::path::Path::new(&e.real).extension().and_then(|x| x.to_str()) {
        Some(x) if !x.is_empty() => format!("{} File", x.to_uppercase()),
        _ => "File".into(),
    }
}

/// `secs` since 1970 as the Date modified column shows it, in local time.
pub fn when(secs: i64) -> String {
    if secs <= 0 {
        return String::new();
    }
    #[cfg(unix)]
    {
        // SAFETY: localtime_r fills the struct given; both live through the call.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        let t: libc::time_t = secs as libc::time_t;
        if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
            return String::new();
        }
        format!("{:04}-{:02}-{:02} {:02}:{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min)
    }
    #[cfg(not(unix))]
    String::new()
}
thread_local! {
    // The window's thread is the only one that draws.
    static SIZE: std::cell::Cell<usize> = const { std::cell::Cell::new(1) };
}

/// Which of `SIZES` the icons are.
pub fn icon_size() -> usize {
    SIZE.with(std::cell::Cell::get)
}

pub fn set_icon_size(i: usize) {
    SIZE.with(|s| s.set(i.min(SIZES.len() - 1)));
}

fn scale() -> i32 {
    SIZES[icon_size()].1
}

/// The icon's width and height, in pixels (thumbnails are made this size).
pub fn icon_px() -> i32 {
    16 * scale()
}

fn cell_w() -> i32 {
    if details() {
        // One to a row.
        return i32::MAX / 4;
    }
    (icon_px() + 40).max(88)
}

fn cell_h() -> i32 {
    if details() {
        return ROW_H;
    }
    icon_px() + 36
}

/// Where the first row of icons (or of details, under the headings) starts.
fn grid_top() -> i32 {
    if details() {
        TOOLBAR_H + 2 + HEADER_H
    } else {
        TOOLBAR_H + 4
    }
}

/// What a click lands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Up,
    Entry(usize),
    Nothing,
    ScrollUp,
    ScrollDown,
    ScrollPage(bool),
    ScrollThumb,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    Open,
    ExtractHere,
    Install,
    Cut,
    Copy,
    Paste,
    Rename,
    CreateShortcut,
    Delete,
    /// Shift+Delete: past the Recycle Bin, for good.
    DeleteForGood,
    Properties,
    NewFolder,
    NewDocument,
    Terminal,
    Refresh,
    ToggleHidden,
    IconSize(usize),
    /// Sort by: this column, the way it starts (or kept, if it's already).
    SortBy(Col),
    /// Sort by: Ascending (true) or Descending.
    SortUp(bool),
    /// The name being typed: Windows' text box menu.
    EditUndo,
    EditCut,
    EditCopy,
    EditPaste,
    EditDelete,
    EditSelectAll,
    /// Opens a menu beside: New (SUB_NEW), View (SUB_VIEW) or Sort by (SUB_SORT).
    Submenu(u8),
}

pub const SUB_NEW: u8 = 0;
pub const SUB_VIEW: u8 = 1;
pub const SUB_SORT: u8 = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextMenu {
    pub rect: Rect,
    pub items: Vec<(&'static str, MenuAction)>,
    pub hover: Option<usize>,
    /// The items shown ticked (the view in use, Show hidden files).
    pub checked: Vec<usize>,
    /// The menu open beside it (New, View), and which it is.
    pub sub: Option<(u8, Box<ContextMenu>)>,
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

fn draw_menu(s: &mut Surface, m: &ContextMenu) {
    let r = m.rect;
    s.fill(r, color::FACE);
    s.bevel(r, Bevel::Window);
    for (i, (label, action)) in m.items.iter().enumerate() {
        let iy = r.y + 3 + i as i32 * MENU_ITEM_H;
        let item_r = Rect::new(r.x + 3, iy, r.w - 6, MENU_ITEM_H);
        let lit = m.hover == Some(i) || m.sub.as_ref().is_some_and(|(k, _)| *action == MenuAction::Submenu(*k));
        let fg = if lit {
            s.fill(item_r, color::NAVY);
            color::WHITE
        } else {
            color::BLACK
        };
        let ty = iy + (MENU_ITEM_H - GLYPH_H) / 2;
        if m.checked.contains(&i) {
            s.fill(Rect::new(r.x + 9, iy + MENU_ITEM_H / 2 - 2, 5, 5), fg);
        }
        s.text(r.x + 20, ty, label, fg);
        if matches!(action, MenuAction::Submenu(_)) {
            s.text(r.right() - 16, ty, ">", fg);
        }
    }
}

/// The prompt's OK and Cancel buttons.
pub fn prompt_buttons(w: i32, h: i32) -> (Rect, Rect) {
    let r = prompt_rect(w, h);
    (Rect::new(r.right() - 170, r.bottom() - 32, 75, 24), Rect::new(r.right() - 87, r.bottom() - 32, 75, 24))
}

const MENU_ITEM_H: i32 = 20;
pub const MENU_W: i32 = 170;

pub fn make_menu(x: i32, y: i32, items: Vec<(&'static str, MenuAction)>, w: i32, h: i32) -> ContextMenu {
    let mh = items.len() as i32 * MENU_ITEM_H + 6;
    let mx = x.clamp(2, (w - MENU_W - 4).max(2));
    let my = y.clamp(TOOLBAR_H, (h - mh - 4).max(TOOLBAR_H));
    ContextMenu { rect: Rect::new(mx, my, MENU_W, mh), items, hover: None, checked: Vec::new(), sub: None }
}

/// Opens `items` beside item `idx` of `menu` (to its right, or its left
/// where the window ends), ticking `checked`.
pub fn open_sub(menu: &mut ContextMenu, idx: usize, kind: u8, items: Vec<(&'static str, MenuAction)>, checked: Vec<usize>, w: i32, h: i32) {
    let y = menu.rect.y + idx as i32 * MENU_ITEM_H;
    let right = menu.rect.right() - 3;
    let x = if right + MENU_W <= w - 4 { right } else { menu.rect.x - MENU_W + 3 };
    let mut sub = make_menu(x, y, items, w, h);
    sub.checked = checked;
    menu.sub = Some((kind, Box::new(sub)));
}

/// The item of `menu` itself (not of the one beside it) at `(x, y)`.
pub fn menu_item_at(menu: &ContextMenu, x: i32, y: i32) -> Option<usize> {
    if !menu.rect.contains(x, y) {
        return None;
    }
    let idx = ((y - menu.rect.y - 3) / MENU_ITEM_H).max(0) as usize;
    (idx < menu.items.len()).then_some(idx)
}

/// What's at `(x, y)`: in the menu beside first, then this one.
pub fn menu_hit(menu: &ContextMenu, x: i32, y: i32) -> Option<(usize, MenuAction)> {
    if let Some(hit) = menu.sub.as_ref().and_then(|(_, sub)| menu_hit(sub, x, y)) {
        return Some(hit);
    }
    if !menu.rect.contains(x, y) {
        return None;
    }
    let idx = ((y - menu.rect.y - 3) / MENU_ITEM_H).max(0) as usize;
    menu.items.get(idx).map(|(_, act)| (idx, *act))
}

pub fn has_scrollbar(w: i32, h: i32, count: usize) -> bool {
    let unconstrained_cols = ((pane(w, h).w - 8) / cell_w()).max(1) as usize;
    count.div_ceil(unconstrained_cols) > visible_rows(h)
}

pub fn scrollbar_layout(w: i32, h: i32, count: usize, scroll: usize) -> Option<Scrollbar> {
    if !has_scrollbar(w, h, count) {
        return None;
    }
    let p_h = (h - TOOLBAR_H - STATUS_H).max(0);
    let r = Rect::new(w - 2 - SCROLLBAR_W, TOOLBAR_H, SCROLLBAR_W, p_h);
    let vis = visible_rows(h);
    Some(Scrollbar::layout(r, scroll, vis, rows(w, count)))
}

/// The white area the icons are in.
fn pane(w: i32, h: i32) -> Rect {
    let side = side_w(w);
    Rect::new(2 + side, TOOLBAR_H, (w - 4 - side).max(0), (h - TOOLBAR_H - STATUS_H).max(0))
}

#[allow(dead_code)]
pub fn pane_icons(w: i32, h: i32, count: usize) -> Rect {
    let sb_w = if has_scrollbar(w, h, count) { SCROLLBAR_W } else { 0 };
    let p = pane(w, h);
    Rect::new(p.x, p.y, (p.w - sb_w).max(0), p.h)
}

/// The height of a row of icons.
pub fn row_h() -> i32 {
    cell_h()
}

/// The top and bottom of the icons' area, in a window `h` high.
pub fn pane_y_range(h: i32) -> (i32, i32) {
    let p = pane(0, h);
    (p.y, p.bottom())
}

/// Icons per row in a window `w` wide.
pub fn columns(w: i32) -> usize {
    ((pane(w, 0).w - 8) / cell_w()).max(1) as usize
}

/// Rows of icons a window `h` high shows.
pub fn visible_rows(h: i32) -> usize {
    ((pane(0, h).h - (grid_top() - TOOLBAR_H)) / cell_h()).max(1) as usize
}

/// Rows to scroll through: the icons', and one empty one after them, so a
/// full folder still has somewhere to click (and to start a box from).
fn rows(w: i32, count: usize) -> usize {
    count.div_ceil(columns(w)) + 1
}

/// How many rows of icons can be scrolled off the top.
pub fn max_scroll(w: i32, h: i32, count: usize) -> usize {
    if count.div_ceil(columns(w)) <= visible_rows(h) {
        return 0;
    }
    rows(w, count).saturating_sub(visible_rows(h))
}

fn cell(w: i32, i: usize, scroll: usize) -> Rect {
    let cols = columns(w);
    let (col, row) = ((i % cols) as i32, (i / cols) as i32 - scroll as i32);
    if details() {
        let p = pane(w, 0);
        return Rect::new(p.x + 2, grid_top() + row * ROW_H, (p.w - 4 - SCROLLBAR_W).max(0), ROW_H);
    }
    Rect::new(pane(w, 0).x + 4 + col * cell_w(), grid_top() + row * cell_h(), cell_w(), cell_h())
}

/// Where entry `i` is drawn: its icon, and its name under it. Between them
/// and the next is empty space, as on Windows.
fn item_rects(w: i32, i: usize, scroll: usize, name: &str) -> (Rect, Rect) {
    let c = cell(w, i, scroll);
    if details() {
        // The whole row is the item, as Windows' details view has it.
        return (Rect::new(c.x + 2, c.y + 2, 16, 16), Rect::new(c.x + 20, c.y, c.w - 20, ROW_H));
    }
    let icon = Rect::new(c.x + (cell_w() - icon_px()) / 2, c.y + 2, icon_px(), icon_px());
    let lw = text_width(&ellipsize(name, (cell_w() / GLYPH_W - 1) as usize));
    let label = Rect::new(c.x + (cell_w() - lw) / 2 - 2, c.y + icon_px() + 6, lw + 4, GLYPH_H + 4);
    (icon, label)
}

/// Where on entry `i` the point `(x, y)` is, as on a 32 by 32 icon (the
/// picture that's dragged): across from the icon's middle, down the icon,
/// then on into the name.
pub fn grab_point(w: i32, i: usize, scroll: usize, name: &str, (x, y): (i32, i32)) -> (i32, i32) {
    let (icon, _) = item_rects(w, i, scroll, name);
    let gx = 16 + x - (icon.x + icon.w / 2);
    let gy = if y < icon.y + icon.h { (y - icon.y) * 32 / icon.h.max(1) } else { 32 + y - (icon.y + icon.h) };
    (gx, gy)
}

/// Where entry `i`'s name is typed while it's renamed (`text` so far).
pub fn rename_rect(w: i32, i: usize, scroll: usize, text: &str) -> Rect {
    let c = cell(w, i, scroll);
    if details() {
        let name_w = col_spans(w)[0].3;
        return Rect::new(c.x + 20, c.y + 1, (name_w - 24).max(60), ROW_H - 2);
    }
    let cw = cell_w();
    let box_w = (text_width(text) + 14).clamp(cw, cw * 2);
    let x = (c.x + (cw - box_w) / 2).max(2);
    Rect::new(x, c.y + icon_px() + 6, box_w, GLYPH_H + 6)
}

/// Whether a box (the selection box) takes in entry `i`.
pub fn item_overlaps(w: i32, i: usize, scroll: usize, name: &str, r: &Rect) -> bool {
    let (icon, label) = item_rects(w, i, scroll, name);
    r.overlaps(&icon) || r.overlaps(&label)
}

pub fn hit(w: i32, h: i32, entries: &[Entry], scroll: usize, x: i32, y: i32) -> Hit {
    let count = entries.len();
    if UP.contains(x, y) {
        return Hit::Up;
    }
    if let Some(sb) = scrollbar_layout(w, h, count, scroll) {
        if sb.up.contains(x, y) {
            return Hit::ScrollUp;
        }
        if sb.down.contains(x, y) {
            return Hit::ScrollDown;
        }
        if sb.thumb.contains(x, y) {
            return Hit::ScrollThumb;
        }
        if sb.track.contains(x, y) {
            return Hit::ScrollPage(y >= sb.thumb.y);
        }
    }
    if !pane(w, h).contains(x, y) {
        return Hit::Nothing;
    }
    // Straight to the cell under the pointer: as quick with thousands.
    let p = pane(w, 0);
    let (cx, cy) = (x - p.x - 4, y - grid_top());
    if cx < 0 || cy < 0 || cx / cell_w() >= columns(w) as i32 {
        return Hit::Nothing;
    }
    let i = (cy / cell_h()) as usize * columns(w) + scroll * columns(w) + (cx / cell_w()) as usize;
    match entries.get(i) {
        Some(e) => {
            let (icon, label) = item_rects(w, i, scroll, &e.name);
            if icon.contains(x, y) || label.contains(x, y) {
                Hit::Entry(i)
            } else {
                Hit::Nothing
            }
        }
        None => Hit::Nothing,
    }
}

/// A small triangle, 7 wide, around `cy`: point up when going up (A to Z).
fn sort_arrow(s: &mut Surface, x: i32, cy: i32, up: bool) {
    for row in 0..4 {
        let half = if up { row } else { 3 - row };
        for dx in -half..=half {
            s.put(x + 3 + dx, cy - 2 + row, color::SHADOW);
        }
    }
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
    renaming: Option<(usize, &ferro_ui::TextEdit)>,
    thumbnails: Option<&std::collections::HashMap<String, ferro_gfx::DecodedImage>>,
    sort: (Col, bool),
) {
    s.fill(Rect::new(0, 0, w, h), color::FACE);

    s.button(UP, false);
    s.text(UP.x + (UP.w - text_width("Up")) / 2, UP.y + (UP.h - GLYPH_H) / 2, "Up", color::BLACK);
    let address = address_rect(w);
    s.fill(address, color::WHITE);
    s.bevel(address, Bevel::Sunken);
    let room = ((address.w - 12) / GLYPH_W).max(0) as usize;
    s.text(address.x + 6, address.y + (address.h - GLYPH_H) / 2, &ellipsize(path, room), color::BLACK);

    let pane = pane(w, h);
    s.fill(pane, color::WHITE);
    s.bevel(pane, Bevel::Sunken);
    s.set_clip(Some(pane.inset(2)));
    let spans = col_spans(w);
    for (i, e) in entries.iter().enumerate() {
        let c = cell(w, i, scroll);
        if c.bottom() <= pane.y || c.y >= pane.bottom() {
            continue;
        }
        if drag_target == Some(i) {
            ferro_ui::focus_rect(s, c.inset(1));
        }
        if details() {
            let is_sel = selected == Some(i) || selected_set.is_some_and(|set| set.contains(&i));
            let fg = if is_sel {
                s.fill(c, color::NAVY);
                color::WHITE
            } else {
                color::BLACK
            };
            s.sprite(c.x + 2, c.y + 2, e.icon.unwrap_or(if e.is_dir { icons::FOLDER } else { icons::FILE }), 1);
            let ty = c.y + (ROW_H - GLYPH_H) / 2;
            let size = if e.is_dir { String::new() } else { crate::ops::human_size(e.size) };
            let cells = [e.name.clone(), when(e.modified), kind_name(e), size];
            for (k, (col, _, x, cw)) in spans.iter().enumerate() {
                let room = ((cw - 10) / GLYPH_W).max(0) as usize;
                let text = ellipsize(&cells[k], room);
                if *col == Col::Name && renaming.is_some_and(|(idx, _)| idx == i) {
                    continue;
                }
                let tx = match col {
                    Col::Name => c.x + 22,
                    Col::Size => x + cw - 8 - text_width(&text),
                    _ => x + 6,
                };
                s.text(tx, ty, &text, fg);
            }
            if let Some((_, edit)) = renaming.filter(|(idx, _)| *idx == i) {
                let r = rename_rect(w, i, scroll, edit.text());
                s.fill(r, color::BLACK);
                edit.draw(s, r.inset(1));
            }
            continue;
        }
        let icon = e.icon.unwrap_or(if e.is_dir { icons::FOLDER } else { icons::FILE });
        let (cw, px) = (cell_w(), icon_px());
        let ix = c.x + (cw - px) / 2;
        let iy = c.y + 2;
        if let Some(thumb) = thumbnails.and_then(|m| m.get(&e.real)) {
            s.draw_image_scaled(Rect::new(ix, iy, px, px), thumb);
            s.bevel(Rect::new(ix - 1, iy - 1, px + 2, px + 2), Bevel::Sunken);
        } else {
            s.sprite(ix, iy, icon, scale());
        }
        if let Some((_, edit)) = renaming.filter(|(idx, _)| *idx == i) {
            let r = rename_rect(w, i, scroll, edit.text());
            s.fill(r, color::BLACK);
            edit.draw(s, r.inset(1));
        } else {
            let label = ellipsize(&e.name, (cw / GLYPH_W - 1) as usize);
            let lw = text_width(&label);
            let (lx, ly) = (c.x + (cw - lw) / 2, c.y + px + 8);
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
            for x in 0..rb.w {
                let col = if (rb.x + x) % 2 == 0 { color::WHITE } else { color::BLACK };
                s.put(rb.x + x, rb.y, col);
                s.put(rb.x + x, rb.bottom() - 1, col);
            }
            for y in 0..rb.h {
                let col = if (rb.y + y) % 2 == 0 { color::WHITE } else { color::BLACK };
                s.put(rb.x, rb.y + y, col);
                s.put(rb.right() - 1, rb.y + y, col);
            }
        }
    }
    // The column headings, over the rows.
    if details() {
        for (col, name, x, cw) in spans {
            let r = Rect::new(x, TOOLBAR_H + 2, cw, HEADER_H);
            s.button(r, false);
            // The heading sorted by keeps room for its arrow at the right.
            let arrow = col == sort.0 && cw > 30;
            let room = cw - 10 - if arrow { 14 } else { 0 };
            s.text(x + 6, r.y + (HEADER_H - GLYPH_H) / 2, &ellipsize(name, (room / GLYPH_W).max(0) as usize), color::BLACK);
            if arrow {
                sort_arrow(s, r.right() - 14, r.y + HEADER_H / 2, sort.1);
            }
        }
    }
    s.set_clip(None);

    if let Some(sb) = scrollbar_layout(w, h, entries.len(), scroll) {
        s.scrollbar(&sb, false, false);
    }

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
    let p1_w = (w * 32 / 100).clamp(110, 200);
    let p2_w = 110;
    let p3_w = (w - 4 - p1_w - p2_w - 8).max(60);

    let p1 = Rect::new(status_bar.x, status_bar.y, p1_w, status_bar.h);
    let p2 = Rect::new(p1.right() + 4, status_bar.y, p2_w, status_bar.h);
    let p3 = Rect::new(p2.right() + 4, status_bar.y, p3_w, status_bar.h);

    s.bevel(p1, Bevel::Field);
    s.bevel(p2, Bevel::Field);
    s.bevel(p3, Bevel::Field);

    let sel_count = selected_set.map(|s| s.len()).unwrap_or(if selected.is_some() { 1 } else { 0 });
    let p1_text = if sel_count == 0 {
        format!("{} object(s)", entries.len())
    } else if sel_count == 1 {
        "1 object(s) selected".to_string()
    } else {
        format!("{sel_count} object(s) selected")
    };
    let room1 = ((p1.w - 8) / GLYPH_W).max(1) as usize;
    s.text(p1.x + 4, p1.y + (p1.h - GLYPH_H) / 2, &ellipsize(&p1_text, room1), color::BLACK);

    let bytes_to_show: u64 = if sel_count > 0 {
        if let Some(set) = selected_set {
            set.iter().filter_map(|&i| entries.get(i)).map(|e| e.size).sum()
        } else if let Some(i) = selected {
            entries.get(i).map(|e| e.size).unwrap_or(0)
        } else {
            0
        }
    } else {
        entries.iter().map(|e| e.size).sum()
    };
    let p2_text = crate::ops::human_size(bytes_to_show);
    let room2 = ((p2.w - 8) / GLYPH_W).max(1) as usize;
    s.text(p2.x + 4, p2.y + (p2.h - GLYPH_H) / 2, &ellipsize(&p2_text, room2), color::BLACK);

    let p3_text = if !status.is_empty() {
        status.to_string()
    } else if sel_count == 1 {
        let name = if let Some(set) = selected_set {
            set.iter().next().and_then(|&i| entries.get(i)).map(|e| e.name.clone())
        } else if let Some(i) = selected {
            entries.get(i).map(|e| e.name.clone())
        } else {
            None
        };
        name.unwrap_or_else(|| path.to_string())
    } else {
        path.to_string()
    };
    let room3 = ((p3.w - 8) / GLYPH_W).max(1) as usize;
    s.text(p3.x + 4, p3.y + (p3.h - GLYPH_H) / 2, &ellipsize(&p3_text, room3), color::BLACK);

    if let Some(m) = menu {
        draw_menu(s, m);
        if let Some((_, sub)) = &m.sub {
            draw_menu(s, sub);
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
            ferro_ui::default_button(s, cancel, "OK", true);
        } else {
            ferro_ui::default_button(s, ok, if p.input.is_some() { "OK" } else { "Yes" }, true);
            ferro_ui::button(s, cancel, if p.input.is_some() { "Cancel" } else { "No" }, true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(n: usize) -> Vec<Entry> {
        (0..n).map(|i| Entry { name: format!("file{i}"), real: format!("file{i}"), is_dir: i == 0, icon: None, size: 1024, modified: 0 }).collect()
    }

    #[test]
    fn icons_flow_into_rows() {
        let e = entries(7);
        assert_eq!(columns(519), 5);
        assert_eq!(columns(60), 1);
        assert_eq!(hit(519, 360, &e, 0, 20, 10), Hit::Up);
        assert_eq!(hit(519, 360, &e, 0, 50, 50), Hit::Entry(0));
        assert_eq!(hit(519, 360, &e, 0, 50 + 88, 50), Hit::Entry(1));
        assert_eq!(hit(519, 360, &e, 0, 50, 50 + 68), Hit::Entry(5));
        // Beside an icon is empty space; so is past the last, and the status bar.
        assert_eq!(hit(519, 360, &e, 0, 10, 50), Hit::Nothing);
        assert_eq!(hit(519, 360, &e, 0, 50 + 3 * 88, 50 + 68), Hit::Nothing);
        assert_eq!(hit(519, 360, &e, 0, 30, 355), Hit::Nothing);
    }

    #[test]
    fn scrolling_moves_rows() {
        // 519 x 360 (no places down the left) shows four rows of five; a full folder gets an empty row after.
        assert_eq!(max_scroll(519, 360, 20), 0);
        assert_eq!(max_scroll(519, 360, 31), 4);
        assert_eq!(hit(519, 360, &entries(31), 1, 50, 50), Hit::Entry(5));
    }

    #[test]
    fn sort_columns_keep_their_names() {
        for (col, _) in SORT_COLS {
            assert_eq!(Col::from_key(col.key()), Some(col));
        }
        assert_eq!(Col::from_key("bogus"), None);
        assert!(Col::Name.starts_up() && Col::Type.starts_up());
        assert!(!Col::Modified.starts_up() && !Col::Size.starts_up());
    }

    #[test]
    fn icon_sizes() {
        let e = entries(7);
        set_icon_size(3);
        assert_eq!(icon_px(), 96);
        assert_eq!(columns(519), 3);
        assert_eq!(hit(519, 360, &e, 0, 80, 80), Hit::Entry(0));
        assert_eq!(hit(519, 360, &e, 0, 80 + 136, 80), Hit::Entry(1));
    }

    #[test]
    fn details_a_row_each_under_headings() {
        let e = entries(5);
        set_icon_size(DETAILS);
        assert_eq!(columns(700), 1);
        let first = cell(700, 0, 0);
        assert_eq!(hit(700, 360, &e, 0, first.x + 300, first.y + 5), Hit::Entry(0));
        assert_eq!(hit(700, 360, &e, 0, first.x + 300, first.y + ROW_H + 5), Hit::Entry(1));
        assert_eq!(hit(700, 360, &e, 1, first.x + 300, first.y + 5), Hit::Entry(1));
        assert_eq!(header_hit(700, first.x + 5, TOOLBAR_H + 5), Some(Col::Name));
        assert_eq!(header_hit(700, first.right() - 5, TOOLBAR_H + 5), Some(Col::Size));
        // Narrow, every column still shows.
        let spans = col_spans(400);
        assert!(spans[3].2 + spans[3].3 <= cell(400, 0, 0).right() + 1);
        assert_eq!(kind_name(&e[0]), "File folder");
        assert_eq!(kind_name(&Entry { real: "a.jpg".into(), is_dir: false, ..e[1].clone() }), "JPG File");
        set_icon_size(1);
    }

    #[test]
    fn places_down_the_left_when_theres_room() {
        assert_eq!(side_rect(519, 360).w, 0);
        let side = side_rect(700, 360);
        assert!(side.w > 100);
        assert_eq!(side_hit(700, 360, 3, side.x + 10, side.y + 4 + SIDE_ROW_H + 2), Some(1));
        assert_eq!(side_hit(700, 360, 3, side.x + 10, side.y + 4 + 5 * SIDE_ROW_H), None);
        // The icons start after it.
        let e = entries(2);
        assert_eq!(hit(700, 360, &e, 0, side.right() + 40, 50), Hit::Entry(0));
        assert_eq!(hit(700, 360, &e, 0, 50, 50), Hit::Nothing);
    }

    #[test]
    fn draws_the_selection() {
        let (w, h) = (520, 360);
        let mut s = Surface::new(w as usize, h as usize);
        let list = entries(3);
        draw(&mut s, w, h, r"C:\Users\atom", &list, Some(1), None, 0, None, "", None, None, None, None, None, None, (Col::Name, true));
        let navy = s.pixels().iter().filter(|p| **p == color::NAVY).count();
        assert!(navy > 100, "the selected name is highlighted");
        draw(&mut s, w, h, r"C:\Users\atom", &list, None, None, 0, None, "", None, None, None, None, None, None, (Col::Name, true));
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
