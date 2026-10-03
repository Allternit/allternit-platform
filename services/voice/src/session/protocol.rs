//! Wire types for Voice Session protocol v1 (`spec/VOICE_SESSION.md`).
//!
//! These are plain serde types with no transport attached: the WebSocket
//! route serialises them as JSON text frames, and the phone call worker can
//! build them directly.

use serde::{Deserialize, Serialize};

/// Protocol version carried in `session.ready`.
pub const PROTOCOL_VERSION: u32 = 1;

/// Turn-detection mode requested by the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TurnMode {
    Smart,
    Vad,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnOptions {
    pub mode: Option<TurnMode>,
    pub silence_ms: Option<u32>,
}

/// Fields shared by `session.start` and `session.update`. Every field is
/// optional; `session.update` only changes the fields it carries.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    /// `light` | `accurate`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stt_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_sample_rate: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub barge_in: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<TurnOptions>,
    /// Play a short cached filler ("One moment.") when no reply text has
    /// arrived `fillerMs` after `turn.ended`. Default off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fillers: Option<bool>,
    /// Wait before a filler, 200..10000 ms (default 1200).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filler_ms: Option<u64>,
}

/// Client → server control messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMessage {
    #[serde(rename = "session.start")]
    SessionStart(SessionOptions),
    #[serde(rename = "session.update")]
    SessionUpdate(SessionOptions),
    #[serde(rename = "speak.delta")]
    SpeakDelta { id: String, text: String },
    /// Pre-render these texts in the session's voice so a later `speak.delta`
    /// with the same text starts at once (phrase cache; a call's opening).
    /// No reply; engines without a cache ignore it.
    #[serde(rename = "speak.prepare")]
    SpeakPrepare { texts: Vec<String> },
    #[serde(rename = "speak.done")]
    SpeakDone { id: String },
    #[serde(rename = "speak.cancel")]
    SpeakCancel {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    #[serde(rename = "mic.mute")]
    MicMute,
    #[serde(rename = "mic.unmute")]
    MicUnmute,
    #[serde(rename = "session.end")]
    SessionEnd,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelIds {
    pub stt: String,
    pub tts: String,
    pub vad: String,
    pub turn: String,
}

/// `device` when the engine runs on the user's machine, `cloud` on Allternit Cloud.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EngineKind {
    Device,
    Cloud,
}

/// Server → client events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerEvent {
    #[serde(rename = "session.ready", rename_all = "camelCase")]
    SessionReady {
        protocol: u32,
        session_id: String,
        engine: EngineKind,
        output_sample_rate: u32,
        models: ModelIds,
        voices: Vec<String>,
    },
    #[serde(rename = "speech.started", rename_all = "camelCase")]
    SpeechStarted { at_ms: u64 },
    #[serde(rename = "speech.stopped", rename_all = "camelCase")]
    SpeechStopped { at_ms: u64 },
    #[serde(rename = "transcript.delta", rename_all = "camelCase")]
    TranscriptDelta {
        segment_id: String,
        text: String,
        #[serde(rename = "final")]
        is_final: bool,
    },
    #[serde(rename = "transcript.final", rename_all = "camelCase")]
    TranscriptFinal { segment_id: String, text: String },
    #[serde(rename = "turn.ended")]
    TurnEnded { text: String, confidence: f32 },
    #[serde(rename = "speak.started")]
    SpeakStarted { id: String },
    #[serde(rename = "speak.ended")]
    SpeakEnded { id: String },
    #[serde(rename = "speak.interrupted", rename_all = "camelCase")]
    SpeakInterrupted { id: String, sent_ms: u64 },
    #[serde(rename = "error")]
    Error {
        code: String,
        message: String,
        fatal: bool,
    },
}

impl ServerEvent {
    pub fn error(code: &str, message: impl Into<String>, fatal: bool) -> Self {
        ServerEvent::Error {
            code: code.to_string(),
            message: message.into(),
            fatal,
        }
    }

    /// The `type` string, for logs and tests.
    pub fn kind(&self) -> &'static str {
        match self {
            ServerEvent::SessionReady { .. } => "session.ready",
            ServerEvent::SpeechStarted { .. } => "speech.started",
            ServerEvent::SpeechStopped { .. } => "speech.stopped",
            ServerEvent::TranscriptDelta { .. } => "transcript.delta",
            ServerEvent::TranscriptFinal { .. } => "transcript.final",
            ServerEvent::TurnEnded { .. } => "turn.ended",
            ServerEvent::SpeakStarted { .. } => "speak.started",
            ServerEvent::SpeakEnded { .. } => "speak.ended",
            ServerEvent::SpeakInterrupted { .. } => "speak.interrupted",
            ServerEvent::Error { .. } => "error",
        }
    }
}

/// Error codes used in `error` events.
pub mod codes {
    pub const BAD_MESSAGE: &str = "bad_message";
    pub const NOT_STARTED: &str = "not_started";
    pub const ALREADY_STARTED: &str = "already_started";
    pub const BAD_OPTION: &str = "bad_option";
    pub const ENGINE_UNAVAILABLE: &str = "engine_unavailable";
    pub const ENGINE_ERROR: &str = "engine_error";
    pub const TURN_UNAVAILABLE: &str = "turn_unavailable";
    pub const SESSION_LIMIT: &str = "session_limit";
    /// A `custom:` voice without a live consent record (or revoked).
    pub const CUSTOM_VOICE_REFUSED: &str = "custom_voice_refused";
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_client_messages() {
        let m: ClientMessage = serde_json::from_value(json!({
            "type": "session.start", "voice": "af", "sttModel": "light",
            "inputSampleRate": 48000, "bargeIn": false,
            "turn": {"mode": "vad", "silenceMs": 500}, "futureField": 1
        }))
        .unwrap();
        let ClientMessage::SessionStart(o) = m else {
            panic!("wrong variant")
        };
        assert_eq!(o.input_sample_rate, Some(48000));
        assert_eq!(o.barge_in, Some(false));
        assert_eq!(o.turn.unwrap().mode, Some(TurnMode::Vad));

        let m: ClientMessage = serde_json::from_value(json!({"type": "speak.cancel"})).unwrap();
        assert_eq!(m, ClientMessage::SpeakCancel { id: None });
        let m: ClientMessage = serde_json::from_value(json!({"type": "mic.mute"})).unwrap();
        assert_eq!(m, ClientMessage::MicMute);
    }

    #[test]
    fn serialises_server_events_with_wire_names() {
        let v = serde_json::to_value(ServerEvent::TranscriptDelta {
            segment_id: "s1".into(),
            text: "hi".into(),
            is_final: false,
        })
        .unwrap();
        assert_eq!(
            v,
            json!({"type": "transcript.delta", "segmentId": "s1", "text": "hi", "final": false})
        );
        let v = serde_json::to_value(ServerEvent::SpeakInterrupted {
            id: "u".into(),
            sent_ms: 120,
        })
        .unwrap();
        assert_eq!(
            v,
            json!({"type": "speak.interrupted", "id": "u", "sentMs": 120})
        );
        let v = serde_json::to_value(ServerEvent::SpeechStarted { at_ms: 5 }).unwrap();
        assert_eq!(v, json!({"type": "speech.started", "atMs": 5}));
    }
}
