//! Deliverable pipeline (consumer-packaged Cowork P3.1): agent-written
//! markdown → office-engine render → persisted finished documents
//! (.docx report / .xlsx sheet / .pptx deck) attached to the canonical run
//! and served for preview/export. Registry = `cowork_run_events`
//! (`deliverable.created`) — no migration; bytes live under
//! `{data_dir}/deliverables/{run_id}/`.
//!
//! Auth: the run owner (user auth) or a fabric-transport principal whose
//! workspace matches the run's (the executing worker delivers its own run's
//! artifacts under its bearer token).

use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::AppState;

pub fn deliverable_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/cowork/runs/:run_id/deliverables", post(create_deliverable))
        .route("/cowork/runs/:run_id/deliverables", get(list_deliverables))
        .route(
            "/cowork/runs/:run_id/deliverables/:name",
            get(get_deliverable),
        )
}

#[derive(Debug, serde::Deserialize)]
struct CreateDeliverableRequest {
    name: String,
    #[serde(default = "default_template")]
    template: String,
    #[serde(default)]
    title: Option<String>,
    markdown: String,
}

fn default_template() -> String {
    "report".to_string()
}

#[derive(Debug)]
struct DeliverableError {
    status: StatusCode,
    body: Json<Value>,
}

impl DeliverableError {
    fn new(status: StatusCode, code: &str, message: impl Into<String>) -> Self {
        Self {
            status,
            body: Json(json!({"error": code, "message": message.into()})),
        }
    }
}

impl IntoResponse for DeliverableError {
    fn into_response(self) -> Response {
        (self.status, self.body).into_response()
    }
}

/// Authorize: run owner via user auth, or a fabric principal whose
/// workspace matches the run's (the executing worker).
fn authorize(
    state: &AppState,
    headers: &axum::http::HeaderMap,
    run_id: &str,
) -> Result<(String, String), DeliverableError> {
    let conn = state
        .db
        .connect()
        .map_err(|e| DeliverableError::new(StatusCode::INTERNAL_SERVER_ERROR, "store_error", e.to_string()))?;
    let run: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT workspace_id, user_id FROM cowork_runs WHERE id = ?1",
            rusqlite::params![run_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    let Some((workspace, owner)) = run else {
        return Err(DeliverableError::new(StatusCode::NOT_FOUND, "run_not_found", "run not found"));
    };
    if let Some(user) = crate::auth::get_user(headers) {
        if owner.as_deref() == Some(user.user_id.as_str()) {
            return Ok((user.user_id, workspace));
        }
    }
    // Worker bearer path: principal token whose workspace matches the run.
    if let Some(token) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    {
        let principal = allternit_cowork_runtime::sqlite_store::authenticate_principal(&conn, token)
            .map_err(|_| DeliverableError::new(StatusCode::FORBIDDEN, "forbidden", "invalid worker credential"))?;
        if principal.workspace == workspace {
            return Ok((principal.id.clone(), workspace));
        }
    }
    Err(DeliverableError::new(
        StatusCode::FORBIDDEN,
        "forbidden",
        "run owner or same-workspace worker principal required",
    ))
}

fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' })
        .collect()
}

fn deliverables_dir(state: &AppState, run_id: &str) -> std::path::PathBuf {
    let base = state
        .db
        .path()
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::temp_dir());
    base.join("deliverables").join(run_id)
}

fn office_engine_url() -> String {
    std::env::var("OFFICE_ENGINE_URL").unwrap_or_else(|_| "http://127.0.0.1:8099".to_string())
}

async fn create_deliverable(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path(run_id): Path<String>,
    Json(req): Json<CreateDeliverableRequest>,
) -> Result<Json<Value>, DeliverableError> {
    let (actor, workspace) = authorize(&state, &headers, &run_id)?;
    if !["report", "sheet", "deck"].contains(&req.template.as_str()) {
        return Err(DeliverableError::new(
            StatusCode::BAD_REQUEST,
            "unknown_template",
            "template must be report | sheet | deck",
        ));
    }
    if req.markdown.trim().is_empty() {
        return Err(DeliverableError::new(StatusCode::BAD_REQUEST, "empty_markdown", "markdown is required"));
    }
    let title = req.title.clone().unwrap_or_else(|| req.name.clone());

    // Render through the office engine (the artifact pipeline round-trip).
    let render = crate::net_client::client()
        .post(format!("{}/deliverable/render", office_engine_url()))
        .json(&json!({
            "template": req.template,
            "title": title,
            "markdown": req.markdown,
        }))
        .send()
        .await
        .map_err(|e| DeliverableError::new(StatusCode::BAD_GATEWAY, "office_engine_unavailable", e.to_string()))?;
    if !render.status().is_success() {
        let body = render.text().await.unwrap_or_default();
        return Err(DeliverableError::new(
            StatusCode::BAD_GATEWAY,
            "render_failed",
            format!("office engine: {body:.300}"),
        ));
    }
    let rendered: Value = render
        .json()
        .await
        .map_err(|e| DeliverableError::new(StatusCode::BAD_GATEWAY, "render_failed", e.to_string()))?;
    let ext = rendered
        .get("ext")
        .and_then(|v| v.as_str())
        .ok_or_else(|| DeliverableError::new(StatusCode::BAD_GATEWAY, "render_failed", "no ext"))?;
    let mime = rendered
        .get("mimeType")
        .and_then(|v| v.as_str())
        .unwrap_or("application/octet-stream")
        .to_string();
    let bytes: Vec<u8> = {
        let b64 = rendered
            .get("bytesBase64")
            .and_then(|v| v.as_str())
            .ok_or_else(|| DeliverableError::new(StatusCode::BAD_GATEWAY, "render_failed", "no bytes"))?;
        base64_decode(b64).ok_or_else(|| DeliverableError::new(StatusCode::BAD_GATEWAY, "render_failed", "bad bytes"))?
    };

    let dir = deliverables_dir(&state, &run_id);
    std::fs::create_dir_all(&dir).map_err(|e| DeliverableError::new(StatusCode::INTERNAL_SERVER_ERROR, "io", e.to_string()))?;
    let file_name = format!("{}.{}", sanitize_name(&req.name), ext);
    let path = dir.join(&file_name);
    std::fs::write(&path, &bytes).map_err(|e| DeliverableError::new(StatusCode::INTERNAL_SERVER_ERROR, "io", e.to_string()))?;

    // Registry: attributed run event (no migration — events are canonical).
    let mut conn = state
        .db
        .connect()
        .map_err(|e| DeliverableError::new(StatusCode::INTERNAL_SERVER_ERROR, "store_error", e.to_string()))?;
    let payload = json!({
        "name": req.name,
        "template": req.template,
        "title": title,
        "file": file_name,
        "mime": mime,
        "size_bytes": bytes.len(),
        "actor": actor,
    });
    let event_key = format!("deliverable.created:{}", req.name);
    allternit_cowork_runtime::sqlite_store::insert_event_idempotent(
        &mut conn,
        &run_id,
        "deliverable.created",
        payload.clone(),
        None,
        None,
        Some(&actor),
        Some(&event_key),
    )
    .map_err(|e| DeliverableError::new(StatusCode::INTERNAL_SERVER_ERROR, "store_error", e.message))?;

    Ok(Json(json!({
        "run_id": run_id,
        "workspace": workspace,
        "name": req.name,
        "template": req.template,
        "file": file_name,
        "mime": mime,
        "size_bytes": bytes.len(),
        "url": format!("/api/v1/cowork/runs/{run_id}/deliverables/{file_name}"),
    })))
}

fn base64_decode(input: &str) -> Option<Vec<u8>> {
    // Minimal base64 (standard alphabet, with padding).
    fn val(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a' + 26) as u32),
            b'0'..=b'9' => Some((c - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut nbits = 0u32;
    for &c in input.as_bytes() {
        if c == b'=' {
            continue;
        }
        let v = val(c)?;
        acc = (acc << 6) | v;
        nbits += 6;
        if nbits >= 8 {
            nbits -= 8;
            out.push((acc >> nbits) as u8);
        }
    }
    Some(out)
}

async fn list_deliverables(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path(run_id): Path<String>,
) -> Result<Json<Value>, DeliverableError> {
    authorize(&state, &headers, &run_id)?;
    let conn = state
        .db
        .connect()
        .map_err(|e| DeliverableError::new(StatusCode::INTERNAL_SERVER_ERROR, "store_error", e.to_string()))?;
    let mut stmt = conn
        .prepare(
            "SELECT payload, created_at FROM cowork_run_events
             WHERE run_id = ?1 AND event_type = 'deliverable.created' ORDER BY created_at ASC",
        )
        .map_err(|e| DeliverableError::new(StatusCode::INTERNAL_SERVER_ERROR, "store_error", e.to_string()))?;
    let items: Vec<Value> = stmt
        .query_map(rusqlite::params![run_id], |r| {
            let raw: String = r.get(0)?;
            let created: String = r.get(1)?;
            Ok(json!({ "created_at": created, "payload": serde_json::from_str::<Value>(&raw).unwrap_or(Value::Null) }))
        })
        .map_err(|e| DeliverableError::new(StatusCode::INTERNAL_SERVER_ERROR, "store_error", e.to_string()))?
        .collect::<Result<_, _>>()
        .map_err(|e| DeliverableError::new(StatusCode::INTERNAL_SERVER_ERROR, "store_error", e.to_string()))?;
    Ok(Json(json!({ "run_id": run_id, "deliverables": items })))
}

async fn get_deliverable(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path((run_id, name)): Path<(String, String)>,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Response, DeliverableError> {
    authorize(&state, &headers, &run_id)?;
    let path = deliverables_dir(&state, &run_id).join(sanitize_name(&name));
    if !path.exists() {
        return Err(DeliverableError::new(StatusCode::NOT_FOUND, "not_found", "deliverable not found"));
    }
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| DeliverableError::new(StatusCode::INTERNAL_SERVER_ERROR, "io", e.to_string()))?;
    let mime = match path.extension().and_then(|e| e.to_str()) {
        Some("docx") => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        Some("xlsx") => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        Some("pptx") => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => "application/octet-stream",
    };
    let mut builder = axum::http::Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime);
    if query.get("download").is_some() {
        builder = builder.header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", sanitize_name(&name)),
        );
    }
    builder
        .body(axum::body::Body::from(bytes))
        .map_err(|e| DeliverableError::new(StatusCode::INTERNAL_SERVER_ERROR, "io", e.to_string()))
}
