//! The sound processing: the equalizer's bands and the bass shelf
//! (biquad filters), the volume boost, and a limiter so the boosted sound
//! never goes past full scale. None of it looks ahead, so it adds no
//! delay: each sample comes out as soon as it goes in.

use crate::settings::{Settings, BANDS};

/// The loudest a sample comes out (just under 0 dBFS), however much it's
/// boosted.
pub const CEILING: f32 = 0.98;
const _: () = assert!(CEILING < 1.0);

/// The bands' width: about an octave each.
const BAND_Q: f64 = 1.41;
/// Where the bass boost's shelf is.
const BASS_HZ: f64 = 100.0;
/// How quickly the boost follows a change, and the limiter lets go.
const GAIN_SECONDS: f64 = 0.010;
const RELEASE_SECONDS: f64 = 0.120;
/// How long On and Off take to fade one into the other (no click).
const FADE_SECONDS: f64 = 0.020;

#[derive(Clone, Copy, Debug, Default)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    /// Each channel's state (transposed direct form II).
    z1: [f64; 2],
    z2: [f64; 2],
    on: bool,
}

impl Biquad {
    fn set(&mut self, c: [f64; 6]) {
        let [b0, b1, b2, a0, a1, a2] = c;
        if !self.on {
            self.z1 = [0.0; 2];
            self.z2 = [0.0; 2];
        }
        (self.b0, self.b1, self.b2, self.a1, self.a2) = (b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0);
        self.on = true;
    }

    /// A peak (or dip) of `db` around `hz` (RBJ's cookbook).
    fn peaking(rate: f64, hz: f64, q: f64, db: f64) -> [f64; 6] {
        let a = 10f64.powf(db / 40.0);
        let w = std::f64::consts::TAU * hz / rate;
        let alpha = w.sin() / (2.0 * q);
        let cos = w.cos();
        [1.0 + alpha * a, -2.0 * cos, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * cos, 1.0 - alpha / a]
    }

    /// Everything under `hz` raised by `db` (a low shelf, slope 1).
    fn low_shelf(rate: f64, hz: f64, db: f64) -> [f64; 6] {
        let a = 10f64.powf(db / 40.0);
        let w = std::f64::consts::TAU * hz / rate;
        let (sin, cos) = (w.sin(), w.cos());
        let k = 2.0 * a.sqrt() * sin / 2.0 * std::f64::consts::SQRT_2;
        [
            a * ((a + 1.0) - (a - 1.0) * cos + k),
            2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
            a * ((a + 1.0) - (a - 1.0) * cos - k),
            (a + 1.0) + (a - 1.0) * cos + k,
            -2.0 * ((a - 1.0) + (a + 1.0) * cos),
            (a + 1.0) + (a - 1.0) * cos - k,
        ]
    }

    #[inline]
    fn run(&mut self, ch: usize, x: f64) -> f64 {
        let y = self.b0 * x + self.z1[ch];
        self.z1[ch] = self.b1 * x - self.a1 * y + self.z2[ch];
        self.z2[ch] = self.b2 * x - self.a2 * y;
        y
    }

    /// Tiny leftovers of silence, which make some CPUs slow, made zero.
    fn flush(&mut self) {
        for z in self.z1.iter_mut().chain(self.z2.iter_mut()) {
            if z.abs() < 1e-20 {
                *z = 0.0;
            }
        }
    }
}

/// One stereo stream's enhancer.
pub struct Enhancer {
    rate: f64,
    /// The bass shelf, then the ten bands; those set to 0 dB are skipped.
    filters: [Biquad; 11],
    on: bool,
    /// The boost (linear), and the one being moved to.
    gain: f64,
    gain_to: f64,
    gain_step: f64,
    /// The limiter's gain now (1: not limiting).
    limit: f64,
    release: f64,
    /// How much of the enhanced sound is heard (0 off, 1 on).
    mix: f64,
    fade: f64,
}

impl Enhancer {
    pub fn new(rate: u32) -> Self {
        let rate = f64::from(rate.max(8000));
        Self {
            rate,
            filters: [Biquad::default(); 11],
            on: false,
            gain: 1.0,
            gain_to: 1.0,
            gain_step: 1.0 - (-1.0 / (GAIN_SECONDS * rate)).exp(),
            limit: 1.0,
            release: 1.0 - (-1.0 / (RELEASE_SECONDS * rate)).exp(),
            mix: 0.0,
            fade: 1.0 / (FADE_SECONDS * rate),
        }
    }

    /// Takes new settings; a stream playing hears them at once.
    pub fn set(&mut self, s: &Settings) {
        if s.on && !self.on && self.mix == 0.0 {
            // Back on from fully off: nothing left over from before.
            for f in &mut self.filters {
                f.on = false;
            }
            self.limit = 1.0;
            self.gain = 10f64.powf(f64::from(s.boost) / 20.0);
        }
        self.on = s.on;
        self.gain_to = 10f64.powf(f64::from(s.boost) / 20.0);
        let nyquist = self.rate / 2.0;
        let bass = f64::from(s.bass);
        if bass.abs() >= 0.05 {
            self.filters[0].set(Biquad::low_shelf(self.rate, BASS_HZ, bass));
        } else {
            self.filters[0].on = false;
        }
        for (i, (&hz, &db)) in BANDS.iter().zip(s.eq.iter()).enumerate() {
            let (hz, db) = (f64::from(hz), f64::from(db));
            // Bands too high for the stream's rate (16 kHz at 22 kHz) aren't there.
            if db.abs() >= 0.05 && hz < nyquist * 0.9 {
                self.filters[i + 1].set(Biquad::peaking(self.rate, hz, BAND_Q, db));
            } else {
                self.filters[i + 1].on = false;
            }
        }
    }

    /// Whether it's off and done fading, so samples go through untouched.
    pub fn bypassed(&self) -> bool {
        !self.on && self.mix == 0.0
    }

    /// Enhances a stereo stream's samples where they are.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if self.bypassed() {
            return;
        }
        let target = if self.on { 1.0 } else { 0.0 };
        let ceiling = f64::from(CEILING);
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let dry = [f64::from(*l), f64::from(*r)].map(|v| if v.is_finite() { v } else { 0.0 });
            let mut x = dry;
            for f in self.filters.iter_mut().filter(|f| f.on) {
                x = [f.run(0, x[0]), f.run(1, x[1])];
            }
            self.gain += (self.gain_to - self.gain) * self.gain_step;
            x = x.map(|v| v * self.gain);
            // The limiter: down at once to what keeps the louder channel
            // under the ceiling, back up slowly.
            let peak = x[0].abs().max(x[1].abs());
            let want = if peak > ceiling { ceiling / peak } else { 1.0 };
            self.limit = if want < self.limit { want } else { self.limit + (want - self.limit) * self.release };
            let wet = x.map(|v| v * self.limit);
            if self.mix != target {
                self.mix = if target > self.mix { (self.mix + self.fade).min(1.0) } else { (self.mix - self.fade).max(0.0) };
            }
            let out = if self.mix >= 1.0 { wet } else { [dry[0] + (wet[0] - dry[0]) * self.mix, dry[1] + (wet[1] - dry[1]) * self.mix] };
            if self.mix == 0.0 {
                // Faded out: from here on, as it came.
                break;
            }
            // Never past the ceiling, even fading in or out, whatever
            // rounding does.
            *l = out[0].clamp(-ceiling, ceiling) as f32;
            *r = out[1].clamp(-ceiling, ceiling) as f32;
        }
        for f in &mut self.filters {
            f.flush();
        }
    }
}
