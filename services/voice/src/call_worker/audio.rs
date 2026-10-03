//! Audio conversion between LiveKit frames and the Voice Session core.
//!
//! - Caller → core: LiveKit delivers interleaved i16 frames (we ask the native
//!   sink for 16 kHz mono, so usually this is a no-op); the core takes PCM16 LE
//!   mono at 16 kHz ([`CORE_INPUT_RATE`]).
//! - Core → caller: the core emits PCM16 LE mono at its `outputSampleRate`
//!   (Kokoro: 24 kHz); the bot track publishes at [`TRACK_RATE`] in 10 ms frames.
//!
//! The resampler is linear interpolation. That is enough here: upsampling TTS
//! adds no aliasing, and caller audio from SIP is already band-limited to the
//! 8 kHz narrowband the carrier sent, so 16 kHz has no content to fold back.

/// Sample rate the Voice Session core expects for mic audio.
pub const CORE_INPUT_RATE: u32 = 16_000;
/// Sample rate of the published bot track.
pub const TRACK_RATE: u32 = 48_000;

/// Average interleaved channels down to mono.
pub fn interleaved_to_mono(data: &[i16], channels: u32) -> Vec<i16> {
    let ch = channels.max(1) as usize;
    if ch == 1 {
        return data.to_vec();
    }
    data.chunks_exact(ch)
        .map(|f| (f.iter().map(|&s| s as i32).sum::<i32>() / ch as i32) as i16)
        .collect()
}

pub fn to_pcm16le(samples: &[i16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 2);
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// PCM16 LE bytes → samples, carrying an odd trailing byte to the next chunk.
#[derive(Default)]
pub struct Pcm16Decoder {
    carry: Option<u8>,
}

impl Pcm16Decoder {
    pub fn decode(&mut self, bytes: &[u8]) -> Vec<i16> {
        let mut out = Vec::with_capacity(bytes.len() / 2 + 1);
        let mut rest = bytes;
        if let Some(lo) = self.carry.take() {
            match rest.split_first() {
                Some((&hi, tail)) => {
                    out.push(i16::from_le_bytes([lo, hi]));
                    rest = tail;
                }
                None => {
                    self.carry = Some(lo);
                    return out;
                }
            }
        }
        let mut it = rest.chunks_exact(2);
        for pair in &mut it {
            out.push(i16::from_le_bytes([pair[0], pair[1]]));
        }
        if let [b] = it.remainder() {
            self.carry = Some(*b);
        }
        out
    }

    pub fn reset(&mut self) {
        self.carry = None;
    }
}

/// Streaming linear resampler. Keeps phase across chunks so frame boundaries
/// don't click.
pub struct Resampler {
    from: u32,
    to: u32,
    step: f64,
    /// Position of the next output sample in input coordinates, relative to the
    /// start of the next chunk (`-1.0` means "the last sample of the previous chunk").
    t: f64,
    prev: Option<i16>,
}

impl Resampler {
    pub fn new(from: u32, to: u32) -> Self {
        Self { from, to, step: from as f64 / to as f64, t: 0.0, prev: None }
    }

    pub fn is_passthrough(&self) -> bool {
        self.from == self.to
    }

    pub fn reset(&mut self) {
        self.t = 0.0;
        self.prev = None;
    }

    pub fn process(&mut self, input: &[i16]) -> Vec<i16> {
        if self.is_passthrough() || input.is_empty() {
            return input.to_vec();
        }
        let n = input.len() as f64;
        let prev = self.prev.unwrap_or(input[0]);
        let at = |i: isize| -> f64 {
            if i < 0 {
                prev as f64
            } else {
                input[i as usize] as f64
            }
        };
        let mut out = Vec::with_capacity((n / self.step) as usize + 2);
        while self.t <= n - 1.0 {
            let i = self.t.floor() as isize;
            let frac = self.t - i as f64;
            let a = at(i);
            let b = if (i + 1) as f64 <= n - 1.0 { at(i + 1) } else { a };
            out.push((a + (b - a) * frac).round().clamp(i16::MIN as f64, i16::MAX as f64) as i16);
            self.t += self.step;
        }
        self.t -= n;
        self.prev = input.last().copied();
        out
    }
}

/// Splits a sample stream into fixed frames (10 ms for the LiveKit source).
pub struct Framer {
    frame: usize,
    buf: Vec<i16>,
}

impl Framer {
    pub fn new(sample_rate: u32, frame_ms: u32) -> Self {
        Self { frame: (sample_rate * frame_ms / 1000) as usize, buf: Vec::new() }
    }

    pub fn frame_len(&self) -> usize {
        self.frame
    }

    pub fn push(&mut self, samples: &[i16]) -> Vec<Vec<i16>> {
        self.buf.extend_from_slice(samples);
        let mut out = Vec::new();
        while self.buf.len() >= self.frame {
            out.push(self.buf.drain(..self.frame).collect());
        }
        out
    }

    /// Remaining samples padded with silence to one frame (end of utterance).
    pub fn flush(&mut self) -> Option<Vec<i16>> {
        if self.buf.is_empty() {
            return None;
        }
        let mut f = std::mem::take(&mut self.buf);
        f.resize(self.frame, 0);
        Some(f)
    }

    /// Drop buffered audio (barge-in).
    pub fn clear(&mut self) {
        self.buf.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_downmix() {
        assert_eq!(interleaved_to_mono(&[100, 300, -50, -150], 2), vec![200, -100]);
        assert_eq!(interleaved_to_mono(&[1, 2, 3], 1), vec![1, 2, 3]);
    }

    #[test]
    fn pcm_roundtrip_with_odd_split() {
        let s = vec![0i16, 1, -1, i16::MAX, i16::MIN, 1234];
        let bytes = to_pcm16le(&s);
        let mut d = Pcm16Decoder::default();
        let mut got = d.decode(&bytes[..5]);
        got.extend(d.decode(&bytes[5..]));
        assert_eq!(got, s);
    }

    #[test]
    fn resample_lengths_across_chunks() {
        // 24 kHz → 48 kHz: 100 ms in 10 chunks = 4800 samples (±1).
        let mut r = Resampler::new(24_000, 48_000);
        let total: usize = (0..10).map(|_| r.process(&vec![1000i16; 240]).len()).sum();
        assert!((4799..=4801).contains(&total), "{total}");
        // 48 kHz → 16 kHz: 480 → 160 per 10 ms.
        let mut r = Resampler::new(48_000, 16_000);
        let total: usize = (0..10).map(|_| r.process(&vec![0i16; 480]).len()).sum();
        assert_eq!(total, 1600);
    }

    #[test]
    fn resample_preserves_dc_and_ramps() {
        let mut r = Resampler::new(24_000, 48_000);
        assert!(r.process(&[500; 64]).iter().all(|&s| s == 500));
        let mut r = Resampler::new(16_000, 48_000);
        let ramp: Vec<i16> = (0..100).map(|i| i * 30).collect();
        let out = r.process(&ramp);
        // Monotone, no overshoot.
        assert!(out.windows(2).all(|w| w[1] >= w[0]));
        assert!(*out.last().unwrap() <= 99 * 30);
    }

    #[test]
    fn resample_continuity_at_boundary() {
        // Same result whether fed in one chunk or two.
        let sig: Vec<i16> = (0..480).map(|i| ((i as f64 / 7.0).sin() * 8000.0) as i16).collect();
        let mut a = Resampler::new(48_000, 16_000);
        let one = a.process(&sig);
        let mut b = Resampler::new(48_000, 16_000);
        let mut two = b.process(&sig[..233]);
        two.extend(b.process(&sig[233..]));
        assert_eq!(one, two);
    }

    #[test]
    fn framer_splits_and_flushes() {
        let mut f = Framer::new(48_000, 10);
        assert_eq!(f.frame_len(), 480);
        assert_eq!(f.push(&[1; 1000]).len(), 2);
        let last = f.flush().unwrap();
        assert_eq!(last.len(), 480);
        assert_eq!(&last[..40], &[1; 40][..]);
        assert!(last[40..].iter().all(|&s| s == 0));
        assert!(f.flush().is_none());
        f.push(&[1; 100]);
        f.clear();
        assert!(f.flush().is_none());
    }
}
