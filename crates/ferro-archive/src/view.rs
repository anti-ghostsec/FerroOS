//! The archive window, drawn: its buttons, the list of what's inside, the
//! status bar, and the boxes over it (where to, password, progress).

use ferro_archive::{format_time, human_size, Listing};
use ferro_gfx::{color, ellipsize, icons, text_width, wrap, Bevel, Rect, Scrollbar, Surface, GLYPH_H, GLYPH_W, SCROLLBAR_W};
use ferro_ui::TextEdit;

pub const W: i32 = 640;
pub const H: i32 = 440;

pub const TB_ALL: Rect = Rect::new(8, 6, 112, 24);
pub const TB_SEL: Rect = Rect::new(124, 6, 152, 24);
pub const TB_HERE: Rect = Rect::new(280, 6, 104, 24);
/// The toolbar buttons, in Tab order.
pub const CONTROLS: [Rect; 3] = [TB_ALL, TB_SEL, TB_HERE];

pub const LIST: Rect = Rect::new(8, 38, W - 16, 372);
const INNER: Rect = LIST.inset(2);
pub const HEADER: Rect = Rect::new(INNER.x, INNER.y, INNER.w - SCROLLBAR_W, 18);
pub const ROWS: Rect = Rect::new(INNER.x, HEADER.y + HEADER.h, HEADER.w, INNER.h - HEADER.h);
pub const SCROLL: Rect = Rect::new(INNER.x + INNER.w - SCROLLBAR_W, INNER.y, SCROLLBAR_W, INNER.h);
pub const ROW_H: i32 = 18;
pub const VISIBLE: usize = (ROWS.h / ROW_H) as usize;
pub const STATUS: Rect = Rect::new(4, H - 26, W - 8, 22);

/// Name, Size, Packed, Modified: where each starts and how wide it is.
pub const COLS: [(&str, i32, i32); 4] = [("Name", 0, 284), ("Size", 284, 84), ("Packed", 368, 84), ("Modified", 452, HEADER.w - 452)];

pub const DLG: Rect = Rect::new((W - 400) / 2, 130, 400, 156);
pub const DLG_FIELD: Rect = Rect::new(DLG.x + 12, DLG.y + 54, DLG.w - 24, 22);
pub const DLG_OK: Rect = Rect::new(DLG.x + DLG.w - 172, DLG.y + DLG.h - 34, 75, 24);
pub const DLG_CANCEL: Rect = Rect::new(DLG.x + DLG.w - 87, DLG.y + DLG.h - 34, 75, 24);
/// The one button of a notice or the progress box: in the middle.
pub const DLG_ONLY: Rect = Rect::new(DLG.x + (DLG.w - 75) / 2, DLG.y + DLG.h - 34, 75, 24);
const BAR: Rect = Rect::new(DLG.x + 12, DLG.y + 66, DLG.w - 24, 20);

/// A box over the window: it has the keyboard and the clicks.
pub enum Dialog {
    /// Where to: Extract all (`which` None) or the selection.
    Folder { edit: TextEdit, which: Option<Vec<usize>> },
    /// The password, typed (shown as stars). `wrong`: the last one wasn't.
    Password { typed: String, wrong: bool },
    /// Unpacking: how far, and Cancel.
    Progress { title: String, percent: u32, done: u64, total: u64, current: String, cancelling: bool },
    /// A notice: OK.
    Message { title: String, text: String },
}

pub struct View<'a> {
    pub name: &'a str,
    pub listing: Option<&'a Listing>,
    pub selected: &'a [bool],
    pub cursor: usize,
    pub scroll: usize,
    pub focus: Option<usize>,
    pub status: &'a str,
    pub dialog: Option<&'a Dialog>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    ExtractAll,
    ExtractSelected,
    ExtractHere,
    Row(usize),
    ScrollUp,
    ScrollDown,
    PageUp,
    PageDown,
    DialogOk,
    DialogCancel,
    DialogField,
    Nothing,
}

pub fn hit(x: i32, y: i32, v: &View) -> Hit {
    if let Some(d) = v.dialog {
        return match d {
            Dialog::Progress { .. } | Dialog::Message { .. } if DLG_ONLY.contains(x, y) => match d {
                Dialog::Progress { .. } => Hit::DialogCancel,
                _ => Hit::DialogOk,
            },
            Dialog::Folder { .. } | Dialog::Password { .. } if DLG_OK.contains(x, y) => Hit::DialogOk,
            Dialog::Folder { .. } | Dialog::Password { .. } if DLG_CANCEL.contains(x, y) => Hit::DialogCancel,
            Dialog::Folder { .. } | Dialog::Password { .. } if DLG_FIELD.contains(x, y) => Hit::DialogField,
            _ => Hit::Nothing,
        };
    }
    let has = v.listing.is_some_and(|l| !l.entries.is_empty());
    if TB_ALL.contains(x, y) && has {
        return Hit::ExtractAll;
    }
    if TB_SEL.contains(x, y) && v.selected.contains(&true) {
        return Hit::ExtractSelected;
    }
    if TB_HERE.contains(x, y) && has {
        return Hit::ExtractHere;
    }
    let total = v.listing.map_or(0, |l| l.entries.len());
    if SCROLL.contains(x, y) {
        let sb = Scrollbar::layout(SCROLL, v.scroll, VISIBLE, total);
        return if sb.up.contains(x, y) {
            Hit::ScrollUp
        } else if sb.down.contains(x, y) {
            Hit::ScrollDown
        } else if y < sb.thumb.y {
            Hit::PageUp
        } else if y >= sb.thumb.bottom() && sb.thumb.h > 0 {
            Hit::PageDown
        } else {
            Hit::Nothing
        };
    }
    if ROWS.contains(x, y) {
        let i = v.scroll + ((y - ROWS.y) / ROW_H) as usize;
        if i < total {
            return Hit::Row(i);
        }
    }
    Hit::Nothing
}

/// `text` right-aligned, ending at `right`.
fn text_right(s: &mut Surface, right: i32, y: i32, text: &str, c: u32) {
    s.text(right - text_width(text), y, text, c);
}

pub fn draw(s: &mut Surface, v: &View) {
    s.fill(Rect::new(0, 0, W, H), color::FACE);
    let has = v.listing.is_some_and(|l| !l.entries.is_empty());
    ferro_ui::button(s, TB_ALL, "Extract all...", has);
    ferro_ui::button(s, TB_SEL, "Extract selected...", v.selected.contains(&true));
    ferro_ui::button(s, TB_HERE, "Extract here", has);
    if let Some(r) = v.focus.and_then(|i| CONTROLS.get(i)) {
        ferro_ui::focus_rect(s, r.inset(3));
    }

    // The list: white, sunken, column headings as buttons.
    s.fill(LIST, color::WHITE);
    s.bevel(LIST, Bevel::Sunken);
    for (label, x, w) in COLS {
        let r = Rect::new(HEADER.x + x, HEADER.y, w, HEADER.h);
        s.button(r, false);
        s.text(r.x + 5, r.y + (r.h - GLYPH_H) / 2, label, color::BLACK);
    }
    let total = v.listing.map_or(0, |l| l.entries.len());
    let sb = Scrollbar::layout(SCROLL, v.scroll, VISIBLE, total);
    s.scrollbar(&sb, false, false);

    if let Some(l) = v.listing {
        s.set_clip(Some(ROWS));
        for (row, e) in l.entries.iter().enumerate().skip(v.scroll).take(VISIBLE) {
            let y = ROWS.y + (row - v.scroll) as i32 * ROW_H;
            let sel = v.selected.get(row).copied().unwrap_or(false);
            let fg = if sel { color::WHITE } else { color::BLACK };
            let ty = y + (ROW_H - GLYPH_H) / 2;
            let name_w = COLS[0].2 - 24;
            let shown = ellipsize(e.name.trim_end_matches('/'), (name_w / GLYPH_W) as usize);
            s.sprite(ROWS.x + 3, y + 1, if e.is_dir { icons::FOLDER } else { icons::FILE }, 1);
            let label_x = ROWS.x + 22;
            if sel {
                s.fill(Rect::new(label_x - 2, y + 1, text_width(&shown) + 4, ROW_H - 2), color::NAVY);
            }
            s.text(label_x, ty, &shown, fg);
            if row == v.cursor && v.dialog.is_none() {
                ferro_ui::focus_rect(s, Rect::new(label_x - 2, y + 1, text_width(&shown) + 4, ROW_H - 2));
            }
            let mut extra = String::new();
            if e.encrypted {
                extra.push('*');
            }
            if !e.is_dir {
                text_right(s, ROWS.x + COLS[1].1 + COLS[1].2 - 6, ty, &format!("{}{extra}", human_size(e.size)), color::BLACK);
                if e.packed > 0 {
                    text_right(s, ROWS.x + COLS[2].1 + COLS[2].2 - 6, ty, &human_size(e.packed), color::BLACK);
                }
            }
            if let Some(t) = e.modified {
                s.text(ROWS.x + COLS[3].1 + 5, ty, &format_time(t), color::BLACK);
            }
        }
        s.set_clip(None);
    } else if v.dialog.is_none() {
        s.text(ROWS.x + 10, ROWS.y + 10, "No archive is open.", color::SHADOW);
    }

    // The status bar.
    s.bevel(STATUS, Bevel::Field);
    let room = ((STATUS.w - 12) / GLYPH_W) as usize;
    s.text(STATUS.x + 6, STATUS.y + (STATUS.h - GLYPH_H) / 2, &ellipsize(v.status, room), color::BLACK);

    if let Some(d) = v.dialog {
        draw_dialog(s, d, v.name);
    }
}

fn draw_dialog(s: &mut Surface, d: &Dialog, name: &str) {
    s.fill(DLG, color::FACE);
    s.bevel(DLG, Bevel::Window);
    let bar = Rect::new(DLG.x + 3, DLG.y + 3, DLG.w - 6, 18);
    s.fill(bar, color::NAVY);
    let title = match d {
        Dialog::Folder { .. } => "Extract",
        Dialog::Password { .. } => "Password",
        Dialog::Progress { title, .. } | Dialog::Message { title, .. } => title,
    };
    s.text_bold(bar.x + 4, bar.y + 5, title, color::WHITE);
    let (x, room) = (DLG.x + 12, ((DLG.w - 24) / GLYPH_W) as usize);
    match d {
        Dialog::Folder { edit, which } => {
            let what = match which {
                None => format!("Extract all of '{name}' to this folder:"),
                Some(w) => format!("Extract the {} item(s) selected to this folder:", w.len()),
            };
            s.text(x, DLG.y + 34, &ellipsize(&what, room), color::BLACK);
            s.bevel(DLG_FIELD, Bevel::Sunken);
            edit.draw(s, DLG_FIELD.inset(2));
            s.text(x, DLG.y + 86, "Folders inside the archive are kept.", color::BLACK);
            ferro_ui::default_button(s, DLG_OK, "OK", true);
            ferro_ui::button(s, DLG_CANCEL, "Cancel", true);
        }
        Dialog::Password { typed, wrong } => {
            let line = if *wrong { "That password is not correct. Try again:".to_owned() } else { format!("'{name}' is password-protected.") };
            s.text(x, DLG.y + 30, &ellipsize(&line, room), if *wrong { color::RED } else { color::BLACK });
            s.text(x, DLG.y + 42, "Password:", color::BLACK);
            let stars = "*".repeat(typed.chars().count());
            ferro_ui::text_field(s, DLG_FIELD, &stars, true);
            ferro_ui::default_button(s, DLG_OK, "OK", true);
            ferro_ui::button(s, DLG_CANCEL, "Cancel", true);
        }
        Dialog::Progress { percent, done, total, current, cancelling, .. } => {
            let line =
                if *cancelling { "Cancelling...".to_owned() } else { format!("Extracting {}", if current.is_empty() { name } else { current }) };
            s.text(x, DLG.y + 34, &ellipsize(&line, room), color::BLACK);
            s.text(x, DLG.y + 50, &format!("{} of {}", human_size(*done), human_size(*total)), color::BLACK);
            progress_bar(s, BAR, *percent);
            ferro_ui::button(s, DLG_ONLY, "Cancel", !cancelling);
        }
        Dialog::Message { text, .. } => {
            for (i, line) in wrap(text, room).iter().take(6).enumerate() {
                s.text(x, DLG.y + 34 + i as i32 * 12, line, color::BLACK);
            }
            ferro_ui::default_button(s, DLG_ONLY, "OK", true);
        }
    }
}

/// Windows 95's progress bar: navy blocks filling a sunken box.
pub fn progress_bar(s: &mut Surface, r: Rect, percent: u32) {
    s.fill(r, color::WHITE);
    s.bevel(r, Bevel::Sunken);
    let inner = r.inset(3);
    let filled = inner.w * percent.min(100) as i32 / 100;
    let (block, gap) = (9, 2);
    let mut bx = inner.x;
    while bx < inner.x + filled {
        let w = block.min(inner.x + filled - bx);
        s.fill(Rect::new(bx, inner.y, w, inner.h), color::NAVY);
        bx += block + gap;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferro_archive::{Entry, Format};

    fn listing(n: usize) -> Listing {
        let e = |i: usize| Entry {
            name: format!("file{i}.txt"),
            size: 10,
            packed: 5,
            modified: Some(1_700_000_000),
            is_dir: false,
            is_link: false,
            encrypted: false,
        };
        Listing { format: Format::Zip, entries: (0..n).map(e).collect() }
    }

    fn view<'a>(l: Option<&'a Listing>, sel: &'a [bool], d: Option<&'a Dialog>) -> View<'a> {
        View { name: "a.zip", listing: l, selected: sel, cursor: 0, scroll: 0, focus: None, status: "", dialog: d }
    }

    #[test]
    fn layout_fits_the_window() {
        assert!(LIST.bottom() < STATUS.y && STATUS.bottom() <= H);
        assert_eq!(COLS.iter().map(|c| c.2).sum::<i32>(), HEADER.w);
        const { assert!(VISIBLE >= 15) };
        assert!(TB_HERE.right() < W && DLG.right() < W && DLG.bottom() < STATUS.y);
        for c in CONTROLS {
            assert!(!c.overlaps(&LIST));
        }
    }

    #[test]
    fn buttons_need_something_to_extract() {
        let l = listing(30);
        let none = vec![false; 30];
        let mut one = none.clone();
        one[3] = true;
        let at = |r: Rect| (r.x + r.w / 2, r.y + r.h / 2);
        let (x, y) = at(TB_ALL);
        assert_eq!(hit(x, y, &view(Some(&l), &none, None)), Hit::ExtractAll);
        assert_eq!(hit(x, y, &view(None, &[], None)), Hit::Nothing);
        let (x, y) = at(TB_SEL);
        assert_eq!(hit(x, y, &view(Some(&l), &none, None)), Hit::Nothing);
        assert_eq!(hit(x, y, &view(Some(&l), &one, None)), Hit::ExtractSelected);
        let (x, y) = at(TB_HERE);
        assert_eq!(hit(x, y, &view(Some(&l), &none, None)), Hit::ExtractHere);
        // Rows, under the headings.
        assert_eq!(hit(ROWS.x + 30, ROWS.y + 1, &view(Some(&l), &none, None)), Hit::Row(0));
        assert_eq!(hit(ROWS.x + 30, ROWS.y + ROW_H * 2 + 3, &view(Some(&l), &none, None)), Hit::Row(2));
        assert_eq!(hit(ROWS.x + 30, HEADER.y + 2, &view(Some(&l), &none, None)), Hit::Nothing);
        let sb = Scrollbar::layout(SCROLL, 0, VISIBLE, 30);
        assert_eq!(hit(sb.down.x + 2, sb.down.y + 2, &view(Some(&l), &none, None)), Hit::ScrollDown);
    }

    #[test]
    fn a_box_takes_the_clicks() {
        let l = listing(3);
        let none = vec![false; 3];
        let d = Dialog::Folder { edit: TextEdit::new("x"), which: None };
        let v = view(Some(&l), &none, Some(&d));
        assert_eq!(hit(TB_ALL.x + 5, TB_ALL.y + 5, &v), Hit::Nothing);
        assert_eq!(hit(DLG_OK.x + 5, DLG_OK.y + 5, &v), Hit::DialogOk);
        assert_eq!(hit(DLG_CANCEL.x + 5, DLG_CANCEL.y + 5, &v), Hit::DialogCancel);
        assert_eq!(hit(DLG_FIELD.x + 5, DLG_FIELD.y + 5, &v), Hit::DialogField);
        let p = Dialog::Progress { title: "x".into(), percent: 5, done: 1, total: 2, current: String::new(), cancelling: false };
        let v = view(Some(&l), &none, Some(&p));
        assert_eq!(hit(DLG_ONLY.x + 5, DLG_ONLY.y + 5, &v), Hit::DialogCancel);
    }

    #[test]
    fn draws_every_state() {
        let l = listing(40);
        let mut sel = vec![false; 40];
        sel[1] = true;
        let mut s = Surface::new(W as usize, H as usize);
        draw(&mut s, &view(Some(&l), &sel, None));
        draw(&mut s, &view(None, &[], None));
        for d in [
            Dialog::Folder { edit: TextEdit::new("C:\\Users\\someone\\a"), which: Some(vec![1]) },
            Dialog::Password { typed: "abc".into(), wrong: true },
            Dialog::Progress { title: "Extracting".into(), percent: 42, done: 4, total: 10, current: "x".into(), cancelling: false },
            Dialog::Message { title: "Archive".into(), text: "A long message that wraps over more than one line of the box, as messages do.".into() },
        ] {
            draw(&mut s, &view(Some(&l), &sel, Some(&d)));
        }
    }
}
