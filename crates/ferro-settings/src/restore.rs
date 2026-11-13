//! System Restore (ferro-settings --restore): the restore points, newest
//! first, and what can be done with each: go back to it, say what it is,
//! delete it; and a new one made now. The work is ferro-restore's (as root):
//! this window reads its index, which says only each point's name and
//! description, and asks it to do things.
//!
//! Making a point or renaming one needs no password (sudo has a rule for
//! those); deleting one, or going back to it, asks for it. While it works
//! the window says so, and afterwards what came of it.

use ferro_gfx::{color, ellipsize, icons, Bevel, Rect, Surface, GLYPH_H, GLYPH_W};
use ferro_ui::{admin, button, default_button, Edit, Key, Keymap, Res, TextEdit};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use std::process::{Command, Stdio};
use std::time::Duration;

const TOOL: &str = "/usr/local/sbin/ferro-restore";
const INDEX: &str = "/var/lib/ferro/restore.index";

const W: i32 = 600;
const H: i32 = 420;
const ROW_H: i32 = 20;
const LIST: Rect = Rect::new(16, 76, W - 32, 248);
/// The rows, below the column names.
const ROWS: Rect = Rect::new(LIST.x + 2, LIST.y + 22, LIST.w - 4, LIST.h - 24);
const BAR_Y: i32 = LIST.y + LIST.h + 10;
const BTN_W: i32 = 96;
const CREATE_BTN: Rect = Rect::new(16, BAR_Y, BTN_W, 24);
const RENAME_BTN: Rect = Rect::new(16 + (BTN_W + 8), BAR_Y, BTN_W, 24);
const DELETE_BTN: Rect = Rect::new(16 + 2 * (BTN_W + 8), BAR_Y, BTN_W, 24);
const RESTORE_BTN: Rect = Rect::new(W - 16 - BTN_W - 16, BAR_Y, BTN_W + 16, 24);
const CLOSE_BTN: Rect = Rect::new(W - 91, H - 36, 75, 24);
/// Typing what a point is: the box, and its OK and Cancel.
const FIELD: Rect = Rect::new(120, BAR_Y + 1, W - 120 - 16 - 2 * 83, 22);
const OK_BTN: Rect = Rect::new(W - 16 - 75 - 83, BAR_Y, 75, 24);
const CANCEL_BTN: Rect = Rect::new(W - 16 - 75, BAR_Y, 75, 24);
const MENU_W: i32 = 120;
const MENU: [&str; 3] = ["Restore...", "Rename...", "Delete"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Point {
    pub id: String,
    pub why: String,
}

impl Point {
    /// 20261113-080502 as 2026-11-13 08:05.
    pub fn when(&self) -> String {
        let d = self.id.as_bytes();
        if d.len() < 13 {
            return self.id.clone();
        }
        let s = |a: usize, b: usize| String::from_utf8_lossy(&d[a..b]).into_owned();
        format!("{}-{}-{} {}:{}", s(0, 4), s(4, 6), s(6, 8), s(9, 11), s(11, 13))
    }
}

/// Whether `id` is a point's name as ferro-restore makes them, and nothing
/// else (it goes on a command line).
pub fn is_id(id: &str) -> bool {
    let b = id.as_bytes();
    b.len() == 15 && b[8] == b'-' && b.iter().enumerate().all(|(i, c)| i == 8 || c.is_ascii_digit())
}

/// The index: `id<TAB>description` a line, newest first.
pub fn parse(index: &str) -> Vec<Point> {
    index
        .lines()
        .filter_map(|l| {
            let (id, why) = l.split_once('\t').unwrap_or((l, ""));
            is_id(id).then(|| Point { id: id.to_owned(), why: why.trim().to_owned() })
        })
        .collect()
}

fn read_points() -> Vec<Point> {
    parse(&std::fs::read_to_string(INDEX).unwrap_or_default())
}

/// What's typed for a description: one line, no control characters, 80 at most.
pub fn clean(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).take(80).collect::<String>().trim().to_owned()
}

enum Mode {
    List,
    /// Typing a description: for a new point (None), or this one.
    Typing(Option<usize>, TextEdit),
    /// Asking whether to delete this one, or go back to it.
    Confirm(Ask, usize),
    /// The right-click menu, at its top-left, for this one.
    Menu(i32, i32, usize),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ask {
    Delete,
    Restore,
}

/// Where what ferro-restore says goes, to tell the last of it after.
fn log_path() -> std::path::PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir).join("ferro-restore.log")
}

/// ferro-restore started as root: without a password where sudo's rule
/// allows it (and `ask` isn't set), else asking for it in the password box.
fn start(args: &[&str], ask: bool) -> Option<std::process::Child> {
    let allowed = !ask
        && Command::new("/usr/bin/sudo")
            .args(["-n", "-l", TOOL])
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
    let log = std::fs::File::create(log_path()).ok()?;
    let err = log.try_clone().ok()?;
    admin::command(TOOL, args, if allowed { None } else { Some("System Restore") }).stdout(log).stderr(err).spawn().ok()
}

/// The last thing ferro-restore said, as one line.
fn last_said() -> String {
    std::fs::read_to_string(log_path()).unwrap_or_default().lines().rev().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").to_owned()
}

fn row_at(points: &[Point], x: i32, y: i32) -> Option<usize> {
    if !ROWS.contains(x, y) {
        return None;
    }
    let i = ((y - ROWS.y) / ROW_H) as usize;
    (i < points.len()).then_some(i)
}

fn menu_rect(x: i32, y: i32) -> Rect {
    let h = MENU.len() as i32 * ROW_H + 6;
    Rect::new(x.min(W - MENU_W - 2), y.min(H - h - 2), MENU_W, h)
}

fn draw(s: &mut Surface, points: &[Point], selected: Option<usize>, mode: &Mode, status: &str) {
    s.fill(Rect::new(0, 0, W, H), color::FACE);
    s.sprite(16, 14, icons::HELP, 2);
    s.text(60, 14, "Restore points are made before every update and before adding", color::BLACK);
    s.text(60, 28, "programs. Going back to one undoes what changed in the system", color::BLACK);
    s.text(60, 42, "since then. Your own files are never touched.", color::BLACK);

    s.fill(LIST, color::WHITE);
    s.bevel(LIST, Bevel::Sunken);
    let head = Rect::new(LIST.x + 2, LIST.y + 2, LIST.w - 4, 20);
    s.fill(head, color::FACE);
    s.bevel(Rect::new(head.x, head.y, 140, head.h), Bevel::Raised);
    s.bevel(Rect::new(head.x + 140, head.y, head.w - 140, head.h), Bevel::Raised);
    s.text(head.x + 6, head.y + 6, "Date and time", color::BLACK);
    s.text(head.x + 146, head.y + 6, "Description", color::BLACK);
    if points.is_empty() {
        s.text(ROWS.x + 6, ROWS.y + 8, "No restore points yet. Create makes one now.", color::SHADOW);
    }
    let room = ((ROWS.w - 150) / GLYPH_W).max(1) as usize;
    for (i, p) in points.iter().enumerate().take((ROWS.h / ROW_H) as usize) {
        let r = Rect::new(ROWS.x, ROWS.y + i as i32 * ROW_H, ROWS.w, ROW_H);
        let ink = if selected == Some(i) {
            s.fill(r, color::NAVY);
            color::WHITE
        } else {
            color::BLACK
        };
        let ty = r.y + (ROW_H - GLYPH_H) / 2;
        s.text(r.x + 4, ty, &p.when(), ink);
        s.text(r.x + 144, ty, &ellipsize(&p.why, room), ink);
    }

    let chosen = selected.is_some_and(|i| i < points.len());
    match mode {
        Mode::Typing(which, edit) => {
            s.text(16, BAR_Y + 8, if which.is_some() { "Description:" } else { "New point:" }, color::BLACK);
            edit.draw(s, FIELD);
            s.bevel(FIELD, Bevel::Sunken);
            default_button(s, OK_BTN, "OK", true);
            button(s, CANCEL_BTN, "Cancel", true);
        }
        Mode::Confirm(ask, i) => {
            let when = points.get(*i).map(Point::when).unwrap_or_default();
            let (q, yes) = match ask {
                Ask::Delete => (format!("Delete the restore point of {when}?"), "Delete"),
                Ask::Restore => (format!("Go back to the point of {when}?"), "Restore"),
            };
            s.text(16, BAR_Y + 8, &q, color::BLACK);
            default_button(s, OK_BTN, yes, true);
            button(s, CANCEL_BTN, "Cancel", true);
        }
        Mode::List | Mode::Menu(..) => {
            button(s, CREATE_BTN, "Create...", true);
            button(s, RENAME_BTN, "Rename...", chosen);
            button(s, DELETE_BTN, "Delete", chosen);
            default_button(s, RESTORE_BTN, "Restore...", chosen);
        }
    }
    s.text(16, H - 28, &ellipsize(status, ((W - 120) / GLYPH_W) as usize), color::BLACK);
    button(s, CLOSE_BTN, "Close", true);

    if let Mode::Menu(x, y, _) = mode {
        let m = menu_rect(*x, *y);
        s.fill(m, color::FACE);
        s.bevel(m, Bevel::Window);
        for (n, label) in MENU.iter().enumerate() {
            s.text(m.x + 18, m.y + 3 + n as i32 * ROW_H + (ROW_H - GLYPH_H) / 2, label, color::BLACK);
        }
    }
}

pub fn run() -> Res<()> {
    let (conn, n) = RustConnection::connect(None)?;
    let screen = conn.setup().roots[n].clone();
    let (win, gc) = (conn.generate_id()?, conn.generate_id()?);
    let (sw, sh) = (i32::from(screen.width_in_pixels), i32::from(screen.height_in_pixels));
    let mask = EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::KEY_PRESS | EventMask::FOCUS_CHANGE | EventMask::STRUCTURE_NOTIFY;
    conn.create_window(
        screen.root_depth,
        win,
        screen.root,
        ((sw - W) / 2) as i16,
        ((sh - H) / 2) as i16,
        W as u16,
        H as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &CreateWindowAux::new().background_pixel(color::FACE).event_mask(mask),
    )?;
    conn.create_gc(gc, win, &CreateGCAux::new())?;
    let atom = |name: &[u8]| -> Res<Atom> { Ok(conn.intern_atom(false, name)?.reply()?.atom) };
    let (wm_protocols, wm_delete) = (atom(b"WM_PROTOCOLS")?, atom(b"WM_DELETE_WINDOW")?);
    let title = "System Restore";
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;
    conn.change_property8(PropMode::REPLACE, win, atom(b"_NET_WM_NAME")?, atom(b"UTF8_STRING")?, title.as_bytes())?;
    conn.change_property32(PropMode::REPLACE, win, wm_protocols, AtomEnum::ATOM, &[wm_delete])?;
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, b"ferro-settings\0ferro-settings\0")?;
    ferro_ui::fixed_size(&conn, win, W, H)?;
    conn.map_window(win)?;
    conn.flush()?;
    let keymap = Keymap::load(&conn)?;

    let mut points = read_points();
    let mut selected = (!points.is_empty()).then_some(0);
    let mut mode = Mode::List;
    let mut status = String::new();
    let mut s = Surface::new(W as usize, H as usize);
    macro_rules! paint {
        () => {{
            draw(&mut s, &points, selected, &mode, &status);
            ferro_ui::show(&conn, win, gc, screen.root_depth, &s, W, H)?;
        }};
    }
    // What's there now, the same one chosen if it's still there.
    macro_rules! reload {
        () => {{
            let was = selected.and_then(|i| points.get(i)).map(|p| p.id.clone());
            points = read_points();
            selected = was.and_then(|id| points.iter().position(|p| p.id == id)).or((!points.is_empty()).then_some(0));
        }};
    }
    // Something done (the tool's word for it, and what to say).
    macro_rules! act {
        ($args:expr, $ask:expr, $doing:expr, $done:expr) => {{
            mode = Mode::List;
            status = $doing.to_owned();
            paint!();
            conn.flush()?;
            // Waited for, the window drawn again when it's uncovered; what's
            // clicked meanwhile doesn't count.
            let ok = match start($args, $ask) {
                Some(mut child) => loop {
                    if let Ok(Some(st)) = child.try_wait() {
                        break st.success();
                    }
                    while let Some(ev) = conn.poll_for_event()? {
                        if matches!(ev, Event::Expose(ref e) if e.count == 0) {
                            paint!();
                        }
                    }
                    std::thread::sleep(Duration::from_millis(60));
                },
                None => false,
            };
            reload!();
            let said = last_said();
            status = if ok {
                $done.to_owned()
            } else if said.is_empty() || said.starts_with("sudo:") {
                "That didn't work (no permission, or it was cancelled).".to_owned()
            } else {
                said
            };
            paint!();
        }};
    }
    // OK on what was typed: a new point, or this one's description.
    macro_rules! submit {
        ($which:expr, $typed:expr) => {{
            let text = clean($typed);
            match $which {
                Some(i) if !text.is_empty() => {
                    let id = points[i].id.clone();
                    act!(&["rename", &id, &text], false, "Renaming...", "Renamed.");
                }
                Some(_) => {}
                None => {
                    let why = if text.is_empty() { "Made by hand".to_owned() } else { text };
                    act!(&["create", &why], false, "Making a restore point (this can take a minute)...", "Restore point made.");
                    selected = (!points.is_empty()).then_some(0);
                    paint!();
                }
            }
        }};
    }
    // Asked first: what going back does is said below the question.
    macro_rules! ask_restore {
        ($i:expr) => {{
            mode = Mode::Confirm(Ask::Restore, $i);
            status = "Programs and settings go back to then; your files stay as they are.".to_owned();
            paint!();
        }};
    }
    // Yes to the question.
    macro_rules! confirmed {
        ($ask:expr, $i:expr) => {{
            let id = points[$i].id.clone();
            match $ask {
                Ask::Delete => act!(&["delete", &id], true, "Deleting...", "Deleted."),
                Ask::Restore => act!(
                    &["restore", &id, "--yes"],
                    true,
                    "Going back to that point: this can take several minutes. Don't turn the computer off.",
                    "Done. Restart the computer to finish going back."
                ),
            }
        }};
    }

    loop {
        match conn.wait_for_event()? {
            Event::Expose(e) if e.count == 0 => paint!(),
            Event::MapNotify(_) => {
                let _ = conn.set_input_focus(InputFocus::PARENT, win, x11rb::CURRENT_TIME);
                conn.flush()?;
            }
            // Back from the terminal: a point may have been made.
            Event::FocusIn(_) if matches!(mode, Mode::List) => {
                reload!();
                paint!();
            }
            Event::ClientMessage(cm) if cm.data.as_data32()[0] == wm_delete => return Ok(()),
            Event::ButtonPress(b) => {
                let (x, y) = (i32::from(b.event_x), i32::from(b.event_y));
                if let Mode::Menu(mx, my, i) = mode {
                    mode = Mode::List;
                    let m = menu_rect(mx, my);
                    if b.detail == 1 && m.contains(x, y) {
                        match ((y - m.y - 3) / ROW_H).max(0) {
                            0 => ask_restore!(i),
                            1 => mode = Mode::Typing(Some(i), TextEdit::new(&points[i].why)),
                            _ => mode = Mode::Confirm(Ask::Delete, i),
                        }
                    }
                    paint!();
                    continue;
                }
                if CLOSE_BTN.contains(x, y) && b.detail == 1 {
                    return Ok(());
                }
                match &mut mode {
                    Mode::Typing(which, edit) => {
                        let which = *which;
                        if FIELD.contains(x, y) {
                            edit.click(FIELD, x, false);
                            paint!();
                        } else if OK_BTN.contains(x, y) {
                            let typed = edit.text().to_owned();
                            submit!(which, &typed);
                        } else if CANCEL_BTN.contains(x, y) {
                            mode = Mode::List;
                            paint!();
                        }
                    }
                    Mode::Confirm(ask, i) => {
                        let (ask, i) = (*ask, *i);
                        if OK_BTN.contains(x, y) {
                            confirmed!(ask, i);
                        } else if CANCEL_BTN.contains(x, y) {
                            mode = Mode::List;
                            status.clear();
                            paint!();
                        }
                    }
                    Mode::List | Mode::Menu(..) => {
                        if let Some(i) = row_at(&points, x, y) {
                            selected = Some(i);
                            if b.detail == 3 {
                                mode = Mode::Menu(x, y, i);
                            }
                            paint!();
                        } else if b.detail == 1 && CREATE_BTN.contains(x, y) {
                            mode = Mode::Typing(None, TextEdit::new("Made by hand"));
                            paint!();
                        } else if let Some(i) = selected.filter(|&i| b.detail == 1 && i < points.len()) {
                            if RENAME_BTN.contains(x, y) {
                                mode = Mode::Typing(Some(i), TextEdit::new(&points[i].why));
                                paint!();
                            } else if DELETE_BTN.contains(x, y) {
                                mode = Mode::Confirm(Ask::Delete, i);
                                paint!();
                            } else if RESTORE_BTN.contains(x, y) {
                                ask_restore!(i);
                            }
                        }
                    }
                }
            }
            Event::KeyPress(k) => {
                let key = keymap.typed(&k);
                let state = u16::from(k.state);
                let (ctrl, shift) = (state & u16::from(KeyButMask::CONTROL) != 0, state & u16::from(KeyButMask::SHIFT) != 0);
                match &mut mode {
                    Mode::Typing(which, edit) => {
                        let which = *which;
                        match edit.key(key, ctrl, shift) {
                            Edit::Changed => paint!(),
                            Edit::Cancel => {
                                mode = Mode::List;
                                paint!();
                            }
                            Edit::Done => {
                                let typed = edit.text().to_owned();
                                submit!(which, &typed);
                            }
                            Edit::Ignored => {}
                        }
                    }
                    Mode::Confirm(ask, i) => {
                        let (ask, i) = (*ask, *i);
                        match key {
                            Key::Enter => confirmed!(ask, i),
                            Key::Escape => {
                                mode = Mode::List;
                                status.clear();
                                paint!();
                            }
                            _ => {}
                        }
                    }
                    Mode::Menu(..) => {
                        mode = Mode::List;
                        paint!();
                    }
                    Mode::List => match key {
                        Key::Escape => return Ok(()),
                        Key::Up | Key::Down if !points.is_empty() => {
                            let i = selected.unwrap_or(0);
                            selected = Some(if key == Key::Up { i.saturating_sub(1) } else { (i + 1).min(points.len() - 1) });
                            paint!();
                        }
                        Key::Delete => {
                            if let Some(i) = selected.filter(|&i| i < points.len()) {
                                mode = Mode::Confirm(Ask::Delete, i);
                                paint!();
                            }
                        }
                        _ => {}
                    },
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_index() {
        let pts = parse("20261113-080502\tBefore FerroOS update\n../etc\tbad\n20261112-1\tshort\n20261101-120000\n");
        assert_eq!(pts.len(), 2);
        assert_eq!(pts[0], Point { id: "20261113-080502".into(), why: "Before FerroOS update".into() });
        assert_eq!(pts[0].when(), "2026-11-13 08:05");
        assert_eq!(pts[1].why, "");
    }

    #[test]
    fn only_real_ids() {
        assert!(is_id("20261113-080502"));
        for bad in ["", "2026111-080502", "20261113-08050x", "--20261113-08", "20261113_080502", "../../etc/x"] {
            assert!(!is_id(bad), "{bad}");
        }
    }

    #[test]
    fn descriptions_are_one_short_line() {
        assert_eq!(clean("  Before\tthe\nupdate  "), "Beforetheupdate");
        assert_eq!(clean(&"x".repeat(200)).len(), 80);
    }

    #[test]
    fn fits() {
        for line in ["Restore points are made before every update and before adding", "programs. Going back to one undoes what changed in the system"]
        {
            assert!(60 + line.len() as i32 * GLYPH_W <= W - 10);
        }
        assert!(DELETE_BTN.right() < RESTORE_BTN.x && FIELD.right() < OK_BTN.x, "buttons overlap");
        const { assert!(BAR_Y + 24 < CLOSE_BTN.y) };
    }
}
