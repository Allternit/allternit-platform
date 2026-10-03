//! Voice Session: realtime speech in/out with turn-taking and barge-in.
//! Protocol: `spec/VOICE_SESSION.md`.
//!
//! - [`core`]: the transport-agnostic state machine ([`VoiceSession`]).
//! - [`ws`]: the `GET /v1/voice/session` WebSocket route.
//! - [`engine`]: the traits an engine implements; `engine_sherpa` is the sherpa-onnx engine, [`mock`] the scripted test engine.
//! - [`turn`]: Smart Turn v3.2 (features + ONNX session).

pub mod core;
pub mod engine;
pub mod engine_sherpa;
pub mod mock;
pub mod protocol;
pub mod resample;
pub mod sentence;
pub mod turn;
pub mod ws;

pub use self::core::{CoreConfig, SessionHandle, SessionInput, SessionOutput, VoiceSession};
pub use self::engine::EngineFactory;
pub use self::protocol::{ClientMessage, ServerEvent};
