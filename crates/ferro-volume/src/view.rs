//! Retro Windows 95 Volume Control view and hit-testing.

use ferro_gfx::{color, icons, Bevel, Rect, Surface, GLYPH_H, GLYPH_W};

pub const W: i32 = 180;
pub const H: i32 = 230;

pub const TRACK_X: i32 = 87;
pub const TRACK_Y: i32 = 50;
pub const TRACK_H: i32 = 100;
pub const TRACK_W: i32 = 6;

pub const KNOB_W: i32 = 26;
pub const KNOB_H: i32 = 14;

pub const MUTE_RECT: Rect = Rect::new(40, 168, 100, 18);
pub const OK_RECT: Rect = Rect::new(55, 195, 70, 22);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Slider(u8),
    ToggleMute,
    Close,
    Nothing,
}

pub struct State {
    pub volume: u8,
    pub muted: bool,
    pub dragging: bool,
}

impl Default for State {
    fn default() -> Self {
        Self { volume: 75, muted: false, dragging: false }
    }
}

pub fn knob_y(volume: u8) -> i32 {
    let vol = volume.min(100) as i32;
    // 100% -> TRACK_Y; 0% -> TRACK_Y + TRACK_H - KNOB_H
    let travel = TRACK_H - KNOB_H;
    TRACK_Y + travel - (vol * travel) / 100
}

pub fn volume_from_y(y: i32) -> u8 {
    let travel = TRACK_H - KNOB_H;
    let clamped = (y - TRACK_Y).clamp(0, travel);
    let inverted = travel - clamped;
    ((inverted * 100 + travel / 2) / travel).clamp(0, 100) as u8
}

pub fn hit(x: i32, y: i32, state: &State) -> Hit {
    if OK_RECT.contains(x, y) {
        return Hit::Close;
    }
    if MUTE_RECT.contains(x, y) {
        return Hit::ToggleMute;
    }
    let k_y = knob_y(state.volume);
    let knob_rect = Rect::new(TRACK_X - (KNOB_W - TRACK_W) / 2, k_y, KNOB_W, KNOB_H);
    if knob_rect.inset(-4).contains(x, y) || ((TRACK_X - 12..=TRACK_X + TRACK_W + 12).contains(&x) && (TRACK_Y..=TRACK_Y + TRACK_H).contains(&y)) {
        return Hit::Slider(volume_from_y(y));
    }
    Hit::Nothing
}

pub fn draw(s: &mut Surface, w: i32, h: i32, state: &State) {
    s.fill(Rect::new(0, 0, w, h), color::FACE);

    // Group box
    let group = Rect::new(10, 10, w - 20, 175);
    s.bevel(group, Bevel::Sunken);
    let title_bg = Rect::new(20, 4, 90, GLYPH_H + 4);
    s.fill(title_bg, color::FACE);
    s.sprite(22, 5, icons::SPEAKER, 1);
    s.text(42, 6, "Volume", color::BLACK);

    // Track
    let track = Rect::new(TRACK_X, TRACK_Y, TRACK_W, TRACK_H);
    s.bevel(track, Bevel::Sunken);

    // Tick marks along the track
    for i in 0..=4 {
        let ty = TRACK_Y + (i * (TRACK_H - 2)) / 4;
        s.hline(TRACK_X - 8, ty, 6, color::SHADOW);
        s.hline(TRACK_X + TRACK_W + 2, ty, 6, color::SHADOW);
    }

    // High / Low labels
    s.text(TRACK_X - 28, TRACK_Y, "Max", color::BLACK);
    s.text(TRACK_X - 28, TRACK_Y + TRACK_H - GLYPH_H, "Min", color::BLACK);

    // Knob
    let k_y = knob_y(state.volume);
    let knob = Rect::new(TRACK_X - (KNOB_W - TRACK_W) / 2, k_y, KNOB_W, KNOB_H);
    s.fill(knob, color::FACE);
    s.bevel(knob, Bevel::Raised);
    // Center ridge on knob
    s.hline(knob.x + 4, knob.y + KNOB_H / 2, KNOB_W - 8, color::SHADOW);

    // Percentage display
    let pct_str = format!("{}%", state.volume);
    let px = (w - (pct_str.len() as i32 * GLYPH_W)) / 2;
    s.text(px, TRACK_Y + TRACK_H + 4, &pct_str, color::NAVY);

    // Mute Checkbox
    let check_box = Rect::new(MUTE_RECT.x, MUTE_RECT.y + 2, 13, 13);
    s.fill(check_box, color::WHITE);
    s.bevel(check_box, Bevel::Sunken);
    if state.muted {
        s.sprite(check_box.x, check_box.y, icons::GLYPH_CHECK, 1);
    }
    s.text(MUTE_RECT.x + 18, MUTE_RECT.y + 3, "Mute", if state.muted { color::RED } else { color::BLACK });

    // OK / Close Button
    s.button(OK_RECT, false);
    s.text(OK_RECT.x + (OK_RECT.w - 2 * GLYPH_W) / 2, OK_RECT.y + (OK_RECT.h - GLYPH_H) / 2, "OK", color::BLACK);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_conversion_endpoints() {
        assert_eq!(knob_y(100), TRACK_Y);
        assert_eq!(knob_y(0), TRACK_Y + TRACK_H - KNOB_H);

        assert_eq!(volume_from_y(TRACK_Y), 100);
        assert_eq!(volume_from_y(TRACK_Y + TRACK_H - KNOB_H), 0);
        assert_eq!(volume_from_y(TRACK_Y - 50), 100);
        assert_eq!(volume_from_y(TRACK_Y + TRACK_H + 50), 0);
    }

    #[test]
    fn volume_hit_tests() {
        let state = State::default();
        assert_eq!(hit(OK_RECT.x + 2, OK_RECT.y + 2, &state), Hit::Close);
        assert_eq!(hit(MUTE_RECT.x + 2, MUTE_RECT.y + 2, &state), Hit::ToggleMute);
        assert!(matches!(hit(TRACK_X, TRACK_Y, &state), Hit::Slider(100)));
        assert_eq!(hit(0, 0, &state), Hit::Nothing);
    }
}
