//! Streaming mono resampler and PCM16 helpers.
//!
//! Clients send mic audio at whatever rate they have (browsers 48 kHz,
//! phone lines 8 kHz); engines want 16 kHz. TTS produces 24 kHz; a phone
//! leg may want 8 or 48 kHz. The resampler keeps its phase and filter
//! history across calls, so frame boundaries never click.
//!
//! Method: when downsampling, a windowed-sinc low-pass at the output
//! Nyquist first (anti-aliasing), then linear interpolation. Cheap enough
//! for 20 ms frames on one core, and plenty for speech.

/// Streaming resampler between two fixed rates.
#[derive(Debug, Clone)]
pub struct Resampler {
    from: u32,
    to: u32,
    /// Input position of the next output sample, relative to `history[0]`.
    pos: f64,
    step: f64,
    /// Low-pass taps (empty when upsampling or same rate).
    taps: Vec<f32>,
    /// Unfiltered input tail needed by the FIR.
    fir_tail: Vec<f32>,
    /// Filtered samples not yet consumed by interpolation (keeps one back).
    history: Vec<f32>,
}

impl Resampler {
    pub fn new(from: u32, to: u32) -> Self {
        assert!(from > 0 && to > 0, "sample rates must be positive");
        let taps = if to < from {
            lowpass_taps(to as f64 / 2.0 * 0.9, from as f64)
        } else {
            Vec::new()
        };
        Self {
            from,
            to,
            pos: 0.0,
            step: from as f64 / to as f64,
            fir_tail: vec![0.0; taps.len().saturating_sub(1)],
            taps,
            history: Vec::new(),
        }
    }

    pub fn from_rate(&self) -> u32 {
        self.from
    }

    pub fn to_rate(&self) -> u32 {
        self.to
    }

    pub fn is_passthrough(&self) -> bool {
        self.from == self.to
    }

    /// Resample the next chunk of input.
    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        if self.is_passthrough() {
            return input.to_vec();
        }
        let filtered = self.filter(input);
        self.history.extend_from_slice(&filtered);
        let mut out = Vec::with_capacity((input.len() as f64 / self.step) as usize + 2);
        // Linear interpolation needs history[i] and history[i + 1].
        while self.pos + 1.0 < self.history.len() as f64 {
            let i = self.pos.floor() as usize;
            let frac = (self.pos - i as f64) as f32;
            out.push(self.history[i] * (1.0 - frac) + self.history[i + 1] * frac);
            self.pos += self.step;
        }
        let consumed = (self.pos.floor() as usize).min(self.history.len());
        self.history.drain(..consumed);
        self.pos -= consumed as f64;
        out
    }

    fn filter(&mut self, input: &[f32]) -> Vec<f32> {
        if self.taps.is_empty() {
            return input.to_vec();
        }
        let n = self.taps.len();
        let mut buf = std::mem::take(&mut self.fir_tail);
        buf.extend_from_slice(input);
        let out: Vec<f32> = (0..input.len())
            .map(|i| {
                let window = &buf[i..i + n];
                window.iter().zip(&self.taps).map(|(x, t)| x * t).sum()
            })
            .collect();
        self.fir_tail = buf[buf.len() - (n - 1)..].to_vec();
        out
    }
}

/// Blackman-windowed sinc low-pass, odd length, unity DC gain.
fn lowpass_taps(cutoff_hz: f64, rate: f64) -> Vec<f32> {
    let len = 31usize;
    let fc = cutoff_hz / rate;
    let mid = (len / 2) as f64;
    let mut taps: Vec<f64> = (0..len)
        .map(|i| {
            let x = i as f64 - mid;
            let sinc = if x == 0.0 {
                2.0 * fc
            } else {
                (2.0 * std::f64::consts::PI * fc * x).sin() / (std::f64::consts::PI * x)
            };
            let w = 0.42 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / (len - 1) as f64).cos()
                + 0.08 * (4.0 * std::f64::consts::PI * i as f64 / (len - 1) as f64).cos();
            sinc * w
        })
        .collect();
    let sum: f64 = taps.iter().sum();
    taps.iter_mut().for_each(|t| *t /= sum);
    taps.into_iter().map(|t| t as f32).collect()
}

/// PCM16 little-endian bytes → f32. A trailing odd byte is ignored.
pub fn pcm16le_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
        .collect()
}

/// f32 → PCM16 little-endian bytes (clamped).
pub fn f32_to_pcm16le(samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 2);
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, rate: u32, secs: f32) -> Vec<f32> {
        let n = (rate as f32 * secs) as usize;
        (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin() * 0.5)
            .collect()
    }

    /// Estimate frequency from rising zero crossings.
    fn est_freq(x: &[f32], rate: u32) -> f32 {
        let crossings = x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        crossings as f32 / (x.len() as f32 / rate as f32)
    }

    fn run_chunked(r: &mut Resampler, x: &[f32], chunk: usize) -> Vec<f32> {
        x.chunks(chunk).flat_map(|c| r.process(c)).collect()
    }

    #[test]
    fn upsamples_8k_to_16k() {
        let x = sine(440.0, 8000, 1.0);
        let mut r = Resampler::new(8000, 16000);
        let y = run_chunked(&mut r, &x, 160); // 20 ms frames
        assert!((y.len() as i64 - 16000).abs() <= 2, "len {}", y.len());
        let f = est_freq(&y, 16000);
        assert!((f - 440.0).abs() < 3.0, "freq {f}");
    }

    #[test]
    fn downsamples_48k_to_16k() {
        let x = sine(440.0, 48000, 1.0);
        let mut r = Resampler::new(48000, 16000);
        let y = run_chunked(&mut r, &x, 960);
        assert!((y.len() as i64 - 16000).abs() <= 2, "len {}", y.len());
        let f = est_freq(&y[100..], 16000);
        assert!((f - 440.0).abs() < 3.0, "freq {f}");
        // Amplitude survives the low-pass.
        let peak = y[200..].iter().cloned().fold(0.0f32, f32::max);
        assert!((peak - 0.5).abs() < 0.03, "peak {peak}");
    }

    #[test]
    fn downsampling_rejects_aliases() {
        // 12 kHz is above the 8 kHz Nyquist of 16 kHz output: must be attenuated.
        let x = sine(12000.0, 48000, 0.5);
        let mut r = Resampler::new(48000, 16000);
        let y = run_chunked(&mut r, &x, 960);
        let rms = (y[100..].iter().map(|s| s * s).sum::<f32>() / (y.len() - 100) as f32).sqrt();
        assert!(rms < 0.02, "alias rms {rms}");
    }

    #[test]
    fn chunking_does_not_change_output() {
        let x = sine(300.0, 24000, 0.2);
        let a = run_chunked(&mut Resampler::new(24000, 16000), &x, 480);
        let b = run_chunked(&mut Resampler::new(24000, 16000), &x, 7);
        assert_eq!(a.len(), b.len());
        assert!(a.iter().zip(&b).all(|(p, q)| (p - q).abs() < 1e-5));
    }

    #[test]
    fn pcm_roundtrip() {
        let x = vec![0.0, 0.5, -0.5, 1.0, -1.0];
        let y = pcm16le_to_f32(&f32_to_pcm16le(&x));
        assert!(x.iter().zip(&y).all(|(a, b)| (a - b).abs() < 1e-3));
    }
}
