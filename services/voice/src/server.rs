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
use self::base64::b64_encode;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt as _;
use tracing::{info, warn};

use crate::audio::{decode_any, f32_to_pcm16le, resample_to_16k, StreamResampler};
use crate::models::PackManager;
use crate::stt::{Segment, SttEngine, SttEvent, SttModel};
use crate::tts::{TtsEngine, VoiceDef, KOKORO_SAMPLE_RATE, VOICES};

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
        Self::with_packs(Arc::new(PackManager::new()))
    }

    /// State over an explicit pack manager (tests point it at a temp dir).
    pub fn with_packs(packs: Arc<PackManager>) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            request_count: Arc::new(RwLock::new(0)),
            stt: Arc::new(SttEngine::new(packs.clone())),
            tts: Arc::new(TtsEngine::new(packs.clone())),
            packs,
        }
    }

    pub fn stt(&self) -> Arc<SttEngine> {
        self.stt.clone()
    }

    pub fn tts(&self) -> Arc<TtsEngine> {
        self.tts.clone()
    }

    pub fn packs(&self) -> Arc<PackManager> {
        self.packs.clone()
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

/// Voice model for TTS. `id/name/language/gender/sample_rate` is the
/// original shape; `label`, `engine` and `assetReady` are added for the
/// allternit-ai voice pickers, which read those names.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceModel {
    pub id: String,
    pub name: String,
    pub language: String,
    pub gender: String,
    pub sample_rate: u32,
    pub label: String,
    pub engine: String,
    #[serde(rename = "assetReady")]
    pub asset_ready: bool,
}

impl VoiceModel {
    fn from_def(v: &VoiceDef, asset_ready: bool) -> Self {
        Self {
            id: v.id.to_string(),
            name: v.name.to_string(),
            language: v.language.to_string(),
            gender: v.gender.to_string(),
            sample_rate: KOKORO_SAMPLE_RATE,
            label: v.name.to_string(),
            engine: "kokoro".to_string(),
            asset_ready,
        }
    }
}

/// JSON error body `{ "error": "..." }` with a status.
fn error_response(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": message.into() }))).into_response()
}

/// Engine errors: a bad voice/model name is the caller's fault (400);
/// anything else (download failed, model failed to load) is 503.
fn engine_error(e: String) -> Response {
    warn!("voice engine error: {e}");
    if e.starts_with("unknown voice") || e.starts_with("unknown STT model") {
        error_response(StatusCode::BAD_REQUEST, e)
    } else {
        error_response(StatusCode::SERVICE_UNAVAILABLE, e)
    }
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
    /// TTS: one sentence's audio (pcm16le mono, base64).
    Audio {
        index: usize,
        text: String,
        sample_rate: u32,
        format: String,
        audio_b64: String,
    },
    /// Stream finished.
    Done { duration_secs: f32 },
    /// Fatal error mid-stream.
    Error { error: String },
}

/// The Voice Session WebSocket route over the server's already-loaded engines.
fn session_router(state: &VoiceServiceState) -> Router {
    use crate::session::engine_sherpa::SherpaEngine;
    use crate::session::ws::{router_with, SessionRouteState, TOKEN_ENV};
    let engine = SherpaEngine::new(state.packs(), state.stt(), state.tts());
    router_with(SessionRouteState::new(
        Arc::new(engine),
        std::env::var(TOKEN_ENV).ok(),
    ))
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
        .route("/v1/models/:pack", post(download_pack))
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
        .with_state(state.clone())
        .merge(session_router(&state))
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
        "stt_ready": state.stt.is_ready(SttModel::Moonshine),
        "stt_accurate_ready": state.stt.is_ready(SttModel::Parakeet),
        "tts_ready": state.tts.is_ready(),
        "num_threads": state.stt.num_threads(),
        "packs": packs,
    }))
}

/// List available voices (the real Kokoro voices of the small pack).
async fn list_voices(State(state): State<VoiceServiceState>) -> Json<Vec<VoiceModel>> {
    let ready = state.packs.is_installed("tts");
    Json(VOICES.iter().map(|v| VoiceModel::from_def(v, ready)).collect())
}

/// Get specific voice
async fn get_voice(
    State(state): State<VoiceServiceState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<VoiceModel>, StatusCode> {
    let ready = state.packs.is_installed("tts");
    VOICES
        .iter()
        .find(|v| v.id == id)
        .map(|v| Json(VoiceModel::from_def(v, ready)))
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
            name: "Parakeet TDT 0.6B v3 int8 (25 European languages, CC-BY-4.0)".to_string(),
            language: "multi".to_string(),
            supports_streaming: true,
        },
    ])
}

/// List model packs and their download state.
async fn list_packs(State(state): State<VoiceServiceState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "packs": state.packs.statuses().await }))
}

/// Start downloading a pack in the background (idempotent). Returns 202
/// with the pack's current status; poll `GET /v1/models` for progress. For
/// Settings → Voice; normal use downloads on first request instead.
async fn download_pack(
    State(state): State<VoiceServiceState>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Response {
    if crate::models::pack(&name).is_none() {
        return error_response(StatusCode::NOT_FOUND, format!("unknown pack: {name}"));
    }
    if !state.packs.is_installed(&name) {
        let packs = state.packs.clone();
        let pack_name = name.clone();
        tokio::spawn(async move {
            if let Err(e) = packs.ensure(&pack_name).await {
                warn!("pack {pack_name} download failed: {e}");
            }
        });
    }
    let status = state
        .packs
        .statuses()
        .await
        .into_iter()
        .find(|p| p.name == name);
    (StatusCode::ACCEPTED, Json(serde_json::json!(status))).into_response()
}

// ─── TTS ────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum AudioFormat {
    Wav,
    Pcm16,
}

impl AudioFormat {
    fn parse(s: Option<&str>) -> Result<Self, String> {
        match s.map(str::trim).unwrap_or("") {
            "" | "wav" => Ok(AudioFormat::Wav),
            "pcm16" | "pcm" | "s16le" => Ok(AudioFormat::Pcm16),
            other => Err(format!(
                "unsupported format '{other}' (expected 'wav' or 'pcm16')"
            )),
        }
    }
}

/// Text-to-speech: returns the audio bytes (`audio/wav` by default,
/// `format: "pcm16"` for raw s16le mono with `X-Sample-Rate`). The stub this
/// replaces returned a JSON descriptor with a dangling `audio_url`.
async fn text_to_speech(
    State(state): State<VoiceServiceState>,
    Json(request): Json<TtsRequest>,
) -> Response {
    {
        let mut count = state.request_count.write().await;
        *count += 1;
    }
    if request.text.trim().is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "text is empty");
    }
    let format = match AudioFormat::parse(request.format.as_deref()) {
        Ok(f) => f,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, e),
    };
    let tts = state.tts.clone();
    let TtsRequest {
        text, voice, speed, ..
    } = request;
    let result = tokio::task::spawn_blocking(move || tts.synthesize(&text, voice.as_deref(), speed))
    .await;
    let (samples, sample_rate) = match result {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => return engine_error(e),
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    let duration_secs = samples.len() as f32 / sample_rate as f32;
    let (bytes, content_type, format_name) = match format {
        AudioFormat::Pcm16 => (f32_to_pcm16le(&samples), "application/octet-stream", "pcm16"),
        AudioFormat::Wav => (
            crate::audio::wav_from_f32(&samples, sample_rate),
            "audio/wav",
            "wav",
        ),
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header("X-Duration-Seconds", format!("{duration_secs:.3}"))
        .header("X-Sample-Rate", sample_rate.to_string())
        .header("X-Format", format_name)
        .body(Body::from(bytes))
        .unwrap_or_else(|e| error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

/// Streaming TTS (NDJSON): the text is split into sentences and each
/// sentence's audio is sent as soon as it is synthesised.
async fn text_to_speech_stream(
    State(state): State<VoiceServiceState>,
    Json(request): Json<TtsRequest>,
) -> Response {
    if request.text.trim().is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "text is empty");
    }
    if let Err(e) = crate::tts::resolve_voice(request.voice.as_deref()) {
        return error_response(StatusCode::BAD_REQUEST, e);
    }
    let text = request.text.clone();
    let tts = state.tts.clone();
    let voice = request.voice.clone();
    let speed = request.speed;

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<String, std::convert::Infallible>>(32);
    tokio::task::spawn_blocking(move || {
        let started = Instant::now();
        let mut total_samples = 0usize;
        let mut sample_rate = KOKORO_SAMPLE_RATE;
        let mut client_gone = false;
        let result = tts.synthesize_stream(&text, voice.as_deref(), speed, |i, chunk, samples, rate| {
            total_samples += samples.len();
            sample_rate = rate;
            let event = StreamEvent::Audio {
                index: i,
                text: chunk.to_string(),
                sample_rate: rate,
                format: "pcm16".to_string(),
                audio_b64: b64_encode(&f32_to_pcm16le(samples)),
            };
            client_gone = send_event(&tx, &event).is_err();
            !client_gone // stop synthesising when the client went away
        });
        if client_gone {
            return;
        }
        if let Err(error) = result {
            let _ = send_event(&tx, &StreamEvent::Error { error });
            return;
        }
        let _ = send_event(
            &tx,
            &StreamEvent::Done {
                duration_secs: total_samples as f32 / sample_rate as f32,
            },
        );
        info!("TTS stream done in {:?}", started.elapsed());
    });

    ndjson_response(rx)
}

type EventTx = tokio::sync::mpsc::Sender<Result<String, std::convert::Infallible>>;

/// Send one NDJSON event from a blocking thread. Err = client gone.
fn send_event(tx: &EventTx, event: &StreamEvent) -> Result<(), ()> {
    let line = serde_json::to_string(event).map_err(|_| ())?;
    tx.blocking_send(Ok(line)).map_err(|_| ())
}

// ─── STT ────────────────────────────────────────────────────────────────────

struct SttForm {
    audio: Vec<u8>,
    language: Option<String>,
    model: Option<String>,
    sample_rate: Option<u32>,
}

async fn read_stt_multipart(mut multipart: Multipart) -> Result<SttForm, Response> {
    let mut form = SttForm {
        audio: Vec::new(),
        language: None,
        model: None,
        sample_rate: None,
    };
    loop {
        match multipart.next_field().await {
            Ok(Some(field)) => {
                let name = field.name().unwrap_or("").to_string();
                let data = field.bytes().await.map_err(|e| {
                    error_response(StatusCode::BAD_REQUEST, format!("multipart field: {e}"))
                })?;
                let text = || String::from_utf8_lossy(&data).trim().to_string();
                match name.as_str() {
                    "audio" | "file" => form.audio = data.to_vec(),
                    "language" => form.language = Some(text()),
                    "model" => form.model = Some(text()),
                    "sample_rate" => form.sample_rate = text().parse().ok(),
                    _ => {}
                }
            }
            Ok(None) => break,
            Err(e) => {
                return Err(error_response(
                    StatusCode::BAD_REQUEST,
                    format!("multipart: {e}"),
                ))
            }
        }
    }
    Ok(form)
}

fn segments_to_response(segments: Vec<Segment>, language: &str) -> SttResponse {
    let text = segments
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    // The offline recognisers do not expose a calibrated score; keep the
    // field (callers read it) with a fixed value for non-empty output.
    let confidence = if text.is_empty() { 0.0 } else { 0.9 };
    SttResponse {
        text,
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

/// `Some(503)` with `code: "voice_pack_downloading"` while `pack` is not
/// installed (kicking off its download if none is running); `None` once it
/// is ready (or if a previous attempt errored, so the engine surfaces the
/// real failure on the normal path).
async fn pack_downloading_response(state: &VoiceServiceState, pack: &str) -> Option<Response> {
    use crate::models::PackStateKind;
    if state.packs.is_installed(pack) {
        return None;
    }
    let current = state
        .packs
        .statuses()
        .await
        .into_iter()
        .find(|p| p.name == pack)?;
    if current.state == PackStateKind::Error {
        return None;
    }
    if current.state != PackStateKind::Downloading {
        let packs = state.packs.clone();
        let name = pack.to_string();
        tokio::spawn(async move {
            if let Err(e) = packs.ensure(&name).await {
                warn!("pack {name} download failed: {e}");
            }
        });
    }
    let size_bytes = crate::models::pack(pack).map(|p| p.files.iter().map(|f| f.size).sum::<u64>());
    let mb = size_bytes.map(|b| (b as f64 / 1_000_000.0).round() as u64);
    let message = match mb {
        Some(mb) => format!("Downloading the voice pack ({mb} MB)… try again in a moment."),
        None => "Downloading the voice pack… try again in a moment.".to_string(),
    };
    let mut resp = (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({
            "error": message,
            "code": "voice_pack_downloading",
            "pack": pack,
            "pct": current.pct,
            "size_bytes": size_bytes,
            "retry_after_secs": 5,
        })),
    )
        .into_response();
    resp.headers_mut()
        .insert(axum::http::header::RETRY_AFTER, axum::http::HeaderValue::from_static("5"));
    Some(resp)
}

async fn run_stt(state: &VoiceServiceState, form: SttForm) -> Result<SttResponse, Response> {
    {
        let mut count = state.request_count.write().await;
        *count += 1;
    }
    if form.audio.is_empty() {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "missing 'audio' field",
        ));
    }
    let lang = form.language.unwrap_or_else(|| "en".to_string());
    let model = SttModel::parse(form.model.as_deref())
        .map_err(|e| error_response(StatusCode::BAD_REQUEST, e))?;
    let (samples, decoded_rate) = decode_any(&form.audio)
        .map_err(|e| error_response(StatusCode::UNSUPPORTED_MEDIA_TYPE, e.to_string()))?;
    // Raw PCM has no header: honour an explicit `sample_rate` field.
    let is_wav = form.audio.starts_with(b"RIFF");
    let rate = if is_wav {
        decoded_rate
    } else {
        form.sample_rate.unwrap_or(decoded_rate)
    };
    // First use: the model pack is not on disk yet. Start the download in the
    // background and answer right away with a structured, retryable 503 so
    // callers (Desktop dictation) can show "Downloading the voice pack…"
    // instead of hanging on a request that blocks for the whole download.
    if let Some(r) = pack_downloading_response(state, model.pack()).await {
        return Err(r);
    }
    let engine = state.stt.clone();
    let started = Instant::now();
    let segments = tokio::task::spawn_blocking(move || {
        let samples = resample_to_16k(&samples, rate);
        engine.transcribe(&samples, model)
    })
    .await
    .map_err(|e| error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(engine_error)?;
    info!(
        "STT {}: {} bytes @ {rate} Hz -> {} segments in {:?}",
        model.id(),
        form.audio.len(),
        segments.len(),
        started.elapsed()
    );
    Ok(segments_to_response(segments, &lang))
}

/// Speech-to-text (multipart; response shape unchanged since the whisper era).
async fn speech_to_text(State(state): State<VoiceServiceState>, multipart: Multipart) -> Response {
    let form = match read_stt_multipart(multipart).await {
        Ok(f) => f,
        Err(r) => return r,
    };
    match run_stt(&state, form).await {
        Ok(resp) => Json(resp).into_response(),
        Err(r) => r,
    }
}

/// Alias used by allternit-ai's SpeechToText.ts, which POSTs to
/// `/v1/stt/transcribe` and reads `{ "transcript": "..." }`.
async fn speech_to_text_transcribe(
    State(state): State<VoiceServiceState>,
    multipart: Multipart,
) -> Response {
    let form = match read_stt_multipart(multipart).await {
        Ok(f) => f,
        Err(r) => return r,
    };
    match run_stt(&state, form).await {
        Ok(resp) => Json(serde_json::json!({
            "transcript": resp.text,
            "text": resp.text,
            "segments": resp.segments,
        }))
        .into_response(),
        Err(r) => r,
    }
}

/// Streaming STT (NDJSON). Body: raw s16le mono PCM, sent in chunks
/// (`?sample_rate=` default 16000, `?model=` moonshine|parakeet). Out:
/// `partial`, `final`, then `done` (or `error`). See spec/API.md.
async fn speech_to_text_stream(
    State(state): State<VoiceServiceState>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
    body: Body,
) -> Response {
    let model = match SttModel::parse(params.get("model").map(String::as_str)) {
        Ok(m) => m,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, e),
    };
    let rate: u32 = match params.get("sample_rate").map(|s| s.parse::<u32>()) {
        None => 16_000,
        Some(Ok(r)) if (4_000..=192_000).contains(&r) => r,
        Some(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "sample_rate must be an integer between 4000 and 192000",
            )
        }
    };
    if let Some(r) = pack_downloading_response(&state, model.pack()).await {
        return r;
    }
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<String, std::convert::Infallible>>(64);
    let (pcm_tx, pcm_rx) = std::sync::mpsc::sync_channel::<Vec<f32>>(64);

    // Async side: request body -> f32 chunks (carrying an odd trailing byte
    // over to the next chunk so samples never shift).
    tokio::spawn(async move {
        let mut data = body.into_data_stream();
        let mut carry: Option<u8> = None;
        while let Some(chunk) = futures_util::StreamExt::next(&mut data).await {
            let bytes = match chunk {
                Ok(b) => b,
                Err(e) => {
                    warn!("STT stream body: {e}");
                    return;
                }
            };
            let mut buf = Vec::with_capacity(bytes.len() + 1);
            buf.extend(carry.take());
            buf.extend_from_slice(&bytes);
            if buf.len() % 2 == 1 {
                carry = buf.pop();
            }
            let samples: Vec<f32> = buf
                .chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
                .collect();
            let pcm_tx = pcm_tx.clone();
            let sent = tokio::task::spawn_blocking(move || pcm_tx.send(samples)).await;
            if !matches!(sent, Ok(Ok(()))) {
                return;
            }
        }
    });

    // Blocking side: own the session; feed it and emit events.
    let engine = state.stt.clone();
    tokio::task::spawn_blocking(move || {
        let mut stream = match engine.stream(model) {
            Ok(s) => s,
            Err(error) => {
                let _ = send_event(&tx, &StreamEvent::Error { error });
                return;
            }
        };
        let resampler = StreamResampler::new(rate);
        let emit = |events: Vec<SttEvent>| -> Result<(), ()> {
            for e in events {
                let event = match e {
                    SttEvent::Partial(text) => StreamEvent::Partial { text },
                    SttEvent::Final(seg) => StreamEvent::Final {
                        text: seg.text,
                        start: seg.start,
                        end: seg.end,
                    },
                };
                send_event(&tx, &event)?;
            }
            Ok(())
        };
        while let Ok(chunk) = pcm_rx.recv() {
            let samples = resampler.push(&chunk, false);
            if emit(stream.feed(&samples)).is_err() {
                return; // client gone
            }
        }
        let tail = resampler.push(&[], true);
        if !tail.is_empty() && emit(stream.feed(&tail)).is_err() {
            return;
        }
        let duration_secs = stream.duration_secs();
        if emit(stream.finish()).is_err() {
            return;
        }
        let _ = send_event(&tx, &StreamEvent::Done { duration_secs });
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
        let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
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
