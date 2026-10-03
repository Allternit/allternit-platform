//! The Voice Session engine traits on top of the Phase 1 sherpa-onnx engine
//! (`crate::stt`, `crate::tts`, `crate::models`).
//!
//! Written against `ao/voice-engine-p1` as of 2026-10-02 and finalised once
//! both branches land; see `docs/VOICE_SESSION_NOTES.md` for the open gaps.
//! - STT: Kimi's engine is offline (VAD segment → Moonshine/Parakeet). The
//!   session runs its own VAD, so this adapter feeds whole segments to
//!   `transcribe_segment`, re-decoding the growing segment for interims.
//! - TTS: `synthesize` returns a whole sentence; it is chunked afterwards.
//! - VAD: Kimi's VAD is private to `SttEngine`, so this builds its own
//!   Silero instance from the same pack file, tuned for turn-taking.
//! - Turn: Smart Turn v3.2 through sherpa-onnx's own onnxruntime.

use std::path::PathBuf;
use std::sync::Arc;

use sherpa_onnx::{SileroVadModelConfig, VadModelConfig, VoiceActivityDetector};

use super::engine::{
    EngineError, EngineFactory, EngineInfo, StreamingStt, SttOptions, Tts, TurnDetector, Vad,
    VadEvent, ENGINE_SAMPLE_RATE,
};
use super::protocol::EngineKind;
use super::turn::{SmartTurn, SMART_TURN_MODEL_ID};
use crate::models::{find_file, PackManager};
use crate::stt::{SttEngine, SttModel};
use crate::tts::{TtsEngine, DEFAULT_VOICE, KOKORO_SAMPLE_RATE, VOICES};

/// Re-decode the growing segment for an interim at most this often.
const INTERIM_EVERY_SAMPLES: usize = ENGINE_SAMPLE_RATE as usize * 6 / 10;
/// Past this length interims stop (each re-decode costs the whole segment).
const INTERIM_MAX_SAMPLES: usize = ENGINE_SAMPLE_RATE as usize * 12;
/// TTS chunk handed to the session (it re-frames to 20 ms on output).
const TTS_CHUNK_SAMPLES: usize = KOKORO_SAMPLE_RATE as usize / 10;

pub struct SherpaEngine {
    packs: PackManager,
    stt: Arc<SttEngine>,
    tts: Arc<TtsEngine>,
    kind: EngineKind,
}

impl SherpaEngine {
    /// Share the server's already-constructed engines (models load once).
    pub fn new(packs: PackManager, stt: Arc<SttEngine>, tts: Arc<TtsEngine>) -> Self {
        let kind = match std::env::var("ALLTERNIT_VOICE_ENGINE_KIND").as_deref() {
            Ok("cloud") => EngineKind::Cloud,
            _ => EngineKind::Device,
        };
        Self {
            packs,
            stt,
            tts,
            kind,
        }
    }

    pub fn from_env() -> Self {
        let packs = PackManager::new();
        let stt = Arc::new(SttEngine::new(packs.clone()));
        let tts = Arc::new(TtsEngine::new(packs.clone()));
        Self::new(packs, stt, tts)
    }

    fn stt_model(opts: &SttOptions) -> SttModel {
        match opts.model.as_str() {
            "accurate" => SttModel::Parakeet,
            _ => SttModel::Moonshine,
        }
    }

    fn turn_dir(&self) -> PathBuf {
        self.packs.pack_dir("smart-turn")
    }
}

impl EngineFactory for SherpaEngine {
    fn info(&self, stt: &SttOptions) -> EngineInfo {
        EngineInfo {
            kind: self.kind,
            stt_model: Self::stt_model(stt).id().to_string(),
            tts_model: "kokoro-en-v0_19".into(),
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
            segment: Vec::new(),
            decoded_at: 0,
        }))
    }

    fn tts(&self) -> Result<Box<dyn Tts>, EngineError> {
        self.tts.prepare().map_err(EngineError::unavailable)?;
        Ok(Box::new(SherpaTts {
            engine: self.tts.clone(),
        }))
    }

    fn vad(&self) -> Result<Box<dyn Vad>, EngineError> {
        let dir = self
            .packs
            .ensure_blocking("small")
            .map_err(|e| EngineError::unavailable(format!("small pack: {e}")))?;
        let model = find_file(&dir, &["silero_vad.onnx"]).ok_or_else(|| {
            EngineError::unavailable(format!("silero_vad.onnx not found in {}", dir.display()))
        })?;
        let config = VadModelConfig {
            silero_vad: SileroVadModelConfig {
                model: Some(model.display().to_string()),
                threshold: 0.5,
                // Short silence: speech.stopped feeds Smart Turn (≤ 300 ms target).
                min_silence_duration: 0.2,
                // Short speech: barge-in must fire within ~200 ms.
                min_speech_duration: 0.1,
                max_speech_duration: 30.0,
                window_size: 512,
            },
            sample_rate: ENGINE_SAMPLE_RATE as i32,
            num_threads: 1,
            provider: Some("cpu".to_string()),
            debug: false,
            ..Default::default()
        };
        let vad = VoiceActivityDetector::create(&config, 60.0)
            .ok_or_else(|| EngineError::unavailable("failed to create Silero VAD"))?;
        Ok(Box::new(SherpaVad {
            vad,
            speaking: false,
        }))
    }

    fn turn_detector(&self) -> Result<Option<Box<dyn TurnDetector>>, EngineError> {
        let path = SmartTurn::ensure_model(&self.turn_dir())?;
        Ok(Some(Box::new(SmartTurn::load(&path, 1)?)))
    }
}

struct SherpaStt {
    engine: Arc<SttEngine>,
    model: SttModel,
    segment: Vec<f32>,
    decoded_at: usize,
}

impl StreamingStt for SherpaStt {
    fn begin(&mut self) {
        self.segment.clear();
        self.decoded_at = 0;
    }

    fn accept(&mut self, samples: &[f32]) -> Result<Option<String>, EngineError> {
        self.segment.extend_from_slice(samples);
        let len = self.segment.len();
        if len - self.decoded_at < INTERIM_EVERY_SAMPLES || len > INTERIM_MAX_SAMPLES {
            return Ok(None);
        }
        self.decoded_at = len;
        self.engine
            .transcribe_segment(&self.segment, self.model)
            .map(Some)
            .map_err(EngineError::failed)
    }

    fn finish(&mut self) -> Result<String, EngineError> {
        let segment = std::mem::take(&mut self.segment);
        self.decoded_at = 0;
        if segment.is_empty() {
            return Ok(String::new());
        }
        self.engine
            .transcribe_segment(&segment, self.model)
            .map_err(EngineError::failed)
    }

    fn abort(&mut self) {
        self.segment.clear();
        self.decoded_at = 0;
    }
}

struct SherpaTts {
    engine: Arc<TtsEngine>,
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
        let (samples, rate) = self
            .engine
            .synthesize(text, Some(voice), None)
            .map_err(EngineError::failed)?;
        if rate as u32 != KOKORO_SAMPLE_RATE {
            return Err(EngineError::failed(format!(
                "TTS returned {rate} Hz, expected {KOKORO_SAMPLE_RATE}"
            )));
        }
        for chunk in samples.chunks(TTS_CHUNK_SAMPLES) {
            if !sink(chunk) {
                break;
            }
        }
        Ok(())
    }
}

struct SherpaVad {
    vad: VoiceActivityDetector,
    speaking: bool,
}

impl Vad for SherpaVad {
    fn accept(&mut self, frame: &[f32]) -> Option<VadEvent> {
        self.vad.accept_waveform(frame);
        // The session keeps its own audio; drop finished segments.
        while !self.vad.is_empty() {
            self.vad.pop();
        }
        let now = self.vad.detected();
        match (self.speaking, now) {
            (false, true) => {
                self.speaking = true;
                Some(VadEvent::SpeechStart)
            }
            (true, false) => {
                self.speaking = false;
                Some(VadEvent::SpeechStop)
            }
            _ => None,
        }
    }

    fn reset(&mut self) {
        self.vad.reset();
        self.speaking = false;
    }
}
