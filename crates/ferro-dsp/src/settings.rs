//! What the Sound Enhancer is set to, in `~/.config/ferro/enhancer.conf`:
//! written by its window (ferro-enhancer), read by the plugin in every
//! program that plays sound. Only the settings: no history of them.
//!
//! ```text
//! on = true
//! boost = 4
//! loudness = 5
//! bass = 3
//! small_speakers = true
//! preset = Music
//! eq = 3 2 1 0 -1 0 1 2 3 3
//! own = 0 0 0 0 0 0 0 0 0 0
//! ```
//!
//! Files from before Loudness and Small speakers load with both off.

use std::path::PathBuf;

/// The equalizer's bands: their middles, in Hz.
pub const BANDS: [f32; 10] = [31.0, 62.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0];
/// And their names, as under the sliders.
pub const BAND_NAMES: [&str; 10] = ["31", "62", "125", "250", "500", "1k", "2k", "4k", "8k", "16k"];
/// How far a band goes up or down, in dB.
pub const EQ_MAX: f32 = 12.0;
/// The most the volume boost and the bass boost add, in dB.
pub const BOOST_MAX: f32 = 12.0;
pub const BASS_MAX: f32 = 12.0;
/// Loudness goes from 0 (off) to this.
pub const LOUDNESS_MAX: f32 = 10.0;

/// The name of the user's own setting of the bands.
pub const OWN: &str = "My own";

/// The ready-made settings of the bands, in dB.
pub const PRESETS: [(&str, [f32; 10]); 5] = [
    ("Flat", [0.0; 10]),
    ("Bass", [6.0, 5.0, 4.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ("Voice", [-4.0, -3.0, -2.0, 0.0, 1.0, 3.0, 4.0, 3.0, 1.0, 0.0]),
    ("Music", [3.0, 2.0, 1.0, 0.0, -1.0, 0.0, 1.0, 2.0, 3.0, 3.0]),
    ("Treble", [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 2.0, 4.0, 5.0, 6.0]),
];

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// Off: every sample goes through as it came.
    pub on: bool,
    /// Louder than 100%, in dB (the limiter keeps it from clipping).
    pub boost: f32,
    /// Quiet and middle parts brought up towards the peaks: 0 (off) to
    /// LOUDNESS_MAX.
    pub loudness: f32,
    /// More bass, in dB (a low shelf; with small_speakers, a peak higher up).
    pub bass: f32,
    /// For laptop-sized speakers: the bass boost higher up, and the deepest
    /// bass (which they can't play) rolled off.
    pub small_speakers: bool,
    /// One of PRESETS' names, or OWN.
    pub preset: String,
    /// The bands as they are now, in dB.
    pub eq: [f32; 10],
    /// The user's own setting of the bands, kept while a preset is chosen.
    pub own: [f32; 10],
}

impl Default for Settings {
    fn default() -> Self {
        Self { on: false, boost: 0.0, loudness: 0.0, bass: 0.0, small_speakers: false, preset: "Flat".into(), eq: [0.0; 10], own: [0.0; 10] }
    }
}

/// The file the settings are in: `~/.config/ferro/enhancer.conf`.
pub fn path() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|h| !h.is_empty()).map(|h| PathBuf::from(h).join(".config/ferro/enhancer.conf"))
}

fn clamp(v: f32, lo: f32, hi: f32) -> f32 {
    if v.is_finite() {
        v.clamp(lo, hi)
    } else {
        0.0
    }
}

fn bands(text: &str) -> Option<[f32; 10]> {
    let mut out = [0.0; 10];
    let mut n = 0;
    for word in text.split_whitespace() {
        if n == 10 {
            return None;
        }
        out[n] = clamp(word.parse().ok()?, -EQ_MAX, EQ_MAX);
        n += 1;
    }
    (n == 10).then_some(out)
}

fn number(v: f32) -> String {
    if v.fract() == 0.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.1}")
    }
}

impl Settings {
    /// Reads the file's text. Lines it doesn't know, and numbers out of
    /// range, don't stop it: those are left as they'd be by default, or
    /// brought in range.
    pub fn parse(text: &str) -> Self {
        let mut s = Self::default();
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else { continue };
            let value = value.trim();
            match key.trim() {
                "on" => s.on = matches!(value, "true" | "1" | "yes" | "on"),
                "boost" => s.boost = value.parse().map_or(0.0, |v| clamp(v, 0.0, BOOST_MAX)),
                "loudness" => s.loudness = value.parse().map_or(0.0, |v| clamp(v, 0.0, LOUDNESS_MAX)),
                "small_speakers" => s.small_speakers = matches!(value, "true" | "1" | "yes" | "on"),
                "bass" => s.bass = value.parse().map_or(0.0, |v| clamp(v, 0.0, BASS_MAX)),
                "preset" if !value.is_empty() => s.preset = value.to_string(),
                "eq" => s.eq = bands(value).unwrap_or(s.eq),
                "own" => s.own = bands(value).unwrap_or(s.own),
                _ => {}
            }
        }
        s
    }

    pub fn to_text(&self) -> String {
        let list = |b: &[f32; 10]| b.iter().map(|&v| number(v)).collect::<Vec<_>>().join(" ");
        format!(
            "# Sound Enhancer (ferro-enhancer). Programs playing sound read it as they play.\n\
             on = {}\nboost = {}\nloudness = {}\nbass = {}\nsmall_speakers = {}\npreset = {}\neq = {}\nown = {}\n",
            self.on,
            number(self.boost),
            number(self.loudness),
            number(self.bass),
            self.small_speakers,
            self.preset,
            list(&self.eq),
            list(&self.own)
        )
    }

    /// The bands a preset (or OWN) stands for.
    pub fn preset_bands(&self, name: &str) -> Option<[f32; 10]> {
        if name == OWN {
            return Some(self.own);
        }
        PRESETS.iter().find(|(n, _)| *n == name).map(|(_, b)| *b)
    }

    /// Chooses a preset (or the user's own bands).
    pub fn choose(&mut self, name: &str) {
        if let Some(b) = self.preset_bands(name) {
            self.eq = b;
            self.preset = name.to_string();
        }
    }

    /// Moves one band: the bands are the user's own from then on.
    pub fn set_band(&mut self, band: usize, db: f32) {
        if band < 10 {
            self.eq[band] = clamp(db, -EQ_MAX, EQ_MAX);
            self.own = self.eq;
            self.preset = OWN.to_string();
        }
    }

    /// Writes the settings: a new file put in the old one's place, so a
    /// program reading it never sees half of one.
    pub fn save(&self) -> std::io::Result<()> {
        let path = path().ok_or_else(|| std::io::Error::other("HOME isn't set"))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("conf.new");
        std::fs::write(&tmp, self.to_text())?;
        std::fs::rename(&tmp, &path)
    }

    pub fn load() -> Self {
        path().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| Self::parse(&t)).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let mut s = Settings { on: true, boost: 4.0, loudness: 7.0, bass: 2.5, small_speakers: true, ..Default::default() };
        s.choose("Music");
        s.own = [1.0, -2.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 12.0];
        assert_eq!(Settings::parse(&s.to_text()), s);
        assert_eq!(Settings::parse(&Settings::default().to_text()), Settings::default());
    }

    #[test]
    fn files_from_before_load_as_they_did() {
        let s = Settings::parse("on = true\nboost = 6\nbass = 3\npreset = Flat\neq = 0 0 0 0 0 0 0 0 0 0\nown = 0 0 0 0 0 0 0 0 0 0\n");
        assert_eq!(s, Settings { on: true, boost: 6.0, bass: 3.0, ..Default::default() });
        assert_eq!((s.loudness, s.small_speakers), (0.0, false));
        assert_eq!(Settings::parse("loudness = 50\n").loudness, LOUDNESS_MAX);
    }

    #[test]
    fn bad_values_are_kept_in_range() {
        let s = Settings::parse("on = maybe\nboost = 99\nbass = -3\neq = 1 2 3\nown = 50 0 0 0 0 0 0 0 0 NaN\nnonsense\n");
        assert!(!s.on);
        assert_eq!(s.boost, BOOST_MAX);
        assert_eq!(s.bass, 0.0);
        assert_eq!(s.eq, [0.0; 10]);
        assert_eq!(s.own[0], EQ_MAX);
        assert_eq!(s.own[9], 0.0);
        assert_eq!(Settings::parse(""), Settings::default());
    }

    #[test]
    fn moving_a_band_makes_them_ones_own() {
        let mut s = Settings::default();
        s.choose("Bass");
        assert_eq!(s.eq, PRESETS[1].1);
        s.set_band(9, 4.0);
        assert_eq!(s.preset, OWN);
        assert_eq!(s.own[0], 6.0);
        assert_eq!(s.own[9], 4.0);
        s.choose("Flat");
        assert_eq!(s.eq, [0.0; 10]);
        s.choose(OWN);
        assert_eq!(s.eq[9], 4.0);
        s.choose("No such preset");
        assert_eq!(s.preset, OWN);
    }
}
