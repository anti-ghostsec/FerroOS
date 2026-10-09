//! Linux evdev key codes -> [`Key`] events, US layout.
//!
//! Lives in the platform-independent library so it can be unit-tested on any
//! host; only the framebuffer backend feeds it real codes.

use crate::{Event, Key, Mods};

const SHIFT_L: u16 = 42;
const SHIFT_R: u16 = 54;
const CTRL_L: u16 = 29;
const CTRL_R: u16 = 97;
const ALT_L: u16 = 56;
const ALT_R: u16 = 100;
const CAPS: u16 = 58;

/// Tracks modifier state across evdev key events.
#[derive(Default)]
pub struct Keyboard {
    shift: [bool; 2],
    ctrl: [bool; 2],
    alt: [bool; 2],
    caps: bool,
}

impl Keyboard {
    /// `value` is evdev's: 0 release, 1 press, 2 autorepeat.
    pub fn event(&mut self, code: u16, value: i32) -> Option<Event> {
        let down = value != 0;
        match code {
            SHIFT_L => self.shift[0] = down,
            SHIFT_R => self.shift[1] = down,
            CTRL_L => self.ctrl[0] = down,
            CTRL_R => self.ctrl[1] = down,
            ALT_L => self.alt[0] = down,
            ALT_R => self.alt[1] = down,
            CAPS if value == 1 => self.caps = !self.caps,
            _ if down => {
                let mods = Mods { shift: self.shift.contains(&true), ctrl: self.ctrl.contains(&true), alt: self.alt.contains(&true) };
                return translate(code, mods.shift, self.caps).map(|key| Event::Key { key, mods });
            }
            _ => {}
        }
        None
    }
}

fn translate(code: u16, shift: bool, caps: bool) -> Option<Key> {
    const ROW_NUM: &[u8; 13] = b"1234567890-=\0";
    const ROW_NUM_S: &[u8; 13] = b"!@#$%^&*()_+\0";
    let ch = |plain: u8, shifted: u8| Some(Key::Char(if shift { shifted } else { plain } as char));
    let letter = |c: u8| {
        let upper = shift ^ caps;
        Some(Key::Char(if upper { c.to_ascii_uppercase() } else { c } as char))
    };
    match code {
        1 => Some(Key::Escape),
        2..=13 => ch(ROW_NUM[usize::from(code - 2)], ROW_NUM_S[usize::from(code - 2)]),
        14 => Some(Key::Backspace),
        15 => Some(Key::Tab),
        16..=25 => letter(b"qwertyuiop"[usize::from(code - 16)]),
        26 => ch(b'[', b'{'),
        27 => ch(b']', b'}'),
        28 | 96 => Some(Key::Enter),
        30..=38 => letter(b"asdfghjkl"[usize::from(code - 30)]),
        39 => ch(b';', b':'),
        40 => ch(b'\'', b'"'),
        41 => ch(b'`', b'~'),
        43 => ch(b'\\', b'|'),
        44..=50 => letter(b"zxcvbnm"[usize::from(code - 44)]),
        51 => ch(b',', b'<'),
        52 => ch(b'.', b'>'),
        53 => ch(b'/', b'?'),
        55 => Some(Key::Char('*')),
        57 => Some(Key::Char(' ')),
        59..=68 => Some(Key::F((code - 58) as u8)),
        87 => Some(Key::F(11)),
        88 => Some(Key::F(12)),
        74 => Some(Key::Char('-')),
        78 => Some(Key::Char('+')),
        98 => Some(Key::Char('/')),
        102 => Some(Key::Home),
        103 => Some(Key::Up),
        104 => Some(Key::PageUp),
        105 => Some(Key::Left),
        106 => Some(Key::Right),
        107 => Some(Key::End),
        108 => Some(Key::Down),
        109 => Some(Key::PageDown),
        110 => Some(Key::Insert),
        111 => Some(Key::Delete),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(kb: &mut Keyboard, code: u16) -> Option<Key> {
        let ev = kb.event(code, 1);
        kb.event(code, 0);
        match ev {
            Some(Event::Key { key, .. }) => Some(key),
            _ => None,
        }
    }

    #[test]
    fn letters_shift_and_caps() {
        let mut kb = Keyboard::default();
        assert_eq!(press(&mut kb, 30), Some(Key::Char('a')));
        kb.event(SHIFT_L, 1);
        assert_eq!(press(&mut kb, 30), Some(Key::Char('A')));
        assert_eq!(press(&mut kb, 2), Some(Key::Char('!')));
        kb.event(SHIFT_L, 0);
        kb.event(CAPS, 1);
        kb.event(CAPS, 0);
        assert_eq!(press(&mut kb, 50), Some(Key::Char('M')));
        assert_eq!(press(&mut kb, 2), Some(Key::Char('1')), "caps doesn't shift digits");
    }

    #[test]
    fn ctrl_and_special_keys() {
        let mut kb = Keyboard::default();
        kb.event(CTRL_L, 1);
        let ev = kb.event(46, 1);
        assert!(matches!(ev, Some(Event::Key { key: Key::Char('c'), mods: Mods { ctrl: true, .. } })));
        kb.event(CTRL_L, 0);
        assert_eq!(press(&mut kb, 28), Some(Key::Enter));
        assert_eq!(press(&mut kb, 103), Some(Key::Up));
        assert_eq!(press(&mut kb, 62), Some(Key::F(4)));
        assert!(kb.event(30, 0).is_none(), "releases produce nothing");
    }
}
