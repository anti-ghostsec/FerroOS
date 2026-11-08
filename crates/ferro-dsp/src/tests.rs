//! The processing, measured.

use crate::settings::{Settings, BANDS, EQ_MAX};
use crate::{Enhancer, CEILING};

const RATE: u32 = 48000;

fn on() -> Settings {
    Settings { on: true, ..Default::default() }
}

/// A plain sum of noise from a fixed seed (no crates needed).
fn noise(n: usize, amp: f32, mut seed: u32) -> Vec<f32> {
    (0..n)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed as f32 / u32::MAX as f32 * 2.0 - 1.0) * amp
        })
        .collect()
}

fn sine(hz: f32, amp: f32, rate: u32, n: usize) -> Vec<f32> {
    (0..n).map(|i| amp * (std::f32::consts::TAU * hz * i as f32 / rate as f32).sin()).collect()
}

/// Runs a stereo stream through, a period (1024 frames) at a time.
fn run(e: &mut Enhancer, l: &mut [f32], r: &mut [f32]) {
    for (a, b) in l.chunks_mut(1024).zip(r.chunks_mut(1024)) {
        e.process(a, b);
    }
}

fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|&v| f64::from(v) * f64::from(v)).sum::<f64>() / x.len() as f64).sqrt()
}

/// How much louder (dB) a sine at `hz` comes out, once settled.
fn gain_at(s: &Settings, hz: f32, rate: u32) -> f64 {
    let n = rate as usize;
    let input = sine(hz, 0.01, rate, n);
    let (mut l, mut r) = (input.clone(), input.clone());
    let mut e = Enhancer::new(rate);
    e.set(s);
    run(&mut e, &mut l, &mut r);
    let tail = n / 2..;
    20.0 * (rms(&l[tail.clone()]) / rms(&input[tail])).log10()
}

#[test]
fn off_leaves_every_sample_as_it_was() {
    let src = noise(48000, 1.5, 7);
    let (mut l, mut r) = (src.clone(), src.iter().map(|v| -v).collect::<Vec<_>>());
    let mut e = Enhancer::new(RATE);
    let mut s = on();
    s.on = false;
    s.boost = 12.0;
    s.bass = 12.0;
    s.eq = [EQ_MAX; 10];
    e.set(&s);
    run(&mut e, &mut l, &mut r);
    assert!(l.iter().zip(&src).all(|(a, b)| a.to_bits() == b.to_bits()));
    assert!(r.iter().zip(&src).all(|(a, b)| a.to_bits() == (-b).to_bits()));
}

#[test]
fn turned_off_it_fades_out_then_leaves_samples_alone() {
    let mut e = Enhancer::new(RATE);
    let mut s = on();
    s.boost = 6.0;
    s.eq[3] = 6.0;
    e.set(&s);
    let (mut l, mut r) = (noise(4800, 0.3, 1), noise(4800, 0.3, 2));
    run(&mut e, &mut l, &mut r);
    s.on = false;
    e.set(&s);
    // The fade is 20 ms: within one period of 1024 frames at 48 kHz.
    let (mut l, mut r) = (noise(2048, 0.3, 3), noise(2048, 0.3, 4));
    run(&mut e, &mut l, &mut r);
    assert!(e.bypassed());
    let src = noise(4096, 0.9, 5);
    let (mut l, mut r) = (src.clone(), src.clone());
    run(&mut e, &mut l, &mut r);
    assert_eq!(l, src);
    assert_eq!(r, src);
}

#[test]
fn on_and_flat_changes_nothing_and_adds_no_delay() {
    let mut e = Enhancer::new(RATE);
    e.set(&on());
    let mut l = vec![0.0f32; 512];
    l[0] = 0.5;
    l[100] = -0.25;
    let mut r = l.clone();
    let want = l.clone();
    run(&mut e, &mut l, &mut r);
    assert_eq!(l, want);
    assert_eq!(r, want);
}

#[test]
fn volume_boost_is_the_gain_asked_for() {
    for boost in [3.0, 6.0, 12.0] {
        let s = Settings { boost, ..on() };
        let g = gain_at(&s, 1000.0, RATE);
        assert!((g - f64::from(boost)).abs() < 0.1, "boost {boost}: {g:.2} dB");
    }
    assert!(gain_at(&on(), 1000.0, RATE).abs() < 0.01);
}

#[test]
fn each_band_lifts_its_own_frequencies() {
    for rate in [44100, 48000] {
        for (i, &hz) in BANDS.iter().enumerate() {
            for db in [6.0, -6.0] {
                let mut s = on();
                s.set_band(i, db);
                let at = gain_at(&s, hz, rate);
                assert!((at - f64::from(db)).abs() < 0.5, "{hz} Hz at {rate}: {at:.2} dB for {db}");
                // Two octaves away, little of it is left.
                for far in [hz / 4.0, hz * 4.0] {
                    if far > 20.0 && far < rate as f32 * 0.45 {
                        let g = gain_at(&s, far, rate);
                        assert!(g.abs() < 1.5, "{hz} Hz band at {far} Hz: {g:.2} dB");
                    }
                }
            }
        }
    }
}

#[test]
fn bass_boost_is_a_low_shelf() {
    let s = Settings { bass: 9.0, ..on() };
    let low = gain_at(&s, 30.0, RATE);
    assert!((low - 9.0).abs() < 1.0, "30 Hz: {low:.2}");
    let mid = gain_at(&s, 2000.0, RATE);
    assert!(mid.abs() < 0.3, "2 kHz: {mid:.2}");
    let s = Settings { bass: 0.0, ..on() };
    assert!(gain_at(&s, 30.0, RATE).abs() < 0.01);
}

#[test]
fn limiter_never_goes_past_full_scale() {
    let mut s = on();
    s.boost = 12.0;
    s.bass = 12.0;
    s.eq = [EQ_MAX; 10];
    for rate in [22050, 44100, 48000, 96000] {
        let mut e = Enhancer::new(rate);
        e.set(&s);
        let n = rate as usize;
        let mut inputs = vec![noise(n, 1.0, 9), sine(50.0, 1.0, rate, n), sine(5000.0, 1.0, rate, n)];
        // Square waves: all at full scale, then silence, then again.
        let square = |i: usize| match ((i / 300) % 7 < 4, (i / 30) % 2) {
            (false, _) => 0.0,
            (true, 0) => 1.0,
            (true, _) => -1.0,
        };
        inputs.push((0..n).map(square).collect());
        for input in inputs {
            let (mut l, mut r) = (input.clone(), input.iter().map(|v| v * 0.5).collect::<Vec<_>>());
            run(&mut e, &mut l, &mut r);
            let peak = l.iter().chain(&r).fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(peak <= CEILING, "{rate}: peak {peak}");
            // And it isn't simply silenced: the boost still makes it loud.
            assert!(rms(&l) > 0.3, "{rate}: rms {}", rms(&l));
        }
    }
}

#[test]
fn bad_samples_come_out_as_silence() {
    let mut e = Enhancer::new(RATE);
    e.set(&Settings { boost: 6.0, ..on() });
    let mut l = vec![f32::NAN, f32::INFINITY, 0.1, f32::NEG_INFINITY];
    let mut r = l.clone();
    for _ in 0..200 {
        let (mut a, mut b) = (l.clone(), r.clone());
        e.process(&mut a, &mut b);
        (l[2], r[2]) = (0.1, 0.1);
        assert!(a.iter().chain(&b).all(|v| v.is_finite() && v.abs() <= 1.0));
    }
}
