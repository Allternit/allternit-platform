//! Speech-to-text: Silero VAD segments the audio, then an offline recogniser
//! transcribes each segment (Moonshine tiny from the `small` pack, Parakeet
//! TDT 0.6B v3 from the `accurate` pack).
//!
//! All audio here is 16 kHz mono f32; callers resample first
//! (`audio::resample_to_16k` / `audio::StreamResampler`).
//!
//! Recognisers are loaded once and shared. Every VAD instance is per request
//! (or per stream), so concurrent requests never share VAD state. Recogniser
//! inference is serialised through [`crate::models::inference_lock`] so the
//! service never runs more than `ALLTERNIT_VOICE_THREADS` (default 2)
//! inference threads at once.

use sherpa_onnx::{
    OfflineModelConfig, OfflineMoonshineModelConfig, OfflineRecognizer, OfflineRecognizerConfig,
    OfflineTransducerModelConfig, SileroVadModelConfig, VadModelConfig, VoiceActivityDetector,
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tracing::info;

use crate::models::{
    find_file, inference_lock, inference_threads, PackManager, MOONSHINE_DIR, PARAKEET_DIR,
    VAD_FILE,
};

pub const SAMPLE_RATE: i32 = 16_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SttModel {
    /// Moonshine tiny EN (small pack), the default.
    Moonshine,
    /// Parakeet TDT 0.6B v3 int8 (accurate pack).
    Parakeet,
}

impl SttModel {
    /// Parse a request `model` value. Accepts model names, pack names and
    /// the ids listed by `GET /v1/stt/models`.
    pub fn parse(s: Option<&str>) -> Result<Self, String> {
        match s.map(str::trim).unwrap_or("") {
            "" | "default" | "small" | "moonshine" | "moonshine-tiny-en" => Ok(SttModel::Moonshine),
            "accurate" | "parakeet" | "parakeet-tdt-0.6b-v3-int8" => Ok(SttModel::Parakeet),
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

    pub fn pack(&self) -> &'static str {
        match self {
            SttModel::Moonshine => "small",
            SttModel::Parakeet => "accurate",
        }
    }
}

/// A transcribed speech segment. Times are seconds from the start of the
/// audio (or of the stream).
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub start: f32,
    pub end: f32,
    pub text: String,
}

/// Event from a streaming session.
#[derive(Debug, Clone, PartialEq)]
pub enum SttEvent {
    /// Interim transcript of the speech in progress (replaces the previous
    /// partial).
    Partial(String),
    /// A finished speech segment.
    Final(Segment),
}

/// VAD tuning. 0.3 s of silence ends a segment: short enough to keep
/// finalisation inside the latency budget, long enough not to split words.
const VAD_THRESHOLD: f32 = 0.5;
const VAD_MIN_SILENCE: f32 = 0.3;
const VAD_MIN_SPEECH: f32 = 0.25;
const VAD_MAX_SPEECH: f32 = 20.0;
const VAD_WINDOW: i32 = 512;
/// Largest chunk to pass to `VoiceActivityDetector::accept_waveform`.
pub const VAD_WINDOW_SAMPLES: usize = VAD_WINDOW as usize;
/// Audio added before the VAD's reported onset so soft word starts are not
/// clipped. Never reaches back past the previous segment's end.
const PRE_ROLL: usize = 16_000 * 3 / 10;

/// Silero VAD tuning. `Default` is what the HTTP routes use.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VadConfig {
    /// Speech probability threshold (0..1).
    pub threshold: f32,
    /// Seconds of silence that end a segment.
    pub min_silence: f32,
    /// Shortest speech (seconds) that counts as a segment.
    pub min_speech: f32,
    /// Longest segment (seconds) before the VAD forces a split.
    pub max_speech: f32,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            threshold: VAD_THRESHOLD,
            min_silence: VAD_MIN_SILENCE,
            min_speech: VAD_MIN_SPEECH,
            max_speech: VAD_MAX_SPEECH,
        }
    }
}

pub struct SttEngine {
    packs: Arc<PackManager>,
    threads: i32,
    moonshine: Mutex<Option<Arc<OfflineRecognizer>>>,
    parakeet: Mutex<Option<Arc<OfflineRecognizer>>>,
}

impl SttEngine {
    pub fn new(packs: Arc<PackManager>) -> Self {
        Self {
            packs,
            threads: inference_threads(),
            moonshine: Mutex::new(None),
            parakeet: Mutex::new(None),
        }
    }

    pub fn num_threads(&self) -> i32 {
        self.threads
    }

    /// True when the recogniser for `model` is loaded.
    pub fn is_ready(&self, model: SttModel) -> bool {
        self.slot(model).lock().map(|g| g.is_some()).unwrap_or(false)
    }

    fn slot(&self, model: SttModel) -> &Mutex<Option<Arc<OfflineRecognizer>>> {
        match model {
            SttModel::Moonshine => &self.moonshine,
            SttModel::Parakeet => &self.parakeet,
        }
    }

    /// Download (first use) and load the VAD + recogniser for `model`.
    /// Blocking: call from `spawn_blocking` or a plain thread inside a tokio
    /// runtime.
    pub fn prepare(&self, model: SttModel) -> Result<(), String> {
        self.vad_model_path()?;
        self.recognizer(model).map(|_| ())
    }

    fn vad_model_path(&self) -> Result<PathBuf, String> {
        let dir = self.packs.ensure_blocking("small")?;
        let path = dir.join(VAD_FILE);
        if path.is_file() {
            Ok(path)
        } else {
            Err(format!("{VAD_FILE} missing in {}", dir.display()))
        }
    }

    fn recognizer(&self, model: SttModel) -> Result<Arc<OfflineRecognizer>, String> {
        let mut slot = self
            .slot(model)
            .lock()
            .map_err(|e| format!("STT engine lock poisoned: {e}"))?;
        if let Some(rec) = slot.as_ref() {
            return Ok(rec.clone());
        }
        let dir = self.packs.ensure_blocking(model.pack())?;
        let rec = match model {
            SttModel::Moonshine => build_moonshine(&dir.join(MOONSHINE_DIR), self.threads)?,
            SttModel::Parakeet => build_parakeet(&dir.join(PARAKEET_DIR), self.threads)?,
        };
        info!("STT ready: {} ({} threads)", model.id(), self.threads);
        let rec = Arc::new(rec);
        *slot = Some(rec.clone());
        Ok(rec)
    }

    /// A fresh Silero VAD with the default tuning.
    pub fn new_vad(&self) -> Result<VoiceActivityDetector, String> {
        self.new_vad_with(VadConfig::default())
    }

    /// A fresh Silero VAD (1 thread; the model is ~2 MB and loads in ms).
    /// Downloads the small pack on first use. Feed it at most
    /// [`VAD_WINDOW_SAMPLES`] samples per `accept_waveform` call: sherpa-onnx
    /// dates a segment's start from the end of the chunk in which speech was
    /// confirmed.
    pub fn new_vad_with(&self, cfg: VadConfig) -> Result<VoiceActivityDetector, String> {
        let model = self.vad_model_path()?;
        let config = VadModelConfig {
            silero_vad: SileroVadModelConfig {
                model: Some(model.display().to_string()),
                threshold: cfg.threshold,
                min_silence_duration: cfg.min_silence,
                min_speech_duration: cfg.min_speech,
                max_speech_duration: cfg.max_speech,
                window_size: VAD_WINDOW,
            },
            sample_rate: SAMPLE_RATE,
            num_threads: 1,
            provider: Some("cpu".to_string()),
            debug: false,
            ..Default::default()
        };
        VoiceActivityDetector::create(&config, 60.0)
            .ok_or_else(|| "failed to create Silero VAD".to_string())
    }

    /// Transcribe one speech segment (no VAD). Blocking.
    pub fn decode(&self, model: SttModel, samples: &[f32]) -> Result<String, String> {
        let rec = self.recognizer(model)?;
        Ok(decode_with(&rec, model, samples))
    }

    /// Same as [`decode`](Self::decode), argument order of the old API.
    pub fn transcribe_segment(&self, samples: &[f32], model: SttModel) -> Result<String, String> {
        self.decode(model, samples)
    }

    /// A VAD-less streaming entry for callers that run their own VAD/turn
    /// detection: feed the samples of one utterance, ask for partials, then
    /// `finish()` for the final text. Blocking (loads the model on first use).
    pub fn segment_stream(&self, model: SttModel) -> Result<SegmentStream, String> {
        Ok(SegmentStream {
            rec: self.recognizer(model)?,
            model,
            samples: Vec::new(),
        })
    }

    /// Segment with the VAD, then transcribe every segment. Blocking.
    pub fn transcribe(&self, samples: &[f32], model: SttModel) -> Result<Vec<Segment>, String> {
        let rec = self.recognizer(model)?;
        let vad = self.new_vad()?;
        let mut out = Vec::new();
        let mut prev_end = 0usize;
        // Feed one VAD window at a time: sherpa-onnx dates a segment's start
        // from the end of the chunk in which speech was confirmed, so big
        // chunks make onsets late by up to a chunk (1 s chunks cost the
        // first word of most utterances).
        for chunk in samples.chunks(VAD_WINDOW as usize) {
            vad.accept_waveform(chunk);
            drain_finals(&vad, &rec, model, samples, &mut prev_end, &mut out);
        }
        vad.flush();
        drain_finals(&vad, &rec, model, samples, &mut prev_end, &mut out);
        Ok(out)
    }

    /// Start a streaming session. Several sessions can run at once; each has
    /// its own VAD. Blocking (may download/load models on first use).
    pub fn stream(self: &Arc<Self>, model: SttModel) -> Result<SttStream, String> {
        let rec = self.recognizer(model)?;
        let vad = self.new_vad()?;
        Ok(SttStream {
            rec,
            model,
            vad,
            history: Vec::new(),
            history_start: 0,
            fed: 0,
            in_speech: false,
            speech_start: 0,
            prev_end: 0,
            last_partial_at: 0,
            last_partial: String::new(),
            partial_cost_samples: 0,
        })
    }
}

/// Longest input Moonshine v2 tiny decodes reliably. Above ~9.3 s its
/// merged decoder fails inside onnxruntime (encoder_attn broadcast error)
/// and sherpa-onnx returns an empty result, so longer segments are split.
const MOONSHINE_MAX: usize = SAMPLE_RATE as usize * 8;
const MOONSHINE_MIN_SPLIT: usize = SAMPLE_RATE as usize * 5;

fn decode_with(rec: &OfflineRecognizer, model: SttModel, samples: &[f32]) -> String {
    if model == SttModel::Moonshine && samples.len() > MOONSHINE_MAX {
        return split_for_moonshine(samples)
            .into_iter()
            .map(|part| decode_one(rec, part))
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
    }
    decode_one(rec, samples)
}

/// Cut `samples` into pieces of at most `MOONSHINE_MAX`, each cut at the
/// quietest 100 ms window between 5 s and 8 s into the remaining audio
/// (the pause between words or phrases), so words are rarely split.
fn split_for_moonshine(samples: &[f32]) -> Vec<&[f32]> {
    const WIN: usize = SAMPLE_RATE as usize / 10;
    let mut parts = Vec::new();
    let mut rest = samples;
    while rest.len() > MOONSHINE_MAX {
        let mut best = MOONSHINE_MAX;
        let mut best_energy = f32::MAX;
        let mut at = MOONSHINE_MIN_SPLIT;
        while at + WIN <= MOONSHINE_MAX {
            let e: f32 = rest[at..at + WIN].iter().map(|x| x * x).sum();
            if e < best_energy {
                best_energy = e;
                best = at + WIN / 2;
            }
            at += WIN / 2;
        }
        let (head, tail) = rest.split_at(best);
        parts.push(head);
        rest = tail;
    }
    parts.push(rest);
    parts
}

fn decode_one(rec: &OfflineRecognizer, samples: &[f32]) -> String {
    if samples.is_empty() {
        return String::new();
    }
    let _guard = inference_lock();
    let stream = rec.create_stream();
    stream.accept_waveform(SAMPLE_RATE, samples);
    rec.decode(&stream);
    stream
        .get_result()
        .map(|r| r.text.trim().to_string())
        .unwrap_or_default()
}

/// Pop every finished VAD segment and transcribe it. `source` is the whole
/// utterance, used to add the pre-roll the VAD cut off.
fn drain_finals(
    vad: &VoiceActivityDetector,
    rec: &OfflineRecognizer,
    model: SttModel,
    source: &[f32],
    prev_end: &mut usize,
    out: &mut Vec<Segment>,
) {
    while let Some(seg) = vad.front() {
        let start = seg.start().max(0) as usize;
        let end = start + seg.n().max(0) as usize;
        vad.pop();
        let from = start.saturating_sub(PRE_ROLL).max(*prev_end);
        *prev_end = end;
        let to = end.min(source.len());
        if from >= to {
            continue;
        }
        let text = decode_with(rec, model, &source[from..to]);
        if !text.is_empty() {
            out.push(Segment {
                start: start as f32 / SAMPLE_RATE as f32,
                end: end as f32 / SAMPLE_RATE as f32,
                text,
            });
        }
    }
}

/// Audio of one utterance, collected without a VAD (see
/// [`SttEngine::segment_stream`]).
pub struct SegmentStream {
    rec: Arc<OfflineRecognizer>,
    model: SttModel,
    samples: Vec<f32>,
}

impl SegmentStream {
    /// Append 16 kHz mono samples.
    pub fn feed(&mut self, samples: &[f32]) {
        self.samples.extend_from_slice(samples);
    }

    /// Interim transcript of everything fed so far (the last 15 s at most).
    /// Blocking; `None` before 0.3 s of audio. Each call re-decodes, so pace
    /// calls by their cost (see `SttStream::feed` for the rule it uses).
    pub fn partial(&self) -> Option<String> {
        if self.samples.len() < SAMPLE_RATE as usize * 3 / 10 {
            return None;
        }
        let from = self.samples.len().saturating_sub(PARTIAL_MAX);
        Some(decode_with(&self.rec, self.model, &self.samples[from..]))
    }

    /// Final transcript of the whole utterance. Blocking.
    pub fn finish(self) -> String {
        decode_with(&self.rec, self.model, &self.samples)
    }

    /// Seconds of audio fed so far.
    pub fn duration_secs(&self) -> f32 {
        self.samples.len() as f32 / SAMPLE_RATE as f32
    }

    /// Drop the audio and start a new utterance.
    pub fn reset(&mut self) {
        self.samples.clear();
    }
}

/// Seconds of in-progress speech between partial transcripts.
const PARTIAL_EVERY: usize = SAMPLE_RATE as usize * 6 / 10;
/// Partials re-decode at most the last 15 s of speech.
const PARTIAL_MAX: usize = SAMPLE_RATE as usize * 15;

/// A streaming STT session: feed 16 kHz mono chunks, get partial and final
/// events back. Drop it to cancel.
pub struct SttStream {
    rec: Arc<OfflineRecognizer>,
    model: SttModel,
    vad: VoiceActivityDetector,
    /// Recent audio (pre-roll + speech in progress), starting at absolute
    /// sample `history_start`.
    history: Vec<f32>,
    history_start: usize,
    /// Total samples fed.
    fed: usize,
    in_speech: bool,
    speech_start: usize,
    /// End of the last finished segment (pre-roll never reaches before it).
    prev_end: usize,
    last_partial_at: usize,
    last_partial: String,
    /// Wall time of the last partial decode, in samples of audio.
    partial_cost_samples: usize,
}

impl SttStream {
    /// Feed one chunk of 16 kHz mono audio. Blocking (runs inference when a
    /// segment finishes or a partial is due).
    pub fn feed(&mut self, samples: &[f32]) -> Vec<SttEvent> {
        self.history.extend_from_slice(samples);
        let mut events = Vec::new();
        // One VAD window per call (see `transcribe`), so onsets are exact.
        for chunk in samples.chunks(VAD_WINDOW as usize) {
            self.fed += chunk.len();
            self.vad.accept_waveform(chunk);
            events.extend(self.drain());
        }

        if !self.in_speech && self.vad.detected() {
            self.in_speech = true;
            self.speech_start = self.fed.saturating_sub(samples.len());
            self.last_partial_at = self.fed;
            self.last_partial.clear();
        }
        // Partials re-decode the whole utterance so far, so space them out
        // by their own cost: at most ~50% of real time goes to partials
        // (matters for Parakeet; Moonshine stays at the 0.6 s floor).
        let due = PARTIAL_EVERY.max(self.partial_cost_samples * 2);
        if self.in_speech && self.fed - self.last_partial_at >= due {
            self.last_partial_at = self.fed;
            let started = std::time::Instant::now();
            let from = self
                .speech_start
                .saturating_sub(PRE_ROLL)
                .max(self.prev_end)
                .max(self.fed.saturating_sub(PARTIAL_MAX));
            let text = decode_with(&self.rec, self.model, self.slice(from, self.fed));
            self.partial_cost_samples =
                (started.elapsed().as_secs_f32() * SAMPLE_RATE as f32) as usize;
            if !text.is_empty() && text != self.last_partial {
                self.last_partial = text.clone();
                events.push(SttEvent::Partial(text));
            }
        }
        if !self.in_speech {
            self.trim_history(self.fed.saturating_sub(PRE_ROLL));
        }
        events
    }

    /// End of input: flush trailing speech. Blocking.
    pub fn finish(mut self) -> Vec<SttEvent> {
        self.vad.flush();
        self.drain()
    }

    /// Seconds of audio fed so far.
    pub fn duration_secs(&self) -> f32 {
        self.fed as f32 / SAMPLE_RATE as f32
    }

    fn drain(&mut self) -> Vec<SttEvent> {
        let mut events = Vec::new();
        while let Some(seg) = self.vad.front() {
            let start = seg.start().max(0) as usize;
            let end = start + seg.n().max(0) as usize;
            // Prefer our own history (adds pre-roll); fall back to the VAD's
            // copy if the history was already trimmed past the onset.
            let from = start
                .saturating_sub(PRE_ROLL)
                .max(self.prev_end)
                .max(self.history_start);
            let text = if from < end && end <= self.history_start + self.history.len() {
                decode_with(&self.rec, self.model, self.slice(from, end))
            } else {
                decode_with(&self.rec, self.model, seg.samples())
            };
            self.vad.pop();
            self.prev_end = end;
            self.in_speech = false;
            self.last_partial.clear();
            self.trim_history(end.saturating_sub(PRE_ROLL).max(self.history_start));
            if !text.is_empty() {
                events.push(SttEvent::Final(Segment {
                    start: start as f32 / SAMPLE_RATE as f32,
                    end: end as f32 / SAMPLE_RATE as f32,
                    text,
                }));
            }
        }
        events
    }

    fn slice(&self, from: usize, to: usize) -> &[f32] {
        let a = from.saturating_sub(self.history_start).min(self.history.len());
        let b = to.saturating_sub(self.history_start).min(self.history.len());
        &self.history[a..b]
    }

    /// Drop history before absolute sample `keep_from`.
    fn trim_history(&mut self, keep_from: usize) {
        if keep_from > self.history_start {
            let n = (keep_from - self.history_start).min(self.history.len());
            self.history.drain(..n);
            self.history_start += n;
        }
    }
}

fn base_model_config(threads: i32) -> OfflineModelConfig {
    OfflineModelConfig {
        num_threads: threads,
        provider: Some("cpu".to_string()),
        debug: false,
        ..Default::default()
    }
}

fn path_of(dir: &Path, names: &[&str], what: &str) -> Result<String, String> {
    find_file(dir, names)
        .map(|p| p.display().to_string())
        .ok_or_else(|| format!("{what} missing in {}", dir.display()))
}

fn build_moonshine(dir: &Path, threads: i32) -> Result<OfflineRecognizer, String> {
    // Moonshine v2 export (2026-02-27): encoder + merged decoder, .ort format.
    let mut model_config = base_model_config(threads);
    model_config.moonshine = OfflineMoonshineModelConfig {
        encoder: Some(path_of(dir, &["encoder_model.ort"], "moonshine encoder")?),
        merged_decoder: Some(path_of(dir, &["decoder_model_merged.ort"], "moonshine decoder")?),
        ..Default::default()
    };
    model_config.tokens = Some(path_of(dir, &["tokens.txt"], "moonshine tokens.txt")?);
    let config = OfflineRecognizerConfig {
        model_config,
        ..Default::default()
    };
    OfflineRecognizer::create(&config).ok_or_else(|| "failed to create Moonshine recogniser".into())
}

fn build_parakeet(dir: &Path, threads: i32) -> Result<OfflineRecognizer, String> {
    let mut model_config = base_model_config(threads);
    model_config.transducer = OfflineTransducerModelConfig {
        encoder: Some(path_of(dir, &["encoder.int8.onnx"], "parakeet encoder")?),
        decoder: Some(path_of(dir, &["decoder.int8.onnx"], "parakeet decoder")?),
        joiner: Some(path_of(dir, &["joiner.int8.onnx"], "parakeet joiner")?),
    };
    model_config.tokens = Some(path_of(dir, &["tokens.txt"], "parakeet tokens.txt")?);
    model_config.model_type = Some("nemo_transducer".to_string());
    let config = OfflineRecognizerConfig {
        model_config,
        ..Default::default()
    };
    OfflineRecognizer::create(&config).ok_or_else(|| "failed to create Parakeet recogniser".into())
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
            SttModel::parse(Some("moonshine-tiny-en")).unwrap(),
            SttModel::Moonshine
        );
        assert_eq!(
            SttModel::parse(Some("parakeet")).unwrap(),
            SttModel::Parakeet
        );
        assert_eq!(
            SttModel::parse(Some("parakeet-tdt-0.6b-v3-int8")).unwrap(),
            SttModel::Parakeet
        );
        assert!(SttModel::parse(Some("whisper")).is_err());
    }

    #[test]
    fn moonshine_split_caps_length_and_cuts_at_pauses() {
        // 20 s of tone with a silent gap at 6.5 s and another at 13 s.
        let sr = SAMPLE_RATE as usize;
        let mut x: Vec<f32> = (0..20 * sr).map(|i| (i as f32 * 0.05).sin()).collect();
        for gap in [6 * sr + sr / 2, 13 * sr] {
            x[gap..gap + sr / 5].iter_mut().for_each(|s| *s = 0.0);
        }
        let parts = split_for_moonshine(&x);
        assert!(parts.iter().all(|p| p.len() <= MOONSHINE_MAX));
        assert_eq!(parts.iter().map(|p| p.len()).sum::<usize>(), x.len());
        // First cut lands inside the first gap.
        let first = parts[0].len();
        assert!(first > 6 * sr + sr / 2 && first < 6 * sr + sr / 2 + sr / 5, "{first}");
        assert!(split_for_moonshine(&x[..4 * sr]).len() == 1);
    }

    #[test]
    fn engine_starts_unloaded_without_touching_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let engine = SttEngine::new(Arc::new(PackManager::with_root(tmp.path().into())));
        assert!(!engine.is_ready(SttModel::Moonshine));
        assert!(!engine.is_ready(SttModel::Parakeet));
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
    }
}
