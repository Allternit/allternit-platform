//! Voice API Service — HTTP server implementation
//!
//! HTTP API service for voice synthesis and recognition.
//! Runs on port 8001.

use axum::{
    extract::{Multipart, State},
    http::StatusCode,
    response::Json,
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::whisper::WhisperEngine;

/// Voice Service State
#[derive(Clone)]
pub struct VoiceServiceState {
    /// Active sessions
    sessions: Arc<RwLock<HashMap<String, VoiceSession>>>,
    /// TTS voice models
    tts_models: Arc<RwLock<Vec<VoiceModel>>>,
    /// STT models
    stt_models: Arc<RwLock<Vec<SttModel>>>,
    /// Request counter for metrics
    request_count: Arc<RwLock<u64>>,
    /// Local whisper.cpp engine (CLI + ggml model)
    whisper: Arc<WhisperEngine>,
}

impl VoiceServiceState {
    pub fn new() -> Self {
        let tts_models = vec![
            VoiceModel {
            id: "default".to_string(),
            name: "Default Voice".to_string(),
            language: "en".to_string(),
            gender: "neutral".to_string(),
            sample_rate: 24000,
            },
            VoiceModel {
            id: "en-us-female".to_string(),
            name: "US English Female".to_string(),
            language: "en-US".to_string(),
            gender: "female".to_string(),
            sample_rate: 24000,
            },
            VoiceModel {
            id: "en-us-male".to_string(),
            name: "US English Male".to_string(),
            language: "en-US".to_string(),
            gender: "male".to_string(),
            sample_rate: 24000,
            },
        ];

        let stt_models = vec![SttModel {
            id: "whisper-base".to_string(),
            name: "Whisper Base".to_string(),
            language: "multilingual".to_string(),
            supports_streaming: true,
        }];

        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            tts_models: Arc::new(RwLock::new(tts_models)),
            stt_models: Arc::new(RwLock::new(stt_models)),
            request_count: Arc::new(RwLock::new(0)),
            whisper: Arc::new(WhisperEngine::resolve()),
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

/// Voice model for TTS
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceModel {
    pub id: String,
    pub name: String,
    pub language: String,
    pub gender: String,
    pub sample_rate: u32,
}

/// STT model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SttModel {
    pub id: String,
    pub name: String,
    pub language: String,
    pub supports_streaming: bool,
}

/// TTS request
#[derive(Debug, Deserialize)]
pub struct TtsRequest {
    pub text: String,
    pub voice_id: Option<String>,
    pub language: Option<String>,
    pub speed: Option<f32>,
}

/// TTS response
#[derive(Debug, Serialize)]
pub struct TtsResponse {
    pub audio_url: String,
    pub duration_secs: f32,
    pub sample_rate: u32,
    pub format: String,
}

/// STT response
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
        
        // TTS endpoints
        .route("/v1/tts", post(text_to_speech))
        .route("/v1/tts/stream", post(text_to_speech_stream))
        
        // STT endpoints
        .route("/v1/stt", post(speech_to_text))
        .route("/v1/stt/stream", post(speech_to_text_stream))
        
        // Session management
        .route("/v1/sessions", get(list_sessions).post(create_session))
        .route("/v1/sessions/:id", get(get_session).delete(delete_session))
        
        // Stats
        .route("/v1/stats", get(get_stats))
        
        .with_state(state)
        .merge(crate::session::ws::router())
}

/// Health check endpoint
async fn health_check(State(state): State<VoiceServiceState>) -> Json<serde_json::Value> {
    let whisper = state.whisper.health();
    Json(serde_json::json!({
        "service": "voice",
        "status": "healthy",
        "version": env!("CARGO_PKG_VERSION"),
        "timestamp": chrono::Utc::now().timestamp_millis(),
        "features": ["tts", "stt", "streaming"],
        "engine": "whisper.cpp",
        "cli": whisper.cli,
        "model": whisper.model,
        "cli_ok": whisper.cli_ok,
        "model_ok": whisper.model_ok,
    }))
}

/// List available voices
async fn list_voices(
    State(state): State<VoiceServiceState>,
) -> Json<Vec<VoiceModel>> {
    let models = state.tts_models.read().await;
    Json(models.clone())
}

/// Get specific voice
async fn get_voice(
    State(state): State<VoiceServiceState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<VoiceModel>, StatusCode> {
    let models = state.tts_models.read().await;
    models.iter()
        .find(|m| m.id == id)
        .cloned()
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

/// List STT models
async fn list_stt_models(
    State(state): State<VoiceServiceState>,
) -> Json<Vec<SttModel>> {
    let models = state.stt_models.read().await;
    Json(models.clone())
}

/// Text-to-speech
async fn text_to_speech(
    State(state): State<VoiceServiceState>,
    Json(request): Json<TtsRequest>,
) -> Result<Json<TtsResponse>, StatusCode> {
    // Increment request counter
    {
        let mut count = state.request_count.write().await;
        *count += 1;
    }

    let voice_id = request.voice_id.unwrap_or_else(|| "default".to_string());
    let models = state.tts_models.read().await;
    
    let voice = models.iter()
        .find(|m| m.id == voice_id)
        .ok_or(StatusCode::BAD_REQUEST)?;

    // Simulate TTS processing
    let text_len = request.text.len() as f32;
    let duration_secs = text_len / 15.0; // Rough estimate: ~15 chars per second

    info!("TTS request: {} chars -> {:.2}s audio", text_len, duration_secs);

    Ok(Json(TtsResponse {
        audio_url: format!("/v1/audio/{}.wav", uuid::Uuid::new_v4().simple()),
        duration_secs,
        sample_rate: voice.sample_rate,
        format: "wav".to_string(),
    }))
}

/// Text-to-speech streaming (SSE)
async fn text_to_speech_stream(
    State(state): State<VoiceServiceState>,
    Json(request): Json<TtsRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // For now, return the same as non-streaming
    let response = text_to_speech(State(state), Json(request)).await?;
    
    Ok(Json(serde_json::json!({
        "stream_url": format!("/v1/streams/{}.mp3", uuid::Uuid::new_v4().simple()),
        "duration_secs": response.duration_secs,
        "format": "mp3",
    })))
}

/// Speech-to-text
async fn speech_to_text(
    State(state): State<VoiceServiceState>,
    mut multipart: Multipart,
) -> Result<Json<SttResponse>, StatusCode> {
    {
        let mut count = state.request_count.write().await;
        *count += 1;
    }

    let mut audio_data = Vec::new();
    let mut language = None;

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

    if audio_data.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let audio_duration = audio_data.len() as f32 / 16000.0 / 2.0;
    let lang = language.clone().unwrap_or_else(|| "en".to_string());
    info!("STT request: {} bytes -> {:.2}s audio", audio_data.len(), audio_duration);

    match state
        .whisper
        .transcribe(&audio_data, language.as_deref())
        .await
    {
        Ok(text) => Ok(Json(SttResponse {
            text: text.clone(),
            confidence: if text.is_empty() { 0.0 } else { 0.9 },
            language: lang.clone(),
            segments: vec![TranscriptSegment {
                start_time: 0.0,
                end_time: audio_duration,
                text,
                confidence: 0.9,
            }],
        })),
        Err(err) => {
            warn!("STT failed: {err}");
            Err(StatusCode::SERVICE_UNAVAILABLE)
        }
    }
}

/// Speech-to-text streaming
async fn speech_to_text_stream(
    State(_state): State<VoiceServiceState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    Ok(Json(serde_json::json!({
        "websocket_url": format!("ws://localhost:8001/v1/stt/ws/{}", uuid::Uuid::new_v4().simple()),
        "status": "ready",
    })))
}

/// List active sessions
async fn list_sessions(
    State(state): State<VoiceServiceState>,
) -> Json<Vec<VoiceSession>> {
    let sessions = state.sessions.read().await;
    Json(sessions.values().cloned().collect())
}

/// Get specific session
async fn get_session(
    State(state): State<VoiceServiceState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<VoiceSession>, StatusCode> {
    let sessions = state.sessions.read().await;
    sessions.get(&id)
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
        info!("Deleted voice session: {}", id);
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

/// Get service stats
async fn get_stats(
    State(state): State<VoiceServiceState>,
) -> Json<serde_json::Value> {
    let sessions = state.sessions.read().await;
    let tts_models = state.tts_models.read().await;
    let stt_models = state.stt_models.read().await;
    let request_count = state.request_count.read().await;

    Json(serde_json::json!({
        "active_sessions": sessions.len(),
        "tts_models": tts_models.len(),
        "stt_models": stt_models.len(),
        "total_requests": *request_count,
        "timestamp": chrono::Utc::now().timestamp_millis(),
    }))
}
