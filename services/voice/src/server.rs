//! Voice API Service — HTTP server implementation
//!
//! HTTP API service for speech-to-text and text-to-speech on sherpa-onnx.
//! Runs on port 8001. Models download on first use (see `models.rs`).

use axum::{
    body::Body,
    extract::{Multipart, State},
    http::{header, StatusCode},
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use base64::b64_encode;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt as _;
use tracing::{info, warn};

use crate::audio::{decode_any, f32_to_pcm16le, resample_to_16k};
use crate::models::PackManager;
use crate::stt::{Segment, SttEngine, SttModel};
use crate::tts::{TtsEngine, KOKORO_SAMPLE_RATE, VOICES};

/// Voice Service State
#[derive(Clone)]
pub struct VoiceServiceState {
    /// Active sessions
    sessions: Arc<RwLock<HashMap<String, VoiceSession>>>,
    /// Request counter for metrics
    request_count: Arc<RwLock<u64>>,
    /// Model pack manager (download/state)
    packs: Arc<PackManager>,
    /// STT engine (lazy)
    stt: Arc<SttEngine>,
    /// TTS engine (lazy)
    tts: Arc<TtsEngine>,
}

impl VoiceServiceState {
    pub fn new() -> Self {
        let packs = PackManager::new();
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            request_count: Arc::new(RwLock::new(0)),
            stt: Arc::new(SttEngine::new(packs.clone())),
            tts: Arc::new(TtsEngine::new(packs.clone())),
            packs: Arc::new(packs),
        }
    }
}

impl Default for VoiceServiceState {
    fn default() -> Self {
        Self::new()
    }
}

/// Voice session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceSession {
    pub session_id: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_activity: chrono::DateTime<chrono::Utc>,
    pub mode: SessionMode,
    pub language: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionMode {
    Tts,
    Stt,
    Both,
}

/// Voice model for TTS (shape unchanged since the stub era)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceModel {
    pub id: String,
    pub name: String,
    pub language: String,
    pub gender: String,
    pub sample_rate: u32,
}

/// STT model (shape unchanged since the stub era)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SttModelInfo {
    pub id: String,
    pub name: String,
    pub language: String,
    pub supports_streaming: bool,
}

/// TTS request (JSON). Accepts both `voice` and the legacy `voice_id`.
#[derive(Debug, Deserialize)]
pub struct TtsRequest {
    pub text: String,
    #[serde(default, alias = "voice_id")]
    pub voice: Option<String>,
    pub language: Option<String>,
    pub speed: Option<f32>,
    /// "wav" (default) or "pcm16".
    pub format: Option<String>,
}

/// TTS non-streaming JSON fallback metadata (returned as headers on the
/// binary response; kept for callers that probe the old shape via HEAD-ish
/// flows). The primary response is the audio bytes themselves.
#[derive(Debug, Serialize)]
pub struct TtsResponse {
    pub duration_secs: f32,
    pub sample_rate: u32,
    pub format: String,
}

/// STT response (shape unchanged since the whisper era)
#[derive(Debug, Serialize)]
pub struct SttResponse {
    pub text: String,
    pub confidence: f32,
    pub language: String,
    pub segments: Vec<TranscriptSegment>,
}

/// Transcript segment
#[derive(Debug, Serialize)]
pub struct TranscriptSegment {
    pub start_time: f32,
    pub end_time: f32,
    pub text: String,
    pub confidence: f32,
}

/// Create session request
#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    pub mode: SessionMode,
    pub language: Option<String>,
}

/// NDJSON event for streaming endpoints (both STT and TTS).
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    /// STT: interim transcript of the in-progress speech segment.
    Partial { text: String },
    /// STT: a finalised speech segment.
    Final { text: String, start: f32, end: f32 },
    /// TTS: one sentence's audio (pcm16le base64).
    Audio {
        index: usize,
        sample_rate: u32,
        format: String,
        audio_b64: String,
    },
    /// Stream finished.
    Done { duration_secs: f32 },
    /// Fatal error mid-stream.
    Error { error: String },
}

/// Create the router with all routes
pub fn create_router(state: VoiceServiceState) -> Router {
    Router::new()
        // Health endpoints
        .route("/health", get(health_check))
        .route("/v1/health", get(health_check))
        // Voice models
        .route("/v1/voices", get(list_voices))
        .route("/v1/voices/:id", get(get_voice))
        // STT models
        .route("/v1/stt/models", get(list_stt_models))
        // Model packs (download state)
        .route("/v1/models", get(list_packs))
        // TTS endpoints
        .route("/v1/tts", post(text_to_speech))
        .route("/v1/tts/stream", post(text_to_speech_stream))
        // STT endpoints
        .route("/v1/stt", post(speech_to_text))
        .route("/v1/stt/transcribe", post(speech_to_text_transcribe))
        .route("/v1/stt/stream", post(speech_to_text_stream))
        // Session management
        .route("/v1/sessions", get(list_sessions).post(create_session))
        .route("/v1/sessions/:id", get(get_session).delete(delete_session))
        // Stats
        .route("/v1/stats", get(get_stats))
        .with_state(state)
}

/// Health check endpoint
async fn health_check(State(state): State<VoiceServiceState>) -> Json<serde_json::Value> {
    let packs = state.packs.statuses().await;
    Json(serde_json::json!({
        "service": "voice",
        "status": "healthy",
        "version": env!("CARGO_PKG_VERSION"),
        "timestamp": chrono::Utc::now().timestamp_millis(),
        "features": ["tts", "stt", "streaming"],
        "engine": "sherpa-onnx",
        "stt_ready": state.stt.is_ready(),
        "tts_ready": state.tts.is_ready(),
        "num_threads": state.stt.num_threads(),
        "packs": packs,
    }))
}

/// List available voices (real Kokoro voices, installed with the small pack)
async fn list_voices() -> Json<Vec<VoiceModel>> {
    Json(
        VOICES
            .iter()
            .map(|v| VoiceModel {
                id: v.id.to_string(),
                name: v.name.to_string(),
                language: v.language.to_string(),
                gender: v.gender.to_string(),
                sample_rate: KOKORO_SAMPLE_RATE,
            })
            .collect(),
    )
}

/// Get specific voice
async fn get_voice(
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<VoiceModel>, StatusCode> {
    VOICES
        .iter()
        .find(|v| v.id == id)
        .map(|v| {
            Json(VoiceModel {
                id: v.id.to_string(),
                name: v.name.to_string(),
                language: v.language.to_string(),
                gender: v.gender.to_string(),
                sample_rate: KOKORO_SAMPLE_RATE,
            })
        })
        .ok_or(StatusCode::NOT_FOUND)
}

/// List STT models
async fn list_stt_models() -> Json<Vec<SttModelInfo>> {
    Json(vec![
        SttModelInfo {
            id: "moonshine-tiny-en".to_string(),
            name: "Moonshine Tiny (English)".to_string(),
            language: "en".to_string(),
            supports_streaming: true,
        },
        SttModelInfo {
            id: "parakeet-tdt-0.6b-v3-int8".to_string(),
            name: "Parakeet TDT 0.6B v3 int8 (English, CC-BY-4.0)".to_string(),
            language: "en".to_string(),
            supports_streaming: true,
        },
    ])
}

/// List model packs and their download state.
async fn list_packs(State(state): State<VoiceServiceState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "packs": state.packs.statuses().await }))
}

// ─── TTS ────────────────────────────────────────────────────────────────────

/// Text-to-speech: returns real audio (WAV by default, `format=pcm16` for
/// raw s16le). The old stub returned a JSON descriptor with a dangling
/// `audio_url`; that shape was a documented bug (see docs/specs/tts-product.md
/// acceptance #7) and is replaced by actual bytes.
async fn text_to_speech(
    State(state): State<VoiceServiceState>,
    Json(request): Json<TtsRequest>,
) -> Result<Response, StatusCode> {
    {
        let mut count = state.request_count.write().await;
        *count += 1;
    }
    if request.text.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let format = request.format.clone().unwrap_or_else(|| "wav".to_string());
    let stt_tts = state.tts.clone();
    let text = request.text.clone();
    let voice = request.voice.clone();
    let speed = request.speed;
    let result =
        tokio::task::spawn_blocking(move || stt_tts.synthesize(&text, voice.as_deref(), speed))
            .await
            .map_err(|e| {
                warn!("TTS task failed: {e}");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
    let (samples, sample_rate) = result.map_err(|e| {
        warn!("TTS failed: {e}");
        StatusCode::SERVICE_UNAVAILABLE
    })?;

    let duration_secs = samples.len() as f32 / sample_rate as f32;
    let meta = TtsResponse {
        duration_secs,
        sample_rate: sample_rate as u32,
        format: format.clone(),
    };
    let bytes = if format == "pcm16" {
        f32_to_pcm16le(&samples)
    } else {
        crate::audio::wav_from_f32(&samples, sample_rate as u32)
    };
    let content_type = if format == "pcm16" {
        "application/octet-stream"
    } else {
        "audio/wav"
    };
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header("X-Duration-Seconds", format!("{duration_secs:.3}"))
        .header("X-Sample-Rate", sample_rate.to_string())
        .header("X-Format", &meta.format)
        .body(Body::from(bytes))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?)
}

/// Streaming TTS (NDJSON): each sentence is synthesised and emitted as soon
/// as it is ready, so playback can start before the whole text is done.
async fn text_to_speech_stream(
    State(state): State<VoiceServiceState>,
    Json(request): Json<TtsRequest>,
) -> Result<Response, StatusCode> {
    if request.text.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let sentences = crate::tts::split_sentences(&request.text);
    let stt_tts = state.tts.clone();
    let voice = request.voice.clone();
    let speed = request.speed;

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<String, std::convert::Infallible>>(32);
    tokio::task::spawn_blocking(move || {
        let total_start = Instant::now();
        let mut total_samples = 0usize;
        for (i, sentence) in sentences.iter().enumerate() {
            let audio = stt_tts.synthesize(sentence, voice.as_deref(), speed);
            let line = match audio {
                Ok((samples, sample_rate)) => {
                    total_samples += samples.len();
                    serde_json::to_string(&StreamEvent::Audio {
                        index: i,
                        sample_rate: sample_rate as u32,
                        format: "pcm16".to_string(),
                        audio_b64: b64_encode(&f32_to_pcm16le(&samples)),
                    })
                }
                Err(e) => serde_json::to_string(&StreamEvent::Error { error: e }),
            };
            if let Ok(line) = line {
                if tx.blocking_send(Ok(line)).is_err() {
                    return; // client went away
                }
            }
        }
        let sample_rate = KOKORO_SAMPLE_RATE;
        let _ = tx.blocking_send(Ok(serde_json::to_string(&StreamEvent::Done {
            duration_secs: total_samples as f32 / sample_rate as f32,
        })
        .unwrap()));
        info!(
            "TTS stream finished: {} sentences in {:?}",
            sentences.len(),
            total_start.elapsed()
        );
    });

    Ok(ndjson_response(rx))
}

// ─── STT ────────────────────────────────────────────────────────────────────

async fn read_stt_multipart(
    mut multipart: Multipart,
) -> Result<(Vec<u8>, Option<String>, Option<String>), StatusCode> {
    let mut audio_data = Vec::new();
    let mut language = None;
    let mut model = None;
    loop {
        match multipart.next_field().await {
            Ok(Some(field)) => {
                let name = field.name().unwrap_or("").to_string();
                let data = match field.bytes().await {
                    Ok(bytes) => bytes,
                    Err(err) => {
                        warn!("STT multipart field read failed: {err}");
                        return Err(StatusCode::BAD_REQUEST);
                    }
                };
                match name.as_str() {
                    "audio" | "file" => audio_data = data.to_vec(),
                    "language" => language = Some(String::from_utf8_lossy(&data).to_string()),
                    "model" => model = Some(String::from_utf8_lossy(&data).to_string()),
                    _ => {}
                }
            }
            Ok(None) => break,
            Err(err) => {
                warn!("STT multipart parse failed: {err}");
                return Err(StatusCode::BAD_REQUEST);
            }
        }
    }
    Ok((audio_data, language, model))
}

fn segments_to_response(segments: Vec<Segment>, language: &str) -> SttResponse {
    let text = segments
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let confidence = if text.is_empty() { 0.0 } else { 0.9 };
    SttResponse {
        text: text.clone(),
        confidence,
        language: language.to_string(),
        segments: segments
            .into_iter()
            .map(|s| TranscriptSegment {
                start_time: s.start,
                end_time: s.end,
                text: s.text,
                confidence,
            })
            .collect(),
    }
}

/// Speech-to-text (multipart, same response shape as the whisper era).
async fn speech_to_text(
    State(state): State<VoiceServiceState>,
    multipart: Multipart,
) -> Result<Json<SttResponse>, StatusCode> {
    {
        let mut count = state.request_count.write().await;
        *count += 1;
    }
    let (audio_data, language, model) = read_stt_multipart(multipart).await?;
    if audio_data.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let lang = language.unwrap_or_else(|| "en".to_string());

    let (samples, rate) = decode_any(&audio_data).map_err(|e| {
        warn!("STT decode failed: {e}");
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    })?;
    let samples = resample_to_16k(&samples, rate);
    let audio_secs = samples.len() as f32 / 16_000.0;
    info!(
        "STT request: {} bytes -> {:.2}s @ {} Hz ({} samples)",
        audio_data.len(),
        audio_secs,
        rate,
        samples.len()
    );

    let stt_model = SttModel::parse(model.as_deref()).map_err(|e| {
        warn!("STT model param: {e}");
        StatusCode::BAD_REQUEST
    })?;
    let engine = state.stt.clone();
    let segments = tokio::task::spawn_blocking(move || engine.transcribe(&samples, stt_model))
        .await
        .map_err(|e| {
            warn!("STT task failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .map_err(|e| {
            warn!("STT failed: {e}");
            StatusCode::SERVICE_UNAVAILABLE
        })?;

    Ok(Json(segments_to_response(segments, &lang)))
}

/// Legacy alias used by allternit-ai's SpeechToText.ts, which POSTs to
/// `/v1/stt/transcribe` and reads `{ "transcript": "..." }`.
async fn speech_to_text_transcribe(
    State(state): State<VoiceServiceState>,
    multipart: Multipart,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let response = speech_to_text(State(state), multipart).await?;
    Ok(Json(serde_json::json!({ "transcript": response.0.text })))
}

/// Streaming STT (NDJSON): chunked 16 kHz s16le mono PCM in; partial and
/// final segments out. One stream at a time (409 while busy). Documented in
/// spec/API.md.
async fn speech_to_text_stream(
    State(state): State<VoiceServiceState>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
    body: Body,
) -> Response {
    let model = match SttModel::parse(params.get("model").map(String::as_str)) {
        Ok(m) => m,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": e })),
            )
                .into_response();
        }
    };
    let engine = state.stt.clone();
    let Some(stream) = engine.begin_stream(model) else {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": "a streaming STT session is already active" })),
        )
            .into_response();
    };
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<String, std::convert::Infallible>>(64);
    let (sample_tx, sample_rx) = std::sync::mpsc::channel::<Vec<f32>>();

    // Async side: pull PCM chunks off the request body.
    tokio::spawn(async move {
        use futures_util::StreamExt;
        let mut stream = body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    let mut samples = Vec::with_capacity(bytes.len() / 2);
                    for c in bytes.chunks_exact(2) {
                        samples.push(i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0);
                    }
                    if sample_tx.send(samples).is_err() {
                        return;
                    }
                }
                Err(e) => {
                    warn!("STT stream body read failed: {e}");
                    return;
                }
            }
        }
    });

    // Blocking side: own the stream session; feed VAD, emit NDJSON events.
    tokio::task::spawn_blocking(move || {
        let send = |tx: &tokio::sync::mpsc::Sender<Result<String, std::convert::Infallible>>,
                    event: &StreamEvent| {
            let line = serde_json::to_string(event).unwrap_or_else(|e| {
                serde_json::to_string(&StreamEvent::Error {
                    error: e.to_string(),
                })
                .unwrap()
            });
            tx.blocking_send(Ok(line))
                .map_err(|_| "client gone".to_string())
        };
        let stream = stream;
        let mut total_samples = 0usize;
        let mut last_partial = std::time::Instant::now()
            .checked_sub(std::time::Duration::from_secs(1))
            .unwrap_or_else(std::time::Instant::now);
        let mut failed = false;
        while let Ok(chunk) = sample_rx.recv() {
            total_samples += chunk.len();
            match stream.feed(&chunk) {
                Ok(segments) => {
                    for seg in segments {
                        if send(
                            &tx,
                            &StreamEvent::Final {
                                text: seg.text,
                                start: seg.start,
                                end: seg.end,
                            },
                        )
                        .is_err()
                        {
                            failed = true;
                            break;
                        }
                    }
                    if failed {
                        break;
                    }
                    if last_partial.elapsed() >= std::time::Duration::from_millis(800) {
                        if let Ok(Some(text)) = stream.partial() {
                            if !text.is_empty()
                                && send(&tx, &StreamEvent::Partial { text }).is_err()
                            {
                                break;
                            }
                        }
                        last_partial = std::time::Instant::now();
                    }
                }
                Err(e) => {
                    let _ = send(&tx, &StreamEvent::Error { error: e });
                    break;
                }
            }
        }
        if !failed {
            match stream.finish() {
                Ok(segments) => {
                    for seg in segments {
                        if send(
                            &tx,
                            &StreamEvent::Final {
                                text: seg.text,
                                start: seg.start,
                                end: seg.end,
                            },
                        )
                        .is_err()
                        {
                            return;
                        }
                    }
                }
                Err(e) => {
                    let _ = send(&tx, &StreamEvent::Error { error: e });
                    return;
                }
            }
            let _ = send(
                &tx,
                &StreamEvent::Done {
                    duration_secs: total_samples as f32 / 16_000.0,
                },
            );
        }
    });

    ndjson_response(rx)
}

fn ndjson_response(
    rx: tokio::sync::mpsc::Receiver<Result<String, std::convert::Infallible>>,
) -> Response {
    let stream = ReceiverStream::new(rx).map(|result| {
        result.map(|mut line| {
            line.push('\n');
            axum::body::Bytes::from(line)
        })
    });
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/x-ndjson")
        .body(Body::from_stream(stream))
        .expect("build ndjson response")
}

// ─── Sessions & stats ───────────────────────────────────────────────────────

/// List active sessions
async fn list_sessions(State(state): State<VoiceServiceState>) -> Json<Vec<VoiceSession>> {
    let sessions = state.sessions.read().await;
    Json(sessions.values().cloned().collect())
}

/// Get specific session
async fn get_session(
    State(state): State<VoiceServiceState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<VoiceSession>, StatusCode> {
    let sessions = state.sessions.read().await;
    sessions
        .get(&id)
        .cloned()
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

/// Create new session
async fn create_session(
    State(state): State<VoiceServiceState>,
    Json(request): Json<CreateSessionRequest>,
) -> Result<Json<VoiceSession>, StatusCode> {
    let session = VoiceSession {
        session_id: uuid::Uuid::new_v4().to_string(),
        created_at: chrono::Utc::now(),
        last_activity: chrono::Utc::now(),
        mode: request.mode,
        language: request.language.unwrap_or_else(|| "en".to_string()),
    };

    let mut sessions = state.sessions.write().await;
    sessions.insert(session.session_id.clone(), session.clone());

    info!("Created voice session: {}", session.session_id);

    Ok(Json(session))
}

/// Delete session
async fn delete_session(
    State(state): State<VoiceServiceState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<StatusCode, StatusCode> {
    let mut sessions = state.sessions.write().await;

    if sessions.remove(&id).is_some() {
        info!("Deleted voice session: {id}");
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

/// Get service stats
async fn get_stats(State(state): State<VoiceServiceState>) -> Json<serde_json::Value> {
    let sessions = state.sessions.read().await;
    let request_count = state.request_count.read().await;

    Json(serde_json::json!({
        "active_sessions": sessions.len(),
        "tts_models": VOICES.len(),
        "stt_models": 2,
        "total_requests": *request_count,
        "timestamp": chrono::Utc::now().timestamp_millis(),
    }))
}

/// Minimal base64 encoder (RFC 4648, no padding variations needed here).
mod base64 {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn b64_encode(data: &[u8]) -> String {
        let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
        for chunk in data.chunks(3) {
            let b0 = chunk[0] as u32;
            let b1 = *chunk.get(1).unwrap_or(&0) as u32;
            let b2 = *chunk.get(2).unwrap_or(&0) as u32;
            let n = (b0 << 16) | (b1 << 8) | b2;
            out.push(ALPHABET[(n >> 18) as usize & 63] as char);
            out.push(ALPHABET[(n >> 12) as usize & 63] as char);
            if chunk.len() > 1 {
                out.push(ALPHABET[(n >> 6) as usize & 63] as char);
            } else {
                out.push('=');
            }
            if chunk.len() > 2 {
                out.push(ALPHABET[n as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
        out
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn rfc4648_vectors() {
            assert_eq!(super::b64_encode(b""), "");
            assert_eq!(super::b64_encode(b"f"), "Zg==");
            assert_eq!(super::b64_encode(b"fo"), "Zm8=");
            assert_eq!(super::b64_encode(b"foo"), "Zm9v");
            assert_eq!(super::b64_encode(b"foob"), "Zm9vYg==");
            assert_eq!(super::b64_encode(b"fooba"), "Zm9vYmE=");
            assert_eq!(super::b64_encode(b"foobar"), "Zm9vYmFy");
        }
    }
}
