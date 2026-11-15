//! Local Brain API routes — memory query and vector search.
//!
//! Provides `/api/local-brain` for semantic search across memory documents,
//! events, and conversations. Falls back to keyword search if no embedding
//! service is available.

use axum::body::Body;
use axum::extract::Extension;
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get},
    Json, Router,
};
use bytes::Bytes;
use futures::StreamExt;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tracing::warn;

use crate::auth::get_user;
use crate::auth::AuthUser;
use crate::AppState;

// Proxy helpers to the Gizzi sidecar HTTP surface.
use crate::agent_session_routes::gizzi_client;

fn unauthorized() -> axum::response::Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({"error": "Unauthorized"})),
    )
        .into_response()
}

pub fn local_brain_router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/local-brain",
            get(local_brain_entry).post(pull_local_brain),
        )
        .route("/local-brain/status", get(brain_status))
        .route(
            "/local-brain/pull-custom",
            axum::routing::post(pull_custom_model),
        )
        // Sidecar-backed arbitrary local model management.
        .route("/local-brain/models", get(list_sidecar_models).post(install_sidecar_model))
        .route("/local-brain/models/search", get(search_sidecar_models))
        .route("/local-brain/models/:tag", delete(remove_sidecar_model))
}

// ─── Data models ──────────────────────────────────────────────────────────────

#[derive(Default, Deserialize)]
#[serde(default)]
struct BrainQuery {
    q: Option<String>,
    limit: Option<usize>,
    #[serde(rename = "type")]
    result_type: Option<String>, // documents | events | conversations | all
}

#[derive(Serialize)]
struct BrainResult {
    id: String,
    #[serde(rename = "type")]
    result_type: String,
    title: String,
    snippet: String,
    score: f64,
    source_url: Option<String>,
    created_at: String,
}

const LOCAL_BRAIN_MODEL: &str = "llama3.2:3b";

// ─── Combined local-brain entrypoint ─────────────────────────────────────────

async fn local_brain_entry(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
    Query(q): Query<BrainQuery>,
) -> impl IntoResponse {
    let ollama_url = state.config.ollama_url();
    match q.q.clone().as_deref().map(str::trim) {
        Some(query) if !query.is_empty() => {
            query_brain_impl(state, user, q, query.to_string()).await
        }
        _ => local_brain_probe(headers, ollama_url).await,
    }
}

// ─── Probe Ollama / Local Brain status ───────────────────────────────────────

async fn local_brain_probe(headers: HeaderMap, ollama_url: String) -> Response {
    let _user = match get_user(&headers) {
        Some(u) => u,
        None => return unauthorized(),
    };

    let client = crate::net_client::client();

    let tags_response = match client
        .get(format!("{}/api/tags", ollama_url.trim_end_matches('/')))
        .timeout(std::time::Duration::from_secs(3))
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => response,
        _ => {
            return Json(json!({
                "ollamaRunning": false,
                "modelReady": false,
                "modelId": LOCAL_BRAIN_MODEL,
                "pulledModels": [],
            }))
            .into_response();
        }
    };

    let payload = match tags_response.json::<serde_json::Value>().await {
        Ok(value) => value,
        Err(_) => {
            return Json(json!({
                "ollamaRunning": false,
                "modelReady": false,
                "modelId": LOCAL_BRAIN_MODEL,
                "pulledModels": [],
            }))
            .into_response();
        }
    };

    let pulled_models = payload
        .get("models")
        .and_then(|models| models.as_array())
        .map(|models| {
            models
                .iter()
                .filter_map(|model| model.get("name").and_then(|name| name.as_str()))
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let model_ready = pulled_models
        .iter()
        .any(|name| name == LOCAL_BRAIN_MODEL || name == &format!("{LOCAL_BRAIN_MODEL}:latest"));

    Json(json!({
        "ollamaRunning": true,
        "modelReady": model_ready,
        "modelId": LOCAL_BRAIN_MODEL,
        "pulledModels": pulled_models,
    }))
    .into_response()
}

// ─── Query brain ──────────────────────────────────────────────────────────────

async fn query_brain_impl(
    state: Arc<AppState>,
    user: AuthUser,
    q: BrainQuery,
    query: String,
) -> Response {
    let db = state.db.clone();
    let user_id = user.user_id;
    let limit = q.limit.unwrap_or(10);
    let result_type = q.result_type.unwrap_or_else(|| "all".to_string());
    let pattern = format!("%{}%", query);

    // Documents are searched through the shared memory-plane index
    // (memory_index): hybrid keyword + vector over their chunks.
    let query_emb = crate::memory_index::global()
        .embed_or_hash(&[query.clone()], crate::memory_index::InputType::Query)
        .await;
    let doc_query = query.clone();
    let results = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut all_results: Vec<BrainResult> = vec![];

        // Search memory_documents
        if result_type == "all" || result_type == "documents" {
            crate::memory_index::chunk_pending_documents(&conn, 64)?;
            let scope = crate::memory_index::Scope { scope: &user_id, target_types: &["chunk"], only_ids: None };
            let hits = crate::memory_index::hybrid_search(
                &conn,
                &scope,
                &doc_query,
                query_emb.vectors.first().map(|v| (query_emb.model.as_str(), v.as_slice())),
                (limit as usize).max(1) * 4,
            )?;
            let mut seen = std::collections::HashSet::new();
            for hit in hits {
                let doc: Option<(String, String, String, Option<String>, String)> = conn
                    .query_row(
                        "SELECT d.id, d.title, c.text, d.source_url, d.created_at
                         FROM memory_index_chunks c JOIN memory_documents d ON d.id = c.source_id
                         WHERE c.id = ?1 AND c.source_type = 'document' AND d.user_id = ?2",
                        params![&hit.target_id, &user_id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
                    )
                    .optional()?;
                let Some((id, title, text, source_url, created_at)) = doc else { continue };
                if !seen.insert(id.clone()) {
                    continue;
                }
                all_results.push(BrainResult {
                    id,
                    result_type: "document".to_string(),
                    title,
                    snippet: text.chars().take(200).collect(),
                    // RRF max is 2/(k+1) for a top hit in both lists; scale to 0..1
                    // so documents sort alongside the fixed-score sections.
                    score: (hit.score * (61.0 / 2.0)).min(1.0),
                    source_url,
                    created_at,
                });
                if seen.len() >= limit as usize {
                    break;
                }
            }
        }

        // Search memory_events
        if result_type == "all" || result_type == "events" {
            let mut stmt = conn.prepare(
                "SELECT id, type, payload, source, timestamp
                 FROM memory_events
                 WHERE user_id = ?1 AND (type LIKE ?2 OR payload LIKE ?2 OR source LIKE ?2)
                 ORDER BY timestamp DESC LIMIT ?3",
            )?;
            let events = stmt.query_map(params![&user_id, &pattern, limit as i64], |row| {
                Ok(BrainResult {
                    id: row.get(0)?,
                    result_type: "event".to_string(),
                    title: row.get(1)?,
                    snippet: row
                        .get::<_, Option<String>>(2)?
                        .unwrap_or_default()
                        .chars()
                        .take(200)
                        .collect(),
                    score: 0.75,
                    source_url: None,
                    created_at: row.get(4)?,
                })
            })?;
            for evt in events {
                if let Ok(e) = evt {
                    all_results.push(e);
                }
            }
        }

        // Search conversations
        if result_type == "all" || result_type == "conversations" {
            let mut stmt = conn.prepare(
                "SELECT c.id, c.title, m.content, c.created_at
                 FROM conversations c
                 LEFT JOIN conversation_messages m ON m.conversation_id = c.id
                 WHERE c.user_id = ?1 AND (c.title LIKE ?2 OR m.content LIKE ?2)
                 GROUP BY c.id
                 ORDER BY c.created_at DESC LIMIT ?3",
            )?;
            let convs = stmt.query_map(params![&user_id, &pattern, limit as i64], |row| {
                Ok(BrainResult {
                    id: row.get(0)?,
                    result_type: "conversation".to_string(),
                    title: row
                        .get::<_, Option<String>>(1)?
                        .unwrap_or_else(|| "Untitled".to_string()),
                    snippet: row
                        .get::<_, Option<String>>(2)?
                        .unwrap_or_default()
                        .chars()
                        .take(200)
                        .collect(),
                    score: 0.8,
                    source_url: None,
                    created_at: row.get(3)?,
                })
            })?;
            for conv in convs {
                if let Ok(c) = conv {
                    all_results.push(c);
                }
            }
        }

        // Sort by score descending and limit
        all_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        all_results.truncate(limit);

        Ok::<_, rusqlite::Error>(all_results)
    })
    .await;

    match results {
        Ok(Ok(data)) => Json(json!({
            "query": query,
            "results": data,
            "count": data.len(),
            "note": "Documents: hybrid keyword + vector search (memory index); events and conversations: keyword search"
        }))
        .into_response(),
        Ok(Err(e)) => {
            warn!("DB error querying brain: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

// ─── Pull Local Brain model over SSE ─────────────────────────────────────────

async fn pull_local_brain(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if get_user(&headers).is_none() {
        return unauthorized();
    }
    stream_ollama_pull(state.config.ollama_url(), LOCAL_BRAIN_MODEL.to_string()).await
}

// ─── Pull an arbitrary Hugging Face GGUF model over SSE ──────────────────────
//
// Companion to `search_huggingface` in provider_routes.rs: installs any
// result from that search the same way `pull_local_brain` installs the fixed
// default, via Ollama's own `hf.co/<repo>` pull support (same convention
// gizzi-code's sidecar uses in cmd/gizzi-code/src/runtime/sidecar/index.ts).

#[derive(Deserialize)]
struct PullCustomModelRequest {
    /// e.g. "bartowski/Llama-3.2-3B-Instruct-GGUF"
    repo: String,
    /// Optional quantization tag, e.g. "Q4_K_M". Ollama picks a default when omitted.
    quant: Option<String>,
}

async fn pull_custom_model(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<PullCustomModelRequest>,
) -> Response {
    if get_user(&headers).is_none() {
        return unauthorized();
    }

    let repo = body.repo.trim();
    if repo.is_empty() || repo.contains(char::is_whitespace) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_repo", "message": "repo must be a non-empty Hugging Face repo id, e.g. \"org/name-GGUF\"" })),
        )
            .into_response();
    }

    let tag = match body.quant.as_deref().map(str::trim).filter(|q| !q.is_empty()) {
        Some(quant) => format!("hf.co/{repo}:{quant}"),
        None => format!("hf.co/{repo}"),
    };

    stream_ollama_pull(state.config.ollama_url(), tag).await
}

/// Streams an Ollama `/api/pull` for `model_tag` back to the client as SSE,
/// re-emitting each JSON progress line Ollama sends. Shared by the fixed
/// Local Brain download and arbitrary Hugging Face installs.
async fn stream_ollama_pull(ollama_url: String, model_tag: String) -> Response {
    let client = crate::net_client::client();
    let tags_url = format!("{}/api/tags", ollama_url.trim_end_matches('/'));
    let pull_url = format!("{}/api/pull", ollama_url.trim_end_matches('/'));

    match client
        .get(&tags_url)
        .timeout(std::time::Duration::from_secs(3))
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => {}
        _ => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "Ollama is not running. Install it at ollama.com then try again."
                })),
            )
                .into_response();
        }
    }

    let stream = async_stream::stream! {
        let send = |payload: serde_json::Value| {
            Ok::<Bytes, std::convert::Infallible>(Bytes::from(format!("data: {}\n\n", payload)))
        };

        let pull_response = match client
            .post(&pull_url)
            .json(&json!({
                "name": model_tag,
                "stream": true,
            }))
            .send()
            .await
        {
            Ok(response) => response,
            Err(err) => {
                yield send(json!({
                    "status": "error",
                    "error": err.to_string(),
                }));
                return;
            }
        };

        if !pull_response.status().is_success() {
            let error_body = pull_response.text().await.unwrap_or_else(|_| "Pull request failed".to_string());
            yield send(json!({
                "status": "error",
                "error": error_body,
            }));
            return;
        }

        let mut line_buffer = String::new();
        let mut byte_stream = pull_response.bytes_stream();
        let mut pull_error: Option<String> = None;

        while let Some(next_chunk) = byte_stream.next().await {
            let chunk = match next_chunk {
                Ok(bytes) => bytes,
                Err(err) => {
                    yield send(json!({
                        "status": "error",
                        "error": err.to_string(),
                    }));
                    return;
                }
            };

            line_buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(newline_index) = line_buffer.find('\n') {
                let line = line_buffer[..newline_index].trim().to_string();
                line_buffer.drain(..=newline_index);

                if line.is_empty() {
                    continue;
                }

                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) {
                    pull_error = pull_error.or_else(|| ollama_error(&value));
                    yield send(value);
                }
            }
        }

        let trailing = line_buffer.trim();
        if !trailing.is_empty() {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(trailing) {
                pull_error = pull_error.or_else(|| ollama_error(&value));
                yield send(value);
            }
        }

        // Ollama reports a failed pull as an `{"error": …}` line on a 200 stream.
        if let Some(error) = pull_error {
            yield send(json!({ "status": "error", "error": error }));
            return;
        }
        yield send(json!({ "status": "success" }));
        // Final event: how the pulled GGUF was checked. Always sent after a
        // successful pull, never skipped.
        yield send(pull_verification_event(&ollama_url, &model_tag).await);
    };

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/event-stream")
        .header("Cache-Control", "no-cache")
        .header("Connection", "keep-alive")
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

// ─── Brain status ─────────────────────────────────────────────────────────────

async fn brain_status(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let _user = match get_user(&headers) {
        Some(u) => u,
        None => return unauthorized(),
    };

    let ollama_url = state.config.ollama_url();
    let embedding_url = state.config.embedding_url();

    let client = crate::net_client::client();

    let ollama_available = client
        .get(format!("{}/api/tags", ollama_url.trim_end_matches('/')))
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .is_ok();

    let embedding_available = client
        .get(format!("{}/health", embedding_url.trim_end_matches('/')))
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .is_ok();

    // The memory index's own local embedding sidecar (ALLTERNIT_EMBED_URL).
    let memory_index_url = crate::memory_index::global().url().map(str::to_string);
    let memory_index_available = match &memory_index_url {
        Some(u) => client
            .get(format!("{u}/health"))
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false),
        None => false,
    };

    let semantic_search = ollama_available || embedding_available || memory_index_available;
    let vector_store = embedding_available || memory_index_available;

    Json(json!({
        "status": if ollama_available || embedding_available { "healthy" } else { "degraded" },
        "services": {
            "memory_index_embeddings": {
                "url": memory_index_url,
                "model": crate::memory_index::global().preferred_model(),
                "available": memory_index_available,
            },
            "ollama": {
                "url": ollama_url,
                "available": ollama_available,
            },
            "embedding": {
                "url": embedding_url,
                "available": embedding_available,
            },
        },
        "capabilities": {
            "keyword_search": true,
            "semantic_search": semantic_search,
            "vector_store": vector_store,
        },
        "note": if semantic_search {
            "Semantic search available"
        } else {
            "Semantic search requires an embedding service (OLLAMA_URL or ALLTERNIT_EMBEDDING_URL)"
        }
    }))
    .into_response()
}

// ─── Sidecar model management proxy ───────────────────────────────────────────

#[derive(Deserialize)]
struct InstallSidecarModelRequest {
    repo_id: String,
    quant_tag: Option<String>,
}

async fn list_sidecar_models(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if get_user(&headers).is_none() {
        return unauthorized();
    }
    let client = gizzi_client(&headers);
    let gizzi_url = state.config.terminal_server_url().trim_end_matches('/').to_string();
    match client
        .get(format!("{}/v1/sidecar/models", gizzi_url))
        .send()
        .await
    {
        Ok(res) => {
            let status = StatusCode::from_u16(res.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let body = res.bytes().await.unwrap_or_default();
            (status, Body::from(body)).into_response()
        }
        Err(err) => {
            warn!("sidecar list models request failed: {}", err);
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": "Gizzi unreachable", "message": err.to_string() })),
            )
                .into_response()
        }
    }
}

#[derive(Deserialize)]
struct SearchSidecarModelsQuery {
    q: Option<String>,
    limit: Option<u32>,
}

async fn search_sidecar_models(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<SearchSidecarModelsQuery>,
) -> Response {
    if get_user(&headers).is_none() {
        return unauthorized();
    }
    let client = gizzi_client(&headers);
    let gizzi_url = state.config.terminal_server_url().trim_end_matches('/').to_string();
    let mut req = client.get(format!("{}/v1/sidecar/models/search", gizzi_url));
    if let Some(q) = query.q {
        req = req.query(&[("q", q)]);
    }
    if let Some(limit) = query.limit {
        req = req.query(&[("limit", limit.to_string())]);
    }
    match req.send().await {
        Ok(res) => {
            let status = StatusCode::from_u16(res.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let body = res.bytes().await.unwrap_or_default();
            (status, Body::from(body)).into_response()
        }
        Err(err) => {
            warn!("sidecar search models request failed: {}", err);
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": "Gizzi unreachable", "message": err.to_string() })),
            )
                .into_response()
        }
    }
}

async fn install_sidecar_model(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<InstallSidecarModelRequest>,
) -> Response {
    if get_user(&headers).is_none() {
        return unauthorized();
    }
    let client = gizzi_client(&headers);
    let gizzi_url = state.config.terminal_server_url().trim_end_matches('/').to_string();
    let upstream = match client
        .post(format!("{}/v1/sidecar/models/install", gizzi_url))
        .json(&json!({
            "repoId": body.repo_id,
            "quantTag": body.quant_tag,
        }))
        .send()
        .await
    {
        Ok(res) => res,
        Err(err) => {
            warn!("sidecar install model request failed: {}", err);
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": "Gizzi unreachable", "message": err.to_string() })),
            )
                .into_response();
        }
    };

    let status = StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let fallback_tag = match body.quant_tag.as_deref().map(str::trim).filter(|q| !q.is_empty()) {
        Some(q) => format!("hf.co/{}:{q}", body.repo_id),
        None => format!("hf.co/{}", body.repo_id),
    };
    let mut upstream_stream = upstream.bytes_stream();
    // Relay Gizzi's SSE unchanged, then append the same final `verification`
    // event the Ollama pull stream sends, checked against the sidecar's Ollama.
    let stream = async_stream::stream! {
        let mut scan = SseScan::default();
        while let Some(chunk) = upstream_stream.next().await {
            match chunk {
                Ok(bytes) => {
                    scan.feed(&bytes);
                    yield Ok::<Bytes, std::io::Error>(bytes);
                }
                Err(err) => {
                    yield Err(std::io::Error::other(err));
                    return;
                }
            }
        }
        if let (Some(tag), false) = (scan.success_tag(&fallback_tag), scan.saw_verification) {
            let event = pull_verification_event(&sidecar_ollama_url(), &tag).await;
            yield Ok(Bytes::from(format!("data: {}\n\n", event)));
        }
    };
    Response::builder()
        .status(status)
        .header("Content-Type", "text/event-stream")
        .header("Cache-Control", "no-cache")
        .header("Connection", "keep-alive")
        .header("X-Accel-Buffering", "no")
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// The Gizzi sidecar's own Ollama (cmd/gizzi-code/src/runtime/sidecar:
/// `ALLTERNIT_SIDECAR_URL`, else 127.0.0.1:11435).
fn sidecar_ollama_url() -> String {
    std::env::var("ALLTERNIT_SIDECAR_URL")
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "http://127.0.0.1:11435".to_string())
}

/// Watches relayed SSE `data:` lines for the install's success and for a
/// verification event already sent upstream.
#[derive(Default)]
struct SseScan {
    buffer: String,
    success: Option<Option<String>>,
    saw_verification: bool,
}

impl SseScan {
    fn feed(&mut self, bytes: &[u8]) {
        self.buffer.push_str(&String::from_utf8_lossy(bytes));
        while let Some(i) = self.buffer.find('\n') {
            let line = self.buffer[..i].trim().to_string();
            self.buffer.drain(..=i);
            let Some(data) = line.strip_prefix("data:") else { continue };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(data.trim()) else { continue };
            match v.get("status").and_then(|s| s.as_str()) {
                Some("success") => {
                    self.success = Some(v.get("tag").and_then(|t| t.as_str()).map(str::to_string))
                }
                Some("verification") => self.saw_verification = true,
                _ => {}
            }
        }
    }

    /// The installed tag when the install succeeded.
    fn success_tag(&self, fallback: &str) -> Option<String> {
        self.success
            .as_ref()
            .map(|t| t.clone().filter(|t| !t.is_empty()).unwrap_or_else(|| fallback.to_string()))
    }
}

// ─── Pull verification (Freeweights manifest vs the Ollama layer digest) ─────
//
// Ollama stores each GGUF as a blob named by its sha256, so the model layer
// digest of an `hf.co/<repo>[:quant]` pull IS the file's sha256. It is
// compared with the signed Freeweights manifest (contracts/model-source).

fn ollama_error(value: &serde_json::Value) -> Option<String> {
    value.get("error").and_then(|e| e.as_str()).map(str::to_string)
}

/// `hf.co/org/repo:Q4_K_M` → `org/repo`. `None` for Ollama library tags.
fn hf_repo_of_tag(tag: &str) -> Option<String> {
    let rest = tag
        .strip_prefix("hf.co/")
        .or_else(|| tag.strip_prefix("huggingface.co/"))?;
    let repo = rest.split(':').next().unwrap_or(rest);
    (repo.split('/').count() == 2 && !repo.contains("..")).then(|| repo.to_string())
}

/// sha256 of the model layer from `/api/show`'s modelfile `FROM …/sha256-<hex>`.
fn model_digest_from_modelfile(modelfile: &str) -> Option<String> {
    modelfile.lines().find_map(|line| {
        let path = line.trim().strip_prefix("FROM ")?.trim();
        let hex = path.rsplit("sha256-").next()?;
        (hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| hex.to_ascii_lowercase())
    })
}

/// The manifest GGUF file whose sha256 equals the layer digest.
fn manifest_file_for_digest<'a>(
    manifest: &'a allternit_model_source::Manifest,
    digest: &str,
) -> Option<&'a allternit_model_source::ManifestFile> {
    manifest
        .files
        .iter()
        .find(|f| f.sha256.eq_ignore_ascii_case(digest))
}

async fn ollama_model_digest(ollama_url: &str, tag: &str) -> Result<String, String> {
    let res = crate::net_client::client()
        .post(format!("{}/api/show", ollama_url.trim_end_matches('/')))
        .json(&json!({ "model": tag }))
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        return Err(format!("Ollama /api/show: HTTP {}", res.status()));
    }
    let v: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    v.get("modelfile")
        .and_then(|m| m.as_str())
        .and_then(model_digest_from_modelfile)
        .ok_or_else(|| "no model layer digest in Ollama's modelfile".to_string())
}

fn now_utc() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Build the event from the pieces (no I/O; unit-tested).
fn verification_event(
    tag: &str,
    repo: Option<&str>,
    digest: Result<&str, &str>,
    manifest: Option<Result<&allternit_model_source::Manifest, &str>>,
    fw_base: &str,
) -> serde_json::Value {
    use allternit_model_source::{SourceKind, VerificationState};
    let unverified = |reason: String, source_url: String| {
        json!({
            "status": "verification",
            "tag": tag,
            "repo": repo,
            "state": VerificationState::Unverified,
            "reason": reason,
            "sha256": digest.ok(),
            "verification": {
                "state": VerificationState::Unverified,
                "source_used": SourceKind::Publisher,
                "source_url": source_url,
                "checked_at": now_utc(),
            },
        })
    };
    let Some(repo) = repo else {
        return unverified(
            "pulled from the Ollama library; Freeweights covers Hugging Face repos only".into(),
            "https://registry.ollama.ai/".into(),
        );
    };
    let hf_url = format!("https://huggingface.co/{repo}/resolve/main/");
    let digest = match digest {
        Ok(d) => d,
        Err(e) => return unverified(format!("could not read the pulled layer digest: {e}"), hf_url),
    };
    match manifest {
        None => unverified(format!("{repo} is not listed in the Freeweights catalog"), hf_url),
        Some(Err(reason)) => unverified(reason.to_string(), hf_url),
        Some(Ok(m)) => match manifest_file_for_digest(m, digest) {
            Some(file) => json!({
                "status": "verification",
                "tag": tag,
                "repo": repo,
                "state": VerificationState::FreeweightsSigned,
                "file": file.path,
                "sha256": digest,
                "verification": {
                    "state": VerificationState::FreeweightsSigned,
                    "source_used": SourceKind::Publisher,
                    "source_url": hf_url,
                    "manifest_commit": m.upstream.commit,
                    "checked_at": now_utc(),
                },
                "detail_url": format!("{fw_base}/{}/{repo}/", m.section),
            }),
            None => {
                let mut ev = unverified(
                    format!(
                        "the pulled file (sha256 {digest}) is not in the signed Freeweights manifest at commit {}; the repo may have changed since it was pinned",
                        m.upstream.commit
                    ),
                    hf_url,
                );
                ev["mismatch"] = json!(true);
                ev
            }
        },
    }
}

/// Final `verification` event for an Ollama pull of `tag`.
async fn pull_verification_event(ollama_url: &str, tag: &str) -> serde_json::Value {
    let repo = hf_repo_of_tag(tag);
    let fw_base = crate::provider_routes::freeweights::base_url();
    let Some(repo_ref) = repo.as_deref() else {
        return verification_event(tag, None, Err("library model"), None, &fw_base);
    };
    let digest = ollama_model_digest(ollama_url, tag).await;
    let fetched = match allternit_model_source::Client::new() {
        Ok(c) => c.fetch_manifest(repo_ref).await,
        Err(e) => Err(e),
    };
    // Ok(Ok) = signed manifest; Ok(Err) = unreachable / not listed; Err = bad manifest.
    let (manifest, reason) = match fetched {
        Ok(Ok(m)) => (Some(m), None),
        Ok(Err(reason)) => (None, Some(reason)),
        Err(e) => (None, Some(format!("the Freeweights manifest failed verification: {e}"))),
    };
    let manifest_arg = match (&manifest, &reason) {
        (Some(m), _) => Some(Ok(m)),
        (None, Some(r)) if r.contains("not in the Freeweights catalog") => None,
        (None, Some(r)) => Some(Err(r.as_str())),
        (None, None) => None,
    };
    verification_event(
        tag,
        Some(repo_ref),
        digest.as_deref().map_err(String::as_str),
        manifest_arg,
        &fw_base,
    )
}

async fn remove_sidecar_model(
    State(state): State<Arc<AppState>>,
    Path(tag): Path<String>,
    headers: HeaderMap,
) -> Response {
    if get_user(&headers).is_none() {
        return unauthorized();
    }
    let client = gizzi_client(&headers);
    let gizzi_url = state.config.terminal_server_url().trim_end_matches('/').to_string();
    let encoded = urlencoding::encode(&tag);
    match client
        .delete(format!("{}/v1/sidecar/models/{}", gizzi_url, encoded))
        .send()
        .await
    {
        Ok(res) => {
            let status = StatusCode::from_u16(res.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let body = res.bytes().await.unwrap_or_default();
            (status, Body::from(body)).into_response()
        }
        Err(err) => {
            warn!("sidecar remove model request failed: {}", err);
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": "Gizzi unreachable", "message": err.to_string() })),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod pull_verification_tests {
    use super::*;

    const DIGEST: &str = "59f9ae8b4372107ae79044278a4406bdfe8f8861fe599aa5848515f0d072be38";

    fn manifest() -> allternit_model_source::Manifest {
        serde_json::from_value(json!({
            "schema_version": 1,
            "id": "org/model-GGUF",
            "section": "alt.freeweights.qwen",
            "upstream": {"host": "huggingface.co", "repo": "org/model-GGUF", "commit": "d4af3be5701549435e0770d625f79dd2def1358f", "url": "https://huggingface.co/org/model-GGUF/tree/d4af3be5701549435e0770d625f79dd2def1358f"},
            "license": {"id": "MIT", "name": "MIT", "file_path": null, "file_sha256": null, "redistribution": "yes"},
            "files": [
                {"path": "model-Q4_K_M.gguf", "size": 10, "sha256": DIGEST.to_ascii_uppercase(), "lfs": true},
                {"path": "model-Q8_0.gguf", "size": 10, "sha256": "0".repeat(64), "lfs": true}
            ],
            "sources": [],
            "created_at": "2026-11-14T00:00:00Z",
            "generator": "test"
        }))
        .unwrap()
    }

    #[test]
    fn layer_digest_comes_from_the_modelfile() {
        let mf = "# Modelfile\nFROM /Users/x/.ollama/models/blobs/sha256-59F9AE8B4372107AE79044278A4406BDFE8F8861FE599AA5848515F0D072BE38\nTEMPLATE x";
        assert_eq!(model_digest_from_modelfile(mf).as_deref(), Some(DIGEST));
        assert_eq!(model_digest_from_modelfile("FROM llama3.2"), None);
        assert_eq!(hf_repo_of_tag("hf.co/org/model-GGUF:Q4_K_M").as_deref(), Some("org/model-GGUF"));
        assert_eq!(hf_repo_of_tag("llama3.2:3b"), None);
    }

    #[test]
    fn digest_matching_a_manifest_file_is_freeweights_signed() {
        let m = manifest();
        let ev = verification_event("hf.co/org/model-GGUF:Q4_K_M", Some("org/model-GGUF"), Ok(DIGEST), Some(Ok(&m)), "https://fw");
        assert_eq!(ev["status"], "verification");
        assert_eq!(ev["state"], "freeweights_signed");
        assert_eq!(ev["file"], "model-Q4_K_M.gguf");
        assert_eq!(ev["verification"]["manifest_commit"], "d4af3be5701549435e0770d625f79dd2def1358f");
        assert_eq!(ev["detail_url"], "https://fw/alt.freeweights.qwen/org/model-GGUF/");
    }

    #[test]
    fn other_outcomes_are_unverified_with_a_reason() {
        let m = manifest();
        let other = "1".repeat(64);
        let changed = verification_event("t", Some("org/model-GGUF"), Ok(&other), Some(Ok(&m)), "https://fw");
        assert_eq!(changed["state"], "unverified");
        assert_eq!(changed["mismatch"], true);
        let unlisted = verification_event("t", Some("org/x"), Ok(DIGEST), None, "https://fw");
        assert_eq!(unlisted["state"], "unverified");
        assert!(unlisted["reason"].as_str().unwrap().contains("not listed"));
        let no_digest = verification_event("t", Some("org/model-GGUF"), Err("show failed"), Some(Ok(&m)), "https://fw");
        assert_eq!(no_digest["state"], "unverified");
        assert_eq!(no_digest["verification"]["state"], "unverified");
    }
}
