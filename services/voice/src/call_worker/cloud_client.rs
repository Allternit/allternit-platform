//! cloud-api client for the call worker (§4.1). The worker never talks to a
//! user's runtime directly: call start, events and turns all go through
//! cloud-api, which answers call start from its bot-config cache and queues
//! everything else per call while the runtime wakes.
//!
//! Every wire field name of the worker ⇄ cloud-api contract is in this file.
//!
//! Routes (base `ALLTERNIT_CLOUD_API_URL`, bearer `ALLTERNIT_VOICE_WORKER_TOKEN`):
//! - `POST /api/v1/voice/calls` → [`StartCallRequest`] / [`StartCallResponse`] (frozen).
//! - `POST /api/v1/voice/calls/{callId}/events` → one [`EventEnvelope`] (proposed).
//! - `POST /api/v1/voice/calls/{callId}/turns` → [`TurnRequest`], reply streamed
//!   as NDJSON [`TurnChunk`]s, or one JSON `{"text": …}` (proposed).

use std::time::Duration;

use futures::stream::BoxStream;
use futures::{StreamExt, TryStreamExt};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

pub const START_CALL_PATH: &str = "/api/v1/voice/calls";

pub fn events_path(call_id: &str) -> String {
    format!("/api/v1/voice/calls/{call_id}/events")
}

pub fn turns_path(call_id: &str) -> String {
    format!("/api/v1/voice/calls/{call_id}/turns")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Inbound,
    Outbound,
}

impl Direction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Direction::Inbound => "inbound",
            Direction::Outbound => "outbound",
        }
    }
}

/// `POST /api/v1/voice/calls` request (§4.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartCallRequest {
    pub bot_id: String,
    pub number_id: String,
    /// Caller E.164 (inbound) or bot number (outbound).
    pub from: String,
    pub to: String,
    pub direction: Direction,
    /// LiveKit room name (`call-…`).
    pub room: String,
    /// From the dispatch rule attributes; lets cloud-api check the bot's owner.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<String>,
    /// SIP Call-ID, for carrier-side debugging.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sip_call_id: Option<String>,
    /// Outbound only: the consent gate's reference.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consent_ref: Option<String>,
}

/// `POST /api/v1/voice/calls` response (§4.1): answered from cloud-api's
/// bot-config cache, never after waiting on the runtime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartCallResponse {
    pub call_id: String,
    pub bot: BotConfig,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotConfig {
    /// Spoken in the disclosure ("you've reached {name}"). Proposed addition;
    /// falls back to `persona.name` when persona is an object.
    #[serde(default)]
    pub name: Option<String>,
    /// String or object; passed through to the relay, not interpreted here.
    #[serde(default)]
    pub persona: serde_json::Value,
    #[serde(default)]
    pub voice_id: Option<String>,
    #[serde(default)]
    pub greeting: Option<String>,
    /// Recording consent configured for this bot. Off unless set.
    #[serde(default)]
    pub recording: bool,
}

impl BotConfig {
    pub fn display_name(&self) -> Option<String> {
        self.name
            .clone()
            .or_else(|| {
                self.persona
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(String::from)
            })
            .filter(|n| !n.trim().is_empty())
    }
}

/// One queued `call.*` event (§4.1). `payload` always carries `callId`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventEnvelope {
    #[serde(rename = "type")]
    pub event_type: String,
    /// `call:<callId>:<type>:<n>`; also sent as the `Idempotency-Key` header.
    pub idempotency_key: String,
    /// Per-call order, 1-based across all event types.
    pub seq: u64,
    /// Wall-clock milliseconds when the worker produced the event.
    pub at_ms: i64,
    pub payload: serde_json::Value,
}

/// `POST /api/v1/voice/calls/{callId}/turns` request (proposed).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnRequest {
    pub call_id: String,
    /// `turn:<callId>:<n>`; cloud-api dedupes retried turns on it.
    pub turn_id: String,
    pub text: String,
    pub confidence: f32,
}

/// One NDJSON line of a streamed turn reply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum TurnChunk {
    Delta { text: String },
    Done,
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CloudError {
    /// Network error, timeout, 408, 429 or 5xx: retry.
    #[error("transient: {0}")]
    Transient(String),
    /// Any other 4xx, or an unparseable body: retrying won't help.
    #[error("permanent: {0}")]
    Permanent(String),
}

impl CloudError {
    pub fn is_transient(&self) -> bool {
        matches!(self, CloudError::Transient(_))
    }

    fn from_status(status: StatusCode, body: &str) -> Self {
        let msg = format!("{status}: {}", body.chars().take(300).collect::<String>());
        if status.is_server_error()
            || status == StatusCode::REQUEST_TIMEOUT
            || status == StatusCode::TOO_MANY_REQUESTS
        {
            CloudError::Transient(msg)
        } else {
            CloudError::Permanent(msg)
        }
    }

    fn from_reqwest(e: reqwest::Error) -> Self {
        if e.is_decode() {
            CloudError::Permanent(e.to_string())
        } else {
            CloudError::Transient(e.to_string())
        }
    }
}

#[derive(Clone)]
pub struct CloudClient {
    base: String,
    token: String,
    http: reqwest::Client,
}

impl CloudClient {
    pub fn new(base: &str, token: &str) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .build()
            .expect("reqwest client");
        Self {
            base: base.trim_end_matches('/').to_string(),
            token: token.to_string(),
            http,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    pub async fn start_call(
        &self,
        req: &StartCallRequest,
        timeout: Duration,
    ) -> Result<StartCallResponse, CloudError> {
        let resp = self
            .http
            .post(self.url(START_CALL_PATH))
            .bearer_auth(&self.token)
            .timeout(timeout)
            .json(req)
            .send()
            .await
            .map_err(CloudError::from_reqwest)?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(CloudError::from_status(status, &body));
        }
        resp.json()
            .await
            .map_err(|e| CloudError::Permanent(e.to_string()))
    }

    pub async fn post_event(&self, call_id: &str, ev: &EventEnvelope) -> Result<(), CloudError> {
        let resp = self
            .http
            .post(self.url(&events_path(call_id)))
            .bearer_auth(&self.token)
            .header("Idempotency-Key", &ev.idempotency_key)
            .timeout(Duration::from_secs(10))
            .json(ev)
            .send()
            .await
            .map_err(CloudError::from_reqwest)?;
        let status = resp.status();
        // 409: cloud-api already has this idempotency key, which is success.
        if status.is_success() || status == StatusCode::CONFLICT {
            return Ok(());
        }
        let body = resp.text().await.unwrap_or_default();
        Err(CloudError::from_status(status, &body))
    }

    /// Send a finished caller turn; returns the reply as a stream of text deltas.
    pub async fn turn(
        &self,
        req: &TurnRequest,
        first_byte_timeout: Duration,
    ) -> Result<BoxStream<'static, Result<String, CloudError>>, CloudError> {
        let send = self
            .http
            .post(self.url(&turns_path(&req.call_id)))
            .bearer_auth(&self.token)
            .header("Idempotency-Key", &req.turn_id)
            .header("Accept", "application/x-ndjson, application/json")
            .json(req)
            .send();
        let resp = tokio::time::timeout(first_byte_timeout, send)
            .await
            .map_err(|_| CloudError::Transient("turn relay timed out".into()))?
            .map_err(CloudError::from_reqwest)?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(CloudError::from_status(status, &body));
        }
        let ndjson = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ct| ct.contains("ndjson"));
        if !ndjson {
            #[derive(Deserialize)]
            struct Whole {
                text: String,
            }
            let whole: Whole = resp
                .json()
                .await
                .map_err(|e| CloudError::Permanent(e.to_string()))?;
            return Ok(futures::stream::once(async move { Ok(whole.text) }).boxed());
        }
        let bytes = resp.bytes_stream().map_err(CloudError::from_reqwest);
        Ok(ndjson_text_stream(bytes).boxed())
    }
}

/// Turn a byte stream of NDJSON [`TurnChunk`]s into text deltas. Ends at
/// `done`; an `error` line becomes an error item.
pub fn ndjson_text_stream<S, B>(bytes: S) -> impl futures::Stream<Item = Result<String, CloudError>>
where
    S: futures::Stream<Item = Result<B, CloudError>> + Send + 'static,
    B: AsRef<[u8]>,
{
    futures::stream::unfold(
        (Box::pin(bytes), Vec::<u8>::new(), false),
        |(mut bytes, mut buf, mut done)| async move {
            loop {
                if done {
                    return None;
                }
                if let Some(nl) = buf.iter().position(|&b| b == b'\n') {
                    let line: Vec<u8> = buf.drain(..=nl).collect();
                    let line = String::from_utf8_lossy(&line).trim().to_string();
                    if line.is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<TurnChunk>(&line) {
                        Ok(TurnChunk::Delta { text }) => {
                            return Some((Ok(text), (bytes, buf, done)))
                        }
                        Ok(TurnChunk::Done) => return None,
                        Ok(TurnChunk::Error { message }) => {
                            done = true;
                            return Some((Err(CloudError::Permanent(message)), (bytes, buf, done)));
                        }
                        // Unknown chunk types are ignored (forward compatible).
                        Err(_) => continue,
                    }
                }
                match bytes.next().await {
                    Some(Ok(chunk)) => buf.extend_from_slice(chunk.as_ref()),
                    Some(Err(e)) => {
                        done = true;
                        return Some((Err(e), (bytes, buf, done)));
                    }
                    None => {
                        if buf.iter().all(|b| b.is_ascii_whitespace()) {
                            return None;
                        }
                        // Last line without a trailing newline.
                        buf.push(b'\n');
                    }
                }
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn start_call_wire_shape() {
        let req = StartCallRequest {
            bot_id: "bot_1".into(),
            number_id: "num_1".into(),
            from: "+15551230000".into(),
            to: "+16512686010".into(),
            direction: Direction::Inbound,
            room: "call-abc".into(),
            owner_id: None,
            sip_call_id: None,
            consent_ref: None,
        };
        assert_eq!(
            serde_json::to_value(&req).unwrap(),
            json!({"botId":"bot_1","numberId":"num_1","from":"+15551230000",
                   "to":"+16512686010","direction":"inbound","room":"call-abc"})
        );
        let resp: StartCallResponse = serde_json::from_value(json!({
            "callId": "call_9",
            "bot": {"persona": {"name": "Acme Plumbing"}, "voiceId": "af_heart",
                    "greeting": "How can I help?", "recording": true}
        }))
        .unwrap();
        assert_eq!(resp.call_id, "call_9");
        assert_eq!(resp.bot.display_name().as_deref(), Some("Acme Plumbing"));
        assert!(resp.bot.recording);
        // recording defaults to off.
        let b: BotConfig = serde_json::from_value(json!({"persona": "terse"})).unwrap();
        assert!(!b.recording);
        assert_eq!(b.display_name(), None);
    }

    #[test]
    fn error_classes() {
        assert!(CloudError::from_status(StatusCode::BAD_GATEWAY, "").is_transient());
        assert!(CloudError::from_status(StatusCode::TOO_MANY_REQUESTS, "").is_transient());
        assert!(CloudError::from_status(StatusCode::REQUEST_TIMEOUT, "").is_transient());
        assert!(!CloudError::from_status(StatusCode::UNPROCESSABLE_ENTITY, "").is_transient());
        assert!(!CloudError::from_status(StatusCode::UNAUTHORIZED, "").is_transient());
    }

    #[tokio::test]
    async fn ndjson_parsing() {
        let chunks: Vec<Result<Vec<u8>, CloudError>> = vec![
            Ok(b"{\"type\":\"delta\",\"text\":\"Hel".to_vec()),
            Ok(b"lo.\"}\n\n{\"type\":\"future\"}\n{\"type\":\"delta\",\"text\":\" Bye\"}".to_vec()),
            Ok(b"\n{\"type\":\"done\"}\n{\"type\":\"delta\",\"text\":\"ignored\"}\n".to_vec()),
        ];
        let out: Vec<_> = ndjson_text_stream(futures::stream::iter(chunks))
            .collect()
            .await;
        assert_eq!(out, vec![Ok("Hello.".to_string()), Ok(" Bye".to_string())]);

        let chunks: Vec<Result<Vec<u8>, CloudError>> = vec![Ok(
            b"{\"type\":\"delta\",\"text\":\"a\"}\n{\"type\":\"error\",\"message\":\"x\"}".to_vec(),
        )];
        let out: Vec<_> = ndjson_text_stream(futures::stream::iter(chunks))
            .collect()
            .await;
        assert_eq!(
            out,
            vec![Ok("a".to_string()), Err(CloudError::Permanent("x".into()))]
        );
    }
}
