//! The one binding between the call worker and the Voice Session core
//! (VAD, turn end, STT, speak, barge-in; Track B, `services/voice/src/session/`).
//!
//! The call logic only sees [`CoreHandle`]: commands in, events out, mirroring
//! Voice Session protocol v1 (`services/voice/spec/VOICE_SESSION.md`) one to one. Today
//! [`connect_ws`] fills it by speaking that protocol over the core's WebSocket
//! (`/v1/voice/session` on this same binary's service), which needs nothing from
//! the core's internal Rust API. Swapping to an in-process core is one more
//! constructor here that fills the same channels; nothing else changes.

use anyhow::{bail, Context, Result};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use super::audio::CORE_INPUT_RATE;

/// Client → core (protocol "Client → server").
#[derive(Debug, Clone, PartialEq)]
pub enum CoreCommand {
    /// Caller audio, PCM16 LE mono at [`CORE_INPUT_RATE`].
    Audio(Vec<u8>),
    SpeakDelta {
        id: String,
        text: String,
    },
    SpeakDone {
        id: String,
    },
    SpeakCancel {
        id: Option<String>,
    },
    MicMute,
    MicUnmute,
    /// `session.update {voice}`: the bot's voice arrives with call start, after
    /// the session is already open.
    SetVoice(String),
    End,
}

/// Core → client (protocol "Server → client").
#[derive(Debug, Clone, PartialEq)]
pub enum CoreEvent {
    SpeechStarted,
    SpeechStopped,
    TranscriptDelta {
        segment_id: String,
        text: String,
    },
    TranscriptFinal {
        segment_id: String,
        text: String,
    },
    TurnEnded {
        text: String,
        confidence: f32,
    },
    SpeakStarted {
        id: String,
    },
    /// Speech audio for the latest `SpeakStarted`, PCM16 LE mono at
    /// [`CoreHandle::output_sample_rate`].
    SpeakAudio(Vec<u8>),
    SpeakEnded {
        id: String,
    },
    SpeakInterrupted {
        id: String,
    },
    Error {
        code: String,
        message: String,
        fatal: bool,
    },
    /// The core went away.
    Closed,
}

pub struct CoreHandle {
    pub tx: mpsc::Sender<CoreCommand>,
    pub rx: mpsc::Receiver<CoreEvent>,
    pub output_sample_rate: u32,
}

/// Settings sent in `session.start` for a phone call.
pub fn session_start(voice: Option<&str>) -> Value {
    let mut v = json!({
        "type": "session.start",
        "language": "en",
        "inputSampleRate": CORE_INPUT_RATE,
        "bargeIn": true,
        "turn": {"mode": "smart"},
    });
    if let Some(voice) = voice.filter(|v| !v.is_empty()) {
        v["voice"] = json!(voice);
    }
    v
}

pub fn command_frame(cmd: &CoreCommand) -> Message {
    let j = match cmd {
        CoreCommand::Audio(b) => return Message::Binary(b.clone()),
        CoreCommand::SpeakDelta { id, text } => {
            json!({"type": "speak.delta", "id": id, "text": text})
        }
        CoreCommand::SpeakDone { id } => json!({"type": "speak.done", "id": id}),
        CoreCommand::SpeakCancel { id: Some(id) } => json!({"type": "speak.cancel", "id": id}),
        CoreCommand::SpeakCancel { id: None } => json!({"type": "speak.cancel"}),
        CoreCommand::MicMute => json!({"type": "mic.mute"}),
        CoreCommand::MicUnmute => json!({"type": "mic.unmute"}),
        CoreCommand::SetVoice(v) => json!({"type": "session.update", "voice": v}),
        CoreCommand::End => json!({"type": "session.end"}),
    };
    Message::Text(j.to_string())
}

/// Parse a JSON server frame. Unknown types return `None` (protocol: clients
/// ignore unknown events and fields).
pub fn parse_event(text: &str) -> Option<CoreEvent> {
    let v: Value = serde_json::from_str(text).ok()?;
    let s = |k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    Some(match v.get("type")?.as_str()? {
        "speech.started" => CoreEvent::SpeechStarted,
        "speech.stopped" => CoreEvent::SpeechStopped,
        "transcript.delta" => CoreEvent::TranscriptDelta {
            segment_id: s("segmentId"),
            text: s("text"),
        },
        "transcript.final" => CoreEvent::TranscriptFinal {
            segment_id: s("segmentId"),
            text: s("text"),
        },
        "turn.ended" => CoreEvent::TurnEnded {
            text: s("text"),
            confidence: v.get("confidence").and_then(Value::as_f64).unwrap_or(1.0) as f32,
        },
        "speak.started" => CoreEvent::SpeakStarted { id: s("id") },
        "speak.ended" => CoreEvent::SpeakEnded { id: s("id") },
        "speak.interrupted" => CoreEvent::SpeakInterrupted { id: s("id") },
        "error" => CoreEvent::Error {
            code: s("code"),
            message: s("message"),
            fatal: v.get("fatal").and_then(Value::as_bool).unwrap_or(false),
        },
        _ => return None,
    })
}

/// Open a Voice Session over WebSocket and wait for `session.ready`.
pub async fn connect_ws(url: &str, token: Option<&str>, voice: Option<&str>) -> Result<CoreHandle> {
    let full = match token {
        Some(t) => format!(
            "{url}{}token={t}",
            if url.contains('?') { '&' } else { '?' }
        ),
        None => url.to_string(),
    };
    let (ws, _) = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        tokio_tungstenite::connect_async(&full),
    )
    .await
    .context("voice session connect timed out")?
    .context("voice session connect failed")?;
    let (mut sink, mut stream) = ws.split();
    sink.send(Message::Text(session_start(voice).to_string()))
        .await?;

    let output_sample_rate = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while let Some(msg) = stream.next().await {
            if let Message::Text(t) = msg? {
                let v: Value = serde_json::from_str(&t).unwrap_or(Value::Null);
                match v.get("type").and_then(Value::as_str) {
                    Some("session.ready") => {
                        return Ok(v
                            .get("outputSampleRate")
                            .and_then(Value::as_u64)
                            .unwrap_or(24_000) as u32)
                    }
                    Some("error") => bail!("voice session refused: {t}"),
                    _ => {}
                }
            }
        }
        bail!("voice session closed before session.ready")
    })
    .await
    .context("voice session.ready timed out")??;

    let (cmd_tx, mut cmd_rx) = mpsc::channel::<CoreCommand>(256);
    let (ev_tx, ev_rx) = mpsc::channel::<CoreEvent>(256);

    tokio::spawn(async move {
        while let Some(cmd) = cmd_rx.recv().await {
            let end = cmd == CoreCommand::End;
            if sink.send(command_frame(&cmd)).await.is_err() || end {
                break;
            }
        }
        let _ = sink.close().await;
    });
    tokio::spawn(async move {
        while let Some(msg) = stream.next().await {
            let ev = match msg {
                Ok(Message::Binary(b)) => Some(CoreEvent::SpeakAudio(b)),
                Ok(Message::Text(t)) => parse_event(&t),
                Ok(Message::Close(_)) | Err(_) => break,
                Ok(_) => None,
            };
            if let Some(ev) = ev {
                if ev_tx.send(ev).await.is_err() {
                    return;
                }
            }
        }
        let _ = ev_tx.send(CoreEvent::Closed).await;
    });

    Ok(CoreHandle {
        tx: cmd_tx,
        rx: ev_rx,
        output_sample_rate,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_protocol_events() {
        assert_eq!(
            parse_event(r#"{"type":"turn.ended","text":"hi there","confidence":0.8}"#),
            Some(CoreEvent::TurnEnded {
                text: "hi there".into(),
                confidence: 0.8
            })
        );
        assert_eq!(
            parse_event(
                r#"{"type":"transcript.delta","segmentId":"s1","text":"hi","final":false}"#
            ),
            Some(CoreEvent::TranscriptDelta {
                segment_id: "s1".into(),
                text: "hi".into()
            })
        );
        assert_eq!(
            parse_event(r#"{"type":"speak.interrupted","id":"u1","sentMs":420}"#),
            Some(CoreEvent::SpeakInterrupted { id: "u1".into() })
        );
        assert_eq!(parse_event(r#"{"type":"something.new"}"#), None);
        assert_eq!(parse_event("garbage"), None);
    }

    #[test]
    fn command_frames() {
        assert_eq!(
            command_frame(&CoreCommand::Audio(vec![1, 2])),
            Message::Binary(vec![1, 2])
        );
        let Message::Text(t) = command_frame(&CoreCommand::SpeakDelta {
            id: "u".into(),
            text: "Hi.".into(),
        }) else {
            panic!()
        };
        let v: Value = serde_json::from_str(&t).unwrap();
        assert_eq!(v, json!({"type":"speak.delta","id":"u","text":"Hi."}));
        let start = session_start(Some("af_heart"));
        assert_eq!(start["inputSampleRate"], 16000);
        assert_eq!(start["voice"], "af_heart");
        assert_eq!(start["bargeIn"], true);
    }

    /// Full handshake against a fake core speaking protocol v1.
    #[tokio::test]
    async fn ws_handshake_and_relay() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (sock, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(sock).await.unwrap();
            let Some(Ok(Message::Text(start))) = ws.next().await else {
                panic!()
            };
            assert!(start.contains("session.start"));
            ws.send(Message::Text(
                r#"{"type":"session.ready","sessionId":"x","outputSampleRate":24000,"protocol":1}"#
                    .into(),
            ))
            .await
            .unwrap();
            let Some(Ok(Message::Binary(b))) = ws.next().await else {
                panic!()
            };
            assert_eq!(b, vec![9, 9]);
            ws.send(Message::Text(
                r#"{"type":"turn.ended","text":"yes","confidence":1}"#.into(),
            ))
            .await
            .unwrap();
            ws.send(Message::Binary(vec![7, 7])).await.unwrap();
        });
        let mut core = connect_ws(&format!("ws://{addr}/v1/voice/session"), Some("t"), None)
            .await
            .unwrap();
        assert_eq!(core.output_sample_rate, 24_000);
        core.tx.send(CoreCommand::Audio(vec![9, 9])).await.unwrap();
        assert_eq!(
            core.rx.recv().await,
            Some(CoreEvent::TurnEnded {
                text: "yes".into(),
                confidence: 1.0
            })
        );
        assert_eq!(
            core.rx.recv().await,
            Some(CoreEvent::SpeakAudio(vec![7, 7]))
        );
        server.await.unwrap();
        assert_eq!(core.rx.recv().await, Some(CoreEvent::Closed));
    }
}
