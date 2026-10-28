//! ferro-clipboard: the clipboard history, as Windows+V has it. The text
//! copied lately (the last 25 pieces), kept in this program's memory only:
//! never written down, gone at log off. What a password manager copies (it
//! marks it so) isn't kept at all.
//!
//!     ferro-clipboard --daemon   keeps the history (the desktop starts it)
//!     ferro-clipboard --pick     shows it: a click puts that back on the
//!                                clipboard, for Ctrl+V

use ferro_gfx::{color, ellipsize, Bevel, Rect, Surface, GLYPH_H, GLYPH_W};
use ferro_ui::{Key, Keymap, Res};
use x11rb::connection::Connection;
use x11rb::protocol::xfixes::{self, ConnectionExt as _};
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

const KEEP: usize = 25;
/// Bigger than this isn't kept (a whole document copied, say).
const MOST: usize = 64 * 1024;
const W: i32 = 360;
const ROW_H: i32 = 26;
const ROWS: usize = 12;
const H: i32 = 14 + ROWS as i32 * ROW_H + 44;

struct Atoms {
    clipboard: Atom,
    targets: Atom,
    utf8: Atom,
    text: Atom,
    incoming: Atom,
    password_hint: Atom,
    daemon: Atom,
    pick: Atom,
    wm_protocols: Atom,
    wm_delete: Atom,
}

impl Atoms {
    fn new(conn: &RustConnection) -> Res<Self> {
        let a = |n: &[u8]| -> Res<Atom> { Ok(conn.intern_atom(false, n)?.reply()?.atom) };
        Ok(Atoms {
            clipboard: a(b"CLIPBOARD")?,
            targets: a(b"TARGETS")?,
            utf8: a(b"UTF8_STRING")?,
            text: a(b"TEXT")?,
            incoming: a(b"_FERRO_CLIPBOARD_IN")?,
            password_hint: a(b"x-kde-passwordManagerHint")?,
            daemon: a(b"_FERRO_CLIPBOARD")?,
            pick: a(b"_FERRO_CLIPBOARD_PICK")?,
            wm_protocols: a(b"WM_PROTOCOLS")?,
            wm_delete: a(b"WM_DELETE_WINDOW")?,
        })
    }
}

/// `text` at the front of `history`: once only, the newest first, KEEP at most.
fn remember(history: &mut Vec<String>, text: String) {
    if text.trim().is_empty() || text.len() > MOST {
        return;
    }
    history.retain(|t| *t != text);
    history.insert(0, text);
    history.truncate(KEEP);
}

/// One line for the list: the text's first line, its whitespace tidied.
fn shown(text: &str) -> String {
    let line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.lines().count() > 1 {
        format!("{line} ...")
    } else {
        line
    }
}

/// The history's window, if a daemon is running (it says so on the root).
fn daemon_window(conn: &RustConnection, root: Window, atoms: &Atoms) -> Option<Window> {
    let reply = conn.get_property(false, root, atoms.daemon, AtomEnum::WINDOW, 0, 1).ok()?.reply().ok()?;
    let win = reply.value32()?.next()?;
    // Still there?
    conn.get_window_attributes(win).ok()?.reply().ok().map(|_| win)
}

fn main() -> Res<()> {
    let mode = std::env::args().nth(1).unwrap_or_default();
    let (conn, n) = RustConnection::connect(None)?;
    let screen = conn.setup().roots[n].clone();
    let atoms = Atoms::new(&conn)?;
    match mode.as_str() {
        "--daemon" => {
            if daemon_window(&conn, screen.root, &atoms).is_some() {
                return Ok(());
            }
            Daemon::new(conn, screen, atoms)?.run()
        }
        "--pick" => match daemon_window(&conn, screen.root, &atoms) {
            Some(win) => {
                let msg = ClientMessageEvent::new(32, win, atoms.pick, [0u32; 5]);
                conn.send_event(false, win, EventMask::NO_EVENT, msg)?;
                conn.flush()?;
                Ok(())
            }
            None => {
                eprintln!("ferro-clipboard: the clipboard history isn't running");
                Ok(())
            }
        },
        _ => {
            eprintln!("usage: ferro-clipboard --daemon | --pick");
            std::process::exit(2);
        }
    }
}

struct Daemon {
    conn: RustConnection,
    screen: Screen,
    atoms: Atoms,
    /// Never shown: owns the clipboard when something is put back, and
    /// receives what's copied.
    hidden: Window,
    history: Vec<String>,
    /// What this program hands out as the clipboard, once something's picked.
    owned: Option<String>,
    picker: Option<Picker>,
    keymap: Keymap,
}

struct Picker {
    win: Window,
    gc: Gcontext,
    hover: Option<usize>,
    scroll: usize,
}

impl Daemon {
    fn new(conn: RustConnection, screen: Screen, atoms: Atoms) -> Res<Self> {
        let hidden = conn.generate_id()?;
        conn.create_window(
            0,
            hidden,
            screen.root,
            -10,
            -10,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            0,
            &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )?;
        conn.change_property32(PropMode::REPLACE, screen.root, atoms.daemon, AtomEnum::WINDOW, &[hidden])?;
        conn.xfixes_query_version(5, 0)?.reply()?;
        conn.xfixes_select_selection_input(hidden, atoms.clipboard, xfixes::SelectionEventMask::SET_SELECTION_OWNER)?;
        conn.flush()?;
        let keymap = Keymap::load(&conn)?;
        Ok(Daemon { conn, screen, atoms, hidden, history: Vec::new(), owned: None, picker: None, keymap })
    }

    fn run(mut self) -> Res<()> {
        loop {
            let event = self.conn.wait_for_event()?;
            self.handle(event)?;
            self.conn.flush()?;
        }
    }

    fn handle(&mut self, event: Event) -> Res<()> {
        match event {
            // Something else was copied: ask what it is (a password? text?).
            Event::XfixesSelectionNotify(e) if e.owner != self.hidden && e.owner != x11rb::NONE => {
                self.owned = None;
                self.conn.convert_selection(self.hidden, self.atoms.clipboard, self.atoms.targets, self.atoms.incoming, e.timestamp)?;
            }
            Event::SelectionNotify(e) if e.requestor == self.hidden && e.property != x11rb::NONE => {
                let reply = self.conn.get_property(true, self.hidden, self.atoms.incoming, AtomEnum::ANY, 0, (MOST / 4 + 1) as u32)?.reply()?;
                if e.target == self.atoms.targets {
                    let targets: Vec<Atom> = reply.value32().map(Iterator::collect).unwrap_or_default();
                    // A password manager says so: not kept.
                    if targets.contains(&self.atoms.password_hint) {
                        return Ok(());
                    }
                    let want = if targets.contains(&self.atoms.utf8) { self.atoms.utf8 } else { AtomEnum::STRING.into() };
                    if targets.contains(&want) {
                        self.conn.convert_selection(self.hidden, self.atoms.clipboard, want, self.atoms.incoming, e.time)?;
                    }
                } else if reply.bytes_after == 0 {
                    remember(&mut self.history, String::from_utf8_lossy(&reply.value).into_owned());
                    if self.picker.is_some() {
                        self.paint()?;
                    }
                }
            }
            Event::SelectionRequest(e) => self.answer(e)?,
            Event::SelectionClear(e) if e.owner == self.hidden => self.owned = None,
            Event::ClientMessage(cm) if cm.type_ == self.atoms.pick => self.open_picker()?,
            Event::ClientMessage(cm) if cm.data.as_data32()[0] == self.atoms.wm_delete => self.close_picker()?,
            Event::Expose(e) if e.count == 0 && self.picker.is_some() => self.paint()?,
            Event::MotionNotify(m) => {
                let hover = self.row_at(i32::from(m.event_x), i32::from(m.event_y));
                if let Some(p) = &mut self.picker {
                    if p.hover != hover {
                        p.hover = hover;
                        self.paint()?;
                    }
                }
            }
            Event::ButtonPress(b) => {
                let (x, y) = (i32::from(b.event_x), i32::from(b.event_y));
                match b.detail {
                    4 | 5 => {
                        if let Some(p) = &mut self.picker {
                            p.scroll =
                                if b.detail == 4 { p.scroll.saturating_sub(1) } else { (p.scroll + 1).min(self.history.len().saturating_sub(ROWS)) };
                        }
                        self.paint()?;
                    }
                    1 if clear_button().contains(x, y) => {
                        self.history.clear();
                        self.paint()?;
                    }
                    1 => {
                        if let Some(i) = self.row_at(x, y) {
                            self.put_back(i)?;
                        }
                    }
                    _ => {}
                }
            }
            Event::KeyPress(k) => {
                let key = self.keymap.typed(&k);
                let Some(p) = &mut self.picker else { return Ok(()) };
                let n = self.history.len();
                match key {
                    Key::Escape => return self.close_picker(),
                    Key::Enter => {
                        if let Some(i) = p.hover {
                            return self.put_back(i);
                        }
                    }
                    Key::Down if n > 0 => p.hover = Some(p.hover.map_or(0, |h| (h + 1).min(n - 1))),
                    Key::Up if n > 0 => p.hover = Some(p.hover.map_or(0, |h| h.saturating_sub(1))),
                    _ => return Ok(()),
                }
                if let Some(h) = p.hover {
                    if h < p.scroll {
                        p.scroll = h;
                    } else if h >= p.scroll + ROWS {
                        p.scroll = h + 1 - ROWS;
                    }
                }
                self.paint()?;
            }
            _ => {}
        }
        Ok(())
    }

    /// A program pasting: what was put back, as the text kind it asks for.
    fn answer(&mut self, e: SelectionRequestEvent) -> Res<()> {
        let property = if e.property == x11rb::NONE { e.target } else { e.property };
        let ok = match &self.owned {
            Some(text) if e.selection == self.atoms.clipboard => {
                if e.target == self.atoms.targets {
                    let kinds = [self.atoms.targets, self.atoms.utf8, self.atoms.text, AtomEnum::STRING.into()];
                    self.conn.change_property32(PropMode::REPLACE, e.requestor, property, AtomEnum::ATOM, &kinds)?;
                    true
                } else if [self.atoms.utf8, self.atoms.text, AtomEnum::STRING.into()].contains(&e.target) {
                    self.conn.change_property8(PropMode::REPLACE, e.requestor, property, e.target, text.as_bytes())?;
                    true
                } else {
                    false
                }
            }
            _ => false,
        };
        let reply = SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: e.time,
            requestor: e.requestor,
            selection: e.selection,
            target: e.target,
            property: if ok { property } else { x11rb::NONE },
        };
        self.conn.send_event(false, e.requestor, EventMask::NO_EVENT, reply)?;
        Ok(())
    }

    /// Entry `i` back on the clipboard (and to the top of the list).
    fn put_back(&mut self, i: usize) -> Res<()> {
        let Some(text) = self.history.get(i).cloned() else { return Ok(()) };
        remember(&mut self.history, text.clone());
        self.owned = Some(text);
        self.conn.set_selection_owner(self.hidden, self.atoms.clipboard, x11rb::CURRENT_TIME)?;
        self.close_picker()
    }

    fn row_at(&self, x: i32, y: i32) -> Option<usize> {
        let p = self.picker.as_ref()?;
        let list = list_rect();
        if !list.contains(x, y) {
            return None;
        }
        let i = p.scroll + ((y - list.y - 2) / ROW_H) as usize;
        (i < self.history.len()).then_some(i)
    }

    fn open_picker(&mut self) -> Res<()> {
        if self.picker.is_some() {
            return self.close_picker();
        }
        let (win, gc) = (self.conn.generate_id()?, self.conn.generate_id()?);
        let (sw, sh) = (i32::from(self.screen.width_in_pixels), i32::from(self.screen.height_in_pixels));
        // Near the pointer, as Windows puts it near where you type.
        let at = self.conn.query_pointer(self.screen.root)?.reply()?;
        let x = (i32::from(at.root_x) - W / 2).clamp(0, (sw - W).max(0));
        let y = (i32::from(at.root_y) + 16).clamp(0, (sh - H - 48).max(0));
        let mask = EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::KEY_PRESS | EventMask::POINTER_MOTION;
        self.conn.create_window(
            self.screen.root_depth,
            win,
            self.screen.root,
            x as i16,
            y as i16,
            W as u16,
            H as u16,
            0,
            WindowClass::INPUT_OUTPUT,
            self.screen.root_visual,
            &CreateWindowAux::new().background_pixel(color::FACE).event_mask(mask),
        )?;
        self.conn.create_gc(gc, win, &CreateGCAux::new())?;
        let title = b"Clipboard";
        self.conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, title)?;
        self.conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, b"ferro-clipboard\0ferro-clipboard\0")?;
        self.conn.change_property32(PropMode::REPLACE, win, self.atoms.wm_protocols, AtomEnum::ATOM, &[self.atoms.wm_delete])?;
        self.conn.map_window(win)?;
        self.picker = Some(Picker { win, gc, hover: (!self.history.is_empty()).then_some(0), scroll: 0 });
        Ok(())
    }

    fn close_picker(&mut self) -> Res<()> {
        if let Some(p) = self.picker.take() {
            self.conn.free_gc(p.gc)?;
            self.conn.destroy_window(p.win)?;
        }
        Ok(())
    }

    fn paint(&self) -> Res<()> {
        let Some(p) = &self.picker else { return Ok(()) };
        let mut s = Surface::new(W as usize, H as usize);
        s.fill(Rect::new(0, 0, W, H), color::FACE);
        let list = list_rect();
        s.fill(list, color::WHITE);
        s.bevel(list, Bevel::Sunken);
        if self.history.is_empty() {
            s.text(list.x + 10, list.y + 12, "Nothing copied yet.", color::SHADOW);
        }
        let room = ((list.w - 16) / GLYPH_W) as usize;
        for (k, text) in self.history.iter().enumerate().skip(p.scroll).take(ROWS) {
            let y = list.y + 2 + (k - p.scroll) as i32 * ROW_H;
            let row = Rect::new(list.x + 2, y, list.w - 4, ROW_H - 2);
            let fg = if p.hover == Some(k) {
                s.fill(row, color::NAVY);
                color::WHITE
            } else {
                color::BLACK
            };
            s.text(row.x + 6, y + (ROW_H - GLYPH_H) / 2 - 1, &ellipsize(&shown(text), room), fg);
        }
        ferro_ui::button(&mut s, clear_button(), "Clear All", !self.history.is_empty());
        s.text(10, H - 26, "Click one, then Ctrl+V.", color::BLACK);
        ferro_ui::show(&self.conn, p.win, p.gc, self.screen.root_depth, &s, W, H)
    }
}

fn list_rect() -> Rect {
    Rect::new(8, 8, W - 16, ROWS as i32 * ROW_H + 4)
}

fn clear_button() -> Rect {
    Rect::new(W - 98, H - 34, 90, 26)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_newest_once() {
        let mut h = Vec::new();
        for t in ["a", "b", "a", "   ", "c"] {
            remember(&mut h, t.to_string());
        }
        assert_eq!(h, ["c", "a", "b"]);
        for i in 0..40 {
            remember(&mut h, i.to_string());
        }
        assert_eq!(h.len(), KEEP);
        remember(&mut h, "x".repeat(MOST + 1));
        assert_eq!(h[0], "39");
    }

    #[test]
    fn one_line_each() {
        assert_eq!(shown("  hello   world "), "hello world");
        assert_eq!(shown("first\nsecond"), "first second ...");
    }
}
