//! Integration tests for the voice service HTTP surface.
//!
//! Routes that need model files (STT/TTS audio) are gated behind
//! `ALLTERNIT_VOICE_MODEL_TESTS=1` so CI without models stays green; the
//! rest run against an engine that never touches the network.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::json;
use tower::ServiceExt;
use std::sync::Arc;
use voice_service::models::PackManager;
use voice_service::{create_router, VoiceServiceState};

/// Router over an empty temp model dir: no test can download or load models
/// by accident, and pack state is deterministic ("missing").
fn app() -> axum::Router {
    let dir = tempfile::tempdir().unwrap().keep();
    create_router(VoiceServiceState::with_packs(Arc::new(
        PackManager::with_root(dir),
    )))
}

/// Router over the real model dir (`~/.allternit/models/voice`), for the
/// gated model tests.
fn model_app() -> axum::Router {
    create_router(VoiceServiceState::new())
}

async fn json_body(response: axum::response::Response) -> serde_json::Value {
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

fn multipart_wav(wav: &[u8], extra: &[(&str, &str)]) -> (String, Vec<u8>) {
    let boundary = "----allternittest";
    let mut body: Vec<u8> = Vec::new();
    for (k, v) in extra {
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n")
                .as_bytes(),
        );
    }
    body.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"audio\"; filename=\"a.wav\"\r\nContent-Type: audio/wav\r\n\r\n").as_bytes(),
    );
    body.extend_from_slice(wav);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

#[tokio::test]
async fn health_check_returns_ok() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["service"], "voice");
    assert_eq!(json["status"], "healthy");
    assert_eq!(json["engine"], "sherpa-onnx");
}

#[tokio::test]
async fn health_reports_sherpa_engine_and_packs() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["engine"], "sherpa-onnx");
    assert_eq!(json["num_threads"], 2);
    assert!(json["stt_ready"].is_boolean());
    assert!(json["tts_ready"].is_boolean());
    // Pack list must be present with both packs, no downloads triggered.
    let packs = json["packs"].as_array().expect("packs array");
    assert_eq!(packs.len(), 3);
    assert!(packs.iter().all(|p| p["state"] == "missing"));
}

#[tokio::test]
async fn list_voices_returns_real_kokoro_voices() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/v1/voices")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let voices: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let voices = voices.as_array().unwrap();
    assert_eq!(voices.len(), 28);
    assert!(voices.iter().any(|v| v["id"] == "af_heart"));
    assert!(voices.iter().any(|v| v["id"] == "bm_george"));
    // Shape compatibility: id/name/language/gender/sample_rate preserved,
    // plus label/engine/assetReady that the allternit-ai pickers read.
    let first = &voices[0];
    for key in [
        "id",
        "name",
        "language",
        "gender",
        "sample_rate",
        "label",
        "engine",
        "assetReady",
    ] {
        assert!(first.get(key).is_some(), "missing key {key}");
    }
    assert_eq!(first["sample_rate"], 24000);
    assert_eq!(first["engine"], "kokoro");
    assert_eq!(first["assetReady"], false);
}

#[tokio::test]
async fn get_voice_roundtrip() {
    let app = app();

    let ok = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/voices/am_michael")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);

    let missing = app
        .oneshot(
            Request::builder()
                .uri("/v1/voices/unknown")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn list_stt_models_includes_both_models() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/v1/stt/models")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let models: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let models = models.as_array().unwrap();
    assert_eq!(models.len(), 2);
    assert!(models.iter().any(|m| m["id"] == "moonshine-tiny-en"));
    assert!(models
        .iter()
        .any(|m| m["id"] == "parakeet-tdt-0.6b-v3-int8"));
}

#[tokio::test]
async fn list_packs_reports_state_without_downloading() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/v1/models")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let packs = json["packs"].as_array().unwrap();
    assert_eq!(packs.len(), 3);
    for p in packs {
        assert!(p["state"].is_string());
        assert!(!p["state"].as_str().unwrap().is_empty());
    }
}

#[tokio::test]
async fn download_unknown_pack_is_404() {
    let response = app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/models/huge")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn stt_rejects_empty_audio() {
    let response = app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/stt")
                .header(
                    "content-type",
                    "multipart/form-data; boundary=----allternit",
                )
                .body(Body::from(
                    "------allternit\r\nContent-Disposition: form-data; name=\"language\"\r\n\r\nen\r\n------allternit--\r\n",
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn tts_rejects_empty_text() {
    let response = app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/tts")
                .header("content-type", "application/json")
                .body(Body::from(json!({ "text": "  " }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn tts_rejects_unknown_voice_and_format_without_models() {
    for body in [
        json!({ "text": "hi", "voice": "nobody" }),
        json!({ "text": "hi", "format": "mp3" }),
    ] {
        let response = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/tts")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(json_body(response).await["error"].is_string());
    }
}

#[tokio::test]
async fn stt_rejects_unsupported_container() {
    // WebM/Matroska magic: no decoder in the service, must be a clear 415.
    let (ct, body) = multipart_wav(&[0x1A, 0x45, 0xDF, 0xA3, 0, 0, 0, 0], &[]);
    let response = app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/stt")
                .header("content-type", ct)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn stt_stream_rejects_bad_sample_rate() {
    let response = app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/stt/stream?sample_rate=12")
                .body(Body::from(vec![0u8; 64]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn stt_stream_rejects_unknown_model() {
    let response = app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/stt/stream?model=whisper")
                .header("content-type", "application/octet-stream")
                .body(Body::from(vec![0u8; 64]))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn create_and_get_session() {
    let app = app();

    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/sessions")
                .header("content-type", "application/json")
                .body(Body::from(json!({ "mode": "tts" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(create_response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(create_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let session_id = created["session_id"].as_str().unwrap();
    assert_eq!(created["mode"], "tts");

    let get_response = app
        .oneshot(
            Request::builder()
                .uri(format!("/v1/sessions/{session_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(get_response.status(), StatusCode::OK);
}

#[tokio::test]
async fn stats_reflect_request_count() {
    let app = app();

    // A rejected (empty-audio) STT request still counts: it exercises the
    // counter without touching models/network.
    let _ = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/stt")
                .header(
                    "content-type",
                    "multipart/form-data; boundary=----allternit",
                )
                .body(Body::from(
                    "------allternit\r\nContent-Disposition: form-data; name=\"audio\"\r\n\r\n\r\n------allternit--\r\n",
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    let stats_response = app
        .oneshot(
            Request::builder()
                .uri("/v1/stats")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(stats_response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(stats_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["total_requests"], 1);
    assert_eq!(json["tts_models"], 28);
    assert_eq!(json["stt_models"], 2);
}

// ── Model-backed tests (require downloads; gated off in CI) ────────────────

fn model_tests_enabled() -> bool {
    std::env::var("ALLTERNIT_VOICE_MODEL_TESTS").ok().as_deref() == Some("1")
}

#[tokio::test]
async fn tts_synthesizes_real_audio() {
    if !model_tests_enabled() {
        eprintln!("skipping: set ALLTERNIT_VOICE_MODEL_TESTS=1");
        return;
    }
    let response = model_app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/tts")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "text": "Allternit is ready.", "voice": "af" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers().clone();
    assert_eq!(headers["content-type"], "audio/wav");
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(body.starts_with(b"RIFF"));
    assert!(body.len() > 1000);
    // Non-silence: some sample exceeds quiet threshold.
    let pcm = &body[44..];
    let has_energy = pcm
        .chunks_exact(2)
        .any(|c| i16::from_le_bytes([c[0], c[1]]).abs() > 64);
    assert!(has_energy, "synthesized audio is silent");
}

#[tokio::test]
async fn stt_transcribes_tts_audio() {
    if !model_tests_enabled() {
        eprintln!("skipping: set ALLTERNIT_VOICE_MODEL_TESTS=1");
        return;
    }
    // Synthesize a short sentence and transcribe it back.
    let tts = model_app();
    let response = tts
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/tts")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "text": "Hello world.", "voice": "af", "format": "pcm16" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let pcm = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    assert!(pcm.len() > 1000);

    // Wrap the pcm16 (24 kHz) into a WAV and POST it to /v1/stt.
    let wav = voice_service::audio::pcm16le_to_wav(&pcm, 24_000, 1);
    let boundary = "----allternittest";
    let mut body: Vec<u8> = Vec::new();
    body.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"audio\"; filename=\"a.wav\"\r\nContent-Type: audio/wav\r\n\r\n").as_bytes(),
    );
    body.extend_from_slice(&wav);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let response = model_app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/stt")
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    // Shape compatibility with the whisper-era response.
    for key in ["text", "confidence", "language", "segments"] {
        assert!(json.get(key).is_some(), "missing key {key}");
    }
    let text = json["text"].as_str().unwrap().to_lowercase();
    assert!(text.contains("hello"), "unexpected transcript: {text}");
}

/// Synthesise `text` as pcm16 at 24 kHz through /v1/tts.
async fn tts_pcm(text: &str) -> Vec<f32> {
    let response = model_app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/tts")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "text": text, "voice": "am_adam", "format": "pcm16" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-sample-rate"], "24000");
    let pcm = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    pcm.chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
        .collect()
}

#[tokio::test]
async fn stt_transcribes_8k_phone_audio() {
    if !model_tests_enabled() {
        eprintln!("skipping: set ALLTERNIT_VOICE_MODEL_TESTS=1");
        return;
    }
    let samples = tts_pcm("The weather is lovely today.").await;
    // 24 kHz -> 8 kHz, as a phone line would deliver it.
    let phone = voice_service::audio::resample(&samples, 24_000, 8_000);
    let wav = voice_service::audio::wav_from_f32(&phone, 8_000);
    for model in ["moonshine", "parakeet"] {
        let (ct, body) = multipart_wav(&wav, &[("model", model)]);
        let response = model_app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/stt")
                    .header("content-type", ct)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let text = json_body(response).await["text"]
            .as_str()
            .unwrap()
            .to_lowercase();
        assert!(text.contains("weather"), "{model}: unexpected transcript: {text}");
    }
}

#[tokio::test]
async fn stt_stream_emits_final_and_done() {
    if !model_tests_enabled() {
        eprintln!("skipping: set ALLTERNIT_VOICE_MODEL_TESTS=1");
        return;
    }
    let mut samples = tts_pcm("Please open the settings page.").await;
    samples.extend(std::iter::repeat_n(0.0, 24_000)); // 1 s trailing silence
    let pcm = voice_service::audio::f32_to_pcm16le(&samples);
    // Odd-sized chunks: the server must carry the split sample over.
    let chunks: Vec<Result<Vec<u8>, std::io::Error>> =
        pcm.chunks(4_801).map(|c| Ok(c.to_vec())).collect();
    let body = Body::from_stream(futures_util::stream::iter(chunks));
    let response = model_app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/stt/stream?sample_rate=24000")
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "application/x-ndjson");
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let events: Vec<serde_json::Value> = String::from_utf8_lossy(&body)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let finals: Vec<String> = events
        .iter()
        .filter(|e| e["type"] == "final")
        .map(|e| e["text"].as_str().unwrap().to_lowercase())
        .collect();
    assert!(
        finals.iter().any(|t| t.contains("settings")),
        "events: {events:?}"
    );
    assert_eq!(events.last().unwrap()["type"], "done");
}

#[tokio::test]
async fn tts_stream_sends_one_audio_event_per_sentence() {
    if !model_tests_enabled() {
        eprintln!("skipping: set ALLTERNIT_VOICE_MODEL_TESTS=1");
        return;
    }
    let response = model_app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/tts/stream")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "text": "First sentence. Second one! Third?" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let events: Vec<serde_json::Value> = String::from_utf8_lossy(&body)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let audio: Vec<_> = events.iter().filter(|e| e["type"] == "audio").collect();
    assert_eq!(audio.len(), 3, "events: {events:?}");
    for (i, e) in audio.iter().enumerate() {
        assert_eq!(e["index"], i);
        assert_eq!(e["sample_rate"], 24000);
        assert!(e["audio_b64"].as_str().unwrap().len() > 1000);
    }
    assert_eq!(events.last().unwrap()["type"], "done");
}
