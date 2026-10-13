//! What the logon screen looks like and where its parts are: the teal
//! desktop and one Windows 95 dialog with a name, a password and OK. Pure
//! geometry and pixels (ferro-gfx), so it's the same in tests as on screen.

use ferro_gfx::{color, Bevel, Rect, Surface, GLYPH_H};
use ferro_ui::{button, text_field};

const W: i32 = 380;
const H: i32 = 170;
const TITLE_H: i32 = 18;
const FIELD_W: i32 = 200;
const FIELD_H: i32 = 22;

/// Which box the typing goes into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Name,
    Password,
}

/// What a click lands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Field(Field),
    Ok,
    ShutDown,
    Nothing,
}

/// Everything the dialog shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Look<'a> {
    pub name: &'a str,
    /// How many characters of password have been typed (never the text).
    pub password_len: usize,
    pub focus: Option<Field>,
    /// A line under the fields, e.g. after a wrong password.
    pub message: &'a str,
    pub desktop_color: Option<u32>,
}

fn dialog(sw: i32, sh: i32) -> Rect {
    Rect::new((sw - W) / 2, (sh - H) / 2, W, H)
}

fn field(d: Rect, which: Field) -> Rect {
    let row = match which {
        Field::Name => 0,
        Field::Password => 1,
    };
    Rect::new(d.x + 150, d.y + TITLE_H + 26 + row * 32, FIELD_W, FIELD_H)
}

fn ok_button(d: Rect) -> Rect {
    Rect::new(d.right() - 2 * 86 - 12, d.bottom() - 36, 80, 24)
}

fn shut_down_button(d: Rect) -> Rect {
    Rect::new(d.right() - 86 - 6, d.bottom() - 36, 80, 24)
}

pub fn hit(sw: i32, sh: i32, x: i32, y: i32) -> Hit {
    let d = dialog(sw, sh);
    if field(d, Field::Name).contains(x, y) {
        Hit::Field(Field::Name)
    } else if field(d, Field::Password).contains(x, y) {
        Hit::Field(Field::Password)
    } else if ok_button(d).contains(x, y) {
        Hit::Ok
    } else if shut_down_button(d).contains(x, y) {
        Hit::ShutDown
    } else {
        Hit::Nothing
    }
}

/// Draws the whole screen, `sw` x `sh`.
pub fn draw(s: &mut Surface, sw: i32, sh: i32, look: &Look) {
    let bg = look.desktop_color.unwrap_or(color::DESKTOP);
    s.fill(Rect::new(0, 0, sw, sh), bg);
    let d = dialog(sw, sh);
    s.fill(d, color::FACE);
    s.bevel(d, Bevel::Window);
    let title = Rect::new(d.x + 3, d.y + 3, d.w - 6, TITLE_H);
    s.fill(title, color::NAVY);
    s.text_bold(title.x + 4, title.y + (TITLE_H - GLYPH_H) / 2, "Welcome to FerroOS", color::WHITE);

    for (which, label) in [(Field::Name, "User name:"), (Field::Password, "Password:")] {
        let f = field(d, which);
        s.text(d.x + 24, f.y + (f.h - GLYPH_H) / 2, label, color::BLACK);
        let stars = "*".repeat(look.password_len);
        let text = if which == Field::Name { look.name } else { &stars };
        text_field(s, f, text, look.focus == Some(which));
    }
    s.text(d.x + 24, d.y + TITLE_H + 92, look.message, color::BLACK);
    button(s, ok_button(d), "OK", true);
    button(s, shut_down_button(d), "Shut Down", true);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_fields_and_buttons() {
        let (sw, sh) = (1024, 768);
        let d = dialog(sw, sh);
        let name = field(d, Field::Name);
        assert_eq!(hit(sw, sh, name.x + 5, name.y + 5), Hit::Field(Field::Name));
        let pass = field(d, Field::Password);
        assert_eq!(hit(sw, sh, pass.x + 5, pass.y + 5), Hit::Field(Field::Password));
        let ok = ok_button(d);
        assert_eq!(hit(sw, sh, ok.x + 5, ok.y + 5), Hit::Ok);
        let off = shut_down_button(d);
        assert_eq!(hit(sw, sh, off.x + 5, off.y + 5), Hit::ShutDown);
        assert_eq!(hit(sw, sh, 5, 5), Hit::Nothing);
        // The buttons don't overlap, and everything is inside the dialog.
        assert!(!ok.overlaps(&off));
        assert!(off.right() <= d.right() && pass.right() <= d.right());
    }

    #[test]
    fn never_draws_the_password() {
        let (sw, sh) = (640, 480);
        let mut a = Surface::new(sw as usize, sh as usize);
        let mut b = Surface::new(sw as usize, sh as usize);
        // Two different passwords of one length look the same: only the
        // length is ever given to the drawing code.
        draw(&mut a, sw, sh, &Look { name: "atom", password_len: 6, focus: Some(Field::Password), message: "", ..Look::default() });
        draw(&mut b, sw, sh, &Look { name: "atom", password_len: 6, focus: Some(Field::Password), message: "", ..Look::default() });
        assert_eq!(a.pixels(), b.pixels());
        draw(&mut b, sw, sh, &Look { name: "atom", password_len: 7, focus: Some(Field::Password), message: "", ..Look::default() });
        assert_ne!(a.pixels(), b.pixels());
    }
}
