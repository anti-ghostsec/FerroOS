//! ferro-notifications: the notifications shown lately (Windows+N), newest
//! first, as Windows' notification list has them. dunst shows them in the
//! corner and keeps them in memory only, never on the disk, until you log
//! off; this asks it for them (`dunstctl history`). Clear All forgets them;
//! Do Not Disturb holds new ones back until it's turned off.

use ferro_gfx::{color, ellipsize, text_width, Bevel, Rect, Surface, GLYPH_H, GLYPH_W};
use ferro_ui::{Key, Keymap, Res};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

mod json;

const W: i32 = 420;
const H: i32 = 480;
const ROW_H: i32 = 52;
const LIST: Rect = Rect::new(10, 10, W - 20, H - 56);

#[derive(Debug, PartialEq, Eq)]
struct Note {
    app: String,
    summary: String,
    body: String,
}

/// The notifications in `dunstctl history`'s answer, newest first.
fn parse(history: &str) -> Vec<Note> {
    let Some(v) = json::parse(history) else { return Vec::new() };
    let field = |n: &json::Value, k: &str| n.get(k).and_then(|f| f.get("data")).and_then(json::Value::as_str).unwrap_or("").to_owned();
    let Some(json::Value::Array(groups)) = v.get("data") else { return Vec::new() };
    groups
        .iter()
        .filter_map(|g| match g {
            json::Value::Array(notes) => Some(notes),
            _ => None,
        })
        .flatten()
        .map(|n| Note { app: field(n, "appname"), summary: field(n, "summary"), body: strip_markup(&field(n, "body")) })
        .collect()
}

/// `<b>` and the like out: text only.
fn strip_markup(s: &str) -> String {
    let mut out = String::new();
    let mut tag = false;
    for c in s.chars() {
        match c {
            '<' => tag = true,
            '>' if tag => tag = false,
            _ if !tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'")
}

fn dunstctl(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("dunstctl").args(args).stderr(std::process::Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn load() -> (Vec<Note>, bool) {
    let notes = dunstctl(&["history"]).map(|h| parse(&h)).unwrap_or_default();
    let paused = dunstctl(&["is-paused"]).is_some_and(|p| p.trim() == "true");
    (notes, paused)
}

fn buttons() -> (Rect, Rect, Rect) {
    let y = H - 38;
    (Rect::new(10, y, 120, 26), Rect::new(140, y, 150, 26), Rect::new(W - 90, y, 80, 26))
}

fn draw(s: &mut Surface, notes: &[Note], paused: bool, scroll: usize) {
    s.fill(Rect::new(0, 0, W, H), color::FACE);
    s.fill(LIST, color::WHITE);
    s.bevel(LIST, Bevel::Sunken);
    let room = ((LIST.w - 20) / GLYPH_W) as usize;
    if notes.is_empty() {
        let said = "No new notifications";
        s.text(LIST.x + (LIST.w - text_width(said)) / 2, LIST.y + 40, said, color::SHADOW);
    }
    let rows = (LIST.h - 4) / ROW_H;
    for (k, n) in notes.iter().skip(scroll).take(rows as usize).enumerate() {
        let y = LIST.y + 4 + k as i32 * ROW_H;
        let app = if n.app.is_empty() { "Notification" } else { &n.app };
        s.text(LIST.x + 10, y + 4, &ellipsize(app, room), color::SHADOW);
        s.text_bold(LIST.x + 10, y + 4 + GLYPH_H + 5, &ellipsize(&n.summary, room), color::BLACK);
        let body = n.body.lines().next().unwrap_or("");
        s.text(LIST.x + 10, y + 4 + 2 * (GLYPH_H + 5), &ellipsize(body, room), color::BLACK);
        s.hline(LIST.x + 6, y + ROW_H - 3, LIST.w - 12, color::LIGHT);
    }
    let (clear, dnd, close) = buttons();
    ferro_ui::button(s, clear, "Clear All", !notes.is_empty());
    ferro_ui::button(s, dnd, if paused { "[v] Do Not Disturb" } else { "[  ] Do Not Disturb" }, true);
    ferro_ui::button(s, close, "Close", true);
}

fn main() -> Res<()> {
    let (conn, n) = RustConnection::connect(None)?;
    let screen = conn.setup().roots[n].clone();
    let (win, gc) = (conn.generate_id()?, conn.generate_id()?);
    let (sw, sh) = (i32::from(screen.width_in_pixels), i32::from(screen.height_in_pixels));
    // Bottom right, over the clock, where Windows keeps it.
    let (x, y) = (sw - W - 10, sh - H - 80);
    conn.create_window(
        screen.root_depth,
        win,
        screen.root,
        x as i16,
        y.max(0) as i16,
        W as u16,
        H as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &CreateWindowAux::new().background_pixel(color::FACE).event_mask(EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::KEY_PRESS),
    )?;
    conn.create_gc(gc, win, &CreateGCAux::new())?;
    let atom = |name: &[u8]| -> Res<Atom> { Ok(conn.intern_atom(false, name)?.reply()?.atom) };
    let (wm_protocols, wm_delete) = (atom(b"WM_PROTOCOLS")?, atom(b"WM_DELETE_WINDOW")?);
    let title = "Notifications";
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;
    conn.change_property8(PropMode::REPLACE, win, atom(b"_NET_WM_NAME")?, atom(b"UTF8_STRING")?, title.as_bytes())?;
    conn.change_property32(PropMode::REPLACE, win, wm_protocols, AtomEnum::ATOM, &[wm_delete])?;
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, b"ferro-notifications\0ferro-notifications\0")?;
    ferro_ui::fixed_size(&conn, win, W, H)?;
    conn.map_window(win)?;
    conn.flush()?;
    let keymap = Keymap::load(&conn)?;
    let (mut notes, mut paused) = load();
    let mut scroll = 0usize;
    let rows = ((LIST.h - 4) / ROW_H) as usize;

    let paint = |notes: &[Note], paused: bool, scroll: usize| -> Res<()> {
        let mut s = Surface::new(W as usize, H as usize);
        draw(&mut s, notes, paused, scroll);
        ferro_ui::show(&conn, win, gc, screen.root_depth, &s, W, H)
    };
    loop {
        match conn.wait_for_event()? {
            Event::Expose(e) if e.count == 0 => paint(&notes, paused, scroll)?,
            Event::ClientMessage(cm) if cm.data.as_data32()[0] == wm_delete => return Ok(()),
            Event::ButtonPress(b) => {
                let (px, py) = (i32::from(b.event_x), i32::from(b.event_y));
                let (clear, dnd, close) = buttons();
                match b.detail {
                    4 => scroll = scroll.saturating_sub(1),
                    5 => scroll = (scroll + 1).min(notes.len().saturating_sub(rows)),
                    1 if clear.contains(px, py) => {
                        dunstctl(&["history-clear"]);
                        (notes, paused) = load();
                        scroll = 0;
                    }
                    1 if dnd.contains(px, py) => {
                        dunstctl(&["set-paused", "toggle"]);
                        (notes, paused) = load();
                    }
                    1 if close.contains(px, py) => return Ok(()),
                    _ => continue,
                }
                paint(&notes, paused, scroll)?;
            }
            Event::KeyPress(k) => match keymap.typed(&k) {
                Key::Escape | Key::Enter => return Ok(()),
                Key::Up => {
                    scroll = scroll.saturating_sub(1);
                    paint(&notes, paused, scroll)?;
                }
                Key::Down => {
                    scroll = (scroll + 1).min(notes.len().saturating_sub(rows));
                    paint(&notes, paused, scroll)?;
                }
                _ => {}
            },
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_dunsts_history() {
        let history = r#"{"type":"aa{sv}","data":[[{"body":{"type":"s","data":"Open it from <b>My Computer</b>."},"message":{"type":"s","data":"x"},"summary":{"type":"s","data":"STICK is ready"},"appname":{"type":"s","data":"Drives"},"id":{"type":"i","data":7}},{"body":{"type":"s","data":"a &amp; b"},"summary":{"type":"s","data":"Download finished"},"appname":{"type":"s","data":"LibreWolf"}}]]}"#;
        let notes = parse(history);
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0], Note { app: "Drives".into(), summary: "STICK is ready".into(), body: "Open it from My Computer.".into() });
        assert_eq!(notes[1].body, "a & b");
        assert!(parse("").is_empty());
        assert!(parse("{\"data\":[[]]}").is_empty());
    }
}
