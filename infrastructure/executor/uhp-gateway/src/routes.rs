//! Axum router: auth + UHP-Version middleware and every full-class handler.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Json, Path, Query, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event as SseEvent, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_stream::StreamExt as _;

use crate::drivers::DriverKind;
use crate::protocol::{
    self, CreateResponseRequest, Discovery, ErrorEnvelope, Harness, HarnessList, HarnessModels,
    HarnessUpsert, SessionShare, SessionList, SkillFiles, Response as UhpResponse,
    ResponseMetadata, ResponseStatus, StreamEvent, TurnList, PROTOCOL_VERSION,
};
use crate::turn::{TurnContext, TurnControl, DEFAULT_TURN_TIMEOUT};
use crate::{model_catalog, resolve_model, AppState, DeleteAck};

pub fn router(state: Arc<AppState>) -> Router {
    let api = Router::new()
        .route("/v1/harnesses", get(list_harnesses).post(create_harness))
        .route(
            "/v1/harnesses/:id",
            get(get_harness).put(update_harness).delete(delete_harness),
        )
        .route("/v1/harnesses/:id/models", get(harness_models))
        .route(
            "/v1/harnesses/:id/skills/:name/files",
            get(get_skill_files),
        )
        .route("/v1/models", get(models_catalog))
        .route("/v1/responses", post(create_response))
        .route("/v1/responses/:id", get(get_response))
        .route("/v1/responses/:id/cancel", post(cancel_response))
        .route("/v1/sessions", get(list_sessions))
        .route("/v1/sessions/:id", get(get_session).delete(delete_session))
        .route("/v1/sessions/:id/turns", get(list_turns))
        .route("/v1/sessions/:id/files", get(list_session_files))
        .route(
            "/v1/sessions/:id/share",
            post(create_share).get(get_share).delete(revoke_share),
        )
        .route("/v1/traces/:id", axum::routing::delete(delete_session))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth_middleware));
    Router::new()
        .route("/v1/uhp", get(discovery))
        .route("/v1/shares/:id", get(shared_view))
        .merge(api)
        .layer(middleware::from_fn(uhp_version_middleware))
        .with_state(state)
}

fn error_response(status: StatusCode, envelope: ErrorEnvelope) -> Response {
    (status, Json(envelope)).into_response()
}

fn invalid_request(message: impl Into<String>, param: Option<&str>) -> Response {
    let mut envelope = ErrorEnvelope::new("invalid_request_error", "invalid_request", message, None);
    envelope.error.param = param.map(str::to_string);
    error_response(StatusCode::BAD_REQUEST, envelope)
}

// ── middleware ───────────────────────────────────────────────────────────────

/// Every response (including errors and discovery) carries `UHP-Version`, and
/// a request pinning an unsupported version is refused, not silently
/// substituted (V-01/V-02/V-03).
async fn uhp_version_middleware(request: Request, next: Next) -> Response {
    let requested = request
        .headers()
        .get("uhp-version")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let mut response = match requested {
        Some(version) if version != PROTOCOL_VERSION => error_response(
            StatusCode::BAD_REQUEST,
            ErrorEnvelope::new(
                "invalid_request_error",
                "unsupported_protocol_version",
                format!("unsupported protocol version: {version}"),
                Some(serde_json::json!({ "supported": [PROTOCOL_VERSION] })),
            ),
        ),
        _ => next.run(request).await,
    };
    response
        .headers_mut()
        .insert("uhp-version", HeaderValue::from_static("2026-08-11"));
    response
}

/// Everything under /v1 except GET /v1/uhp needs `Authorization: Bearer <token>`.
async fn auth_middleware(State(state): State<Arc<AppState>>, request: Request, next: Next) -> Response {
    let expected = format!("Bearer {}", state.token);
    let authorized = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(|value| value == expected)
        .unwrap_or(false);
    if !authorized {
        return error_response(
            StatusCode::UNAUTHORIZED,
            ErrorEnvelope::new(
                "authentication_error",
                "invalid_credential",
                "missing or invalid bearer token",
                None,
            ),
        );
    }
    next.run(request).await
}

// ── discovery ────────────────────────────────────────────────────────────────

async fn discovery() -> Response {
    Json(Discovery::current()).into_response()
}

// ── harnesses ────────────────────────────────────────────────────────────────

const KNOWN_BASES: [&str; 12] = [
    "kimi",
    "kimi-code",
    "claude",
    "claude-code",
    "codex",
    "gemini",
    "qwen",
    "opencode",
    "cline",
    "pi",
    "dsh",
    "deepseek",
];

async fn list_harnesses(State(state): State<Arc<AppState>>) -> Response {
    match state.store.list_harnesses().await {
        Ok(harnesses) => Json(HarnessList { harnesses }).into_response(),
        Err(err) => internal_error(&format!("database error: {err}")),
    }
}

async fn create_harness(
    State(state): State<Arc<AppState>>,
    body: Result<Json<HarnessUpsert>, JsonRejection>,
) -> Response {
    let Json(body) = match body {
        Ok(body) => body,
        Err(rejection) => return invalid_request(rejection.body_text(), None),
    };
    let Some(base) = body.base.clone() else {
        return invalid_request("missing required field: base", Some("base"));
    };
    if DriverKind::from_base(&base).is_none() {
        let mut envelope = ErrorEnvelope::new(
            "invalid_request_error",
            "unsupported_base",
            format!("unsupported harness base: {base}"),
            Some(serde_json::json!({ "supported": KNOWN_BASES })),
        );
        envelope.error.param = Some("base".into());
        return error_response(StatusCode::UNPROCESSABLE_ENTITY, envelope);
    }
    let extra = protocol::normalize_harness_extra(body.extra.clone());
    if let Err(message) = protocol::validate_skills(&extra) {
        return error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorEnvelope::new("invalid_request_error", "invalid_skill", message, None),
        );
    }
    let id = body
        .id
        .clone()
        .filter(|id| id.starts_with("chrn_"))
        .unwrap_or_else(protocol::harness_id);
    if let Ok(Some(_)) = state.store.get_harness(&id).await {
        return error_response(
            StatusCode::CONFLICT,
            ErrorEnvelope::new("invalid_request_error", "harness_exists", "a harness with this id already exists", None),
        );
    }
    let harness = Harness {
        id,
        object: Some("harness".into()),
        name: body.name.clone().unwrap_or_else(|| base.clone()),
        base,
        base_label: None,
        default_model: body.default_model.clone(),
        created_at: Some(protocol::now_unix()),
        extra,
    };
    if let Err(err) = state.store.create_harness(&harness).await {
        return internal_error(&format!("database error: {err}"));
    }
    Json(harness).into_response()
}

async fn get_harness(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match state.store.get_harness(&id).await {
        Ok(Some(harness)) => Json(harness).into_response(),
        Ok(None) => not_found("harness_not_found", "no harness with this id"),
        Err(err) => internal_error(&format!("database error: {err}")),
    }
}

async fn update_harness(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Result<Json<HarnessUpsert>, JsonRejection>,
) -> Response {
    let Json(body) = match body {
        Ok(body) => body,
        Err(rejection) => return invalid_request(rejection.body_text(), None),
    };
    let Ok(Some(existing)) = state.store.get_harness(&id).await else {
        return not_found("harness_not_found", "no harness with this id");
    };
    if let Some(base) = body.base.clone() {
        if base != existing.base {
            let mut envelope = ErrorEnvelope::new(
                "invalid_request_error",
                "harness_base_immutable",
                "harness base is immutable",
                None,
            );
            envelope.error.param = Some("base".into());
            return error_response(StatusCode::UNPROCESSABLE_ENTITY, envelope);
        }
    }
    let mut extra = existing.extra.clone();
    extra.extend(protocol::normalize_harness_extra(body.extra.clone()));
    if let Err(message) = protocol::validate_skills(&extra) {
        return error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorEnvelope::new("invalid_request_error", "invalid_skill", message, None),
        );
    }
    let harness = Harness {
        id: id.clone(),
        object: Some("harness".into()),
        name: body.name.clone().unwrap_or(existing.name),
        base: existing.base,
        base_label: None,
        default_model: body.default_model.clone().or(existing.default_model),
        created_at: existing.created_at,
        extra,
    };
    if let Err(err) = state.store.update_harness(&harness).await {
        return internal_error(&format!("database error: {err}"));
    }
    Json(harness).into_response()
}

async fn delete_harness(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match state.store.delete_harness(&id).await {
        Ok(true) => Json(DeleteAck {
            id,
            object: "harness".into(),
            deleted: true,
        })
        .into_response(),
        Ok(false) => not_found("harness_not_found", "no harness with this id"),
        Err(err) => internal_error(&format!("database error: {err}")),
    }
}

async fn models_catalog() -> Response {
    Json(model_catalog()).into_response()
}

async fn harness_models(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let Ok(Some(harness)) = state.store.get_harness(&id).await else {
        return not_found("harness_not_found", "no harness with this id");
    };
    let catalog = model_catalog();
    let backend = catalog.backends.get(&harness.base);
    let default = harness
        .default_model
        .clone()
        .or_else(|| backend.map(|backend| backend.default.clone()))
        .unwrap_or_else(|| "default".into());
    Json(HarnessModels {
        harness_id: id,
        backend: harness.base,
        default,
        fallback: None,
        models: backend.map(|backend| backend.models.clone()).unwrap_or_default(),
    })
    .into_response()
}

// ── responses ────────────────────────────────────────────────────────────────

async fn create_response(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    body: Result<Json<CreateResponseRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match body {
        Ok(body) => body,
        Err(rejection) => return invalid_request(rejection.body_text(), None),
    };

    // Idempotency: a replayed key returns the stored response, never a second turn.
    let idempotency_key = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if let Some(key) = &idempotency_key {
        if let Ok(Some(response_id)) = state.store.idempotent_lookup(key).await {
            if let Ok(Some(stored)) = state.store.get_response(&response_id).await {
                return Json(stored).into_response();
            }
        }
    }

    let prompt = match request.prompt_text() {
        Ok(prompt) => prompt,
        Err(message) => return invalid_request(message, Some("input")),
    };

    let harnesses = match state.store.list_harnesses().await {
        Ok(harnesses) => harnesses,
        Err(err) => return internal_error(&format!("database error: {err}")),
    };
    let harness = match request.harness_id() {
        Some(id) => match harnesses.iter().find(|h| h.id == id) {
            Some(harness) => harness.clone(),
            None => return not_found("harness_not_found", "no harness with this id"),
        },
        None => match harnesses.first() {
            Some(harness) => harness.clone(),
            None => return not_found("harness_not_found", "this server has no configured harnesses"),
        },
    };
    let driver = DriverKind::from_base(&harness.base).unwrap_or(DriverKind::Kimi);

    // Session: continue the previous response's session, or open a new one.
    let (session_id, _session_row) = if let Some(previous) = &request.previous_response_id {
        let Ok(Some(previous_response)) = state.store.get_response(previous).await else {
            return not_found("response_not_found", "no response with this previous_response_id");
        };
        let Some(session_id) = previous_response.metadata.session_id.clone() else {
            return invalid_request("previous response has no session", Some("previous_response_id"));
        };
        let Ok(Some(row)) = state.store.get_session(&session_id).await else {
            return not_found("session_not_found", "the previous response's session no longer exists");
        };
        (session_id, row)
    } else {
        let session_id = protocol::session_id();
        let cwd = state
            .data_dir
            .join("sessions")
            .join(&session_id)
            .join("workspace")
            .display()
            .to_string();
        if let Err(err) = state
            .store
            .create_session(&session_id, &harness.id, &cwd, &format!("uhp-{session_id}"))
            .await
        {
            return internal_error(&format!("database error: {err}"));
        }
        let Ok(Some(row)) = state.store.get_session(&session_id).await else {
            return internal_error("session row missing after create");
        };
        (session_id, row)
    };

    let (final_model, requested_model, model_fallback) =
        resolve_model(&harness.base, request.model.as_deref());
    let ignored_fields = request.ignored_fields();
    let timeout = request
        .timeout_seconds
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_TURN_TIMEOUT);

    let response = UhpResponse {
        id: protocol::response_id(),
        object: "response".into(),
        created_at: protocol::now_unix(),
        status: ResponseStatus::InProgress,
        error: None,
        previous_response_id: request.previous_response_id.clone(),
        model: final_model.clone(),
        output: Vec::new(),
        store: request.store.unwrap_or(true),
        usage: None,
        metadata: ResponseMetadata {
            session_id: Some(session_id.clone()),
            harness_id: Some(harness.id.clone()),
            requested_model,
            model_fallback,
            ignored_fields: (!ignored_fields.is_empty()).then_some(ignored_fields),
            extra: Default::default(),
        },
    };
    if let Err(err) = state.store.insert_response(&response).await {
        return internal_error(&format!("database error: {err}"));
    }
    if let Some(key) = &idempotency_key {
        let _ = state
            .store
            .store_idempotency(key, &response.id)
            .await;
    }

    let (events_tx, events_rx) = mpsc::unbounded_channel::<StreamEvent>();
    let control = Arc::new(TurnControl::default());
    state
        .turns
        .lock()
        .expect("turn registry poisoned")
        .insert(response.id.clone(), control.clone());

    let turn_state = state.clone();
    let turn_response = response.clone();
    let turn_id = response.id.clone();
    let cli_model = request.model.as_ref().map(|_| final_model);
    let wih_id = request.wih_id().map(str::to_string);
    let join = tokio::spawn(async move {
        let ctx = TurnContext::from(&turn_state);
        let final_response = crate::turn::run_turn(
            ctx,
            session_id,
            turn_response,
            prompt,
            cli_model,
            timeout,
            driver,
            wih_id,
            control,
            events_tx,
        )
        .await;
        turn_state
            .turns
            .lock()
            .expect("turn registry poisoned")
            .remove(&turn_id);
        final_response
    });

    if request.stream == Some(true) {
        let mut sequence: u64 = 0;
        let stream = UnboundedReceiverStream::new(events_rx).map(move |event| {
            let mut value = serde_json::to_value(&event).unwrap_or_default();
            value["sequence_number"] = serde_json::json!(sequence);
            sequence += 1;
            Ok::<SseEvent, std::convert::Infallible>(
                SseEvent::default().data(value.to_string()),
            )
        });
        return Sse::new(stream).into_response();
    }
    if request.background == Some(true) {
        return Json(response).into_response();
    }
    let final_response = join.await.unwrap_or(response.clone());
    Json(final_response).into_response()
}

async fn get_response(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match state.store.get_response(&id).await {
        Ok(Some(response)) => Json(response).into_response(),
        Ok(None) => not_found("response_not_found", "no response with this id"),
        Err(err) => internal_error(&format!("database error: {err}")),
    }
}

async fn cancel_response(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let Ok(Some(response)) = state.store.get_response(&id).await else {
        return not_found("response_not_found", "no response with this id");
    };
    if response.status.is_terminal() {
        // Already terminal: 200 and the status is unchanged (C-02).
        return Json(response).into_response();
    }
    let control = state
        .turns
        .lock()
        .expect("turn registry poisoned")
        .get(&id)
        .cloned();
    match control {
        Some(control) => {
            control.cancel_requested.store(true, Ordering::SeqCst);
            Json(response).into_response()
        }
        None => {
            // In the store but no live turn (e.g. this server restarted):
            // settle it as cancelled rather than leaving a zombie in_progress.
            let mut response = response;
            response.status = ResponseStatus::Cancelled;
            let _ = state.store.update_response(&response).await;
            Json(response).into_response()
        }
    }
}

// ── sessions ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct SessionQuery {
    limit: Option<i64>,
    cursor: Option<i64>,
}

async fn list_sessions(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SessionQuery>,
) -> Response {
    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    match state.store.list_sessions(limit, query.cursor).await {
        Ok((sessions, next_cursor)) => Json(SessionList {
            sessions,
            next_cursor,
        })
        .into_response(),
        Err(err) => internal_error(&format!("database error: {err}")),
    }
}

async fn get_session(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match state.store.get_session(&id).await {
        Ok(Some(row)) => Json(protocol::Session {
            id: row.id,
            object: "session".into(),
            harness_id: row.harness_id,
            title: row.title,
            status: row.status,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
        .into_response(),
        Ok(None) => not_found("session_not_found", "no session with this id"),
        Err(err) => internal_error(&format!("database error: {err}")),
    }
}

async fn list_session_files(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match state.store.get_session(&id).await {
        Ok(Some(_)) => Json(serde_json::json!({ "files": [] })).into_response(),
        Ok(None) => not_found("session_not_found", "no session with this id"),
        Err(err) => internal_error(&format!("database error: {err}")),
    }
}

async fn list_turns(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match state.store.get_session(&id).await {
        Ok(Some(_)) => {}
        Ok(None) => return not_found("session_not_found", "no session with this id"),
        Err(err) => return internal_error(&format!("database error: {err}")),
    }
    match state.store.list_turns(&id).await {
        Ok(turns) => Json(TurnList { turns }).into_response(),
        Err(err) => internal_error(&format!("database error: {err}")),
    }
}

async fn delete_session(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match state.store.delete_session(&id).await {
        Ok(true) => Json(DeleteAck {
            id,
            object: "session".into(),
            deleted: true,
        })
        .into_response(),
        Ok(false) => not_found("session_not_found", "no session with this id"),
        Err(err) => internal_error(&format!("database error: {err}")),
    }
}

async fn get_skill_files(
    State(state): State<Arc<AppState>>,
    Path((id, name)): Path<(String, String)>,
) -> Response {
    let Ok(Some(harness)) = state.store.get_harness(&id).await else {
        return not_found("harness_not_found", "no harness with this id");
    };
    match protocol::skill_files(&harness.extra, &name) {
        Some(files) => Json(SkillFiles { files }).into_response(),
        None => not_found("skill_not_found", "no skill with this name on the harness"),
    }
}

fn share_body(id: &str, session_id: &str) -> SessionShare {
    SessionShare {
        id: id.to_string(),
        url: format!("/v1/shares/{id}"),
        object: "session.share".into(),
        session_id: Some(session_id.to_string()),
        enabled: Some(true),
    }
}

async fn create_share(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match state.store.get_session(&id).await {
        Ok(Some(_)) => {}
        Ok(None) => return not_found("session_not_found", "no session with this id"),
        Err(err) => return internal_error(&format!("database error: {err}")),
    }
    let share_id = protocol::share_id();
    if let Err(err) = state.store.insert_share(&share_id, &id).await {
        return internal_error(&format!("database error: {err}"));
    }
    Json(share_body(&share_id, &id)).into_response()
}

async fn get_share(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match state.store.latest_share(&id).await {
        Ok(Some((share_id, session_id))) => Json(share_body(&share_id, &session_id)).into_response(),
        Ok(None) => not_found("share_not_found", "this session has no published share"),
        Err(err) => internal_error(&format!("database error: {err}")),
    }
}

async fn revoke_share(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match state.store.get_session(&id).await {
        Ok(Some(_)) => {}
        Ok(None) => return not_found("session_not_found", "no session with this id"),
        Err(err) => return internal_error(&format!("database error: {err}")),
    }
    match state.store.revoke_shares_for_session(&id).await {
        Ok(_) => Json(DeleteAck {
            id,
            object: "session.share".into(),
            deleted: true,
        })
        .into_response(),
        Err(err) => internal_error(&format!("database error: {err}")),
    }
}

/// Unauthenticated read-only view (R-01). Writes on this path are unrouted
/// (404/405), which is what R-04 requires.
async fn shared_view(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let Ok(Some((share_id, session_id))) = state.store.get_share(&id).await else {
        return not_found("not_found", "no such share");
    };
    let Ok(Some(row)) = state.store.get_session(&session_id).await else {
        return not_found("not_found", "no such share");
    };
    Json(serde_json::json!({
        "object": "session.shared",
        "id": share_id,
        "session": {
            "id": row.id,
            "object": "session",
            "harness_id": row.harness_id,
            "title": row.title,
            "status": row.status,
            "created_at": row.created_at,
            "updated_at": row.updated_at,
        }
    }))
    .into_response()
}

// ── error helpers ────────────────────────────────────────────────────────────

fn not_found(code: &str, message: &str) -> Response {
    error_response(
        StatusCode::NOT_FOUND,
        ErrorEnvelope::new("invalid_request_error", code, message, None),
    )
}

fn internal_error(message: &str) -> Response {
    tracing::error!(%message, "internal error");
    error_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        ErrorEnvelope::new("server_error", "server_error", "internal server error", None),
    )
}
