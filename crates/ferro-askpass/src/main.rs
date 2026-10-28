//! ferro-askpass: the "Run as administrator" password box.
//!
//! sudo runs it (`sudo -A`, with `SUDO_ASKPASS` pointing here) when a
//! program needs the administrator's say-so and there's no terminal to ask
//! in: a shortcut "as Administrator" (through FerroOS's `pkexec`), Restricted
//! folders, Task Manager ending someone else's process. It prints what's
//! typed on one line for sudo and exits 0; Cancel, Esc or closing it exits 1,
//! and sudo then gives up.
//!
//! `FERRO_ADMIN_FOR` (set by whoever asks) names what wants the rights, so
//! the box can say so; without it, sudo's own prompt is shown.

use ferro_gfx::{color, ellipsize, Rect, Surface, GLYPH_W};
use ferro_ui::{button, focus_rect, text_field_with_cursor, Key, Keymap, Res};
use std::io::Write;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

const W: i32 = 400;
const H: i32 = 150;
const FIELD: Rect = Rect::new(14, 70, W - 28, 24);
const OK_BTN: Rect = Rect::new(W - 175, 110, 75, 24);
const CANCEL_BTN: Rect = Rect::new(W - 90, 110, 75, 24);

/// The two lines over the password field.
fn lines(asking_for: Option<&str>, sudo_prompt: &str) -> [String; 2] {
    match asking_for.map(str::trim).filter(|s| !s.is_empty()) {
        Some(what) => [format!("'{what}' wants to make changes to this computer."), "Type your password to allow it.".into()],
        None => [sudo_prompt.trim().trim_end_matches(':').to_owned(), "Type your password to allow it.".into()],
    }
}

/// What the field shows: a star for each letter typed.
fn masked(typed: &str) -> String {
    "*".repeat(typed.chars().count())
}

/// Leaves the password nowhere in memory once it's been used.
fn wipe(s: &mut String) {
    // SAFETY: zeroes are valid UTF-8.
    unsafe { s.as_bytes_mut().fill(0) };
    s.clear();
}

fn draw(s: &mut Surface, text: &[String; 2], typed: &str) {
    s.fill(Rect::new(0, 0, W, H), color::FACE);
    let room = ((W - 28) / GLYPH_W).max(1) as usize;
    s.text(14, 16, &ellipsize(&text[0], room), color::BLACK);
    s.text(14, 36, &ellipsize(&text[1], room), color::BLACK);
    let shown = masked(typed);
    text_field_with_cursor(s, FIELD, &shown, shown.chars().count(), None, true);
    // OK is the default button: Enter presses it.
    button(s, OK_BTN, "OK", true);
    focus_rect(s, OK_BTN.inset(2));
    button(s, CANCEL_BTN, "Cancel", true);
}

fn answer(mut typed: String) -> ! {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{typed}");
    let _ = out.flush();
    wipe(&mut typed);
    std::process::exit(0)
}

fn main() -> Res<()> {
    let sudo_prompt = std::env::args().nth(1).unwrap_or_else(|| "Password:".into());
    let asking_for = std::env::var("FERRO_ADMIN_FOR").ok();
    let text = lines(asking_for.as_deref(), &sudo_prompt);
    let mut typed = String::new();

    let (conn, screen_num) = RustConnection::connect(None)?;
    let screen = &conn.setup().roots[screen_num];
    let (sw, sh) = (i32::from(screen.width_in_pixels), i32::from(screen.height_in_pixels));
    let win = conn.generate_id()?;
    let gc = conn.generate_id()?;
    let values = CreateWindowAux::new()
        .background_pixel(color::FACE)
        .event_mask(EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::KEY_PRESS | EventMask::STRUCTURE_NOTIFY);
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
        &values,
    )?;
    conn.create_gc(gc, win, &CreateGCAux::new())?;
    let atom = |name: &[u8]| -> Res<Atom> { Ok(conn.intern_atom(false, name)?.reply()?.atom) };
    let (wm_name, utf8) = (atom(b"_NET_WM_NAME")?, atom(b"UTF8_STRING")?);
    let (protocols, delete) = (atom(b"WM_PROTOCOLS")?, atom(b"WM_DELETE_WINDOW")?);
    let (wtype, dialog) = (atom(b"_NET_WM_WINDOW_TYPE")?, atom(b"_NET_WM_WINDOW_TYPE_DIALOG")?);
    let (state, above) = (atom(b"_NET_WM_STATE")?, atom(b"_NET_WM_STATE_ABOVE")?);
    let title = "Run as administrator";
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;
    conn.change_property8(PropMode::REPLACE, win, wm_name, utf8, title.as_bytes())?;
    conn.change_property32(PropMode::REPLACE, win, wtype, AtomEnum::ATOM, &[dialog])?;
    conn.change_property32(PropMode::REPLACE, win, state, AtomEnum::ATOM, &[above])?;
    conn.change_property32(PropMode::REPLACE, win, protocols, AtomEnum::ATOM, &[delete])?;
    conn.map_window(win)?;
    conn.flush()?;

    let keymap = Keymap::load(&conn)?;
    let mut surface = Surface::new(W as usize, H as usize);
    let mut repaint = |typed: &str| -> Res<()> {
        draw(&mut surface, &text, typed);
        ferro_ui::show(&conn, win, gc, screen.root_depth, &surface, W, H)
    };
    repaint(&typed)?;

    loop {
        match conn.wait_for_event()? {
            Event::Expose(_) => repaint(&typed)?,
            Event::MapNotify(_) => {
                // Typing goes here at once, without a click first.
                let _ = conn.set_input_focus(InputFocus::PARENT, win, x11rb::CURRENT_TIME);
                conn.flush()?;
            }
            Event::ClientMessage(cm) if cm.data.as_data32()[0] == delete => std::process::exit(1),
            Event::ButtonPress(bp) => {
                let (x, y) = (i32::from(bp.event_x), i32::from(bp.event_y));
                if OK_BTN.contains(x, y) {
                    answer(typed);
                } else if CANCEL_BTN.contains(x, y) {
                    wipe(&mut typed);
                    std::process::exit(1);
                }
            }
            Event::KeyPress(kp) => {
                match keymap.typed(&kp) {
                    Key::Enter => answer(typed),
                    Key::Escape => {
                        wipe(&mut typed);
                        std::process::exit(1);
                    }
                    Key::Backspace => {
                        typed.pop();
                    }
                    Key::Char(c) if !c.is_control() && typed.len() < 512 => typed.push(c),
                    _ => continue,
                }
                repaint(&typed)?;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_what_wants_the_rights() {
        let [first, _] = lines(Some("bleachbit"), "[sudo] password for me: ");
        assert_eq!(first, "'bleachbit' wants to make changes to this computer.");
        let [first, _] = lines(None, "[sudo] password for me: ");
        assert_eq!(first, "[sudo] password for me");
        let [first, _] = lines(Some("  "), "Password:");
        assert_eq!(first, "Password");
    }

    #[test]
    fn shows_only_stars_and_forgets() {
        assert_eq!(masked("pässwörd"), "********");
        let mut s = String::from("secret");
        wipe(&mut s);
        assert!(s.is_empty());
    }
}
