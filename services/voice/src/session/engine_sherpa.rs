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

use super::engine::{
    EngineError, EngineFactory, EngineInfo, StreamingStt, SttOptions, Tts, TurnDetector, Vad,
    VadEvent, ENGINE_SAMPLE_RATE,
};
use super::protocol::EngineKind;
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
    kind: EngineKind,
}

impl SherpaEngine {
    /// Share the server's already-constructed engines (models load once).
    pub fn new(packs: Arc<PackManager>, stt: Arc<SttEngine>, tts: Arc<TtsEngine>) -> Self {
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
        }))
    }

    fn tts(&self) -> Result<Box<dyn Tts>, EngineError> {
        self.tts.prepare().map_err(EngineError::unavailable)?;
        Ok(Box::new(SherpaTts {
            engine: self.tts.clone(),
        }))
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
}

impl SherpaStt {
    fn samples(&self) -> usize {
        (self.stream.duration_secs() * ENGINE_SAMPLE_RATE as f32).round() as usize
    }
}

impl StreamingStt for SherpaStt {
    fn begin(&mut self) {
        self.stream.reset();
        self.decoded_at = 0;
    }

    fn accept(&mut self, samples: &[f32]) -> Result<Option<String>, EngineError> {
        self.stream.feed(samples);
        let len = self.samples();
        if len - self.decoded_at.min(len) < INTERIM_EVERY_SAMPLES || len > INTERIM_MAX_SAMPLES {
            return Ok(None);
        }
        self.decoded_at = len;
        Ok(self.stream.partial())
    }

    fn finish(&mut self) -> Result<String, EngineError> {
        self.decoded_at = 0;
        if self.stream.duration_secs() == 0.0 {
            return Ok(String::new());
        }
        let fresh = self
            .engine
            .segment_stream(self.model)
            .map_err(EngineError::failed)?;
        Ok(std::mem::replace(&mut self.stream, fresh).finish())
    }

    fn abort(&mut self) {
        self.stream.reset();
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
