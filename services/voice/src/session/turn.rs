//! Smart Turn v3.2 end-of-turn detection.
//!
//! Model: pipecat-ai Smart Turn v3.2, CPU (int8) ONNX, BSD-2-Clause,
//! from huggingface.co/pipecat-ai/smart-turn-v3 at a pinned revision (the
//! project publishes weights there, not as GitHub release assets). Input is
//! Whisper log-mel features (80 × 800) of the last 8 s of 16 kHz audio,
//! output is the probability that the speaker has finished.
//!
//! Runtime: the ONNX Runtime that sherpa-onnx already links statically.
//! The `ort` crate is built with `alternative-backend` (it links nothing)
//! and is pointed at that runtime's `OrtGetApiBase`, so the binary carries
//! exactly one onnxruntime. Feature extraction is plain Rust and always
//! compiled (and tested); only the ONNX session needs the `sherpa` feature.

use std::f64::consts::PI;

/// Pinned Smart Turn model.
pub struct ModelPin {
    pub file_name: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub size: u64,
    pub license: &'static str,
}

pub const SMART_TURN_V3_2: ModelPin = ModelPin {
    file_name: "smart-turn-v3.2-cpu.onnx",
    url: "https://huggingface.co/pipecat-ai/smart-turn-v3/resolve/f766f81d3cfdf7737ac64aad813d91bbfd56bf93/smart-turn-v3.2-cpu.onnx",
    sha256: "2bb026316b14a660486a75b1733cd3fbab8c2fd0314dc9af7be49f8cca967e4f",
    size: 8_679_182,
    license: "BSD-2-Clause",
};

/// Model id reported in `session.ready`.
pub const SMART_TURN_MODEL_ID: &str = "smart-turn-v3.2";

const SAMPLE_RATE: usize = 16_000;
const N_FFT: usize = 400;
const HOP: usize = 160;
const N_MELS: usize = 80;
const N_FRAMES: usize = 800;
const N_SAMPLES: usize = 8 * SAMPLE_RATE;
const N_BINS: usize = N_FFT / 2 + 1;

/// Whisper-style log-mel extractor, matching `transformers`'
/// `WhisperFeatureExtractor(chunk_length=8)` with `do_normalize=True`
/// as Smart Turn's reference `inference.py` calls it.
pub struct WhisperFeatures {
    window: Vec<f64>,
    /// Dense mel filter bank, `N_MELS × N_BINS`.
    filters: Vec<f64>,
    /// Twiddles `exp(-2πi k / N_FFT)`.
    twiddles: Vec<(f64, f64)>,
}

impl Default for WhisperFeatures {
    fn default() -> Self {
        Self::new()
    }
}

impl WhisperFeatures {
    pub fn new() -> Self {
        // Periodic Hann.
        let window = (0..N_FFT)
            .map(|n| 0.5 - 0.5 * (2.0 * PI * n as f64 / N_FFT as f64).cos())
            .collect();
        let twiddles = (0..N_FFT)
            .map(|k| {
                let a = -2.0 * PI * k as f64 / N_FFT as f64;
                (a.cos(), a.sin())
            })
            .collect();
        Self {
            window,
            filters: slaney_mel_filters(),
            twiddles,
        }
    }

    /// Features for `audio` (16 kHz mono), as a row-major `[80, 800]` buffer.
    /// Uses the last 8 s; shorter audio is zero-padded at the start.
    pub fn compute(&self, audio: &[f32]) -> Vec<f32> {
        let mut x = vec![0.0f64; N_SAMPLES];
        let take = audio.len().min(N_SAMPLES);
        for (dst, src) in x[N_SAMPLES - take..]
            .iter_mut()
            .zip(&audio[audio.len() - take..])
        {
            *dst = *src as f64;
        }
        // Zero-mean unit-variance over the padded 8 s, as HF does without an attention mask.
        let mean = x.iter().sum::<f64>() / N_SAMPLES as f64;
        let var = x.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / N_SAMPLES as f64;
        let scale = 1.0 / (var + 1e-7).sqrt();
        x.iter_mut().for_each(|v| *v = (*v - mean) * scale);

        // Centered frames with reflect padding.
        let pad = N_FFT / 2;
        let reflect = |i: isize| -> f64 {
            let n = N_SAMPLES as isize;
            let j = if i < 0 {
                -i
            } else if i >= n {
                2 * (n - 1) - i
            } else {
                i
            };
            x[j as usize]
        };

        let mut mel = vec![0.0f64; N_MELS * N_FRAMES];
        let mut buf = vec![(0.0f64, 0.0f64); N_FFT];
        let mut scratch = vec![(0.0f64, 0.0f64); N_FFT];
        let mut power = vec![0.0f64; N_BINS];
        for t in 0..N_FRAMES {
            let start = (t * HOP) as isize - pad as isize;
            for (n, b) in buf.iter_mut().enumerate() {
                *b = (reflect(start + n as isize) * self.window[n], 0.0);
            }
            fft(&mut buf, &mut scratch, &self.twiddles, 1);
            for (k, p) in power.iter_mut().enumerate() {
                *p = buf[k].0 * buf[k].0 + buf[k].1 * buf[k].1;
            }
            for m in 0..N_MELS {
                let row = &self.filters[m * N_BINS..(m + 1) * N_BINS];
                let e: f64 = row.iter().zip(&power).map(|(f, p)| f * p).sum();
                mel[m * N_FRAMES + t] = e.max(1e-10).log10();
            }
        }
        let max = mel.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        mel.iter()
            .map(|v| ((v.max(max - 8.0) + 4.0) / 4.0) as f32)
            .collect()
    }
}

/// Mixed-radix decimation-in-time FFT for sizes that divide `N_FFT`.
/// `stride` maps this level's twiddles onto the `N_FFT` table.
fn fft(x: &mut [(f64, f64)], scratch: &mut [(f64, f64)], tw: &[(f64, f64)], stride: usize) {
    let n = x.len();
    if n == 1 {
        return;
    }
    let p = (2..=n)
        .find(|&p| n.is_multiple_of(p))
        .expect("n > 1 has a factor");
    let m = n / p;
    // Gather the p decimated subsequences into scratch, transform each.
    for r in 0..p {
        for k in 0..m {
            scratch[r * m + k] = x[r + p * k];
        }
    }
    for r in 0..p {
        // `x` is free until the combine step, so it doubles as the child's scratch.
        fft(
            &mut scratch[r * m..(r + 1) * m],
            &mut x[r * m..(r + 1) * m],
            tw,
            stride * p,
        );
    }
    // Combine: X[k + m q] = Σ_r W_n^{r (k + m q)} Y_r[k].
    for q in 0..p {
        for k in 0..m {
            let idx = k + m * q;
            let mut acc = (0.0, 0.0);
            for r in 0..p {
                let y = scratch[r * m + k];
                let w = tw[((r * idx) % n) * stride];
                acc.0 += y.0 * w.0 - y.1 * w.1;
                acc.1 += y.0 * w.1 + y.1 * w.0;
            }
            x[idx] = acc;
        }
    }
}

fn hz_to_mel(f: f64) -> f64 {
    let (f_sp, min_log_hz) = (200.0 / 3.0, 1000.0);
    let min_log_mel = min_log_hz / f_sp;
    let logstep = 6.4f64.ln() / 27.0;
    if f >= min_log_hz {
        min_log_mel + (f / min_log_hz).ln() / logstep
    } else {
        f / f_sp
    }
}

fn mel_to_hz(m: f64) -> f64 {
    let (f_sp, min_log_hz) = (200.0 / 3.0, 1000.0);
    let min_log_mel = min_log_hz / f_sp;
    let logstep = 6.4f64.ln() / 27.0;
    if m >= min_log_mel {
        min_log_hz * (logstep * (m - min_log_mel)).exp()
    } else {
        f_sp * m
    }
}

/// `transformers.audio_utils.mel_filter_bank(201, 80, 0, 8000, 16000,
/// norm="slaney", mel_scale="slaney")`, row-major `[mel][bin]`.
fn slaney_mel_filters() -> Vec<f64> {
    let (lo, hi) = (hz_to_mel(0.0), hz_to_mel(8000.0));
    let filter_freqs: Vec<f64> = (0..N_MELS + 2)
        .map(|i| mel_to_hz(lo + (hi - lo) * i as f64 / (N_MELS + 1) as f64))
        .collect();
    let fft_freqs: Vec<f64> = (0..N_BINS)
        .map(|k| (SAMPLE_RATE as f64 / 2.0) * k as f64 / (N_BINS - 1) as f64)
        .collect();
    let mut out = vec![0.0; N_MELS * N_BINS];
    for m in 0..N_MELS {
        let (f0, f1, f2) = (filter_freqs[m], filter_freqs[m + 1], filter_freqs[m + 2]);
        let enorm = 2.0 / (f2 - f0);
        for (k, &f) in fft_freqs.iter().enumerate() {
            let down = (f - f0) / (f1 - f0);
            let up = (f2 - f) / (f2 - f1);
            out[m * N_BINS + k] = down.min(up).max(0.0) * enorm;
        }
    }
    out
}

#[cfg(feature = "sherpa")]
pub use onnx::SmartTurn;

#[cfg(feature = "sherpa")]
mod onnx {
    use super::{WhisperFeatures, N_FRAMES, N_MELS, SMART_TURN_V3_2};
    use crate::session::engine::{EngineError, TurnDetector};
    use std::path::{Path, PathBuf};
    use std::sync::Once;

    unsafe extern "C" {
        /// Exported by the onnxruntime that sherpa-onnx links.
        fn OrtGetApiBase() -> *const ort::sys::OrtApiBase;
    }

    static API: Once = Once::new();

    /// Point `ort` at sherpa-onnx's onnxruntime (once per process).
    fn ensure_api() -> Result<(), EngineError> {
        let mut err = None;
        API.call_once(|| {
            // SAFETY: OrtGetApiBase is the stable C entry point of the linked
            // onnxruntime; GetApi returns a static table or null.
            let api = unsafe { ((*OrtGetApiBase()).GetApi)(ort::sys::ORT_API_VERSION) };
            if api.is_null() {
                err = Some(EngineError::unavailable(
                    "linked onnxruntime does not support the ort API version",
                ));
                return;
            }
            // SAFETY: `api` is non-null and points to a static OrtApi table.
            ort::set_api(unsafe { std::ptr::read(api) });
        });
        match err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    pub struct SmartTurn {
        session: ort::session::Session,
        features: WhisperFeatures,
    }

    impl SmartTurn {
        pub fn load(model: &Path, threads: usize) -> Result<Self, EngineError> {
            ensure_api()?;
            fn fail(e: impl std::fmt::Display) -> EngineError {
                EngineError::unavailable(format!("smart turn model: {e}"))
            }
            let session = ort::session::Session::builder()
                .map_err(fail)?
                .with_intra_threads(threads.max(1))
                .map_err(fail)?
                .commit_from_file(model)
                .map_err(fail)?;
            let mut detector = Self {
                session,
                features: WhisperFeatures::new(),
            };
            // The first run allocates and optimises lazily (~1 s); pay for it
            // here, not on the user's first turn.
            detector.predict(&[0.0; 16_000])?;
            Ok(detector)
        }

        /// Download (if missing) and verify the pinned model under `dir`.
        /// Blocking; call from a blocking context inside a tokio runtime.
        pub fn ensure_model(dir: &Path) -> Result<PathBuf, EngineError> {
            let pin = &SMART_TURN_V3_2;
            let path = dir.join(pin.file_name);
            if path.is_file() && sha256_file(&path)? == pin.sha256 {
                return Ok(path);
            }
            std::fs::create_dir_all(dir)
                .map_err(|e| EngineError::unavailable(format!("create {}: {e}", dir.display())))?;
            let handle = tokio::runtime::Handle::try_current().map_err(|_| {
                EngineError::unavailable("smart turn download needs a tokio runtime")
            })?;
            let bytes = handle.block_on(async {
                let resp = reqwest::get(pin.url).await.map_err(|e| e.to_string())?;
                if !resp.status().is_success() {
                    return Err(format!("GET {}: HTTP {}", pin.url, resp.status()));
                }
                resp.bytes().await.map_err(|e| e.to_string())
            });
            let bytes =
                bytes.map_err(|e| EngineError::unavailable(format!("smart turn download: {e}")))?;
            let got = sha256_hex(&bytes);
            if got != pin.sha256 {
                return Err(EngineError::unavailable(format!(
                    "smart turn sha256 mismatch: expected {}, got {got}",
                    pin.sha256
                )));
            }
            let tmp = path.with_extension("onnx.part");
            std::fs::write(&tmp, &bytes)
                .and_then(|_| std::fs::rename(&tmp, &path))
                .map_err(|e| EngineError::unavailable(format!("write {}: {e}", path.display())))?;
            Ok(path)
        }
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        use sha2::Digest;
        sha2::Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    fn sha256_file(path: &Path) -> Result<String, EngineError> {
        let bytes = std::fs::read(path)
            .map_err(|e| EngineError::unavailable(format!("read {}: {e}", path.display())))?;
        Ok(sha256_hex(&bytes))
    }

    impl TurnDetector for SmartTurn {
        fn predict(&mut self, audio: &[f32]) -> Result<f32, EngineError> {
            let feats = self.features.compute(audio);
            let input = ort::value::Tensor::from_array(([1usize, N_MELS, N_FRAMES], feats))
                .map_err(|e| EngineError::failed(format!("smart turn input: {e}")))?;
            let outputs = self
                .session
                .run(ort::inputs!["input_features" => input])
                .map_err(|e| EngineError::failed(format!("smart turn run: {e}")))?;
            let (_, data) = outputs[0]
                .try_extract_tensor::<f32>()
                .map_err(|e| EngineError::failed(format!("smart turn output: {e}")))?;
            data.first()
                .copied()
                .ok_or_else(|| EngineError::failed("smart turn returned no output"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fft_matches_naive_dft() {
        let tw: Vec<(f64, f64)> = (0..N_FFT)
            .map(|k| {
                let a = -2.0 * PI * k as f64 / N_FFT as f64;
                (a.cos(), a.sin())
            })
            .collect();
        let input: Vec<(f64, f64)> = (0..N_FFT)
            .map(|i| (((i * 7919) % 101) as f64 / 50.0 - 1.0, 0.0))
            .collect();
        let mut x = input.clone();
        let mut scratch = vec![(0.0, 0.0); N_FFT];
        fft(&mut x, &mut scratch, &tw, 1);
        for k in [0usize, 1, 7, 100, 199, 200, 399] {
            let mut acc = (0.0, 0.0);
            for (n, v) in input.iter().enumerate() {
                let w = tw[(k * n) % N_FFT];
                acc.0 += v.0 * w.0 - v.1 * w.1;
                acc.1 += v.0 * w.1 + v.1 * w.0;
            }
            assert!(
                (acc.0 - x[k].0).abs() < 1e-9 && (acc.1 - x[k].1).abs() < 1e-9,
                "bin {k}"
            );
        }
    }

    #[test]
    fn mel_filters_match_slaney_reference_points() {
        let f = slaney_mel_filters();
        // Every filter has some weight; first filter peaks in the lowest bins.
        for m in 0..N_MELS {
            assert!(
                f[m * N_BINS..(m + 1) * N_BINS].iter().any(|v| *v > 0.0),
                "empty filter {m}"
            );
        }
        assert!(f[1] > 0.0);
        assert!((hz_to_mel(1000.0) - 15.0).abs() < 1e-9);
        assert!((mel_to_hz(hz_to_mel(4321.0)) - 4321.0).abs() < 1e-6);
    }

    #[test]
    fn features_shape_and_range() {
        let tone: Vec<f32> = (0..SAMPLE_RATE * 2)
            .map(|i| {
                (2.0 * std::f32::consts::PI * 440.0 * i as f32 / SAMPLE_RATE as f32).sin() * 0.3
            })
            .collect();
        let feats = WhisperFeatures::new().compute(&tone);
        assert_eq!(feats.len(), N_MELS * N_FRAMES);
        let max = feats.iter().cloned().fold(f32::MIN, f32::max);
        let min = feats.iter().cloned().fold(f32::MAX, f32::min);
        // Whisper scaling: max - 8 floor, (x + 4) / 4.
        assert!(max - min <= 2.0 + 1e-4, "dynamic range {min}..{max}");
        // The 440 Hz bin region is louder than a high band in the last frame.
        let last = N_FRAMES - 1;
        assert!(feats[5 * N_FRAMES + last] > feats[70 * N_FRAMES + last]);
    }
}
