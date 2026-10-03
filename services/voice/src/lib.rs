//! Allternit voice service: local STT + TTS on sherpa-onnx.
//!
//! - [`models`]: model packs (first-use download, pinned sha256).
//! - [`stt`]: Silero VAD + Moonshine / Parakeet recognisers.
//! - [`tts`]: Kokoro TTS and sentence splitting.
//! - [`audio`]: WAV/G.711 decoding, resampling, WAV encoding.
//! - [`server`]: the HTTP API (`spec/API.md`).

pub mod audio;
pub mod models;
pub mod server;
pub mod stt;
pub mod tts;

pub use server::{create_router, VoiceServiceState};
