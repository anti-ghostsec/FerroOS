//! The Sound Enhancer window's drawing and what a click lands on.

use ferro_dsp::settings::{Settings, BAND_NAMES, BASS_MAX, BOOST_MAX, EQ_MAX, LOUDNESS_MAX, OWN, PRESETS};
use ferro_gfx::{color, text_width, Bevel, Rect, Surface, GLYPH_H};

pub const W: i32 = 500;
pub const H: i32 = 452;

const ON: Rect = Rect::new(138, 12, 52, 24);
const OFF: Rect = Rect::new(190, 12, 52, 24);

const VOLUME: Rect = Rect::new(8, 54, W - 16, 134);
const SLIDER_X: i32 = 130;
const SLIDER_W: i32 = 282;
const BOOST_Y: i32 = 70;
const LOUD_Y: i32 = 98;
const BASS_Y: i32 = 126;
/// The Small speakers tick box, and where a click ticks it (its words too).
const SMALL_BOX: Rect = Rect::new(22, 158, 13, 13);
const SMALL_HIT: Rect = Rect::new(18, 154, 420, 21);

const EQ: Rect = Rect::new(8, 204, W - 16, 206);
const PRESET_Y: i32 = 218;
const PRESET_X: i32 = 82;
const PRESET_W: i32 = 64;
const BAND_X: i32 = 78;
const BAND_GAP: i32 = 42;
const TRACK_Y: i32 = 254;
const TRACK_H: i32 = 112;
const KNOB_H: i32 = 10;

pub const RESET: Rect = Rect::new(W - 168, H - 32, 76, 23);
pub const CLOSE: Rect = Rect::new(W - 84, H - 32, 76, 23);

/// What's being dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drag {
    Boost,
    Loudness,
    Bass,
    Band(usize),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    On,
    Off,
    Slider(Drag, f32),
    SmallSpeakers,
    /// A preset by its place: PRESETS, then the user's own.
    Preset(usize),
    Reset,
    Close,
    Nothing,
}

pub struct State {
    pub settings: Settings,
    pub dragging: Option<Drag>,
    /// How the sound comes to go through it (ferro-audio's `~/.asoundrc`),
    /// and whether it does (false: in red, it doesn't).
    pub chain: (String, bool),
}

/// The presets' names, the user's own last.
pub fn preset_names() -> impl Iterator<Item = &'static str> {
    PRESETS.iter().map(|(n, _)| *n).chain([OWN])
}

fn preset_rect(i: usize) -> Rect {
    Rect::new(PRESET_X + i as i32 * (PRESET_W + 2), PRESET_Y, PRESET_W, 22)
}

fn band_x(i: usize) -> i32 {
    BAND_X + i as i32 * BAND_GAP
}

fn travel() -> i32 {
    TRACK_H - KNOB_H
}

/// A band's value (dB) for a height in the window, and back.
pub fn band_from_y(y: i32) -> f32 {
    let t = travel();
    let from_top = (y - TRACK_Y - KNOB_H / 2).clamp(0, t);
    let steps = (2.0 * EQ_MAX) as i32;
    (EQ_MAX - ((from_top * steps + t / 2) / t) as f32).clamp(-EQ_MAX, EQ_MAX)
}

pub fn band_y(db: f32) -> i32 {
    let steps = 2.0 * EQ_MAX;
    TRACK_Y + ((EQ_MAX - db.clamp(-EQ_MAX, EQ_MAX)) * travel() as f32 / steps).round() as i32
}

/// A horizontal slider's value (0..max dB, whole dB) for a place, and back.
pub fn level_from_x(x: i32, max: f32) -> f32 {
    let t = (x - SLIDER_X).clamp(0, SLIDER_W);
    ((t as f32 * max / SLIDER_W as f32).round()).clamp(0.0, max)
}

pub fn level_x(v: f32, max: f32) -> i32 {
    SLIDER_X + (v.clamp(0.0, max) * SLIDER_W as f32 / max).round() as i32
}

/// Where a drag at (x, y) puts what's dragged.
pub fn drag_value(d: Drag, x: i32, y: i32) -> f32 {
    match d {
        Drag::Boost => level_from_x(x, BOOST_MAX),
        Drag::Loudness => level_from_x(x, LOUDNESS_MAX),
        Drag::Bass => level_from_x(x, BASS_MAX),
        Drag::Band(_) => band_from_y(y),
    }
}

pub fn hit(x: i32, y: i32) -> Hit {
    if ON.contains(x, y) {
        return Hit::On;
    }
    if OFF.contains(x, y) {
        return Hit::Off;
    }
    for (row, d) in [(BOOST_Y, Drag::Boost), (LOUD_Y, Drag::Loudness), (BASS_Y, Drag::Bass)] {
        if Rect::new(SLIDER_X - 8, row - 2, SLIDER_W + 16, 24).contains(x, y) {
            return Hit::Slider(d, drag_value(d, x, y));
        }
    }
    if SMALL_HIT.contains(x, y) {
        return Hit::SmallSpeakers;
    }
    if let Some(i) = (0..PRESETS.len() + 1).find(|&i| preset_rect(i).contains(x, y)) {
        return Hit::Preset(i);
    }
    for i in 0..BAND_NAMES.len() {
        if Rect::new(band_x(i) - 16, TRACK_Y - 4, 32, TRACK_H + 8).contains(x, y) {
            return Hit::Slider(Drag::Band(i), band_from_y(y));
        }
    }
    if RESET.contains(x, y) {
        return Hit::Reset;
    }
    if CLOSE.contains(x, y) {
        return Hit::Close;
    }
    Hit::Nothing
}

fn db_text(v: f32) -> String {
    if v > 0.0 {
        format!("+{v:.0} dB")
    } else {
        format!("{v:.0} dB")
    }
}

/// A button that stays in while chosen (On, Off, the preset in use).
fn toggle(s: &mut Surface, r: Rect, label: &str, down: bool) {
    s.fill(r, if down { color::LIGHT } else { color::FACE });
    s.bevel(r, if down { Bevel::Pressed } else { Bevel::Raised });
    let d = i32::from(down);
    let x = r.x + (r.w - text_width(label)) / 2 + d;
    let y = r.y + (r.h - GLYPH_H) / 2 + d;
    if down {
        s.text_bold(x, y, label, color::BLACK);
    } else {
        s.text(x, y, label, color::BLACK);
    }
}

fn level_row(s: &mut Surface, y: i32, label: &str, v: f32, max: f32, value: &str) {
    s.text(20, y + 6, label, color::BLACK);
    let track = Rect::new(SLIDER_X, y + 8, SLIDER_W, 4);
    s.bevel(track, Bevel::Sunken);
    for t in 0..=4 {
        s.vline(SLIDER_X + t * SLIDER_W / 4, y + 17, 3, color::SHADOW);
    }
    let kx = level_x(v, max);
    let knob = Rect::new(kx - 5, y, 11, 20);
    s.fill(knob, color::FACE);
    s.bevel(knob, Bevel::Raised);
    s.text(SLIDER_X + SLIDER_W + 16, y + 6, value, color::NAVY);
}

pub fn draw(s: &mut Surface, st: &State) {
    let set = &st.settings;
    s.fill(Rect::new(0, 0, W, H), color::FACE);

    s.text_bold(14, 20, "Sound Enhancer:", color::BLACK);
    toggle(s, ON, "On", set.on);
    toggle(s, OFF, "Off", !set.on);
    let (note, c) = if set.on { ("Louder, fuller sound", color::NAVY) } else { ("The sound plays as it is", color::SHADOW) };
    s.text(256, 20, note, c);

    s.group_box(VOLUME, "Volume");
    level_row(s, BOOST_Y, "Volume boost", set.boost, BOOST_MAX, &db_text(set.boost));
    let loudness = if set.loudness == 0.0 { "Off".to_string() } else { format!("{:.0} of {LOUDNESS_MAX:.0}", set.loudness) };
    level_row(s, LOUD_Y, "Loudness", set.loudness, LOUDNESS_MAX, &loudness);
    level_row(s, BASS_Y, "Bass boost", set.bass, BASS_MAX, &db_text(set.bass));
    s.fill(SMALL_BOX, color::WHITE);
    s.bevel(SMALL_BOX, Bevel::Sunken);
    if set.small_speakers {
        s.sprite(SMALL_BOX.x + 3, SMALL_BOX.y + 3, ferro_gfx::icons::GLYPH_CHECK, 1);
    }
    s.text(SMALL_BOX.x + 20, SMALL_BOX.y + 3, "Small speakers", color::BLACK);
    s.text(SMALL_BOX.x + 20 + text_width("Small speakers "), SMALL_BOX.y + 3, "(bass higher up, no rattle)", color::SHADOW);

    s.group_box(EQ, "Equalizer");
    s.text(20, PRESET_Y + 7, "Preset:", color::BLACK);
    for (i, name) in preset_names().enumerate() {
        toggle(s, preset_rect(i), name, set.preset == name);
    }
    // The scale: +12 at the top, 0 in the middle, -12 at the bottom.
    for (db, label) in [(EQ_MAX, "+12"), (0.0, "0"), (-EQ_MAX, "-12")] {
        let y = band_y(db) + KNOB_H / 2;
        s.text(BAND_X - 26 - text_width(label) + 8, y - GLYPH_H / 2, label, color::BLACK);
        s.hline(BAND_X - 18, y, band_x(9) - BAND_X + 36, if db == 0.0 { color::SHADOW } else { color::LIGHT });
    }
    for (i, name) in BAND_NAMES.iter().enumerate() {
        let cx = band_x(i);
        s.bevel(Rect::new(cx - 2, TRACK_Y, 5, TRACK_H), Bevel::Sunken);
        let knob = Rect::new(cx - 10, band_y(set.eq[i]), 21, KNOB_H);
        s.fill(knob, color::FACE);
        s.bevel(knob, Bevel::Raised);
        s.hline(knob.x + 3, knob.y + KNOB_H / 2, knob.w - 6, color::SHADOW);
        s.text(cx - text_width(name) / 2, TRACK_Y + TRACK_H + 6, name, color::BLACK);
        let v = format!("{:+.0}", set.eq[i]);
        let v = if set.eq[i] == 0.0 { "0".to_string() } else { v };
        s.text(cx - text_width(&v) / 2, TRACK_Y + TRACK_H + 18, &v, color::NAVY);
    }
    s.text(22, TRACK_Y + TRACK_H + 6, "Hz", color::SHADOW);

    // How it's heard: at once, or why not.
    let (line, ok) = &st.chain;
    s.text(12, H - 24, &ferro_gfx::ellipsize(line, 39), if *ok { color::BLACK } else { color::RED });
    ferro_ui::button(s, RESET, "Reset", true);
    ferro_ui::default_button(s, CLOSE, "Close", true);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> State {
        State { settings: Settings::default(), dragging: None, chain: ("Changes are heard at once.".into(), true) }
    }

    #[test]
    fn band_positions_round_trip() {
        for db in -12..=12 {
            let db = db as f32;
            assert_eq!(band_from_y(band_y(db) + KNOB_H / 2), db, "{db}");
        }
        assert_eq!(band_from_y(0), EQ_MAX);
        assert_eq!(band_from_y(H), -EQ_MAX);
        for v in 0..=12 {
            let v = v as f32;
            assert_eq!(level_from_x(level_x(v, BOOST_MAX), BOOST_MAX), v);
        }
        assert_eq!(level_from_x(-50, BASS_MAX), 0.0);
        assert_eq!(level_from_x(W, BASS_MAX), BASS_MAX);
    }

    #[test]
    fn everything_fits_and_clicks_land() {
        // The last preset button, the last band, the dB text: inside their boxes.
        let last = preset_rect(PRESETS.len());
        assert!(last.x + last.w <= EQ.x + EQ.w - 6);
        assert!(band_x(9) + 16 <= EQ.x + EQ.w - 6);
        assert!(SLIDER_X + SLIDER_W + 16 + text_width("+12 dB").max(text_width("10 of 10")) <= VOLUME.x + VOLUME.w);
        const { assert!(SMALL_HIT.y + SMALL_HIT.h < VOLUME.y + VOLUME.h && SMALL_HIT.y > BASS_Y + 20) };
        assert!(SMALL_BOX.x + 20 + text_width("Small speakers (bass higher up, no rattle)") <= SMALL_HIT.x + SMALL_HIT.w);
        const { assert!(VOLUME.y + VOLUME.h + 12 <= EQ.y) };
        const { assert!(TRACK_Y + TRACK_H + 18 + GLYPH_H <= EQ.y + EQ.h) };
        const { assert!(EQ.y + EQ.h < RESET.y) };

        assert_eq!(hit(ON.x + 3, ON.y + 3), Hit::On);
        assert_eq!(hit(OFF.x + 3, OFF.y + 3), Hit::Off);
        assert_eq!(hit(level_x(4.0, BOOST_MAX), BOOST_Y + 10), Hit::Slider(Drag::Boost, 4.0));
        assert_eq!(hit(level_x(12.0, BASS_MAX), BASS_Y + 10), Hit::Slider(Drag::Bass, 12.0));
        assert_eq!(hit(level_x(7.0, LOUDNESS_MAX), LOUD_Y + 10), Hit::Slider(Drag::Loudness, 7.0));
        assert_eq!(hit(SMALL_BOX.x + 4, SMALL_BOX.y + 4), Hit::SmallSpeakers);
        assert_eq!(hit(SMALL_BOX.x + 60, SMALL_BOX.y + 4), Hit::SmallSpeakers);
        assert_eq!(hit(band_x(3), band_y(-6.0) + 5), Hit::Slider(Drag::Band(3), -6.0));
        let p = preset_rect(2);
        assert_eq!(hit(p.x + 2, p.y + 2), Hit::Preset(2));
        assert_eq!(preset_names().nth(5), Some(OWN));
        assert_eq!(hit(RESET.x + 2, RESET.y + 2), Hit::Reset);
        assert_eq!(hit(CLOSE.x + 2, CLOSE.y + 2), Hit::Close);
        assert_eq!(hit(2, H - 2), Hit::Nothing);
    }

    #[test]
    fn draws_in_every_state() {
        let mut s = Surface::new(W as usize, H as usize);
        let mut st = state();
        draw(&mut s, &st);
        st.settings.on = true;
        st.settings.choose("Voice");
        st.settings.boost = 12.0;
        st.settings.loudness = 10.0;
        st.settings.small_speakers = true;
        st.chain = ("Not in use: ~/.asoundrc is your own".into(), false);
        draw(&mut s, &st);
    }
}
