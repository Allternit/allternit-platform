//! The Voice Session engine traits on top of the Phase 1 sherpa-onnx engine
//! (`crate::stt`, `crate::tts`, `crate::models`). Shares the server's loaded
//! engines, so models load once per process.
//! - STT: `SttEngine::segment_stream` (VAD-less): the session runs its own
//!   VAD, so one utterance is fed in, interims are re-decoded every 0.6 s, and
//!   the final is one decode at segment end.
//! - TTS: `TtsEngine::synthesize_stream` (the `allternit-tts` child process),
//!   one callback per rendered chunk. Kokoro renders a sentence in one pass,
//!   so a chunk is a sentence (the first one cut at its first clause).
//! - VAD: `SttEngine::new_vad_with`, tuned for turn-taking.
//! - Turn: Smart Turn v3.2 from the `small` pack, through sherpa-onnx's own
//!   onnxruntime.

use std::sync::Arc;

use sherpa_onnx::VoiceActivityDetector;

use crate::custom_voice::consent::RECHECK_AFTER;
use crate::custom_voice::{is_custom, CustomVoices, OpenVoice};

use super::engine::{
    EngineError, EngineFactory, EngineInfo, StreamingStt, SttOptions, Tts, TurnDetector, Vad,
    VadEvent, ENGINE_SAMPLE_RATE,
};
use super::protocol::EngineKind;
use super::trim::{trim_silence, word_overlap, PAD_SAMPLES};
use super::turn::{SmartTurn, SMART_TURN_MODEL_ID};
use crate::models::{find_file, PackManager, KOKORO_DIR, SMART_TURN_FILE};
use crate::stt::{SegmentStream, SttEngine, SttModel, VadConfig, VAD_WINDOW_SAMPLES};
use crate::tts::{TtsEngine, DEFAULT_VOICE, KOKORO_SAMPLE_RATE, VOICES};

/// Re-decode the growing segment for an interim at most this often.
const INTERIM_EVERY_SAMPLES: usize = ENGINE_SAMPLE_RATE as usize * 6 / 10;
/// Past this length interims stop (each re-decode costs the whole segment).
const INTERIM_MAX_SAMPLES: usize = ENGINE_SAMPLE_RATE as usize * 12;
/// Audio handed to the session sink at a time (it re-frames to 20 ms on output).
const TTS_CHUNK_SAMPLES: usize = KOKORO_SAMPLE_RATE as usize / 10;
/// Turn-taking VAD: a short silence feeds Smart Turn (speech.stopped ≤ 300 ms)...
const VAD_MIN_SILENCE: f32 = 0.2;
/// ...and a short speech lets barge-in fire within ~200 ms.
const VAD_MIN_SPEECH: f32 = 0.1;

pub struct SherpaEngine {
    packs: Arc<PackManager>,
    stt: Arc<SttEngine>,
    tts: Arc<TtsEngine>,
    custom: Arc<CustomVoices>,
    kind: EngineKind,
}

impl SherpaEngine {
    /// Share the server's already-constructed engines (models load once).
    pub fn new(packs: Arc<PackManager>, stt: Arc<SttEngine>, tts: Arc<TtsEngine>) -> Self {
        let kind = match std::env::var("ALLTERNIT_VOICE_ENGINE_KIND").as_deref() {
            Ok("cloud") => EngineKind::Cloud,
            _ => EngineKind::Device,
        };
        // Until `with_custom` gives it a consent checker, custom voices are refused.
        let custom = CustomVoices::new(packs.clone(), None);
        Self {
            packs,
            stt,
            tts,
            custom,
            kind,
        }
    }

    /// Serve custom voices through `custom` (it carries the consent checker).
    pub fn with_custom(mut self, custom: Arc<CustomVoices>) -> Self {
        self.custom = custom;
        self
    }

    fn stt_model(opts: &SttOptions) -> SttModel {
        match opts.model.as_str() {
            "accurate" => SttModel::Parakeet,
            _ => SttModel::Moonshine,
        }
    }

}

impl EngineFactory for SherpaEngine {
    fn info(&self, stt: &SttOptions) -> EngineInfo {
        EngineInfo {
            kind: self.kind,
            stt_model: Self::stt_model(stt).id().to_string(),
            tts_model: KOKORO_DIR.into(),
            vad_model: "silero-vad".into(),
            turn_model: SMART_TURN_MODEL_ID.into(),
            voices: VOICES.iter().map(|v| v.id.to_string()).collect(),
            default_voice: DEFAULT_VOICE.into(),
        }
    }

    fn stt(&self, opts: &SttOptions) -> Result<Box<dyn StreamingStt>, EngineError> {
        let model = Self::stt_model(opts);
        self.stt.prepare(model).map_err(EngineError::unavailable)?;
        Ok(Box::new(SherpaStt {
            engine: self.stt.clone(),
            model,
            stream: self.stt.segment_stream(model).map_err(EngineError::unavailable)?,
            decoded_at: 0,
            last_interim: None,
        }))
    }

    fn prepare_phrases(&self, texts: &[String], voice: &str) {
        self.tts.prewarm_fixed_phrases(Some(voice));
        if !texts.is_empty() {
            self.tts.register_phrases(texts, Some(voice));
        }
    }

    fn tts(&self) -> Result<Box<dyn Tts>, EngineError> {
        self.tts_for(None)
    }

    fn tts_for(&self, owner: Option<&str>) -> Result<Box<dyn Tts>, EngineError> {
        self.tts.prepare().map_err(EngineError::unavailable)?;
        Ok(Box::new(SherpaTts {
            engine: self.tts.clone(),
            custom: self.custom.clone(),
            owner: owner.map(str::to_string),
            open: None,
        }))
    }

    fn check_voice(&self, owner: Option<&str>, voice: &str) -> Result<(), EngineError> {
        if is_custom(voice) {
            self.custom.authorize(owner, voice)?;
        }
        Ok(())
    }

    fn vad(&self) -> Result<Box<dyn Vad>, EngineError> {
        let vad = self
            .stt
            .new_vad_with(VadConfig {
                min_silence: VAD_MIN_SILENCE,
                min_speech: VAD_MIN_SPEECH,
                ..VadConfig::default()
            })
            .map_err(EngineError::unavailable)?;
        Ok(Box::new(SherpaVad {
            vad,
            speaking: false,
        }))
    }

    fn turn_detector(&self) -> Result<Option<Box<dyn TurnDetector>>, EngineError> {
        let dir = self
            .packs
            .ensure_blocking("small")
            .map_err(|e| EngineError::unavailable(format!("small pack: {e}")))?;
        let path = find_file(&dir, &[SMART_TURN_FILE]).ok_or_else(|| {
            EngineError::unavailable(format!("{SMART_TURN_FILE} not found in {}", dir.display()))
        })?;
        Ok(Some(Box::new(SmartTurn::load(&path, 1)?)))
    }
}

struct SherpaStt {
    engine: Arc<SttEngine>,
    model: SttModel,
    stream: SegmentStream,
    /// Samples fed when the last interim was decoded.
    decoded_at: usize,
    /// Last interim text and the segment length (samples) it was decoded from.
    last_interim: Option<(String, usize)>,
}

/// A final below this word overlap with a near-complete interim is re-decoded.
const MIN_INTERIM_OVERLAP: f32 = 0.8;
/// The interim must cover this share of the segment to be trusted.
const INTERIM_TRUST: f32 = 0.9;
/// Tighter pad for the re-decode.
const TIGHT_PAD: usize = PAD_SAMPLES / 2;

impl SherpaStt {
    fn samples(&self) -> usize {
        (self.stream.duration_secs() * ENGINE_SAMPLE_RATE as f32).round() as usize
    }
}

impl StreamingStt for SherpaStt {
    fn begin(&mut self) {
        self.stream.reset();
        self.decoded_at = 0;
        self.last_interim = None;
    }

    fn accept(&mut self, samples: &[f32]) -> Result<Option<String>, EngineError> {
        self.stream.feed(samples);
        let len = self.samples();
        if len - self.decoded_at.min(len) < INTERIM_EVERY_SAMPLES || len > INTERIM_MAX_SAMPLES {
            return Ok(None);
        }
        self.decoded_at = len;
        let interim = self.stream.partial();
        if let Some(text) = &interim {
            self.last_interim = Some((text.clone(), len));
        }
        Ok(interim)
    }

    fn finish(&mut self) -> Result<String, EngineError> {
        self.decoded_at = 0;
        let interim = self.last_interim.take();
        let total = self.samples();
        if total == 0 {
            self.stream.reset();
            return Ok(String::new());
        }
        let audio = self.stream.samples().to_vec();
        self.stream.reset();
        let decode = |pad: usize| {
            self.engine
                .decode(self.model, trim_silence(&audio, pad))
                .map_err(EngineError::failed)
        };
        let text = decode(PAD_SAMPLES)?;
        // Guard: a near-complete interim that the final disagrees with means
        // the final decode went wrong. Re-decode tighter; keep the re-decode
        // only if it agrees better with the interim (never swap in the
        // interim text itself).
        if let Some((interim_text, at)) = interim {
            let trusted = at as f32 >= INTERIM_TRUST * total as f32;
            if trusted && word_overlap(&text, &interim_text) < MIN_INTERIM_OVERLAP {
                let again = decode(TIGHT_PAD)?;
                if word_overlap(&again, &interim_text) > word_overlap(&text, &interim_text) {
                    return Ok(again);
                }
            }
        }
        Ok(text)
    }

    fn abort(&mut self) {
        self.stream.reset();
        self.decoded_at = 0;
        self.last_interim = None;
    }
}

struct SherpaTts {
    engine: Arc<TtsEngine>,
    custom: Arc<CustomVoices>,
    owner: Option<String>,
    /// The custom voice in use and when its consent was last confirmed.
    open: Option<(OpenVoice, std::time::Instant)>,
}

impl SherpaTts {
    /// The custom voice `voice`, with consent confirmed within `RECHECK_AFTER`.
    fn custom_voice(&mut self, voice: &str) -> Result<&OpenVoice, EngineError> {
        let id = voice.strip_prefix(crate::custom_voice::CUSTOM_PREFIX).unwrap_or(voice);
        let fresh = matches!(&self.open, Some((o, at)) if o.id == id && at.elapsed() < RECHECK_AFTER);
        if !fresh {
            let still = match self.custom.authorize(self.owner.as_deref(), voice) {
                Ok(g) => g,
                Err(e) => {
                    self.open = None; // consent gone: drop the conditioned voice
                    return Err(e);
                }
            };
            match &mut self.open {
                Some((o, at)) if o.id == id && o.clip_sha256 == still.clip_sha256 => {
                    *at = std::time::Instant::now()
                }
                _ => {
                    self.open = None;
                    let opened = self.custom.open(self.owner.as_deref(), voice)?;
                    self.open = Some((opened, std::time::Instant::now()));
                }
            }
        }
        Ok(&self.open.as_ref().expect("opened above").0)
    }
}

impl Tts for SherpaTts {
    fn sample_rate(&self) -> u32 {
        KOKORO_SAMPLE_RATE
    }

    fn synthesize(
        &mut self,
        text: &str,
        voice: &str,
        sink: &mut dyn FnMut(&[f32]) -> bool,
    ) -> Result<(), EngineError> {
        if is_custom(voice) {
            return self.custom_voice(voice)?.synthesize(text, sink);
        }
        let mut result = Ok(());
        self.engine
            .synthesize_stream(text, Some(voice), None, |_, _, samples, rate| {
                if rate != KOKORO_SAMPLE_RATE {
                    result = Err(EngineError::failed(format!(
                        "TTS returned {rate} Hz, expected {KOKORO_SAMPLE_RATE}"
                    )));
                    return false;
                }
                samples.chunks(TTS_CHUNK_SAMPLES).all(&mut *sink)
            })
            .map_err(EngineError::failed)?;
        result
    }
}

struct SherpaVad {
    vad: VoiceActivityDetector,
    speaking: bool,
}

impl Vad for SherpaVad {
    fn accept(&mut self, frame: &[f32]) -> Option<VadEvent> {
        // The engine VAD wants at most one window per call.
        let mut event = None;
        for part in frame.chunks(VAD_WINDOW_SAMPLES) {
            self.vad.accept_waveform(part);
            // The session keeps its own audio; drop finished segments.
            while !self.vad.is_empty() {
                self.vad.pop();
            }
            let now = self.vad.detected();
            match (self.speaking, now) {
                (false, true) => {
                    self.speaking = true;
                    event = Some(VadEvent::SpeechStart);
                }
                (true, false) => {
                    self.speaking = false;
                    event = Some(VadEvent::SpeechStop);
                }
                _ => {}
            }
        }
        event
    }

    fn reset(&mut self) {
        self.vad.reset();
        self.speaking = false;
    }
}
