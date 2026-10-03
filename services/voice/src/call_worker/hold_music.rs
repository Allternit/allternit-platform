//! Hold music: a soft chord pad synthesized in code.
//!
//! No audio file ships with the worker, so there is nothing to license. The loop
//! is 8 s at 48 kHz (the room track rate). Every partial is `k / 8` Hz, so it
//! completes a whole number of cycles in the loop, and the slow level swell is
//! one cycle per loop. Sample `n` is computed from the integer phase
//! `(k * n) mod LOOP_SAMPLES`, so the loop wraps with no click and no drift
//! however long the call is held.

use std::f64::consts::TAU;

use super::audio::TRACK_RATE;

/// Samples per published frame (10 ms at [`TRACK_RATE`]).
pub const FRAME_SAMPLES: usize = (TRACK_RATE / 100) as usize;
/// Loop length in seconds.
const LOOP_SECS: u32 = 8;
/// Samples in one loop.
pub const LOOP_SAMPLES: usize = (TRACK_RATE * LOOP_SECS) as usize;
/// Peak level, about -20 dBFS: a quiet pad that doesn't startle the caller.
const PEAK: f64 = 0.10;

/// One partial: frequency in eighths of a hertz (`k`), relative level, and the
/// phase offset of its swell in fractions of the loop.
struct Partial {
    k: u32,
    gain: f64,
    swell_phase: f64,
}

/// Cmaj9 voiced in the telephone band (G.711 passes 300–3400 Hz; a pad below
/// that would vanish on the line), each note doubled with a 0.25 Hz detuned
/// copy so the pad shimmers slowly instead of sounding like a test tone.
fn partials() -> Vec<Partial> {
    // C4 E4 G4 B4 D5 and C5, rounded to a multiple of 1/8 Hz.
    let notes = [261.63, 329.63, 392.00, 493.88, 587.33, 523.25];
    let gains = [1.0, 0.8, 0.7, 0.45, 0.4, 0.5];
    let mut out = Vec::new();
    for (i, (f, g)) in notes.iter().zip(gains).enumerate() {
        let k = (f * 8.0_f64).round() as u32;
        out.push(Partial { k, gain: g, swell_phase: i as f64 / notes.len() as f64 });
        out.push(Partial { k: k + 2, gain: g * 0.6, swell_phase: (i as f64 / notes.len() as f64) + 0.5 });
    }
    out
}

fn raw_sample(parts: &[Partial], n: usize) -> f64 {
    let m = (n % LOOP_SAMPLES) as u64;
    let total = LOOP_SAMPLES as u64;
    let mut s = 0.0;
    for p in parts {
        let phase = ((p.k as u64 * m) % total) as f64 / total as f64;
        let swell = 0.75 + 0.25 * (TAU * (m as f64 / total as f64 + p.swell_phase)).sin();
        s += p.gain * swell * (TAU * phase).sin();
    }
    s
}

/// Endless, seamless source of 10 ms frames of hold music.
pub struct HoldMusic {
    loop_pcm: Vec<i16>,
    pos: usize,
}

impl Default for HoldMusic {
    fn default() -> Self {
        Self::new()
    }
}

impl HoldMusic {
    pub fn new() -> Self {
        let parts = partials();
        let raw: Vec<f64> = (0..LOOP_SAMPLES).map(|n| raw_sample(&parts, n)).collect();
        let max = raw.iter().fold(0.0_f64, |m, s| m.max(s.abs())).max(f64::EPSILON);
        let scale = PEAK / max * i16::MAX as f64;
        let loop_pcm = raw.iter().map(|s| (s * scale).round() as i16).collect();
        Self { loop_pcm, pos: 0 }
    }

    /// The next frame; wraps to the start of the loop forever.
    pub fn next_frame(&mut self) -> Vec<i16> {
        let mut f = Vec::with_capacity(FRAME_SAMPLES);
        for _ in 0..FRAME_SAMPLES {
            f.push(self.loop_pcm[self.pos]);
            self.pos = (self.pos + 1) % LOOP_SAMPLES;
        }
        f
    }

    /// Start over at the top of the loop (a new hold begins quietly).
    pub fn rewind(&mut self) {
        self.pos = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(s: &[i16]) -> f64 {
        (s.iter().map(|x| (*x as f64).powi(2)).sum::<f64>() / s.len() as f64).sqrt()
    }

    #[test]
    fn loop_is_a_whole_number_of_frames() {
        assert_eq!(LOOP_SAMPLES % FRAME_SAMPLES, 0);
        assert_eq!(FRAME_SAMPLES, 480);
    }

    #[test]
    fn frames_are_quiet_but_audible() {
        let mut m = HoldMusic::new();
        let mut peak = 0i16;
        let mut total = Vec::new();
        for _ in 0..(LOOP_SAMPLES / FRAME_SAMPLES) {
            let f = m.next_frame();
            assert_eq!(f.len(), FRAME_SAMPLES);
            peak = peak.max(f.iter().map(|s| s.abs()).max().unwrap());
            total.extend(f);
        }
        // About -20 dBFS peak, never clipping, not silence.
        assert!(peak <= (i16::MAX as f64 * PEAK) as i16 + 2, "peak {peak}");
        assert!(peak > 2500, "peak {peak}");
        assert!(rms(&total) > 500.0, "rms {}", rms(&total));
    }

    #[test]
    fn loop_wraps_without_a_click() {
        let parts = partials();
        // The synthesis function itself is exactly periodic...
        for n in [0usize, 1, 479, 12_345, 100_000] {
            assert_eq!(raw_sample(&parts, n), raw_sample(&parts, n + LOOP_SAMPLES), "n={n}");
        }
        // ...so the step across the seam is no bigger than any other step.
        let mut m = HoldMusic::new();
        let mut all = Vec::new();
        for _ in 0..(2 * LOOP_SAMPLES / FRAME_SAMPLES) {
            all.extend(m.next_frame());
        }
        let max_step = |range: std::ops::Range<usize>| {
            range.map(|i| (all[i + 1] as i32 - all[i] as i32).abs()).max().unwrap()
        };
        let inside = max_step(0..LOOP_SAMPLES - 1);
        let seam = (all[LOOP_SAMPLES] as i32 - all[LOOP_SAMPLES - 1] as i32).abs();
        assert!(seam <= inside, "seam step {seam} exceeds in-loop max {inside}");
        // And the second pass is a bit-exact repeat of the first.
        assert_eq!(&all[..LOOP_SAMPLES], &all[LOOP_SAMPLES..]);
    }

    #[test]
    fn rewind_restarts_the_loop() {
        let mut m = HoldMusic::new();
        let first = m.next_frame();
        m.next_frame();
        m.rewind();
        assert_eq!(m.next_frame(), first);
    }

    #[test]
    fn energy_sits_in_the_telephone_band() {
        // Goertzel at the lowest partial vs. a sub-300 Hz bin: the pad has
        // essentially nothing below the G.711 passband.
        let mut m = HoldMusic::new();
        let s: Vec<f64> = (0..4800).flat_map(|_| m.next_frame()).take(48_000).map(f64::from).collect();
        let power = |f: f64| {
            let (mut re, mut im) = (0.0, 0.0);
            for (i, x) in s.iter().enumerate() {
                let a = TAU * f * i as f64 / TRACK_RATE as f64;
                re += x * a.cos();
                im += x * a.sin();
            }
            re * re + im * im
        };
        assert!(power(392.0) > 100.0 * power(150.0));
    }
}
