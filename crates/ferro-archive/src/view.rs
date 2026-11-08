//! The archive window, drawn: its buttons, the list of what's inside, the
//! status bar, and the boxes over it (where to, password, progress).

use ferro_archive::create::{Level, NewFormat};
use ferro_archive::{format_time, human_size, Listing};
use ferro_gfx::{color, ellipsize, icons, text_width, wrap, Bevel, Rect, Scrollbar, Surface, GLYPH_H, GLYPH_W, SCROLLBAR_W};
use ferro_ui::TextEdit;

pub const W: i32 = 640;
pub const H: i32 = 440;

pub const TB_ALL: Rect = Rect::new(8, 6, 112, 24);
pub const TB_SEL: Rect = Rect::new(124, 6, 152, 24);
pub const TB_HERE: Rect = Rect::new(280, 6, 104, 24);
pub const TB_ADD: Rect = Rect::new(W - 8 - 144, 6, 144, 24);
/// The toolbar buttons, in Tab order.
pub const CONTROLS: [Rect; 4] = [TB_ALL, TB_SEL, TB_HERE, TB_ADD];

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

/// The Add to archive box: bigger, with its own fields and choices.
pub const ADD: Rect = Rect::new((W - 460) / 2, 48, 460, 336);
const AX: i32 = ADD.x + 12;
pub const ADD_FILES: Rect = Rect::new(AX, ADD.y + 44, ADD.w - 24, 22);
pub const ADD_DEST: Rect = Rect::new(AX, ADD.y + 90, ADD.w - 24, 22);
pub const ADD_7Z: Rect = Rect::new(AX + 64, ADD.y + 122, 150, 16);
pub const ADD_ZIP: Rect = Rect::new(AX + 224, ADD.y + 122, 80, 16);
pub const ADD_STORE: Rect = Rect::new(AX + 104, ADD.y + 148, 72, 16);
pub const ADD_NORMAL: Rect = Rect::new(AX + 184, ADD.y + 148, 80, 16);
pub const ADD_MAX: Rect = Rect::new(AX + 272, ADD.y + 148, 96, 16);
pub const ADD_PW: Rect = Rect::new(AX, ADD.y + 192, ADD.w - 24, 22);
pub const ADD_OK: Rect = Rect::new(ADD.x + ADD.w - 172, ADD.y + ADD.h - 34, 75, 24);
pub const ADD_CANCEL: Rect = Rect::new(ADD.x + ADD.w - 87, ADD.y + ADD.h - 34, 75, 24);
/// Add to the open archive, or make a new one (when one's open that can
/// be added to).
pub const ADD_INTO: Rect = Rect::new(AX, ADD.y + 70, 226, 16);
pub const ADD_NEW: Rect = Rect::new(AX + 232, ADD.y + 70, 150, 16);
/// The Add box's typing fields, in Tab order.
pub const ADD_FIELDS: [Rect; 3] = [ADD_FILES, ADD_DEST, ADD_PW];

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
    /// Add to archive: what, into what, how. `field`: which of
    /// `ADD_FIELDS` has the typing. The password is shown as stars.
    /// `into`: the open archive's name and format, when it can be added to;
    /// `use_into`: whether to (else a new archive, `dest`).
    Add {
        files: TextEdit,
        dest: TextEdit,
        password: String,
        field: usize,
        format: NewFormat,
        level: Level,
        error: Option<String>,
        into: Option<(String, NewFormat)>,
        use_into: bool,
    },
    /// Picking files and folders to add: in `dir`, what's there.
    Pick(Picker),
}

/// One thing in the picker's list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickItem {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

/// The file picker: a folder, what's in it, what's picked.
pub struct Picker {
    pub dir: std::path::PathBuf,
    /// The folder as shown (`C:\Users\...`).
    pub shown: String,
    pub items: Vec<PickItem>,
    pub selected: Vec<bool>,
    pub cursor: usize,
    pub anchor: usize,
    pub scroll: usize,
    /// Why the list is empty, if it couldn't be read.
    pub note: Option<String>,
}

pub const PICK: Rect = Rect::new((W - 500) / 2, 22, 500, 384);
pub const PICK_PATH: Rect = Rect::new(PICK.x + 76, PICK.y + 28, PICK.w - 76 - 12 - 58, 22);
pub const PICK_UP: Rect = Rect::new(PICK.x + PICK.w - 12 - 52, PICK.y + 28, 52, 22);
pub const PICK_LIST: Rect = Rect::new(PICK.x + 12, PICK.y + 58, PICK.w - 24, 250);
const PICK_INNER: Rect = PICK_LIST.inset(2);
pub const PICK_ROWS: Rect = Rect::new(PICK_INNER.x, PICK_INNER.y, PICK_INNER.w - SCROLLBAR_W, PICK_INNER.h);
pub const PICK_SCROLL: Rect = Rect::new(PICK_INNER.x + PICK_INNER.w - SCROLLBAR_W, PICK_INNER.y, SCROLLBAR_W, PICK_INNER.h);
pub const PICK_VISIBLE: usize = (PICK_ROWS.h / ROW_H) as usize;
pub const PICK_OK: Rect = Rect::new(PICK.x + PICK.w - 172, PICK.y + PICK.h - 34, 75, 24);
pub const PICK_CANCEL: Rect = Rect::new(PICK.x + PICK.w - 87, PICK.y + PICK.h - 34, 75, 24);

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
    AddToArchive,
    AddField(usize),
    AddFormat(NewFormat),
    AddLevel(Level),
    AddInto(bool),
    PickUp,
    PickRow(usize),
    PickScroll(isize),
    Nothing,
}

pub fn hit(x: i32, y: i32, v: &View) -> Hit {
    if let Some(d) = v.dialog {
        if let Dialog::Pick(p) = d {
            if PICK_OK.contains(x, y) {
                return Hit::DialogOk;
            }
            if PICK_CANCEL.contains(x, y) {
                return Hit::DialogCancel;
            }
            if PICK_UP.contains(x, y) {
                return Hit::PickUp;
            }
            if PICK_SCROLL.contains(x, y) {
                let sb = Scrollbar::layout(PICK_SCROLL, p.scroll, PICK_VISIBLE, p.items.len());
                return if sb.up.contains(x, y) {
                    Hit::PickScroll(-1)
                } else if sb.down.contains(x, y) {
                    Hit::PickScroll(1)
                } else if y < sb.thumb.y {
                    Hit::PickScroll(-(PICK_VISIBLE as isize))
                } else if y >= sb.thumb.bottom() && sb.thumb.h > 0 {
                    Hit::PickScroll(PICK_VISIBLE as isize)
                } else {
                    Hit::Nothing
                };
            }
            if PICK_ROWS.contains(x, y) {
                let i = p.scroll + ((y - PICK_ROWS.y) / ROW_H) as usize;
                if i < p.items.len() {
                    return Hit::PickRow(i);
                }
            }
            return Hit::Nothing;
        }
        if let Dialog::Add { into, .. } = d {
            if into.is_some() {
                if ADD_INTO.contains(x, y) {
                    return Hit::AddInto(true);
                }
                if ADD_NEW.contains(x, y) {
                    return Hit::AddInto(false);
                }
            }
            let choices = [
                (ADD_OK, Hit::DialogOk),
                (ADD_CANCEL, Hit::DialogCancel),
                (ADD_FILES, Hit::AddField(0)),
                (ADD_DEST, Hit::AddField(1)),
                (ADD_PW, Hit::AddField(2)),
                (ADD_7Z, Hit::AddFormat(NewFormat::SevenZ)),
                (ADD_ZIP, Hit::AddFormat(NewFormat::Zip)),
                (ADD_STORE, Hit::AddLevel(Level::Store)),
                (ADD_NORMAL, Hit::AddLevel(Level::Normal)),
                (ADD_MAX, Hit::AddLevel(Level::Maximum)),
            ];
            return choices.iter().find(|(r, _)| r.contains(x, y)).map_or(Hit::Nothing, |(_, h)| *h);
        }
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
    if TB_ADD.contains(x, y) {
        return Hit::AddToArchive;
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
    ferro_ui::button(s, TB_ADD, "Add to archive...", true);
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

/// A Windows 95 option button: a white round-ish well, a dot when chosen.
fn radio(s: &mut Surface, r: Rect, label: &str, on: bool) {
    let well = Rect::new(r.x, r.y + 2, 12, 12);
    s.fill(well, color::WHITE);
    s.bevel(well, Bevel::Sunken);
    if on {
        s.fill(Rect::new(well.x + 4, well.y + 4, 4, 4), color::BLACK);
    }
    s.text(r.x + 18, r.y + 4, label, color::BLACK);
}

fn draw_add(s: &mut Surface, d: &Dialog) {
    let Dialog::Add { files, dest, password, field, format, level, error, into, use_into } = d else { return };
    s.fill(ADD, color::FACE);
    s.bevel(ADD, Bevel::Window);
    let bar = Rect::new(ADD.x + 3, ADD.y + 3, ADD.w - 6, 18);
    s.fill(bar, color::NAVY);
    s.text_bold(bar.x + 4, bar.y + 5, "Add to archive", color::WHITE);
    // Only the field with the typing shows its caret and selection.
    let edit = |s: &mut Surface, r: Rect, e: &TextEdit, on: bool| {
        if on {
            s.bevel(r, Bevel::Sunken);
            e.draw(s, r.inset(2));
        } else {
            ferro_ui::text_field(s, r, e.text(), false);
        }
    };
    s.text(AX, ADD.y + 32, "Files and folders to add (separate them with ;):", color::BLACK);
    edit(s, ADD_FILES, files, *field == 0);
    match into {
        Some((name, _)) => {
            radio(s, ADD_INTO, &ellipsize(&format!("Add to '{name}'"), ((ADD_INTO.w - 18) / GLYPH_W) as usize), *use_into);
            radio(s, ADD_NEW, "New archive:", !*use_into);
        }
        None => {
            s.text(AX, ADD.y + 78, "Archive to make:", color::BLACK);
        }
    }
    if *use_into {
        // Not used: greyed.
        s.fill(ADD_DEST, color::FACE);
        s.bevel(ADD_DEST, Bevel::Sunken);
        s.text_disabled(ADD_DEST.x + 5, ADD_DEST.y + (ADD_DEST.h - GLYPH_H) / 2, &ellipsize(dest.text(), ((ADD_DEST.w - 12) / GLYPH_W) as usize));
    } else {
        edit(s, ADD_DEST, dest, *field == 1);
    }
    s.text(AX, ADD.y + 126, "Format:", color::BLACK);
    let shown_format = match (into, use_into) {
        (Some((_, f)), true) => *f,
        _ => *format,
    };
    radio(s, ADD_7Z, "7z (smallest)", shown_format == NewFormat::SevenZ);
    radio(s, ADD_ZIP, "zip", shown_format == NewFormat::Zip);
    s.text(AX, ADD.y + 152, "Compression:", color::BLACK);
    radio(s, ADD_STORE, "Store", *level == Level::Store);
    radio(s, ADD_NORMAL, "Normal", *level == Level::Normal);
    radio(s, ADD_MAX, "Maximum", *level == Level::Maximum);
    s.text(AX, ADD.y + 180, "Password (optional):", color::BLACK);
    ferro_ui::text_field(s, ADD_PW, &"*".repeat(password.chars().count()), *field == 2);
    let note = match (shown_format, *use_into) {
        (NewFormat::SevenZ, false) => "Password: AES-256, and the file names are hidden too.",
        (NewFormat::Zip, false) => "Password: AES-256; the file names can still be read.",
        (NewFormat::SevenZ, true) => "The archive is saved again whole, with this password.",
        (NewFormat::Zip, true) => "New files get this password (AES-256); the rest stay.",
    };
    s.text(AX, ADD.y + 222, note, color::BLACK);
    s.text(AX, ADD.y + 236, "Only names, contents and times go in: no owners.", color::BLACK);
    if let Some(e) = error {
        for (i, line) in wrap(e, ((ADD.w - 24) / GLYPH_W) as usize).iter().take(2).enumerate() {
            s.text(AX, ADD.y + 256 + i as i32 * 12, line, color::RED);
        }
    }
    ferro_ui::default_button(s, ADD_OK, "OK", true);
    ferro_ui::button(s, ADD_CANCEL, "Cancel", true);
}

fn draw_pick(s: &mut Surface, p: &Picker) {
    s.fill(PICK, color::FACE);
    s.bevel(PICK, Bevel::Window);
    let bar = Rect::new(PICK.x + 3, PICK.y + 3, PICK.w - 6, 18);
    s.fill(bar, color::NAVY);
    s.text_bold(bar.x + 4, bar.y + 5, "Add to archive: pick files and folders", color::WHITE);
    s.text(PICK.x + 12, PICK_PATH.y + 7, "Look in:", color::BLACK);
    let room = ((PICK_PATH.w - 12) / GLYPH_W) as usize;
    // The end of a long folder name is the part that tells them apart.
    let n = p.shown.chars().count();
    let shown = if n > room { format!("...{}", p.shown.chars().skip(n + 3 - room).collect::<String>()) } else { p.shown.clone() };
    ferro_ui::text_field(s, PICK_PATH, &shown, false);
    ferro_ui::button(s, PICK_UP, "Up", p.dir.parent().is_some());
    s.fill(PICK_LIST, color::WHITE);
    s.bevel(PICK_LIST, Bevel::Sunken);
    let sb = Scrollbar::layout(PICK_SCROLL, p.scroll, PICK_VISIBLE, p.items.len());
    s.scrollbar(&sb, false, false);
    s.set_clip(Some(PICK_ROWS));
    for (row, it) in p.items.iter().enumerate().skip(p.scroll).take(PICK_VISIBLE) {
        let y = PICK_ROWS.y + (row - p.scroll) as i32 * ROW_H;
        let ty = y + (ROW_H - GLYPH_H) / 2;
        let sel = p.selected.get(row).copied().unwrap_or(false);
        s.sprite(PICK_ROWS.x + 3, y + 1, if it.is_dir { icons::FOLDER } else { icons::FILE }, 1);
        let name = ellipsize(&it.name, ((PICK_ROWS.w - 22 - 96) / GLYPH_W) as usize);
        let label_x = PICK_ROWS.x + 22;
        if sel {
            s.fill(Rect::new(label_x - 2, y + 1, text_width(&name) + 4, ROW_H - 2), color::NAVY);
        }
        s.text(label_x, ty, &name, if sel { color::WHITE } else { color::BLACK });
        if row == p.cursor {
            ferro_ui::focus_rect(s, Rect::new(label_x - 2, y + 1, text_width(&name) + 4, ROW_H - 2));
        }
        if !it.is_dir {
            text_right(s, PICK_ROWS.right() - 6, ty, &human_size(it.size), color::BLACK);
        }
    }
    s.set_clip(None);
    if let Some(note) = &p.note {
        s.text(PICK_ROWS.x + 8, PICK_ROWS.y + 8, &ellipsize(note, ((PICK_ROWS.w - 16) / GLYPH_W) as usize), color::SHADOW);
    } else if p.items.is_empty() {
        s.text(PICK_ROWS.x + 8, PICK_ROWS.y + 8, "This folder is empty.", color::SHADOW);
    }
    let picked = p.selected.iter().filter(|s| **s).count();
    let line =
        if picked == 0 { "Click to pick; Ctrl or Shift to pick more.".to_owned() } else { format!("{picked} picked. Ctrl or Shift to pick more.") };
    s.text(PICK.x + 12, PICK_LIST.bottom() + 8, &line, color::BLACK);
    s.text(PICK.x + 12, PICK_LIST.bottom() + 20, "Double-click a folder to open it.", color::BLACK);
    ferro_ui::default_button(s, PICK_OK, "Add...", picked > 0);
    ferro_ui::button(s, PICK_CANCEL, "Cancel", true);
}

fn draw_dialog(s: &mut Surface, d: &Dialog, name: &str) {
    if let Dialog::Add { .. } = d {
        return draw_add(s, d);
    }
    if let Dialog::Pick(p) = d {
        return draw_pick(s, p);
    }
    s.fill(DLG, color::FACE);
    s.bevel(DLG, Bevel::Window);
    let bar = Rect::new(DLG.x + 3, DLG.y + 3, DLG.w - 6, 18);
    s.fill(bar, color::NAVY);
    let title = match d {
        Dialog::Folder { .. } => "Extract",
        Dialog::Password { .. } => "Password",
        Dialog::Progress { title, .. } | Dialog::Message { title, .. } => title,
        Dialog::Add { .. } | Dialog::Pick(_) => "Add to archive",
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
        Dialog::Progress { title, percent, done, total, current, cancelling } => {
            // "Extracting x.txt", "Adding...", "Opening x.jpg".
            let what = if !current.is_empty() {
                current.as_str()
            } else if title == "Extracting" {
                name
            } else {
                "..."
            };
            let line = if *cancelling { "Cancelling...".to_owned() } else { format!("{title} {what}").replace(" ...", "...") };
            s.text(x, DLG.y + 34, &ellipsize(&line, room), color::BLACK);
            s.text(x, DLG.y + 50, &format!("{} of {}", human_size(*done), human_size(*total)), color::BLACK);
            progress_bar(s, BAR, *percent);
            ferro_ui::button(s, DLG_ONLY, "Cancel", !cancelling);
        }
        Dialog::Add { .. } | Dialog::Pick(_) => {}
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
            Dialog::Add {
                files: TextEdit::new("C:\\Users\\someone\\Trip"),
                dest: TextEdit::new("C:\\Users\\someone\\Trip.7z"),
                password: "pw".into(),
                field: 2,
                format: NewFormat::SevenZ,
                level: Level::Normal,
                error: Some("There's already something by that name.".into()),
                into: Some(("Trip.7z".into(), NewFormat::SevenZ)),
                use_into: true,
            },
            Dialog::Pick(Picker {
                dir: "/".into(),
                shown: "C:\\Users\\someone\\a very long folder name that goes on and on".into(),
                items: vec![PickItem { name: "Photos".into(), is_dir: true, size: 0 }, PickItem { name: "a.txt".into(), is_dir: false, size: 12 }],
                selected: vec![true, false],
                cursor: 1,
                anchor: 0,
                scroll: 0,
                note: None,
            }),
        ] {
            draw(&mut s, &view(Some(&l), &sel, Some(&d)));
        }
    }
}

#[cfg(test)]
mod add_tests {
    use super::*;

    #[test]
    fn the_add_box_fits_and_answers() {
        assert!(ADD.bottom() < STATUS.y && ADD.right() < W);
        assert!(!TB_ADD.overlaps(&TB_HERE));
        let rows = [ADD_FILES, ADD_INTO, ADD_NEW, ADD_DEST, ADD_7Z, ADD_ZIP, ADD_STORE, ADD_NORMAL, ADD_MAX, ADD_PW, ADD_OK, ADD_CANCEL];
        for (i, a) in rows.iter().enumerate() {
            assert!(ADD.contains(a.x, a.y) && ADD.contains(a.right() - 1, a.bottom() - 1), "{i}");
            for b in &rows[i + 1..] {
                assert!(!a.overlaps(b), "{a:?} {b:?}");
            }
        }
        let d = Dialog::Add {
            files: TextEdit::new(""),
            dest: TextEdit::new(""),
            password: String::new(),
            field: 0,
            format: NewFormat::SevenZ,
            level: Level::Normal,
            error: None,
            into: Some(("a.zip".into(), NewFormat::Zip)),
            use_into: true,
        };
        let v = View { name: "", listing: None, selected: &[], cursor: 0, scroll: 0, focus: None, status: "", dialog: Some(&d) };
        assert_eq!(hit(ADD_ZIP.x + 3, ADD_ZIP.y + 3, &v), Hit::AddFormat(NewFormat::Zip));
        assert_eq!(hit(ADD_MAX.x + 3, ADD_MAX.y + 3, &v), Hit::AddLevel(Level::Maximum));
        assert_eq!(hit(ADD_PW.x + 3, ADD_PW.y + 3, &v), Hit::AddField(2));
        assert_eq!(hit(ADD_OK.x + 3, ADD_OK.y + 3, &v), Hit::DialogOk);
        assert_eq!(hit(ADD_NEW.x + 3, ADD_NEW.y + 3, &v), Hit::AddInto(false));
        let none = View { dialog: None, ..v };
        assert_eq!(hit(TB_ADD.x + 3, TB_ADD.y + 3, &none), Hit::AddToArchive);
    }
}

#[cfg(test)]
mod pick_tests {
    use super::*;

    #[test]
    fn the_picker_fits_and_answers() {
        assert!(PICK.bottom() < STATUS.y && PICK.right() < W && PICK.y > 0);
        assert!(PICK_LIST.bottom() + 28 < PICK_OK.y);
        assert!(!PICK_PATH.overlaps(&PICK_UP) && PICK_UP.right() <= PICK.right() - 8);
        const { assert!(PICK_VISIBLE >= 12) };
        let p = Picker {
            dir: "/x".into(),
            shown: String::new(),
            items: (0..30).map(|i| PickItem { name: format!("f{i}"), is_dir: false, size: 1 }).collect(),
            selected: vec![false; 30],
            cursor: 0,
            anchor: 0,
            scroll: 0,
            note: None,
        };
        let d = Dialog::Pick(p);
        let v = View { name: "", listing: None, selected: &[], cursor: 0, scroll: 0, focus: None, status: "", dialog: Some(&d) };
        assert_eq!(hit(PICK_ROWS.x + 30, PICK_ROWS.y + ROW_H + 2, &v), Hit::PickRow(1));
        assert_eq!(hit(PICK_UP.x + 3, PICK_UP.y + 3, &v), Hit::PickUp);
        assert_eq!(hit(PICK_OK.x + 3, PICK_OK.y + 3, &v), Hit::DialogOk);
        assert_eq!(hit(PICK_CANCEL.x + 3, PICK_CANCEL.y + 3, &v), Hit::DialogCancel);
        let sb = Scrollbar::layout(PICK_SCROLL, 0, PICK_VISIBLE, 30);
        assert_eq!(hit(sb.down.x + 2, sb.down.y + 2, &v), Hit::PickScroll(1));
    }
}
