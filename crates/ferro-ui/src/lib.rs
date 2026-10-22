//! ferro-ui: what FerroOS Heavy's own windows (the logon screen, the
//! installer) share. Showing a picture drawn with ferro-gfx in an X11
//! window, turning key presses into what was typed, and the two controls
//! every dialog has: a text box and a button.

use ferro_gfx::{color, text_width, Bevel, Rect, Surface, GLYPH_H, GLYPH_W};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::rust_connection::RustConnection;

pub type Res<T> = Result<T, Box<dyn std::error::Error>>;

// The few keys that aren't characters (X11 "keysyms").
const KEY_BACKSPACE: u32 = 0xff08;
const KEY_TAB: u32 = 0xff09;
const KEY_RETURN: u32 = 0xff0d;
const KEY_ESCAPE: u32 = 0xff1b;
const KEY_UP: u32 = 0xff52;
const KEY_DOWN: u32 = 0xff54;
const KEY_KP_ENTER: u32 = 0xff8d;

/// What a key press means to a dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Backspace,
    /// Tab: on to the next control.
    Next,
    Enter,
    Escape,
    Up,
    Down,
    Other,
}

/// Turns the two symbols on a key (plain and shifted) and the modifier
/// state into what was typed.
pub fn key(plain: u32, shifted: u32, shift: bool, caps: bool) -> Key {
    match plain {
        KEY_BACKSPACE => return Key::Backspace,
        KEY_TAB => return Key::Next,
        KEY_RETURN | KEY_KP_ENTER => return Key::Enter,
        KEY_ESCAPE => return Key::Escape,
        KEY_UP => return Key::Up,
        KEY_DOWN => return Key::Down,
        _ => {}
    }
    let letter = (0x61..=0x7a).contains(&plain);
    // Caps Lock only changes letters; Shift picks the key's second symbol.
    let sym = if shift != (caps && letter) && shifted != 0 { shifted } else { plain };
    match char::from_u32(sym) {
        // Latin-1 symbols are their own characters.
        Some(c) if (0x20..0x7f).contains(&sym) || (0xa0..=0xff).contains(&sym) => Key::Char(c),
        _ => Key::Other,
    }
}

/// The server's keyboard map: which symbols are on each key.
pub struct Keymap {
    first: u8,
    /// The plain and the shifted symbol of each key.
    keys: Vec<(u32, u32)>,
}

impl Keymap {
    pub fn load(conn: &RustConnection) -> Res<Self> {
        let setup = conn.setup();
        let (first, count) = (setup.min_keycode, setup.max_keycode - setup.min_keycode + 1);
        let map = conn.get_keyboard_mapping(first, count)?.reply()?;
        let per = usize::from(map.keysyms_per_keycode).max(1);
        let keys = map.keysyms.chunks(per).map(|k| (k[0], k.get(1).copied().unwrap_or(0))).collect();
        Ok(Self { first, keys })
    }

    pub fn typed(&self, e: &KeyPressEvent) -> Key {
        let Some(&(plain, shifted)) = self.keys.get(usize::from(e.detail.saturating_sub(self.first))) else { return Key::Other };
        key(plain, shifted, e.state.contains(KeyButMask::SHIFT), e.state.contains(KeyButMask::LOCK))
    }
}

/// Puts the top-left `w` x `h` of a drawn picture into a window.
pub fn show(conn: &RustConnection, win: u32, gc: u32, depth: u8, s: &Surface, w: i32, h: i32) -> Res<()> {
    if w <= 0 || h <= 0 {
        return Ok(());
    }
    let px = s.pixels();
    // In pieces, so no request outgrows what the server accepts.
    let rows_per = (48 * 1024 / (w as usize * 4)).max(1) as i32;
    let mut row = 0;
    let mut data = Vec::with_capacity((w * rows_per * 4) as usize);
    while row < h {
        let n = rows_per.min(h - row);
        data.clear();
        for p in &px[(row * w) as usize..((row + n) * w) as usize] {
            data.extend_from_slice(&p.to_le_bytes());
        }
        conn.put_image(ImageFormat::Z_PIXMAP, win, gc, w as u16, n as u16, 0, row as i16, 0, depth, &data)?;
        row += n;
    }
    conn.flush()?;
    Ok(())
}

/// Takes the whole keyboard and pointer for `win`, as a lock or logon
/// screen must: until the connection closes, no other program gets a key
/// or a click, and none of the window manager's shortcuts work. Retries
/// for a few seconds, since an open menu or a drag may hold them briefly.
/// `false` if they couldn't be had.
pub fn grab_input(conn: &RustConnection, win: u32, pointer_events: EventMask) -> Res<bool> {
    for _ in 0..100 {
        let kb = conn.grab_keyboard(false, win, x11rb::CURRENT_TIME, GrabMode::ASYNC, GrabMode::ASYNC)?.reply()?.status;
        if kb == GrabStatus::SUCCESS {
            let ptr = conn
                .grab_pointer(false, win, pointer_events, GrabMode::ASYNC, GrabMode::ASYNC, x11rb::NONE, x11rb::NONE, x11rb::CURRENT_TIME)?
                .reply()?
                .status;
            if ptr == GrabStatus::SUCCESS {
                return Ok(true);
            }
            conn.ungrab_keyboard(x11rb::CURRENT_TIME)?;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    Ok(false)
}

/// Lets go of what [`grab_input`] took.
pub fn release_input(conn: &RustConnection) -> Res<()> {
    conn.ungrab_pointer(x11rb::CURRENT_TIME)?;
    conn.ungrab_keyboard(x11rb::CURRENT_TIME)?;
    conn.flush()?;
    Ok(())
}

/// Puts `win` back on top of everything (after a VisibilityNotify says
/// something covered it).
pub fn raise(conn: &RustConnection, win: u32) -> Res<()> {
    conn.configure_window(win, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE))?;
    conn.flush()?;
    Ok(())
}

/// Forgets a typed secret: its memory is overwritten, not just let go.
pub fn forget(secret: &mut String) {
    let mut bytes = std::mem::take(secret).into_bytes();
    bytes.fill(0);
    // Keeps the overwriting from being optimized away as pointless.
    std::hint::black_box(&bytes);
}

/// A white box holding typed text, with the caret when it has the typing.
pub fn text_field(s: &mut Surface, r: Rect, text: &str, focused: bool) {
    s.fill(r, color::WHITE);
    s.bevel(r, Bevel::Sunken);
    // The end of a long entry is what's being typed.
    let room = ((r.w - 12) / GLYPH_W).max(1) as usize;
    let count = text.chars().count();
    let shown: String = text.chars().skip(count.saturating_sub(room)).collect();
    let end = r.x + 5 + s.text(r.x + 5, r.y + (r.h - GLYPH_H) / 2, &shown, color::BLACK);
    if focused {
        s.vline(end + 1, r.y + 4, r.h - 8, color::BLACK);
    }
}

/// A push button; greyed out when it can't be used yet.
pub fn button(s: &mut Surface, r: Rect, label: &str, enabled: bool) {
    s.button(r, false);
    let (x, y) = (r.x + (r.w - text_width(label)) / 2, r.y + (r.h - GLYPH_H) / 2);
    if enabled {
        s.text(x, y, label, color::BLACK);
    } else {
        s.text_disabled(x, y, label);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_become_characters() {
        // The "a" key and the "1" key (which has "!" on it).
        assert_eq!(key(0x61, 0x41, false, false), Key::Char('a'));
        assert_eq!(key(0x61, 0x41, true, false), Key::Char('A'));
        assert_eq!(key(0x31, 0x21, false, false), Key::Char('1'));
        assert_eq!(key(0x31, 0x21, true, false), Key::Char('!'));
        // Caps Lock makes letters capital, Shift undoes it, digits don't care.
        assert_eq!(key(0x61, 0x41, false, true), Key::Char('A'));
        assert_eq!(key(0x61, 0x41, true, true), Key::Char('a'));
        assert_eq!(key(0x31, 0x21, false, true), Key::Char('1'));
        assert_eq!(key(0x20, 0x20, false, false), Key::Char(' '));
        assert_eq!(key(KEY_RETURN, 0, false, false), Key::Enter);
        assert_eq!(key(KEY_TAB, 0, false, false), Key::Next);
        assert_eq!(key(KEY_BACKSPACE, 0, false, false), Key::Backspace);
        assert_eq!(key(KEY_UP, 0, false, false), Key::Up);
        assert_eq!(key(KEY_DOWN, 0, false, false), Key::Down);
        // Shift by itself, the other arrows and the like type nothing.
        assert_eq!(key(0xffe1, 0, false, false), Key::Other);
        assert_eq!(key(0xff51, 0, false, false), Key::Other);
    }

    #[test]
    fn a_long_entry_shows_its_end() {
        let mut a = Surface::new(120, 30);
        let mut b = Surface::new(120, 30);
        let r = Rect::new(2, 2, 100, 22);
        // Eleven characters fit; what's before them isn't drawn.
        text_field(&mut a, r, "0123456789abcdefghij", false);
        text_field(&mut b, r, "9abcdefghij", false);
        assert_eq!(a.pixels(), b.pixels());
    }

    #[test]
    fn the_caret_follows_the_text() {
        let mut s = Surface::new(120, 30);
        s.clear(color::FACE);
        let r = Rect::new(10, 2, 100, 22);
        text_field(&mut s, r, "ab", true);
        // Just right of the two characters, and nowhere left of the box.
        let at = |x: i32, y: i32| s.pixels()[(y * 120 + x) as usize];
        assert_eq!(at(r.x + 5 + 2 * GLYPH_W + 1, r.y + 10), color::BLACK);
        assert!((0..r.x).all(|x| at(x, r.y + 10) != color::BLACK));
    }
}
