//! Unit tests for the media plane: scripted-transport client wiring,
//! key resolution, catalog flags, and end-to-end core handler flows.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use axum::http::StatusCode;
use serde_json::{json, Value};

use super::catalog;
use super::clients::{
    generate_flux_images, generate_gpt_images, poll_video, submit_video, ImageEntry,
    MediaTransport, ProviderKey, VideoBackend, VideoJobHandle, VideoJobSubmit, VideoPoll,
};
use super::handlers::{
    generate_image_core, get_video_job_core, job_artifact_core, store_uploaded_artifact_core,
    submit_video_job_core, GenerateImageRequest, SubmitVideoRequest, MAX_UPLOAD_BYTES,
};
use super::{get_artifact, get_job, resolve_provider_key};
use crate::db::DbHandle;
use crate::llm_gateway::route_credentials::upsert_credential;

// ─── Scripted mock transport ─────────────────────────────────────────────────

#[derive(Clone)]
enum ScriptedResponse {
    Json(Value),
    Bytes(Vec<u8>, String),
}

#[derive(Debug, Clone)]
struct RecordedRequest {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body: Option<Value>,
}

/// Scripts `(status, response)` pairs per `METHOD URL`; pops one per call and
/// records every request for assertion.
struct MockTransport {
    scripted: Mutex<HashMap<String, VecDeque<(u16, ScriptedResponse)>>>,
    recorded: Mutex<Vec<RecordedRequest>>,
}

impl MockTransport {
    fn new() -> Self {
        Self {
            scripted: Mutex::new(HashMap::new()),
            recorded: Mutex::new(Vec::new()),
        }
    }

    fn script_json(&self, method: &str, url: &str, status: u16, body: Value) {
        self.script(method, url, status, ScriptedResponse::Json(body));
    }

    fn script_bytes(&self, method: &str, url: &str, status: u16, bytes: &[u8], content_type: &str) {
        self.script(
            method,
            url,
            status,
            ScriptedResponse::Bytes(bytes.to_vec(), content_type.to_string()),
        );
    }

    fn script(&self, method: &str, url: &str, status: u16, response: ScriptedResponse) {
        self.scripted
            .lock()
            .unwrap()
            .entry(format!("{method} {url}"))
            .or_default()
            .push_back((status, response));
    }

    fn recorded(&self) -> Vec<RecordedRequest> {
        self.recorded.lock().unwrap().clone()
    }

    fn recorded_for(&self, method: &str, url: &str) -> Vec<RecordedRequest> {
        self.recorded()
            .into_iter()
            .filter(|r| r.method == method && r.url == url)
            .collect()
    }

    fn pop(&self, method: &str, url: &str) -> Result<(u16, ScriptedResponse), String> {
        self.scripted
            .lock()
            .unwrap()
            .get_mut(&format!("{method} {url}"))
            .and_then(|queue| queue.pop_front())
            .ok_or_else(|| format!("no scripted response for {method} {url}"))
    }

    fn record(&self, method: &str, url: &str, headers: &[(&str, &str)], body: Option<Value>) {
        self.recorded.lock().unwrap().push(RecordedRequest {
            method: method.to_string(),
            url: url.to_string(),
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            body,
        });
    }

    fn header<'a>(req: &'a RecordedRequest, name: &str) -> Option<&'a str> {
        req.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

#[async_trait::async_trait]
impl MediaTransport for MockTransport {
    async fn post_json(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        body: &Value,
    ) -> Result<(u16, Value), String> {
        self.record("POST", url, headers, Some(body.clone()));
        let (status, response) = self.pop("POST", url)?;
        match response {
            ScriptedResponse::Json(value) => Ok((status, value)),
            ScriptedResponse::Bytes(_, _) => Err("scripted bytes for JSON call".to_string()),
        }
    }

    async fn get_json(
        &self,
        url: &str,
        headers: &[(&str, &str)],
    ) -> Result<(u16, Value), String> {
        self.record("GET", url, headers, None);
        let (status, response) = self.pop("GET", url)?;
        match response {
            ScriptedResponse::Json(value) => Ok((status, value)),
            ScriptedResponse::Bytes(_, _) => Err("scripted bytes for JSON call".to_string()),
        }
    }

    async fn get_bytes(
        &self,
        url: &str,
        headers: &[(&str, &str)],
    ) -> Result<(u16, Vec<u8>, String), String> {
        self.record("GET", url, headers, None);
        let (status, response) = self.pop("GET", url)?;
        match response {
            ScriptedResponse::Bytes(bytes, content_type) => Ok((status, bytes, content_type)),
            ScriptedResponse::Json(_) => Err("scripted JSON for bytes call".to_string()),
        }
    }
}

fn key() -> ProviderKey {
    ProviderKey {
        api_key: "test-key".to_string(),
        base_url: None,
    }
}

fn t2v_submit() -> VideoJobSubmit {
    VideoJobSubmit {
        prompt: "a cat juggling".to_string(),
        duration_secs: 6,
        resolution: "768P".to_string(),
        aspect_ratio: "16:9".to_string(),
        image_url: None,
        fast: true,
    }
}

fn db_with_credential(provider: &str) -> DbHandle {
    let db = DbHandle::new_memory().unwrap();
    upsert_credential(&db, "u1", None, provider, "test-key", None, None, true).unwrap();
    db
}

// ─── MiniMax wiring ─────────────────────────────────────────────────────────

#[tokio::test]
async fn minimax_submit_poll_download_wiring() {
    let t = MockTransport::new();
    t.script_json(
        "POST",
        "https://api.minimax.io/v2/video_generation",
        200,
        json!({"task_id": "t1"}),
    );
    t.script_json(
        "GET",
        "https://api.minimax.io/v2/query/video_generation/t1",
        200,
        json!({"task": {"status": "Processing"}}),
    );
    t.script_json(
        "GET",
        "https://api.minimax.io/v2/query/video_generation/t1",
        200,
        json!({"task": {"status": "succeeded", "content": {"url": "https://cdn.example/v.mp4"}}}),
    );
    t.script_bytes(
        "GET",
        "https://cdn.example/v.mp4",
        200,
        b"mp4-bytes",
        "video/mp4",
    );

    let handle = submit_video(&t, VideoBackend::MinimaxH3, &key(), &t2v_submit())
        .await
        .unwrap();
    assert_eq!(handle.provider_task_id, "t1");

    // Bearer auth on both submit and poll.
    let submits = t.recorded_for("POST", "https://api.minimax.io/v2/video_generation");
    assert_eq!(
        MockTransport::header(&submits[0], "authorization"),
        Some("Bearer test-key")
    );

    assert_eq!(
        poll_video(&t, VideoBackend::MinimaxH3, &key(), &handle).await.unwrap(),
        VideoPoll::Processing
    );
    assert_eq!(
        poll_video(&t, VideoBackend::MinimaxH3, &key(), &handle).await.unwrap(),
        VideoPoll::Succeeded {
            video_url: "https://cdn.example/v.mp4".to_string()
        }
    );

    // Poll sequence: exactly two GETs in order.
    let polls = t.recorded_for("GET", "https://api.minimax.io/v2/query/video_generation/t1");
    assert_eq!(polls.len(), 2);

    // Queued mapping (Preparing/Queueing).
    t.script_json(
        "GET",
        "https://api.minimax.io/v2/query/video_generation/t1",
        200,
        json!({"task": {"status": "Queueing"}}),
    );
    assert_eq!(
        poll_video(&t, VideoBackend::MinimaxH3, &key(), &handle).await.unwrap(),
        VideoPoll::Queued
    );
}

#[tokio::test]
async fn minimax_i2v_uses_first_frame_and_adaptive_ratio() {
    let t = MockTransport::new();
    t.script_json(
        "POST",
        "https://api.minimax.io/v2/video_generation",
        200,
        json!({"task_id": "t2"}),
    );
    let submit = VideoJobSubmit {
        image_url: Some("https://cdn.example/frame.png".to_string()),
        ..t2v_submit()
    };
    submit_video(&t, VideoBackend::MinimaxH3, &key(), &submit)
        .await
        .unwrap();
    let body = t.recorded_for("POST", "https://api.minimax.io/v2/video_generation")[0]
        .body
        .clone()
        .unwrap();
    let content = body["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "text");
    assert_eq!(content[1]["type"], "image_url");
    assert_eq!(content[1]["role"], "first_frame");
    assert_eq!(content[1]["image_url"]["url"], "https://cdn.example/frame.png");
    assert_eq!(body["ratio"], "adaptive");
}

#[tokio::test]
async fn minimax_failed_maps_error_message() {
    let t = MockTransport::new();
    let handle = VideoJobHandle {
        provider_task_id: "t9".to_string(),
        poll_status_url: None,
        poll_response_url: None,
    };
    t.script_json(
        "GET",
        "https://api.minimax.io/v2/query/video_generation/t9",
        200,
        json!({"task": {"status": "failed", "error": {"message": "content blocked"}}}),
    );
    assert_eq!(
        poll_video(&t, VideoBackend::MinimaxH3, &key(), &handle).await.unwrap(),
        VideoPoll::Failed {
            message: "content blocked".to_string()
        }
    );
}

// ─── fal wiring ──────────────────────────────────────────────────────────────

const FAL_SUBMIT_URL: &str =
    "https://queue.fal.run/bytedance/seedance-2.0/fast/text-to-video";

fn fal_handle() -> VideoJobHandle {
    VideoJobHandle {
        provider_task_id: "req-1".to_string(),
        poll_status_url: Some("https://queue.fal.run/requests/req-1/status".to_string()),
        poll_response_url: Some("https://queue.fal.run/requests/req-1/response".to_string()),
    }
}

#[tokio::test]
async fn fal_submit_poll_download_wiring() {
    let t = MockTransport::new();
    t.script_json(
        "POST",
        FAL_SUBMIT_URL,
        200,
        json!({
            "request_id": "req-1",
            "status_url": "https://queue.fal.run/requests/req-1/status",
            "response_url": "https://queue.fal.run/requests/req-1/response",
        }),
    );
    t.script_json(
        "GET",
        "https://queue.fal.run/requests/req-1/status",
        200,
        json!({"status": "IN_QUEUE", "queue_position": 3}),
    );
    t.script_json(
        "GET",
        "https://queue.fal.run/requests/req-1/status",
        200,
        json!({"status": "COMPLETED"}),
    );
    t.script_json(
        "GET",
        "https://queue.fal.run/requests/req-1/response",
        200,
        json!({"video": {"url": "https://fal.media/out.mp4", "content_type": "video/mp4"}}),
    );

    let handle = submit_video(&t, VideoBackend::FalSeedance, &key(), &t2v_submit())
        .await
        .unwrap();
    assert_eq!(handle.provider_task_id, "req-1");
    assert_eq!(
        handle.poll_status_url.as_deref(),
        Some("https://queue.fal.run/requests/req-1/status")
    );

    // fal uses `Authorization: Key`, duration goes as a string.
    let submits = t.recorded_for("POST", FAL_SUBMIT_URL);
    assert_eq!(
        MockTransport::header(&submits[0], "authorization"),
        Some("Key test-key")
    );
    assert_eq!(submits[0].body.as_ref().unwrap()["duration"], "6");

    assert_eq!(
        poll_video(&t, VideoBackend::FalSeedance, &key(), &handle).await.unwrap(),
        VideoPoll::Queued
    );
    assert_eq!(
        poll_video(&t, VideoBackend::FalSeedance, &key(), &handle).await.unwrap(),
        VideoPoll::Succeeded {
            video_url: "https://fal.media/out.mp4".to_string()
        }
    );
}

#[tokio::test]
async fn fal_completed_with_error_maps_to_failed() {
    let t = MockTransport::new();
    t.script_json(
        "GET",
        "https://queue.fal.run/requests/req-1/status",
        200,
        json!({"status": "COMPLETED", "error": "nsfw content", "error_type": "content_policy"}),
    );
    assert_eq!(
        poll_video(&t, VideoBackend::FalSeedance, &key(), &fal_handle())
            .await
            .unwrap(),
        VideoPoll::Failed {
            message: "nsfw content".to_string()
        }
    );
}

#[tokio::test]
async fn fal_standard_tier_uses_standard_endpoint() {
    let t = MockTransport::new();
    t.script_json(
        "POST",
        "https://queue.fal.run/bytedance/seedance-2.0/standard/text-to-video",
        200,
        json!({"request_id": "req-2", "status_url": "https://queue.fal.run/s", "response_url": "https://queue.fal.run/r"}),
    );
    let submit = VideoJobSubmit {
        fast: false,
        ..t2v_submit()
    };
    submit_video(&t, VideoBackend::FalSeedance, &key(), &submit)
        .await
        .unwrap();
    assert_eq!(t.recorded_for("POST", "https://queue.fal.run/bytedance/seedance-2.0/standard/text-to-video").len(), 1);
}

#[tokio::test]
async fn fal_i2v_uses_image_to_video_endpoint() {
    let t = MockTransport::new();
    t.script_json(
        "POST",
        "https://queue.fal.run/bytedance/seedance-2.0/fast/image-to-video",
        200,
        json!({"request_id": "req-3", "status_url": "https://queue.fal.run/s", "response_url": "https://queue.fal.run/r"}),
    );
    let submit = VideoJobSubmit {
        image_url: Some("https://example.com/frame.png".to_string()),
        ..t2v_submit()
    };
    submit_video(&t, VideoBackend::FalSeedance, &key(), &submit)
        .await
        .unwrap();
    let requests = t.recorded_for("POST", "https://queue.fal.run/bytedance/seedance-2.0/fast/image-to-video");
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].body.as_ref().unwrap()["image_url"],
        "https://example.com/frame.png"
    );
}

// ─── gpt-image + flux image generation ───────────────────────────────────────

#[tokio::test]
async fn gpt_image_handles_b64_and_url_entries() {
    let t = MockTransport::new();
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(b"png-from-b64");
    t.script_json(
        "POST",
        "https://api.openai.com/v1/images/generations",
        200,
        json!({"data": [
            {"b64_json": b64},
            {"url": "https://oai.example/img2.png"},
        ]}),
    );
    t.script_bytes(
        "GET",
        "https://oai.example/img2.png",
        200,
        b"png-from-url",
        "image/png",
    );

    let entries = generate_gpt_images(&t, &key(), "a dog", "1024x1024", "medium", 2)
        .await
        .unwrap();
    assert_eq!(entries.len(), 2);
    assert!(matches!(entries[0], ImageEntry::B64 { .. }));
    assert!(matches!(entries[1], ImageEntry::Url { .. }));

    // Size/quality pass through.
    let body = t.recorded_for("POST", "https://api.openai.com/v1/images/generations")[0]
        .body
        .clone()
        .unwrap();
    assert_eq!(body["size"], "1024x1024");
    assert_eq!(body["quality"], "medium");
    assert_eq!(body["n"], 2);
    assert_eq!(
        MockTransport::header(&t.recorded_for("POST", "https://api.openai.com/v1/images/generations")[0], "authorization"),
        Some("Bearer test-key")
    );

    // Cost math: medium × 2 = 0.106.
    assert!((catalog::estimate_gpt_image_cost("medium", 2).unwrap() - 0.106).abs() < 1e-9);
}

#[tokio::test]
async fn flux_sync_generate_downloads_images() {
    let t = MockTransport::new();
    t.script_json(
        "POST",
        "https://fal.run/fal-ai/flux/schnell",
        200,
        json!({"images": [{
            "url": "https://fal.media/flux1.png",
            "width": 1024,
            "height": 1024,
            "content_type": "image/png",
        }]}),
    );
    t.script_bytes("GET", "https://fal.media/flux1.png", 200, b"flux-png", "image/png");

    let entries = generate_flux_images(&t, &key(), "a landscape", 1, None)
        .await
        .unwrap();
    assert_eq!(entries.len(), 1);
    match &entries[0] {
        ImageEntry::Url { url, width, height, content_type } => {
            assert_eq!(url, "https://fal.media/flux1.png");
            assert_eq!(*width, Some(1024));
            assert_eq!(*height, Some(1024));
            assert_eq!(content_type.as_deref(), Some("image/png"));
        }
        _ => panic!("expected URL entry"),
    }
    assert_eq!(
        MockTransport::header(&t.recorded_for("POST", "https://fal.run/fal-ai/flux/schnell")[0], "authorization"),
        Some("Key test-key")
    );
}

// ─── Catalog + key resolution ────────────────────────────────────────────────

#[test]
fn platform_funded_flag_gates_env_lane() {
    std::env::remove_var("ALLTERNIT_MEDIA_PLATFORM_FUNDED");
    std::env::remove_var("MINIMAX_API_KEY");
    assert!(!catalog::platform_funded_enabled());

    // Env key present but flag unset → still disabled.
    std::env::set_var("MINIMAX_API_KEY", "platform-key");
    assert!(!catalog::platform_funded_enabled());

    // Flag on → enabled.
    std::env::set_var("ALLTERNIT_MEDIA_PLATFORM_FUNDED", "1");
    assert!(catalog::platform_funded_enabled());
    std::env::set_var("ALLTERNIT_MEDIA_PLATFORM_FUNDED", "true");
    assert!(catalog::platform_funded_enabled());
    std::env::set_var("ALLTERNIT_MEDIA_PLATFORM_FUNDED", "yes");
    assert!(!catalog::platform_funded_enabled());

    std::env::remove_var("ALLTERNIT_MEDIA_PLATFORM_FUNDED");
    std::env::remove_var("MINIMAX_API_KEY");
}

#[test]
fn catalog_marks_byok_and_platform_availability() {
    let db = db_with_credential("minimax");
    let catalog = catalog::catalog_for_user(&db, "u1");
    let providers = catalog["providers"].as_array().unwrap();
    assert_eq!(providers.len(), 4);

    let minimax = providers.iter().find(|p| p["id"] == "minimax-h3").unwrap();
    assert_eq!(minimax["byok"]["configured"], true);
    assert_eq!(minimax["platform_funded"]["enabled"], false);
    assert_eq!(
        minimax["models"][0]["resolutions"][0]["price_per_second"],
        0.08
    );

    // fal shares the "fal" credential id with flux-fal.
    let fal = providers.iter().find(|p| p["id"] == "fal-seedance").unwrap();
    assert_eq!(fal["byok"]["configured"], false);
    let flux = providers.iter().find(|p| p["id"] == "flux-fal").unwrap();
    assert_eq!(flux["byok"]["configured"], false);

    let openai = providers.iter().find(|p| p["id"] == "gpt-image").unwrap();
    assert_eq!(openai["byok"]["configured"], false);
    assert!(openai["note"].as_str().unwrap().contains("Batch API"));
}

#[test]
fn resolve_key_prefers_byok_then_platform_env() {
    // BYOK credential present → used (base_url override honored).
    let db = db_with_credential("minimax");
    let key = resolve_provider_key(&db, "u1", "minimax-h3").unwrap().unwrap();
    assert_eq!(key.api_key, "test-key");

    // No credential, flag off → none.
    assert!(resolve_provider_key(&db, "u1", "fal-seedance").unwrap().is_none());

    // Flag on + env key → platform lane.
    std::env::set_var("ALLTERNIT_MEDIA_PLATFORM_FUNDED", "1");
    std::env::set_var("FAL_KEY", "platform-fal-key");
    let key = resolve_provider_key(&db, "u1", "fal-seedance").unwrap().unwrap();
    assert_eq!(key.api_key, "platform-fal-key");
    assert!(key.base_url.is_none());
    std::env::remove_var("ALLTERNIT_MEDIA_PLATFORM_FUNDED");
    std::env::remove_var("FAL_KEY");
}

#[test]
fn video_cost_estimates_use_catalog_prices() {
    assert!((catalog::estimate_video_cost("minimax-h3", "768P", true, 6).unwrap() - 0.48).abs() < 1e-9);
    assert!((catalog::estimate_video_cost("minimax-h3", "2K", true, 6).unwrap() - 0.78).abs() < 1e-9);
    assert!((catalog::estimate_video_cost("fal-seedance", "720p", true, 5).unwrap() - 1.2095).abs() < 1e-9);
    assert!((catalog::estimate_video_cost("fal-seedance", "720p", false, 5).unwrap() - 1.517).abs() < 1e-9);
    assert!((catalog::estimate_video_cost("fal-seedance", "1080p", false, 5).unwrap() - 3.41).abs() < 1e-9);
}

// ─── Core handler flows (mock transport + memory db) ─────────────────────────

#[tokio::test]
async fn submit_without_key_returns_no_provider_key() {
    let db = DbHandle::new_memory().unwrap();
    let t = MockTransport::new();
    let req = SubmitVideoRequest {
        provider: "minimax-h3".to_string(),
        prompt: "hello".to_string(),
        duration: None,
        resolution: None,
        aspect_ratio: None,
        image_url: None,
        fast: None,
    };
    let (status, body) = submit_video_job_core(&db, &t, "u1", req).await.unwrap_err();
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "no_provider_key");
    assert!(body["message"].as_str().unwrap().contains("minimax"));
}

#[tokio::test]
async fn submit_validates_duration() {
    let db = db_with_credential("minimax");
    let t = MockTransport::new();
    let req = SubmitVideoRequest {
        provider: "minimax-h3".to_string(),
        prompt: "hello".to_string(),
        duration: Some(20),
        resolution: None,
        aspect_ratio: None,
        image_url: None,
        fast: None,
    };
    let (status, _) = submit_video_job_core(&db, &t, "u1", req).await.unwrap_err();
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn full_video_job_lifecycle_persists_artifact() {
    let db = db_with_credential("minimax");
    let t = MockTransport::new();
    t.script_json(
        "POST",
        "https://api.minimax.io/v2/video_generation",
        200,
        json!({"task_id": "t1"}),
    );
    t.script_json(
        "GET",
        "https://api.minimax.io/v2/query/video_generation/t1",
        200,
        json!({"task": {"status": "Processing"}}),
    );
    t.script_json(
        "GET",
        "https://api.minimax.io/v2/query/video_generation/t1",
        200,
        json!({"task": {"status": "succeeded", "content": {"url": "https://cdn.example/v.mp4"}}}),
    );
    t.script_bytes("GET", "https://cdn.example/v.mp4", 200, b"final-mp4", "video/mp4");

    let req = SubmitVideoRequest {
        provider: "minimax-h3".to_string(),
        prompt: "hello world".to_string(),
        duration: Some(6),
        resolution: None,
        aspect_ratio: None,
        image_url: None,
        fast: None,
    };
    let submitted = submit_video_job_core(&db, &t, "u1", req).await.unwrap();
    assert_eq!(submitted["status"], "queued");
    assert_eq!(submitted["provider"], "minimax-h3");
    assert!((submitted["estimated_cost_usd"].as_f64().unwrap() - 0.48).abs() < 1e-9);
    let job_id = submitted["job_id"].as_str().unwrap().to_string();

    // Row persisted with the provider task id.
    let row = get_job(&db, &job_id, "u1").unwrap().unwrap();
    assert_eq!(row.provider_task_id.as_deref(), Some("t1"));
    assert_eq!(row.status, "queued");

    // First poll: still processing.
    let payload = get_video_job_core(&db, &t, "u1", &job_id).await.unwrap();
    assert_eq!(payload["status"], "processing");

    // Second poll: succeeded + artifact downloaded and linked.
    let payload = get_video_job_core(&db, &t, "u1", &job_id).await.unwrap();
    assert_eq!(payload["status"], "succeeded");
    let artifact_url = payload["video"]["artifact_url"].as_str().unwrap();
    assert!(artifact_url.starts_with("/api/v1/media/artifacts/"));
    assert_eq!(payload["video"]["content_type"], "video/mp4");

    let artifact_id = artifact_url.rsplit('/').next().unwrap();
    let artifact = get_artifact(&db, artifact_id, "u1").unwrap().unwrap();
    assert_eq!(artifact.bytes, b"final-mp4");
    assert_eq!(artifact.job_id.as_deref(), Some(job_id.as_str()));

    // Cached terminal response — no new provider calls needed.
    let before = t.recorded().len();
    let payload = get_video_job_core(&db, &t, "u1", &job_id).await.unwrap();
    assert_eq!(payload["status"], "succeeded");
    assert_eq!(t.recorded().len(), before);

    // Download resolution → artifact id.
    let (_, resolved_id) = job_artifact_core(&db, &t, "u1", &job_id).await.unwrap();
    assert_eq!(resolved_id, artifact_id);

    // Ownership: another user cannot see the job or the artifact.
    assert!(get_job(&db, &job_id, "u2").unwrap().is_none());
    assert!(get_artifact(&db, artifact_id, "u2").unwrap().is_none());
}

#[tokio::test]
async fn failed_video_job_persists_error() {
    let db = db_with_credential("fal");
    let t = MockTransport::new();
    t.script_json(
        "POST",
        FAL_SUBMIT_URL,
        200,
        json!({
            "request_id": "req-1",
            "status_url": "https://queue.fal.run/requests/req-1/status",
            "response_url": "https://queue.fal.run/requests/req-1/response",
        }),
    );
    t.script_json(
        "GET",
        "https://queue.fal.run/requests/req-1/status",
        200,
        json!({"status": "COMPLETED", "error": "prompt rejected"}),
    );

    let req = SubmitVideoRequest {
        provider: "fal-seedance".to_string(),
        prompt: "hello".to_string(),
        duration: None,
        resolution: None,
        aspect_ratio: None,
        image_url: None,
        fast: None,
    };
    let submitted = submit_video_job_core(&db, &t, "u1", req).await.unwrap();
    let job_id = submitted["job_id"].as_str().unwrap().to_string();

    let payload = get_video_job_core(&db, &t, "u1", &job_id).await.unwrap();
    assert_eq!(payload["status"], "failed");
    assert_eq!(payload["error"], "prompt rejected");
    let row = get_job(&db, &job_id, "u1").unwrap().unwrap();
    assert_eq!(row.status, "failed");
}

#[tokio::test]
async fn image_generate_persists_both_entry_kinds() {
    let db = db_with_credential("openai");
    let t = MockTransport::new();
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(b"png-from-b64");
    t.script_json(
        "POST",
        "https://api.openai.com/v1/images/generations",
        200,
        json!({"data": [{"b64_json": b64}, {"url": "https://oai.example/img2.png"}]}),
    );
    t.script_bytes("GET", "https://oai.example/img2.png", 200, b"png-from-url", "image/png");

    let req = GenerateImageRequest {
        provider: "gpt-image".to_string(),
        prompt: "two icons".to_string(),
        size: Some("1024x1024".to_string()),
        quality: Some("medium".to_string()),
        n: Some(2),
        reference_image: None,
    };
    let payload = generate_image_core(&db, &t, "u1", req).await.unwrap();
    assert_eq!(payload["images"].as_array().unwrap().len(), 2);
    assert!((payload["estimated_cost_usd"].as_f64().unwrap() - 0.106).abs() < 1e-9);

    let url0 = payload["images"][0]["artifact_url"].as_str().unwrap();
    let artifact0 = get_artifact(&db, url0.rsplit('/').next().unwrap(), "u1").unwrap().unwrap();
    assert_eq!(artifact0.bytes, b"png-from-b64");
    assert_eq!(artifact0.content_type, "image/png");

    let url1 = payload["images"][1]["artifact_url"].as_str().unwrap();
    let artifact1 = get_artifact(&db, url1.rsplit('/').next().unwrap(), "u1").unwrap().unwrap();
    assert_eq!(artifact1.bytes, b"png-from-url");
}

#[tokio::test]
async fn image_generate_flux_persists_artifacts() {
    let db = db_with_credential("fal");
    let t = MockTransport::new();
    t.script_json(
        "POST",
        "https://fal.run/fal-ai/flux/schnell",
        200,
        json!({"images": [{"url": "https://fal.media/flux1.png", "width": 1024, "height": 1024, "content_type": "image/png"}]}),
    );
    t.script_bytes("GET", "https://fal.media/flux1.png", 200, b"flux-png", "image/png");

    let req = GenerateImageRequest {
        provider: "flux-fal".to_string(),
        prompt: "an icon".to_string(),
        size: None,
        quality: None,
        n: Some(1),
        reference_image: None,
    };
    let payload = generate_image_core(&db, &t, "u1", req).await.unwrap();
    assert_eq!(payload["images"].as_array().unwrap().len(), 1);
    assert_eq!(payload["images"][0]["width"], 1024);
    assert!((payload["estimated_cost_usd"].as_f64().unwrap() - 0.003).abs() < 1e-9);
}

#[tokio::test]
async fn image_generate_requires_key() {
    let db = DbHandle::new_memory().unwrap();
    let t = MockTransport::new();
    let req = GenerateImageRequest {
        provider: "gpt-image".to_string(),
        prompt: "x".to_string(),
        size: None,
        quality: None,
        n: None,
        reference_image: None,
    };
    let (status, body) = generate_image_core(&db, &t, "u1", req).await.unwrap_err();
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "no_provider_key");
}

// ─── Axum handler smoke tests (auth + catalog) ───────────────────────────────

fn test_db() -> DbHandle {
    DbHandle::new_memory().unwrap()
}

async fn test_app_state(db: DbHandle, temp: &std::path::Path) -> Arc<crate::AppState> {
    let config = crate::AppConfig {
        company: Default::default(),
        user: Default::default(),
    };
    let auth_config = crate::auth::AuthConfig::from_app_config(&config);
    let jwks = crate::auth::JwksManager::new(&auth_config);
    let rails = crate::rails::RailsState::new(temp.join("rails"))
        .await
        .expect("test rails");
    let desktop_host_registry = crate::desktop_host_registry::DesktopHostRegistry::new(db.clone());
    Arc::new(crate::AppState {
        config,
        db: db.clone(),
        data_dir: temp.to_path_buf(),
        jwks,
        auth_config,
        vm_driver: None,
        incus_driver: None,
        desktop_host_registry,
        desktop_host_provisioner: None,
        bot_desktop_sessions: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        computer_guest_tokens: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        rails,
        vm_sessions: crate::vm_session_routes::new_vm_session_store(),
        cowork_scheduler: None,
        cowork_background: None,
        cowork_run_manager: None,
        webhook_secret: None,
        office_runtime: Arc::new(tokio::sync::RwLock::new(
            crate::office_routes::OfficeRuntimeFile::default(),
        )),
        office_cli_docs: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        office_cli_watches: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        office_cli_mcp_sessions: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        design_skill_cache: crate::design_connector_routes::DesignSkillCache::new(),
        #[cfg(unix)]
        terminal_sessions: crate::terminal_routes::TerminalSessionStore::new(),
        mcp_dispatcher: crate::mcp_dispatcher::McpDispatcher::new(),
        approval_store: Arc::new(crate::permission_policy::ApprovalStore::new()),
        passkey_state: None,
        resource_class_catalog: crate::fabric::sku::ResourceClassCatalog::builtin(),
        fabric_node_provider: allternit_computer_cloud::providers::fabric_node::FabricNodeProvider::new(
            std::sync::Arc::new(allternit_computer_cloud::providers::fabric_node::FabricNodePool::new()),
            "__test__".to_string(),
        ),
        fabric_provider_registry: allternit_computer_cloud::fabric::FabricProviderRegistry::empty(),
        fabric_scheduler: crate::fabric::Scheduler::new(crate::fabric::CostEngine::default_engine()),
        fabric_price_cache: crate::fabric::PriceCache::new(db.clone()),
        os_control_plane: None,
        dp_jwks: crate::auth_dp_jwt::DataPlaneJwks::disabled(),
        deployment_scheduler: Arc::new(crate::deployment_scheduler::DeploymentSchedulerState::new()),
    })
}

#[tokio::test]
async fn catalog_route_requires_auth_and_returns_availability() {
    let temp = tempfile::tempdir().unwrap();
    let db = test_db();
    upsert_credential(&db, "u1", None, "openai", "sk-openai", None, None, true).unwrap();
    let state = test_app_state(db, temp.path()).await;
    let app = crate::provider_routes::provider_router().with_state(state);

    // No auth header → 401.
    let req = axum::http::Request::builder()
        .method("GET")
        .uri("/media/catalog")
        .body(axum::body::Body::empty())
        .unwrap();
    let res = tower::ServiceExt::oneshot(app.clone(), req).await.unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::UNAUTHORIZED);

    // With auth → 200 and the caller's BYOK flags.
    let req = axum::http::Request::builder()
        .method("GET")
        .uri("/media/catalog")
        .header("x-allternit-user-id", "u1")
        .body(axum::body::Body::empty())
        .unwrap();
    let res = tower::ServiceExt::oneshot(app, req).await.unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::OK);
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    let providers = body["providers"].as_array().unwrap();
    let openai = providers.iter().find(|p| p["id"] == "gpt-image").unwrap();
    assert_eq!(openai["byok"]["configured"], true);
}

#[tokio::test]
async fn video_job_route_requires_auth_and_rejects_missing_key() {
    let temp = tempfile::tempdir().unwrap();
    let state = test_app_state(test_db(), temp.path()).await;
    let app = crate::provider_routes::provider_router().with_state(state);

    let body = json!({
        "provider": "minimax-h3",
        "prompt": "a cat",
    });
    // No auth → 401.
    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/media/video/jobs")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();
    let res = tower::ServiceExt::oneshot(app.clone(), req).await.unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::UNAUTHORIZED);

    // Auth but no key → 400 no_provider_key.
    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/media/video/jobs")
        .header("content-type", "application/json")
        .header("x-allternit-user-id", "u1")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();
    let res = tower::ServiceExt::oneshot(app, req).await.unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::BAD_REQUEST);
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(payload["error"], "no_provider_key");
}

// ─── App uploads (native lane) ───────────────────────────────────────────────

#[test]
fn uploaded_video_is_stored_for_its_user_only() {
    let db = DbHandle::new_memory().unwrap();
    let value = store_uploaded_artifact_core(&db, "u1", Some("video/mp4; codecs=avc1"), b"mp4-bytes").unwrap();
    let id = value["artifact_id"].as_str().unwrap();
    assert_eq!(value["artifact_url"], format!("/api/v1/media/artifacts/{id}"));
    assert_eq!(value["content_type"], "video/mp4");
    let row = get_artifact(&db, id, "u1").unwrap().unwrap();
    assert_eq!(row.bytes, b"mp4-bytes");
    assert_eq!(row.content_type, "video/mp4");
    assert!(get_artifact(&db, id, "u2").unwrap().is_none());
}

#[test]
fn uploads_refuse_non_media_empty_and_oversized_files() {
    let db = DbHandle::new_memory().unwrap();
    for ct in [None, Some("text/html"), Some("application/javascript"), Some("image/svg+xml")] {
        let (status, _) = store_uploaded_artifact_core(&db, "u1", ct, b"x").unwrap_err();
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST, "{ct:?}");
    }
    let (status, _) = store_uploaded_artifact_core(&db, "u1", Some("video/mp4"), b"").unwrap_err();
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    let big = vec![0u8; MAX_UPLOAD_BYTES + 1];
    let (status, _) = store_uploaded_artifact_core(&db, "u1", Some("video/mp4"), &big).unwrap_err();
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn image_generate_with_reference_uses_gpt_image_edits() {
    let db = db_with_credential("openai");
    let t = MockTransport::new();
    use base64::Engine;
    let out = base64::engine::general_purpose::STANDARD.encode(b"sprite-png");
    t.script_json(
        "POST",
        "https://api.openai.com/v1/images/edits",
        200,
        json!({"data": [{"b64_json": out}]}),
    );
    let photo = format!(
        "data:image/jpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(b"jpeg-bytes")
    );
    let req = GenerateImageRequest {
        provider: "gpt-image".to_string(),
        prompt: "pixel-art sprite of this person".to_string(),
        size: None,
        quality: Some("low".to_string()),
        n: Some(1),
        reference_image: Some(photo.clone()),
    };
    let payload = generate_image_core(&db, &t, "u1", req).await.unwrap();
    assert_eq!(payload["images"].as_array().unwrap().len(), 1);
    assert!(t.recorded_for("POST", "https://api.openai.com/v1/images/generations").is_empty());
    let body = t.recorded_for("POST", "https://api.openai.com/v1/images/edits")[0]
        .body
        .clone()
        .unwrap();
    assert_eq!(body["model"], "gpt-image-2");
    assert_eq!(body["images"][0]["image_url"], photo);
    assert_eq!(body["quality"], "low");
    let url = payload["images"][0]["artifact_url"].as_str().unwrap();
    let artifact = get_artifact(&db, url.rsplit('/').next().unwrap(), "u1").unwrap().unwrap();
    assert_eq!(artifact.bytes, b"sprite-png");
}

#[tokio::test]
async fn image_generate_rejects_bad_reference_images() {
    let db = db_with_credential("fal");
    let t = MockTransport::new();
    let req = |provider: &str, reference: &str| GenerateImageRequest {
        provider: provider.to_string(),
        prompt: "sprite".to_string(),
        size: None,
        quality: None,
        n: None,
        reference_image: Some(reference.to_string()),
    };
    // FLUX has no reference lane.
    let err = generate_image_core(&db, &t, "u1", req("flux-fal", "data:image/png;base64,AAAA"))
        .await
        .unwrap_err();
    assert_eq!(err.0, StatusCode::BAD_REQUEST);
    // Not a data URL / wrong type / bad base64.
    for bad in ["https://x.example/p.png", "data:image/gif;base64,AAAA", "data:image/png;base64,@@@"] {
        let err = generate_image_core(&db, &t, "u1", req("gpt-image", bad)).await.unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST, "{bad}");
    }
    assert!(t.recorded().is_empty());
}

// ─── LLM gateway images (/v1/images/*) ───────────────────────────────────────

#[tokio::test]
async fn gateway_images_without_key_is_none() {
    let db = DbHandle::new_memory().unwrap();
    let t = MockTransport::new();
    let req = super::handlers::GatewayImageRequest {
        prompt: "a fox".to_string(),
        reference_data_url: None,
        mask_data_url: None,
        size: "1024x1024".to_string(),
        quality: "standard".to_string(),
        n: 1,
    };
    let out = super::handlers::gateway_gpt_images(&db, &t, "u1", &req).await.unwrap();
    assert!(out.is_none());
    assert!(t.recorded().is_empty());
}

#[tokio::test]
async fn gateway_edit_maps_size_quality_and_mask() {
    let db = db_with_credential("openai");
    let t = MockTransport::new();
    t.script_json(
        "POST",
        "https://api.openai.com/v1/images/edits",
        200,
        json!({"data": [{"url": "https://oai.example/e.png"}]}),
    );
    t.script_bytes("GET", "https://oai.example/e.png", 200, b"edited", "image/png");
    let req = super::handlers::GatewayImageRequest {
        prompt: "make it a sprite".to_string(),
        reference_data_url: Some("data:image/png;base64,AAAA".to_string()),
        mask_data_url: Some("data:image/png;base64,BBBB".to_string()),
        size: "1792x1024".to_string(),
        quality: "hd".to_string(),
        n: 1,
    };
    let entries = super::handlers::gateway_gpt_images(&db, &t, "u1", &req).await.unwrap().unwrap();
    let body = t.recorded_for("POST", "https://api.openai.com/v1/images/edits")[0].body.clone().unwrap();
    assert_eq!(body["size"], "1536x1024");
    assert_eq!(body["quality"], "high");
    assert_eq!(body["images"][0]["image_url"], "data:image/png;base64,AAAA");
    assert_eq!(body["mask"]["image_url"], "data:image/png;base64,BBBB");

    // b64_json response format downloads URL entries.
    let data = crate::llm_gateway::images::entries_to_image_data(&t, entries, "b64_json").await.unwrap();
    use base64::Engine;
    assert_eq!(data[0].b64_json.as_deref(), Some(base64::engine::general_purpose::STANDARD.encode(b"edited").as_str()));
}

#[test]
fn gateway_image_data_url_sniffs_type() {
    use base64::Engine;
    let enc = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
    let png = enc(&[0x89, b'P', b'N', b'G', 0, 0]);
    assert_eq!(super::handlers::base64_image_data_url(&png).unwrap(), format!("data:image/png;base64,{png}"));
    assert!(super::handlers::base64_image_data_url(&enc(&[0xFF, 0xD8, 0xFF, 0])).unwrap().starts_with("data:image/jpeg"));
    assert!(super::handlers::base64_image_data_url(&enc(b"RIFF\0\0\0\0WEBPVP8 ")).unwrap().starts_with("data:image/webp"));
    assert!(super::handlers::base64_image_data_url(&enc(b"<svg/>")).is_none());
    assert!(super::handlers::base64_image_data_url("@@@").is_none());
}
