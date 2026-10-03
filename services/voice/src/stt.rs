//! Speech-to-text: Silero VAD segmentation + offline recogniser
//! (Moonshine tiny for the `small` pack, Parakeet 0.6B for `accurate`).

use sherpa_onnx::{
    OfflineModelConfig, OfflineMoonshineModelConfig, OfflineRecognizer, OfflineRecognizerConfig,
    OfflineTransducerModelConfig, SileroVadModelConfig, VadModelConfig, VoiceActivityDetector,
};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tracing::info;

use crate::models::{find_file, PackManager};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SttModel {
    /// Moonshine tiny (small pack) — default.
    Moonshine,
    /// Parakeet TDT 0.6B v3 int8 (accurate pack).
    Parakeet,
}

impl SttModel {
    pub fn parse(s: Option<&str>) -> Result<Self, String> {
        match s.map(str::trim).unwrap_or("") {
            "" | "moonshine" | "small" | "default" => Ok(SttModel::Moonshine),
            "parakeet" | "accurate" => Ok(SttModel::Parakeet),
            other => Err(format!(
                "unknown STT model '{other}' (expected 'moonshine' or 'parakeet')"
            )),
        }
    }

    pub fn id(&self) -> &'static str {
        match self {
            SttModel::Moonshine => "moonshine-tiny-en",
            SttModel::Parakeet => "parakeet-tdt-0.6b-v3-int8",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Segment {
    pub start: f32,
    pub end: f32,
    pub text: String,
}

/// Lazily-initialised engines. Built on first use so server startup (and the
/// test suite) never touches the network or loads models.
pub struct SttEngine {
    pub manager: PackManager,
    threads: i32,
    inner: Mutex<Option<SttInner>>,
    /// Dedicated VAD + recogniser for the streaming endpoint (single
    /// concurrent stream; a second stream gets 409 until the first ends).
    stream: Mutex<Option<SttInner>>,
    stream_busy: std::sync::atomic::AtomicBool,
}

struct SttInner {
    vad: VoiceActivityDetector,
    moonshine: OfflineRecognizer,
    parakeet: Option<OfflineRecognizer>,
}

impl SttEngine {
    pub fn new(manager: PackManager) -> Self {
        let threads = std::env::var("ALLTERNIT_VOICE_THREADS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(2);
        Self {
            manager,
            threads,
            inner: Mutex::new(None),
            stream: Mutex::new(None),
            stream_busy: std::sync::atomic::AtomicBool::new(false),
        }
    }

    pub fn num_threads(&self) -> i32 {
        self.threads
    }

    fn get_inner(&self) -> Result<std::sync::MutexGuard<'_, Option<SttInner>>, String> {
        self.inner
            .lock()
            .map_err(|e| format!("STT engine lock poisoned: {e}"))
    }

    /// Build (or reuse) the VAD + recognisers. Blocking; call from
    /// `spawn_blocking`.
    pub fn prepare(&self, model: SttModel) -> Result<(), String> {
        let mut guard = self.get_inner()?;
        if guard.is_some() {
            // Ensure the requested recogniser specifically is ready.
            if model == SttModel::Parakeet && guard.as_ref().unwrap().parakeet.is_none() {
                let dir = self
                    .manager
                    .ensure_blocking("accurate")
                    .map_err(|e| format!("accurate pack: {e}"))?;
                let rec = build_parakeet(&dir, self.threads)?;
                guard.as_mut().unwrap().parakeet = Some(rec);
            }
            return Ok(());
        }
        let fresh = self.build_inner(model)?;
        info!(
            "STT ready: VAD + moonshine{}",
            if fresh.parakeet.is_some() {
                " + parakeet"
            } else {
                ""
            }
        );
        *guard = Some(fresh);
        Ok(())
    }

    fn build_inner(&self, model: SttModel) -> Result<SttInner, String> {
        let small_dir = self
            .manager
            .ensure_blocking("small")
            .map_err(|e| format!("small pack: {e}"))?;
        let vad = build_vad(&small_dir, self.threads)?;
        let moonshine = build_moonshine(&small_dir, self.threads)?;
        let parakeet = if model == SttModel::Parakeet {
            let dir = self
                .manager
                .ensure_blocking("accurate")
                .map_err(|e| format!("accurate pack: {e}"))?;
            Some(build_parakeet(&dir, self.threads)?)
        } else {
            None
        };
        Ok(SttInner {
            vad,
            moonshine,
            parakeet,
        })
    }

    /// Try to begin a streaming STT session. Only one stream may be active
    /// at a time (single dedicated VAD/recogniser); a second attempt gets
    /// `None` and the caller should answer 409.
    pub fn begin_stream(self: &Arc<Self>, model: SttModel) -> Option<SttStream> {
        self.stream_busy
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            )
            .ok()?;
        Some(SttStream {
            engine: self.clone(),
            model,
            tail: Mutex::new(Vec::new()),
        })
    }

    fn end_stream(&self) {
        if let Ok(mut guard) = self.stream.lock() {
            if let Some(inner) = guard.as_ref() {
                inner.vad.reset();
            }
        }
        self.stream_busy
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }

    /// True when the small pack (VAD + default recogniser) is loaded.
    pub fn is_ready(&self) -> bool {
        self.get_inner()
            .ok()
            .and_then(|g| g.is_some())
            .unwrap_or(false)
    }

    /// Segment + transcribe 16 kHz mono f32 audio. Blocking.
    pub fn transcribe(&self, samples: &[f32], model: SttModel) -> Result<Vec<Segment>, String> {
        self.prepare(model)?;
        let guard = self.get_inner()?;
        let inner = guard.as_ref().ok_or("STT engine not initialised")?;
        let segments = collect_vad_segments(&inner.vad, samples)?;

        let mut out = Vec::new();
        for seg in segments {
            let text = recognize(inner, model, &seg.samples)?;
            let text = text.trim().to_string();
            if text.is_empty() {
                continue;
            }
            out.push(Segment {
                start: seg.start,
                end: seg.end,
                text,
            });
        }
        Ok(out)
    }

    /// Transcribe a single speech segment without running the VAD
    /// (used for streaming partials on the in-progress segment).
    pub fn transcribe_segment(&self, samples: &[f32], model: SttModel) -> Result<String, String> {
        self.prepare(model)?;
        let guard = self.get_inner()?;
        let inner = guard.as_ref().ok_or("STT engine not initialised")?;
        let seg = VadSegment {
            start: 0.0,
            end: samples.len() as f32 / 16_000.0,
            samples: samples.to_vec(),
        };
        Ok(recognize(inner, model, &seg.samples)?.trim().to_string())
    }
}

struct VadSegment {
    start: f32,
    end: f32,
    samples: Vec<f32>,
}

/// Run the VAD over the whole buffer and return finished speech segments.
/// `start` is in seconds, relative to the start of `samples`.
pub(crate) fn collect_vad_segments(
    vad: &VoiceActivityDetector,
    samples: &[f32],
) -> Result<Vec<VadSegment>, String> {
    const CHUNK: usize = 16_000; // feed 1 s at a time
    vad.reset();
    let mut out = Vec::new();

    let mut offset = 0usize;
    while offset < samples.len() {
        let end = (offset + CHUNK).min(samples.len());
        vad.accept_waveform(&samples[offset..end]);
        drain_segments(vad, &mut out);
        offset = end;
    }
    vad.flush();
    drain_segments(vad, &mut out);
    Ok(out)
}

fn drain_segments(vad: &VoiceActivityDetector, out: &mut Vec<VadSegment>) {
    // SpeechSegment.start() is relative to all input since the last reset.
    while let Some(front) = vad.front() {
        let start = front.start() as f32 / 16_000.0;
        let end = start + front.n() as f32 / 16_000.0;
        out.push(VadSegment {
            start,
            end,
            samples: front.samples().to_vec(),
        });
        vad.pop();
    }
}

fn recognize(inner: &SttInner, model: SttModel, samples: &[f32]) -> Result<String, String> {
    let rec = match model {
        SttModel::Moonshine => &inner.moonshine,
        SttModel::Parakeet => inner
            .parakeet
            .as_ref()
            .ok_or("parakeet recogniser not loaded (request model=moonshine or download the accurate pack)")?,
    };
    let stream = rec.create_stream();
    stream.accept_waveform(16_000, samples);
    rec.decode(&stream);
    stream
        .get_result()
        .map(|r| r.text)
        .ok_or_else(|| "recogniser returned no result".to_string())
}

fn base_model_config(threads: i32) -> OfflineModelConfig {
    OfflineModelConfig {
        num_threads: threads,
        provider: Some("cpu".to_string()),
        debug: false,
        ..Default::default()
    }
}

fn build_vad(dir: &Path, threads: i32) -> Result<VoiceActivityDetector, String> {
    let model = find_file(dir, &["silero_vad.onnx"])
        .ok_or_else(|| format!("silero_vad.onnx not found in {}", dir.display()))?;
    let silero = SileroVadModelConfig {
        model: Some(model.display().to_string()),
        threshold: 0.5,
        min_silence_duration: 0.5,
        min_speech_duration: 0.25,
        max_speech_duration: 20.0,
        window_size: 512,
    };
    let config = VadModelConfig {
        silero_vad: silero,
        sample_rate: 16_000,
        num_threads: threads,
        provider: Some("cpu".to_string()),
        debug: false,
        ..Default::default()
    };
    VoiceActivityDetector::create(&config, 90.0)
        .ok_or_else(|| "failed to create Silero VAD".to_string())
}

fn build_moonshine(dir: &Path, threads: i32) -> Result<OfflineRecognizer, String> {
    // Support both upstream layouts:
    // - 2026 quantized: `encoder_model.ort` + `decoder_model_merged.ort`
    // - classic int8: `preprocess.onnx` + `encode.int8.onnx` + decoders
    let enc_quant = find_file(dir, &["encoder_model.ort"]);
    let moonshine = if let Some(encoder) = enc_quant {
        let decoder = find_file(dir, &["decoder_model_merged.ort"])
            .ok_or_else(|| "decoder_model_merged.ort missing".to_string())?;
        OfflineMoonshineModelConfig {
            encoder: Some(encoder.display().to_string()),
            merged_decoder: Some(decoder.display().to_string()),
            ..Default::default()
        }
    } else {
        let preprocessor = find_file(dir, &["preprocess.onnx"])
            .ok_or_else(|| "moonshine preprocess.onnx missing".to_string())?;
        let encoder = find_file(dir, &["encode.int8.onnx", "encode.onnx"])
            .ok_or_else(|| "moonshine encoder missing".to_string())?;
        let uncached = find_file(dir, &["uncached_decode.int8.onnx", "uncached_decode.onnx"])
            .ok_or_else(|| "moonshine uncached decoder missing".to_string())?;
        let cached = find_file(dir, &["cached_decode.int8.onnx", "cached_decode.onnx"])
            .ok_or_else(|| "moonshine cached decoder missing".to_string())?;
        OfflineMoonshineModelConfig {
            preprocessor: Some(preprocessor.display().to_string()),
            encoder: Some(encoder.display().to_string()),
            uncached_decoder: Some(uncached.display().to_string()),
            cached_decoder: Some(cached.display().to_string()),
            ..Default::default()
        }
    };
    let tokens = find_file(dir, &["tokens.txt"])
        .ok_or_else(|| "moonshine tokens.txt missing".to_string())?;
    let mut model_config = base_model_config(threads);
    model_config.moonshine = moonshine;
    model_config.tokens = Some(tokens.display().to_string());
    let config = OfflineRecognizerConfig {
        model_config,
        ..Default::default()
    };
    OfflineRecognizer::create(&config)
        .ok_or_else(|| "failed to create Moonshine recogniser".to_string())
}

fn build_parakeet(dir: &Path, threads: i32) -> Result<OfflineRecognizer, String> {
    let encoder = find_file(dir, &["encoder.int8.onnx", "encoder.onnx"])
        .ok_or_else(|| "parakeet encoder missing".to_string())?;
    let decoder = find_file(dir, &["decoder.int8.onnx", "decoder.onnx"])
        .ok_or_else(|| "parakeet decoder missing".to_string())?;
    let joiner = find_file(dir, &["joiner.int8.onnx", "joiner.onnx"])
        .ok_or_else(|| "parakeet joiner missing".to_string())?;
    let tokens =
        find_file(dir, &["tokens.txt"]).ok_or_else(|| "parakeet tokens.txt missing".to_string())?;
    let mut model_config = base_model_config(threads);
    model_config.transducer = OfflineTransducerModelConfig {
        encoder: Some(encoder.display().to_string()),
        decoder: Some(decoder.display().to_string()),
        joiner: Some(joiner.display().to_string()),
    };
    model_config.tokens = Some(tokens.display().to_string());
    model_config.model_type = Some("nemo_transducer".to_string());
    let config = OfflineRecognizerConfig {
        model_config,
        ..Default::default()
    };
    OfflineRecognizer::create(&config)
        .ok_or_else(|| "failed to create Parakeet recogniser".to_string())
}

/// An active streaming STT session. Feed 16 kHz mono f32 chunks; finished
/// speech segments come back from `feed`/`finish` with transcripts attached.
/// Dropping the session resets the VAD and frees the stream slot.
pub struct SttStream {
    engine: Arc<SttEngine>,
    model: SttModel,
    /// Rolling copy of audio not yet covered by a final segment; used for
    /// throttled partial transcripts.
    tail: Mutex<Vec<f32>>,
}

impl SttStream {
    /// Feed one chunk; returns finished (transcribed) segments, if any.
    pub fn feed(&self, samples: &[f32]) -> Result<Vec<Segment>, String> {
        self.engine.prepare_stream(self.model)?;
        {
            let mut tail = self
                .tail
                .lock()
                .map_err(|e| format!("tail lock poisoned: {e}"))?;
            tail.extend_from_slice(samples);
            const MAX_TAIL: usize = 16_000 * 15;
            if tail.len() > MAX_TAIL {
                let drop = tail.len() - MAX_TAIL;
                tail.drain(0..drop);
            }
        }
        let guard = self
            .engine
            .stream
            .lock()
            .map_err(|e| format!("stream lock poisoned: {e}"))?;
        let inner = guard.as_ref().ok_or("stream not initialised")?;
        inner.vad.accept_waveform(samples);
        let raw = drain_segments_vec(&inner.vad);
        let segments = transcribe_all(inner, self.model, raw)?;
        if !segments.is_empty() {
            if let Ok(mut tail) = self.tail.lock() {
                tail.clear();
            }
        }
        Ok(segments)
    }

    /// Throttled partial transcript of the in-progress speech (None when
    /// there is not enough un-finalised audio yet).
    pub fn partial(&self) -> Result<Option<String>, String> {
        let tail = self
            .tail
            .lock()
            .map_err(|e| format!("tail lock poisoned: {e}"))?;
        if tail.len() < 16_000 {
            return Ok(None);
        }
        self.engine.stream_transcribe(tail.as_slice(), self.model)
    }

    /// End of input: flush trailing speech and return its segments.
    pub fn finish(self) -> Result<Vec<Segment>, String> {
        let result = (|| {
            let guard = self
                .engine
                .stream
                .lock()
                .map_err(|e| format!("stream lock poisoned: {e}"))?;
            if let Some(inner) = guard.as_ref() {
                inner.vad.flush();
                let raw = drain_segments_vec(&inner.vad);
                return transcribe_all(inner, self.model, raw);
            }
            Ok(Vec::new())
        })();
        self.engine.end_stream();
        result
    }
}

impl Drop for SttStream {
    fn drop(&mut self) {
        self.engine.end_stream();
    }
}

impl SttEngine {
    fn prepare_stream(&self, model: SttModel) -> Result<(), String> {
        let mut guard = self
            .stream
            .lock()
            .map_err(|e| format!("stream lock poisoned: {e}"))?;
        match guard.as_ref() {
            Some(inner) => {
                if model == SttModel::Parakeet && inner.parakeet.is_none() {
                    let dir = self
                        .manager
                        .ensure_blocking("accurate")
                        .map_err(|e| format!("accurate pack: {e}"))?;
                    let rec = build_parakeet(&dir, self.threads)?;
                    guard.as_mut().unwrap().parakeet = Some(rec);
                }
                Ok(())
            }
            None => {
                let fresh = self.build_inner(model)?;
                info!("streaming STT engine ready");
                *guard = Some(fresh);
                Ok(())
            }
        }
    }

    /// Transcribe an arbitrary sample buffer with the streaming engine's
    /// recogniser (used for partials; models stay hot).
    pub fn stream_transcribe(
        &self,
        samples: &[f32],
        model: SttModel,
    ) -> Result<Option<String>, String> {
        self.prepare_stream(model)?;
        if samples.is_empty() {
            return Ok(None);
        }
        let guard = self
            .stream
            .lock()
            .map_err(|e| format!("stream lock poisoned: {e}"))?;
        let inner = guard.as_ref().ok_or("stream not initialised")?;
        let text = recognize(inner, model, samples)?;
        Ok(Some(text.trim().to_string()))
    }
}

fn drain_segments_vec(vad: &VoiceActivityDetector) -> Vec<VadSegment> {
    let mut out = Vec::new();
    while let Some(front) = vad.front() {
        let start = front.start() as f32 / 16_000.0;
        let end = start + front.n() as f32 / 16_000.0;
        out.push(VadSegment {
            start,
            end,
            samples: front.samples().to_vec(),
        });
        vad.pop();
    }
    out
}

fn transcribe_all(
    inner: &SttInner,
    model: SttModel,
    raw: Vec<VadSegment>,
) -> Result<Vec<Segment>, String> {
    let mut out = Vec::new();
    for seg in raw {
        let text = recognize(inner, model, &seg.samples)?.trim().to_string();
        if text.is_empty() {
            continue;
        }
        out.push(Segment {
            start: seg.start,
            end: seg.end,
            text,
        });
    }
    Ok(out)
}

/// Helper: locate model files for the bench harness (e.g. tokens path).
pub fn model_files_hint(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for suffix in [
        "encoder_model.ort",
        "decoder_model_merged.ort",
        "tokens.txt",
    ] {
        if let Some(p) = find_file(dir, &[suffix]) {
            out.push(p);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stt_model_parse() {
        assert_eq!(SttModel::parse(None).unwrap(), SttModel::Moonshine);
        assert_eq!(SttModel::parse(Some("")).unwrap(), SttModel::Moonshine);
        assert_eq!(SttModel::parse(Some("small")).unwrap(), SttModel::Moonshine);
        assert_eq!(
            SttModel::parse(Some("parakeet")).unwrap(),
            SttModel::Parakeet
        );
        assert!(SttModel::parse(Some("whisper")).is_err());
    }

    #[test]
    fn default_threads_is_two() {
        // num_threads must default to 2 for the target-machine standard.
        let engine = SttEngine::new(PackManager::new());
        assert_eq!(engine.num_threads(), 2);
    }
}
