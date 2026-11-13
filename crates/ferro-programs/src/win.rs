//! A FerroOS window of a fixed size, in the middle of the screen, and the
//! bits every window here draws the same way.

use ferro_gfx::{color, Bevel, Rect, Surface};
use ferro_ui::{Keymap, Res};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

pub struct Win {
    pub conn: RustConnection,
    pub win: Window,
    pub gc: Gcontext,
    pub depth: u8,
    pub delete: Atom,
    pub keymap: Keymap,
    pub w: i32,
    pub h: i32,
}

impl Win {
    pub fn open(title: &str, w: i32, h: i32) -> Res<Win> {
        let (conn, n) = RustConnection::connect(None)?;
        let screen = conn.setup().roots[n].clone();
        let (win, gc) = (conn.generate_id()?, conn.generate_id()?);
        let (sw, sh) = (i32::from(screen.width_in_pixels), i32::from(screen.height_in_pixels));
        let mask = EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::KEY_PRESS | EventMask::STRUCTURE_NOTIFY;
        conn.create_window(
            screen.root_depth,
            win,
            screen.root,
            ((sw - w) / 2).max(0) as i16,
            ((sh - h) / 2).max(0) as i16,
            w as u16,
            h as u16,
            0,
            WindowClass::INPUT_OUTPUT,
            screen.root_visual,
            &CreateWindowAux::new().background_pixel(color::FACE).event_mask(mask),
        )?;
        conn.create_gc(gc, win, &CreateGCAux::new())?;
        let atom = |name: &[u8]| -> Res<Atom> { Ok(conn.intern_atom(false, name)?.reply()?.atom) };
        let (protocols, delete) = (atom(b"WM_PROTOCOLS")?, atom(b"WM_DELETE_WINDOW")?);
        conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;
        conn.change_property8(PropMode::REPLACE, win, atom(b"_NET_WM_NAME")?, atom(b"UTF8_STRING")?, title.as_bytes())?;
        conn.change_property32(PropMode::REPLACE, win, protocols, AtomEnum::ATOM, &[delete])?;
        conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, b"ferro-programs\0ferro-programs\0")?;
        ferro_ui::fixed_size(&conn, win, w, h)?;
        conn.map_window(win)?;
        conn.flush()?;
        let keymap = Keymap::load(&conn)?;
        Ok(Win { conn, win, gc, depth: screen.root_depth, delete, keymap, w, h })
    }

    pub fn show(&self, s: &Surface) -> Res<()> {
        ferro_ui::show(&self.conn, self.win, self.gc, self.depth, s, self.w, self.h)
    }

    pub fn focus(&self) -> Res<()> {
        let _ = self.conn.set_input_focus(InputFocus::PARENT, self.win, x11rb::CURRENT_TIME);
        self.conn.flush()?;
        Ok(())
    }
}

/// Ctrl and Shift, as held for a key.
pub fn mods(state: KeyButMask) -> (bool, bool) {
    let s = u16::from(state);
    (s & u16::from(KeyButMask::CONTROL) != 0, s & u16::from(KeyButMask::SHIFT) != 0)
}

/// Windows 95's progress bar: a sunken box, filled with navy blocks as far
/// as `fraction` (0 to 1).
pub fn progress_bar(s: &mut Surface, r: Rect, fraction: f32) {
    s.fill(r, color::WHITE);
    s.bevel(r, Bevel::Sunken);
    let inner = Rect::new(r.x + 3, r.y + 3, r.w - 6, r.h - 6);
    let filled = (inner.w as f32 * fraction.clamp(0.0, 1.0)) as i32;
    let (block, gap) = (9, 2);
    let mut x = inner.x;
    while x + block <= inner.x + filled {
        s.fill(Rect::new(x, inner.y, block, inner.h), color::NAVY);
        x += block + gap;
    }
}

/// A list box's look: white, sunken.
pub fn list_box(s: &mut Surface, r: Rect) {
    s.fill(r, color::WHITE);
    s.bevel(r, Bevel::Sunken);
}
