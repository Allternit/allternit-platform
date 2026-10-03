//! Engine traits the Voice Session core drives.
//!
//! The core never names a concrete model. The sherpa-onnx engine implements
//! these in `engine_sherpa.rs`; tests use `mock.rs`. Every trait object is
//! moved onto a worker thread, so all are `Send`, and all calls are blocking.
//!
//! All audio crossing these traits is mono f32 in -1..1. Input audio is
//! always 16 kHz ([`ENGINE_SAMPLE_RATE`]); the core resamples client rates.

use crate::session::protocol::EngineKind;

/// The rate every engine consumes.
pub const ENGINE_SAMPLE_RATE: u32 = 16_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineError {
    /// One of `protocol::codes`.
    pub code: &'static str,
    pub message: String,
}

impl EngineError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::new(crate::session::protocol::codes::ENGINE_UNAVAILABLE, message)
    }
    pub fn failed(message: impl Into<String>) -> Self {
        Self::new(crate::session::protocol::codes::ENGINE_ERROR, message)
    }
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for EngineError {}

/// Streaming speech-to-text over one VAD speech segment at a time.
///
/// The core calls `begin` at speech start, `accept` for every 16 kHz frame
/// of the segment, and `finish` when the VAD says the segment is over.
pub trait StreamingStt: Send {
    fn begin(&mut self);
    /// Feed more audio. Returns an interim hypothesis for the whole segment
    /// so far when the engine has a new one (implementations throttle this).
    fn accept(&mut self, samples: &[f32]) -> Result<Option<String>, EngineError>;
    /// The segment ended: return its final text (may be empty).
    fn finish(&mut self) -> Result<String, EngineError>;
    /// Drop the in-progress segment without decoding it (mute, end).
    fn abort(&mut self);
}

/// Text-to-speech for one sentence at a time.
pub trait Tts: Send {
    /// Native output rate of `synthesize`.
    fn sample_rate(&self) -> u32;
    /// Synthesize `text` with `voice`, delivering audio through `sink` as it
    /// is produced. `sink` returns `false` to stop early (barge-in/cancel).
    fn synthesize(
        &mut self,
        text: &str,
        voice: &str,
        sink: &mut dyn FnMut(&[f32]) -> bool,
    ) -> Result<(), EngineError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadEvent {
    SpeechStart,
    SpeechStop,
}

/// Voice activity detection over a continuous 16 kHz stream.
pub trait Vad: Send {
    /// Feed one frame (any length). Returns a transition if one happened.
    fn accept(&mut self, frame: &[f32]) -> Option<VadEvent>;
    fn reset(&mut self);
}

/// End-of-turn classifier (Smart Turn).
pub trait TurnDetector: Send {
    /// Probability in 0..1 that the speaker has finished, from the last
    /// ≤ 8 s of 16 kHz audio ending at the speech stop.
    fn predict(&mut self, audio: &[f32]) -> Result<f32, EngineError>;
}

/// What a session was asked for in `session.start` / `session.update`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SttOptions {
    /// `light` | `accurate`.
    pub model: String,
    pub language: String,
}

/// Static facts about an engine, reported in `session.ready`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineInfo {
    pub kind: EngineKind,
    pub stt_model: String,
    pub tts_model: String,
    pub vad_model: String,
    pub turn_model: String,
    pub voices: Vec<String>,
    pub default_voice: String,
}

/// Builds engine instances for one session. Construction may load models
/// and is called from a blocking context.
pub trait EngineFactory: Send + Sync + 'static {
    fn info(&self, stt: &SttOptions) -> EngineInfo;
    fn stt(&self, opts: &SttOptions) -> Result<Box<dyn StreamingStt>, EngineError>;
    fn tts(&self) -> Result<Box<dyn Tts>, EngineError>;
    /// The TTS for a session owned by `owner` (a signed-in Cloud Voice user,
    /// or a bot's owner on a phone call). Engines with per-owner voices
    /// (custom voices) override this; the default ignores the owner.
    fn tts_for(&self, _owner: Option<&str>) -> Result<Box<dyn Tts>, EngineError> {
        self.tts()
    }
    /// Is `voice` usable by `owner` right now? Blocking (may ask the cloud).
    /// Called before a session starts and when it switches voice. The default
    /// accepts; the sherpa engine uses it to enforce custom-voice consent.
    fn check_voice(&self, _owner: Option<&str>, _voice: &str) -> Result<(), EngineError> {
        Ok(())
    }
    fn vad(&self) -> Result<Box<dyn Vad>, EngineError>;
    /// `Ok(None)` when no Smart Turn model is available; `smart` mode then
    /// falls back to `vad` with a non-fatal `turn_unavailable` error.
    fn turn_detector(&self) -> Result<Option<Box<dyn TurnDetector>>, EngineError>;
}

/// The factory used when this build has no speech engine compiled in.
/// It refuses every session with a clear error rather than faking audio.
pub struct UnavailableEngine;

impl UnavailableEngine {
    const WHY: &'static str =
        "this voice service build has no speech engine (test-only placeholder engine)";
}

impl EngineFactory for UnavailableEngine {
    fn info(&self, stt: &SttOptions) -> EngineInfo {
        EngineInfo {
            kind: EngineKind::Device,
            stt_model: stt.model.clone(),
            tts_model: String::new(),
            vad_model: String::new(),
            turn_model: String::new(),
            voices: Vec::new(),
            default_voice: String::new(),
        }
    }
    fn stt(&self, _: &SttOptions) -> Result<Box<dyn StreamingStt>, EngineError> {
        Err(EngineError::unavailable(Self::WHY))
    }
    fn tts(&self) -> Result<Box<dyn Tts>, EngineError> {
        Err(EngineError::unavailable(Self::WHY))
    }
    fn vad(&self) -> Result<Box<dyn Vad>, EngineError> {
        Err(EngineError::unavailable(Self::WHY))
    }
    fn turn_detector(&self) -> Result<Option<Box<dyn TurnDetector>>, EngineError> {
        Ok(None)
    }
}
