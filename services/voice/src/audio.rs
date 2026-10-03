//! Audio decoding, resampling, and WAV encoding helpers.
//!
//! The STT pipeline accepts any sample rate: everything is decoded to
//! mono `f32` and resampled to 16 kHz before it reaches the VAD/recogniser.
//! 8 kHz phone audio (including G.711 μ-law WAV) is explicitly supported.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AudioError {
    #[error("invalid WAV: {0}")]
    InvalidWav(String),
    #[error("unsupported audio container (expected RIFF/WAV or raw s16le PCM)")]
    UnsupportedContainer,
}

/// Decode an arbitrary audio blob to mono `f32` samples plus the sample rate.
///
/// - RIFF/WAVE containers are parsed (PCM16, float32, μ-law, A-law; any rate,
///   any channel count — downmixed to mono).
/// - Anything else is treated as raw 16 kHz s16le mono PCM (the capture
///   format the previous whisper path accepted).
pub fn decode_any(bytes: &[u8]) -> Result<(Vec<f32>, u32), AudioError> {
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WAVE" {
        decode_wav(bytes)
    } else if bytes.len() >= 4 && bytes.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        Err(AudioError::UnsupportedContainer)
    } else {
        let n = bytes.len() / 2;
        let mut samples = Vec::with_capacity(n);
        for chunk in bytes.chunks_exact(2) {
            samples.push(i16::from_le_bytes([chunk[0], chunk[1]]) as f32 / 32768.0);
        }
        Ok((samples, 16_000))
    }
}

/// Parse a RIFF/WAVE file into mono f32 samples.
pub fn decode_wav(bytes: &[u8]) -> Result<(Vec<f32>, u32), AudioError> {
    if bytes.len() < 12 {
        return Err(AudioError::InvalidWav("truncated header".into()));
    }
    let mut pos = 12usize;
    let mut fmt: Option<FmtChunk> = None;
    let mut data: Option<&[u8]> = None;

    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        let body_start = pos + 8;
        let body_end = body_start.saturating_add(size).min(bytes.len());
        match id {
            b"fmt " => {
                if size < 16 || body_end > bytes.len() {
                    return Err(AudioError::InvalidWav("bad fmt chunk".into()));
                }
                let b = &bytes[body_start..body_end];
                let format_tag = u16::from_le_bytes([b[0], b[1]]);
                let tag = if format_tag == 0xFFFE && size >= 40 {
                    // WAVE_FORMAT_EXTENSIBLE: real tag lives in the extension.
                    u16::from_le_bytes([b[24], b[25]])
                } else {
                    format_tag
                };
                fmt = Some(FmtChunk {
                    tag,
                    channels: u16::from_le_bytes([b[2], b[3]]).max(1),
                    sample_rate: u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
                    bits: u16::from_le_bytes([b[14], b[15]]),
                });
            }
            b"data" => {
                data = Some(&bytes[body_start..body_end]);
            }
            _ => {}
        }
        pos = body_start + size + (size & 1); // chunks are 2-byte aligned
    }

    let fmt = fmt.ok_or_else(|| AudioError::InvalidWav("missing fmt chunk".into()))?;
    let data = data.ok_or_else(|| AudioError::InvalidWav("missing data chunk".into()))?;
    if fmt.sample_rate == 0 {
        return Err(AudioError::InvalidWav("zero sample rate".into()));
    }

    let samples = decode_pcm(data, &fmt);
    let mono = downmix(samples, fmt.channels);
    Ok((mono, fmt.sample_rate))
}

struct FmtChunk {
    tag: u16,
    channels: u16,
    sample_rate: u32,
    bits: u16,
}

const TAG_PCM: u16 = 1;
const TAG_FLOAT: u16 = 3;
const TAG_ALAW: u16 = 6;
const TAG_MULAW: u16 = 7;

fn decode_pcm(data: &[u8], fmt: &FmtChunk) -> Vec<f32> {
    match (fmt.tag, fmt.bits) {
        (TAG_PCM, 16) => data
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
            .collect(),
        (TAG_PCM, 8) => data.iter().map(|&b| (b as f32 - 128.0) / 128.0).collect(),
        (TAG_PCM, 24) => data
            .chunks_exact(3)
            .map(|c| {
                let v =
                    i32::from_le_bytes([c[0], c[1], c[2], if c[2] & 0x80 != 0 { 0xFF } else { 0 }]);
                v as f32 / 8_388_608.0
            })
            .collect(),
        (TAG_PCM, 32) => data
            .chunks_exact(4)
            .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f32 / 2_147_483_648.0)
            .collect(),
        (TAG_FLOAT, 32) => data
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect(),
        (TAG_MULAW, _) => data.iter().map(|&b| decode_mulaw(b)).collect(),
        (TAG_ALAW, _) => data.iter().map(|&b| decode_alaw(b)).collect(),
        _ => Vec::new(),
    }
}

/// Average channels to mono (interleaved input).
fn downmix(samples: Vec<f32>, channels: u16) -> Vec<f32> {
    let channels = channels as usize;
    if channels <= 1 {
        return samples;
    }
    let frames = samples.len() / channels;
    let mut out = Vec::with_capacity(frames);
    for frame in samples.chunks_exact(channels) {
        out.push(frame.iter().sum::<f32>() / channels as f32);
    }
    out
}

/// G.711 μ-law decode (standard bias algorithm).
pub fn decode_mulaw(b: u8) -> f32 {
    const BIAS: i32 = 0x84;
    let u = !b;
    let sign = u & 0x80;
    let exponent = ((u >> 4) & 0x07) as i32;
    let mantissa = (u & 0x0F) as i32;
    let magnitude = (((mantissa << 3) + BIAS) << exponent) - BIAS;
    let v = if sign != 0 { -magnitude } else { magnitude };
    v as f32 / 32768.0
}

/// G.711 A-law decode (canonical two's-complement G.711, no bias term).
pub fn decode_alaw(a: u8) -> f32 {
    let u = a ^ 0x55;
    let sign = u & 0x80;
    let exponent = ((u >> 4) & 0x07) as i32;
    let mantissa = (u & 0x0F) as i32;
    let sample = if exponent == 0 {
        (mantissa << 4) + 8
    } else {
        ((mantissa << 3) + 0x108) << (exponent - 1)
    };
    let v = if sign != 0 { sample } else { -sample };
    v as f32 / 32768.0
}

/// Resample to 16 kHz with sherpa-onnx's windowed-sinc resampler (Kaldi's
/// `LinearResample`: a proper low-pass filter despite the name). Keeps the
/// whole 300–3400 Hz PSTN band for 8 kHz input.
pub fn resample_to_16k(input: &[f32], from_rate: u32) -> Vec<f32> {
    resample(input, from_rate, 16_000)
}

/// Resample between arbitrary rates (one shot, flushes the filter tail).
pub fn resample(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || input.is_empty() {
        return input.to_vec();
    }
    if from_rate == 0 || to_rate == 0 {
        return Vec::new();
    }
    match sherpa_onnx::LinearResampler::create(from_rate as i32, to_rate as i32) {
        Some(r) => r.resample(input, true),
        None => Vec::new(),
    }
}

/// Streaming resampler to 16 kHz (keeps filter state between chunks).
pub struct StreamResampler {
    inner: Option<sherpa_onnx::LinearResampler>,
}

impl StreamResampler {
    pub fn new(from_rate: u32) -> Self {
        let inner = if from_rate == 16_000 || from_rate == 0 {
            None
        } else {
            sherpa_onnx::LinearResampler::create(from_rate as i32, 16_000)
        };
        Self { inner }
    }

    pub fn push(&self, samples: &[f32], flush: bool) -> Vec<f32> {
        match &self.inner {
            Some(r) => r.resample(samples, flush),
            None => samples.to_vec(),
        }
    }
}

/// Wrap raw s16le PCM in a WAV header.
pub fn pcm16le_to_wav(pcm: &[u8], sample_rate: u32, channels: u16) -> Vec<u8> {
    let data_len = pcm.len() as u32;
    let byte_rate = sample_rate * u32::from(channels) * 2;
    let block_align = channels * 2;
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(pcm);
    out
}

/// Convert f32 samples (-1..1) to s16le bytes.
pub fn f32_to_pcm16le(samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 2);
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// Encode f32 samples as a 16-bit mono WAV blob.
pub fn wav_from_f32(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    pcm16le_to_wav(&f32_to_pcm16le(samples), sample_rate, 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_pcm16_roundtrip() {
        let samples: Vec<f32> = (0..1600).map(|i| (i as f32 * 0.001).sin()).collect();
        let pcm = f32_to_pcm16le(&samples);
        let wav = pcm16le_to_wav(&pcm, 16_000, 1);
        let (decoded, rate) = decode_wav(&wav).unwrap();
        assert_eq!(rate, 16_000);
        assert_eq!(decoded.len(), samples.len());
        for (a, b) in decoded.iter().zip(samples.iter()) {
            assert!((a - b).abs() < 1e-3, "sample mismatch {a} vs {b}");
        }
    }

    #[test]
    fn wav_mulaw_8k_decodes_and_resamples() {
        // 8 kHz μ-law WAV of silence codes (0xFF ≈ 0 in μ-law).
        let pcm: Vec<u8> = vec![0xFF; 8000];
        let data_len = pcm.len() as u32;
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&7u16.to_le_bytes()); // μ-law
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&8000u32.to_le_bytes());
        out.extend_from_slice(&8000u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&8u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        out.extend_from_slice(&pcm);

        let (decoded, rate) = decode_wav(&out).unwrap();
        assert_eq!(rate, 8_000);
        assert_eq!(decoded.len(), 8000);
        assert!(decoded.iter().all(|s| s.abs() < 0.01));
        let resampled = resample_to_16k(&decoded, 8_000);
        assert!(
            (resampled.len() as i64 - 16_000).abs() <= 16,
            "{}",
            resampled.len()
        );
    }

    #[test]
    fn resample_48k_length_and_silence() {
        let input = vec![0.0f32; 48_000];
        let out = resample_to_16k(&input, 48_000);
        assert!((out.len() as i64 - 16_000).abs() <= 16, "{}", out.len());
        assert!(out.iter().all(|s| s.abs() < 1e-6));
    }

    fn tone(freq: f32, rate: u32, secs: f32) -> Vec<f32> {
        (0..(rate as f32 * secs) as usize)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin())
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|s| s * s).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    #[test]
    fn resample_8k_keeps_whole_phone_band() {
        // 300 Hz and 3.4 kHz (the PSTN band edges) must both survive
        // 8 kHz -> 16 kHz. A full-scale sine has RMS 0.707.
        for f in [300.0, 1000.0, 3400.0] {
            let out = resample_to_16k(&tone(f, 8_000, 1.0), 8_000);
            let r = rms(&out[2_000..14_000]);
            assert!(r > 0.6, "{f} Hz attenuated to rms {r}");
        }
    }

    #[test]
    fn resample_48k_rejects_above_nyquist() {
        // 12 kHz at 48 kHz must be filtered out (16 kHz output has an 8 kHz
        // Nyquist), while 1 kHz passes.
        let hi = resample_to_16k(&tone(12_000.0, 48_000, 1.0), 48_000);
        let lo = resample_to_16k(&tone(1_000.0, 48_000, 1.0), 48_000);
        assert!(rms(&hi[2_000..14_000]) < 0.05);
        assert!(rms(&lo[2_000..14_000]) > 0.6);
    }

    #[test]
    fn stream_resampler_matches_one_shot_length() {
        let input = tone(440.0, 8_000, 2.0);
        let r = StreamResampler::new(8_000);
        let chunks: Vec<&[f32]> = input.chunks(1_000).collect();
        let mut out = Vec::new();
        for (i, c) in chunks.iter().enumerate() {
            out.extend(r.push(c, i + 1 == chunks.len()));
        }
        assert!((out.len() as i64 - 32_000).abs() <= 32, "{}", out.len());
    }

    #[test]
    fn mulaw_matches_g711_table() {
        // Reference values from the ITU G.711 μ-law decode table (16-bit).
        for (code, expect) in [
            (0x00u8, -32124i32),
            (0x7F, 0),
            (0x80, 32124),
            (0xFF, 0),
            (0xEF, 132),
            (0xDE, 428),
        ] {
            let got = (decode_mulaw(code) * 32768.0).round() as i32;
            assert_eq!(got, expect, "code {code:#04x}");
        }
    }

    #[test]
    fn mulaw_known_values() {
        // G.711 μ-law canonical pairs: 0xFF ≈ 0, 0x80 max positive, 0x00 max negative.
        assert!(decode_mulaw(0xFF).abs() < 0.01);
        assert!(decode_mulaw(0x80) > 0.9);
        assert!(decode_mulaw(0x00) < -0.9);
    }

    #[test]
    fn alaw_known_values() {
        // G.711 A-law: 0xD5 is the zero code, 0xAA max positive, 0x55 ≈ zero-negative.
        assert!(decode_alaw(0xD5).abs() < 0.01);
        assert!(decode_alaw(0xD5) > 0.0);
        assert!(decode_alaw(0xAA) > 0.7);
        assert!(decode_alaw(0x55).abs() < 0.01);
        assert!(decode_alaw(0x55) < 0.0);
    }

    #[test]
    fn raw_pcm_fallback() {
        let pcm = f32_to_pcm16le(&[0.0, 0.5, -0.5]);
        let (decoded, rate) = decode_any(&pcm).unwrap();
        assert_eq!(rate, 16_000);
        assert_eq!(decoded.len(), 3);
    }
}
