//! The question: a program wants to see what's on the screen. Shown on
//! every desktop and over everything, on the gate's own connection to the
//! display, which no program can reach: no other program can press its
//! buttons (pretend clicks and key presses don't get through the gate), and
//! the two that allow are greyed out for a moment at first, so a click
//! meant for something else doesn't land on them. Enter, Esc, closing it or
//! two minutes without an answer: Don't allow.

use ferro_gfx::{color, ellipsize, text_width, Rect, Surface, GLYPH_W};
use ferro_ui::{button, default_button, dialog_answer, Keymap, Res};
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    /// This once (a few seconds, for a screenshot's several pieces).
    Once,
    /// Until the program quits.
    UntilQuit,
    No,
}

const W: i32 = 470;
const H: i32 = 128;
const ONCE_BTN: Rect = Rect::new(14, 92, 96, 24);
const QUIT_BTN: Rect = Rect::new(120, 92, 172, 24);
const NO_BTN: Rect = Rect::new(W - 110, 92, 96, 24);
/// The allowing buttons wake up after this long.
const ARMED_AFTER: Duration = Duration::from_millis(1000);
const GIVE_UP_AFTER: Duration = Duration::from_secs(120);

pub fn lines(name: &str, pid: u32) -> [String; 3] {
    [
        format!("'{name}' (process {pid}) wants to see what's"),
        "on your screen, including other programs' windows.".into(),
        "Close or hide anything private first, then choose.".into(),
    ]
}

fn draw(s: &mut Surface, text: &[String; 3], armed: bool) {
    s.fill(Rect::new(0, 0, W, H), color::FACE);
    let room = ((W - 28) / GLYPH_W).max(1) as usize;
    for (i, line) in text.iter().enumerate() {
        s.text(14, 14 + 18 * i as i32, &ellipsize(line, room), color::BLACK);
    }
    button(s, ONCE_BTN, "Allow once", armed);
    button(s, QUIT_BTN, "Allow until it quits", armed);
    default_button(s, NO_BTN, "Don't allow", true);
    debug_assert!(text_width("Allow until it quits") < QUIT_BTN.w);
}

/// Asks, on `conn` (the gate's own, straight to the display); `No` if it can't be shown.
pub fn ask(conn: &RustConnection, screen_num: usize, name: &str, pid: u32) -> Answer {
    show(conn, screen_num, name, pid).unwrap_or(Answer::No)
}

fn show(conn: &RustConnection, screen_num: usize, name: &str, pid: u32) -> Res<Answer> {
    let text = lines(name, pid);
    let screen = &conn.setup().roots[screen_num];
    let (sw, sh) = (i32::from(screen.width_in_pixels), i32::from(screen.height_in_pixels));
    let win = conn.generate_id()?;
    let gc = conn.generate_id()?;
    let values = CreateWindowAux::new().background_pixel(color::FACE).event_mask(
        EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::KEY_PRESS | EventMask::STRUCTURE_NOTIFY | EventMask::VISIBILITY_CHANGE,
    );
    conn.create_window(
        screen.root_depth,
        win,
        screen.root,
        ((sw - W) / 2) as i16,
        ((sh - H) / 3) as i16,
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
    let (state, above, sticky) = (atom(b"_NET_WM_STATE")?, atom(b"_NET_WM_STATE_ABOVE")?, atom(b"_NET_WM_STATE_STICKY")?);
    let desktop = atom(b"_NET_WM_DESKTOP")?;
    let title = "Screen access";
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;
    conn.change_property8(PropMode::REPLACE, win, wm_name, utf8, title.as_bytes())?;
    conn.change_property32(PropMode::REPLACE, win, wtype, AtomEnum::ATOM, &[dialog])?;
    conn.change_property32(PropMode::REPLACE, win, state, AtomEnum::ATOM, &[above, sticky])?;
    // On whichever desktop is shown, and on the others if it's switched.
    conn.change_property32(PropMode::REPLACE, win, desktop, AtomEnum::CARDINAL, &[0xFFFF_FFFF])?;
    conn.change_property32(PropMode::REPLACE, win, protocols, AtomEnum::ATOM, &[delete])?;
    ferro_ui::fixed_size(conn, win, W, H)?;
    conn.map_window(win)?;
    conn.flush()?;
    ferro_ui::sound("ding");

    let keymap = Keymap::load(conn)?;
    let mut surface = Surface::new(W as usize, H as usize);
    let mut armed = false;
    let mut repaint = |armed: bool| -> Res<()> {
        draw(&mut surface, &text, armed);
        ferro_ui::show(conn, win, gc, screen.root_depth, &surface, W, H)
    };
    repaint(armed)?;

    let opened = Instant::now();
    let answer = loop {
        if !armed && opened.elapsed() >= ARMED_AFTER {
            armed = true;
            repaint(armed)?;
        }
        if opened.elapsed() >= GIVE_UP_AFTER {
            break Answer::No;
        }
        let Some(event) = conn.poll_for_event()? else {
            std::thread::sleep(Duration::from_millis(40));
            continue;
        };
        match event {
            Event::Expose(e) if e.count == 0 => repaint(armed)?,
            Event::MapNotify(_) => {
                let _ = conn.set_input_focus(InputFocus::PARENT, win, x11rb::CURRENT_TIME);
                conn.flush()?;
            }
            // Something put over it: back on top.
            Event::VisibilityNotify(v) if v.state != Visibility::UNOBSCURED => ferro_ui::raise(conn, win)?,
            Event::ClientMessage(cm) if cm.data.as_data32()[0] == delete => break Answer::No,
            Event::DestroyNotify(_) => break Answer::No,
            Event::ButtonPress(bp) if bp.detail == 1 => {
                let (x, y) = (i32::from(bp.event_x), i32::from(bp.event_y));
                if armed && ONCE_BTN.contains(x, y) {
                    break Answer::Once;
                } else if armed && QUIT_BTN.contains(x, y) {
                    break Answer::UntilQuit;
                } else if NO_BTN.contains(x, y) {
                    break Answer::No;
                }
            }
            // Only Don't allow has a key: nothing typed meant for another window allows it.
            Event::KeyPress(kp) if dialog_answer(keymap.typed(&kp)).is_some() => break Answer::No,
            _ => {}
        }
    };
    let _ = conn.destroy_window(win);
    let _ = conn.free_gc(gc);
    let _ = conn.flush();
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_who_and_fits() {
        let [first, ..] = lines("evil", 4242);
        assert_eq!(first, "'evil' (process 4242) wants to see what's");
        for l in lines("firefox-esr", 4_194_304) {
            assert!(text_width(&l) <= W - 28, "{l}");
        }
        assert!(text_width("Allow until it quits") < QUIT_BTN.w);
        assert!(ONCE_BTN.right() < QUIT_BTN.x && QUIT_BTN.right() < NO_BTN.x);
    }
}
