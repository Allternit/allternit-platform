//! Page runtime API: storage, consents and AI for `page` / `card` artifacts,
//! plus the org admin's "Shared outside" list. Child module of
//! `artifacts_v2` so it reuses that file's caller resolution and access
//! checks (`load_with_access`): every route authenticates the caller, then
//! needs at least `view` access to the artifact (404 otherwise, so existence
//! isn't revealed). Rules live in `artifacts::runtime`.
//!
//! * `GET  /api/v2/artifact-runtime/:id/context`
//! * `GET  /api/v2/artifact-runtime/:id/storage?scope&prefix`
//! * `GET|PUT|DELETE /api/v2/artifact-runtime/:id/storage/:key?scope`
//! * `GET|PUT /api/v2/artifact-runtime/:id/consents`
//! * `POST /api/v2/artifact-runtime/:id/ai` (billed to the viewer)
//! * `GET  /api/v2/org/artifact-shared-outside` (org admins)
//!
//! Connector calls run in the app with the viewer's own credentials (the MCP
//! Apps bridge); the server stores the viewer's approval and per-tool "off"
//! list and says whether connectors are allowed at all.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::{
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::get,
    routing::post,
    Json, Router,
};
use bytes::Bytes;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use super::{caller, load_with_access, org_settings, parse_json, ts, ArtifactRow, Result};
use crate::artifacts::access::{Access, Caller};
use crate::artifacts::error::ArtifactError;
use crate::artifacts::runtime::{self as rules, Allowed, Denied, RuntimeSubject, Scope};
use crate::artifacts::sharing::OrgSettings;
use crate::model_router::{ChatCompletionRequest, Message};
use crate::services::{inference_pool, inference_settlement};
use crate::{ApiError, ApiState};

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v2/artifact-runtime/:id/context", get(get_context))
        .route(
            "/api/v2/artifact-runtime/:id/storage",
            get(list_storage),
        )
        .route(
            "/api/v2/artifact-runtime/:id/storage/:key",
            get(get_storage)
                .put(put_storage)
                .delete(delete_storage)
                .layer(DefaultBodyLimit::max(rules::MAX_STORAGE_REQUEST_BYTES)),
        )
        .route(
            "/api/v2/artifact-runtime/:id/consents",
            get(get_consents).put(put_consent),
        )
        .route(
            "/api/v2/artifact-runtime/:id/ai",
            post(complete_ai).layer(DefaultBodyLimit::max(1024 * 1024)),
        )
        .route("/api/v2/org/artifact-shared-outside", get(shared_outside))
}

// ---------------------------------------------------------------------------
// Shared pieces
// ---------------------------------------------------------------------------

struct Ctx {
    row: ArtifactRow,
    caller: Caller,
    access: Access,
    settings: Option<OrgSettings>,
}

impl Ctx {
    fn allowed(&self) -> Allowed {
        let subject = RuntimeSubject {
            kind: &self.row.kind,
            runtime_version: self.row.runtime_version,
            capabilities: &self.row.capabilities,
            artifact_org: self.row.org_id.as_deref(),
            owner_id: &self.row.owner_id,
            viewer_id: &self.caller.id,
            viewer_org: self.caller.org_id.as_deref(),
        };
        rules::allowed(&subject, self.settings.as_ref())
    }
}

async fn ctx(state: &ApiState, headers: &HeaderMap, id: &str) -> Result<Ctx> {
    let who = caller(state, headers).await?;
    let (row, access) = load_with_access(&state.db, id, &who).await?;
    // The artifact's org decides the switches, not the viewer's.
    let settings = match row.org_id.as_deref() {
        Some(org) => Some(org_settings(&state.db, org).await?),
        None => None,
    };
    Ok(Ctx { row, caller: who, access, settings })
}

fn deny(reason: Denied) -> ArtifactError {
    let status = match reason {
        Denied::NotDeclared | Denied::LegacyArtifact | Denied::NotAPage | Denied::NotStorageKind => StatusCode::CONFLICT,
        Denied::OrgOff | Denied::OutsideInvitee => StatusCode::FORBIDDEN,
    };
    let code = match reason {
        Denied::NotDeclared => "capability_not_declared",
        Denied::OrgOff => "org_disabled",
        Denied::OutsideInvitee => "outside_invitee",
        Denied::LegacyArtifact => "legacy_artifact",
        Denied::NotAPage => "not_a_page",
        Denied::NotStorageKind => "not_a_storage_kind",
    };
    ArtifactError::coded(status, code, reason.message())
}

fn parse_scope(raw: Option<&str>) -> Result<Scope> {
    Scope::parse(raw.unwrap_or("personal"))
        .ok_or_else(|| ArtifactError::bad_request("scope must be 'personal' or 'shared'"))
}

async fn consent_row(db: &PgPool, id: &str, user: &str, capability: &str) -> Result<Option<(bool, Vec<String>)>> {
    let row = sqlx::query_as::<_, (bool, Value)>(
        "SELECT granted, denied_tools FROM artifact_consents \
         WHERE artifact_id = $1 AND user_id = $2 AND capability = $3",
    )
    .bind(id)
    .bind(user)
    .bind(capability)
    .fetch_optional(db)
    .await?;
    Ok(row.map(|(granted, tools)| {
        let tools = tools
            .as_array()
            .map(|list| list.iter().filter_map(|t| t.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        (granted, tools)
    }))
}

fn consent_required(capability: &'static str) -> ArtifactError {
    ArtifactError::coded(
        StatusCode::FORBIDDEN,
        "consent_required",
        "The viewer hasn't approved this yet",
    )
    .with("capability", json!(capability))
}

async fn used_bytes(db: &PgPool, id: &str) -> Result<i64> {
    Ok(sqlx::query_scalar::<_, Option<i64>>(
        "SELECT SUM(bytes)::BIGINT FROM artifact_storage WHERE artifact_id = $1",
    )
    .bind(id)
    .fetch_one(db)
    .await?
    .unwrap_or(0))
}

fn allowed_json(result: std::result::Result<(), Denied>) -> Value {
    match result {
        Ok(()) => json!({ "ok": true }),
        Err(reason) => json!({ "ok": false, "reason": reason, "message": reason.message() }),
    }
}

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

async fn get_context(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    let c = ctx(&state, &headers, &id).await?;
    let allowed = c.allowed();
    let mut consents = serde_json::Map::new();
    for capability in rules::CONSENT_CAPABILITIES {
        let row = consent_row(&state.db, &c.row.id, &c.caller.id, capability).await?;
        consents.insert(
            (*capability).to_string(),
            json!({
                "granted": row.as_ref().is_some_and(|(g, _)| *g),
                "decided": row.is_some(),
                "denied_tools": row.map(|(_, t)| t).unwrap_or_default(),
            }),
        );
    }
    Ok(Json(json!({
        "artifact_id": c.row.id,
        "kind": c.row.kind,
        "access": c.access.as_str(),
        "capabilities": c.row.capabilities,
        "allowed": {
            "storage": allowed_json(allowed.storage),
            "ai": allowed_json(allowed.ai),
            "connectors": allowed_json(allowed.connectors),
        },
        "consents": consents,
        "storage": { "used_bytes": used_bytes(&state.db, &c.row.id).await?, "limit_bytes": rules::MAX_STORAGE_BYTES },
    })))
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ScopeQuery {
    scope: Option<String>,
    prefix: Option<String>,
}

async fn list_storage(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<ScopeQuery>,
) -> Result<Json<Value>> {
    let c = ctx(&state, &headers, &id).await?;
    c.allowed().storage.map_err(deny)?;
    let scope = parse_scope(q.scope.as_deref())?;
    let prefix = q.prefix.unwrap_or_default();
    if !rules::valid_prefix(&prefix) {
        return Err(ArtifactError::bad_request("invalid prefix"));
    }
    let rows = sqlx::query_as::<_, (String, i32)>(
        "SELECT key, bytes FROM artifact_storage \
         WHERE artifact_id = $1 AND scope = $2 AND user_id = $3 AND key LIKE $4 ESCAPE '\\' \
         ORDER BY key LIMIT $5",
    )
    .bind(&c.row.id)
    .bind(scope.as_str())
    .bind(scope.owner_column(&c.caller.id))
    .bind(rules::prefix_pattern(&prefix))
    .bind(rules::MAX_LIST_KEYS)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({
        "scope": scope.as_str(),
        "keys": rows.iter().map(|(k, _)| k).collect::<Vec<_>>(),
        "truncated": rows.len() as i64 >= rules::MAX_LIST_KEYS,
    })))
}

fn check_key(key: &str) -> Result<()> {
    if rules::valid_key(key) {
        Ok(())
    } else {
        Err(ArtifactError::bad_request("key must be 1-200 printable characters"))
    }
}

async fn get_storage(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path((id, key)): Path<(String, String)>,
    Query(q): Query<ScopeQuery>,
) -> Result<Json<Value>> {
    let c = ctx(&state, &headers, &id).await?;
    c.allowed().storage.map_err(deny)?;
    check_key(&key)?;
    let scope = parse_scope(q.scope.as_deref())?;
    let value = sqlx::query_scalar::<_, String>(
        "SELECT value FROM artifact_storage \
         WHERE artifact_id = $1 AND scope = $2 AND user_id = $3 AND key = $4",
    )
    .bind(&c.row.id)
    .bind(scope.as_str())
    .bind(scope.owner_column(&c.caller.id))
    .bind(&key)
    .fetch_optional(&state.db)
    .await?;
    Ok(Json(json!({ "key": key, "scope": scope.as_str(), "value": value })))
}

/// Shared writes need the viewer's consent first (their first write asks).
async fn require_shared_consent(db: &PgPool, c: &Ctx, scope: Scope) -> Result<()> {
    if scope == Scope::Shared {
        let granted = consent_row(db, &c.row.id, &c.caller.id, rules::CAP_STORAGE_SHARED)
            .await?
            .is_some_and(|(g, _)| g);
        if !granted {
            return Err(consent_required(rules::CAP_STORAGE_SHARED));
        }
    }
    Ok(())
}

async fn put_storage(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path((id, key)): Path<(String, String)>,
    Query(q): Query<ScopeQuery>,
    body: Bytes,
) -> Result<Json<Value>> {
    let c = ctx(&state, &headers, &id).await?;
    c.allowed().storage.map_err(deny)?;
    check_key(&key)?;
    let scope = parse_scope(q.scope.as_deref())?;
    let payload: Value = parse_json(&body)?;
    let Some(value) = payload.get("value").and_then(Value::as_str) else {
        return Err(ArtifactError::bad_request("body must be {\"value\": string}"));
    };
    if value.len() > rules::MAX_VALUE_BYTES {
        return Err(ArtifactError::coded(
            StatusCode::PAYLOAD_TOO_LARGE,
            "value_too_large",
            "A stored value can be at most 5 MB",
        ));
    }
    require_shared_consent(&state.db, &c, scope).await?;
    let owner = scope.owner_column(&c.caller.id);
    let new_bytes = rules::entry_bytes(&key, value);

    // One writer per artifact at a time, so the 20 MB cap can't be raced past.
    let mut tx = state.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(&c.row.id)
        .execute(&mut *tx)
        .await?;
    let others: i64 = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT SUM(bytes)::BIGINT FROM artifact_storage WHERE artifact_id = $1 \
         AND NOT (scope = $2 AND user_id = $3 AND key = $4)",
    )
    .bind(&c.row.id)
    .bind(scope.as_str())
    .bind(&owner)
    .bind(&key)
    .fetch_one(&mut *tx)
    .await?
    .unwrap_or(0);
    if !rules::fits_quota(others, new_bytes) {
        return Err(ArtifactError::coded(
            StatusCode::PAYLOAD_TOO_LARGE,
            "storage_full",
            "This artifact's storage is full (20 MB)",
        )
        .with("used_bytes", json!(others))
        .with("limit_bytes", json!(rules::MAX_STORAGE_BYTES)));
    }
    sqlx::query(
        "INSERT INTO artifact_storage (artifact_id, scope, user_id, key, value, bytes, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, now()) \
         ON CONFLICT (artifact_id, scope, user_id, key) \
         DO UPDATE SET value = EXCLUDED.value, bytes = EXCLUDED.bytes, updated_at = now()",
    )
    .bind(&c.row.id)
    .bind(scope.as_str())
    .bind(&owner)
    .bind(&key)
    .bind(value)
    .bind(new_bytes as i32)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(json!({ "key": key, "scope": scope.as_str(), "bytes": new_bytes })))
}

async fn delete_storage(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path((id, key)): Path<(String, String)>,
    Query(q): Query<ScopeQuery>,
) -> Result<StatusCode> {
    let c = ctx(&state, &headers, &id).await?;
    c.allowed().storage.map_err(deny)?;
    check_key(&key)?;
    let scope = parse_scope(q.scope.as_deref())?;
    require_shared_consent(&state.db, &c, scope).await?;
    sqlx::query(
        "DELETE FROM artifact_storage WHERE artifact_id = $1 AND scope = $2 AND user_id = $3 AND key = $4",
    )
    .bind(&c.row.id)
    .bind(scope.as_str())
    .bind(scope.owner_column(&c.caller.id))
    .bind(&key)
    .execute(&state.db)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Consents
// ---------------------------------------------------------------------------

async fn get_consents(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    let c = ctx(&state, &headers, &id).await?;
    let rows = sqlx::query_as::<_, (String, bool, Value)>(
        "SELECT capability, granted, denied_tools FROM artifact_consents WHERE artifact_id = $1 AND user_id = $2",
    )
    .bind(&c.row.id)
    .bind(&c.caller.id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({
        "items": rows.into_iter().map(|(capability, granted, denied_tools)| json!({
            "capability": capability, "granted": granted, "denied_tools": denied_tools,
        })).collect::<Vec<_>>(),
    })))
}

async fn put_consent(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<Value>> {
    let c = ctx(&state, &headers, &id).await?;
    let payload: Value = parse_json(&body)?;
    let capability = payload.get("capability").and_then(Value::as_str).unwrap_or_default();
    let Some(capability) = rules::CONSENT_CAPABILITIES.iter().copied().find(|k| *k == capability) else {
        return Err(ArtifactError::unprocessable(
            "invalid_consent",
            "capability must be storage_shared, ai or connectors",
        ));
    };
    let Some(granted) = payload.get("granted").and_then(Value::as_bool) else {
        return Err(ArtifactError::unprocessable("invalid_consent", "granted must be a boolean"));
    };
    let denied: Vec<String> = match payload.get("denied_tools") {
        None | Some(Value::Null) => Vec::new(),
        Some(list) => list
            .as_array()
            .filter(|a| a.len() <= 200)
            .and_then(|a| a.iter().map(|t| t.as_str().filter(|s| s.len() <= 200).map(str::to_string)).collect())
            .ok_or_else(|| ArtifactError::unprocessable("invalid_consent", "denied_tools must be a list of tool names"))?,
    };
    // A viewer can only approve what the page declared and the org allows.
    if granted {
        let allowed = c.allowed();
        let gate = match capability {
            rules::CAP_STORAGE_SHARED => allowed.storage,
            rules::CAP_AI => allowed.ai,
            _ => allowed.connectors,
        };
        gate.map_err(deny)?;
    }
    sqlx::query(
        "INSERT INTO artifact_consents (artifact_id, user_id, capability, granted, denied_tools, updated_at) \
         VALUES ($1, $2, $3, $4, $5, now()) \
         ON CONFLICT (artifact_id, user_id, capability) \
         DO UPDATE SET granted = EXCLUDED.granted, denied_tools = EXCLUDED.denied_tools, updated_at = now()",
    )
    .bind(&c.row.id)
    .bind(&c.caller.id)
    .bind(capability)
    .bind(granted)
    .bind(json!(denied))
    .execute(&state.db)
    .await?;
    Ok(Json(json!({ "capability": capability, "granted": granted, "denied_tools": denied })))
}

// ---------------------------------------------------------------------------
// AI (billed to the viewer)
// ---------------------------------------------------------------------------

fn ai_calls() -> &'static Mutex<HashMap<String, VecDeque<Instant>>> {
    static CALLS: OnceLock<Mutex<HashMap<String, VecDeque<Instant>>>> = OnceLock::new();
    CALLS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// At most `AI_CALLS_PER_MINUTE` per viewer and artifact.
fn ai_rate_ok(viewer: &str, artifact: &str) -> bool {
    let now = Instant::now();
    let mut map = ai_calls().lock().unwrap_or_else(|e| e.into_inner());
    let window = map.entry(format!("{viewer}:{artifact}")).or_default();
    while window.front().is_some_and(|t| now.duration_since(*t) > Duration::from_secs(60)) {
        window.pop_front();
    }
    if window.len() >= rules::AI_CALLS_PER_MINUTE {
        return false;
    }
    window.push_back(now);
    if map.len() > 10_000 {
        map.retain(|_, w| w.back().is_some_and(|t| now.duration_since(*t) <= Duration::from_secs(60)));
    }
    true
}

fn ai_model() -> String {
    std::env::var("ARTIFACT_RUNTIME_MODEL")
        .ok()
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| "llama-3.1-8b".to_string())
}

async fn complete_ai(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<Value>> {
    let c = ctx(&state, &headers, &id).await?;
    c.allowed().ai.map_err(deny)?;
    let consented = consent_row(&state.db, &c.row.id, &c.caller.id, rules::CAP_AI)
        .await?
        .is_some_and(|(g, _)| g);
    if !consented {
        return Err(consent_required(rules::CAP_AI));
    }
    let payload: Value = parse_json(&body)?;
    // Accept {prompt} or {messages: [{role, content}]}; content is text only.
    let mut messages: Vec<(String, String)> = Vec::new();
    if let Some(prompt) = payload.get("prompt").and_then(Value::as_str) {
        if let Some(system) = payload.get("system").and_then(Value::as_str) {
            messages.push(("system".into(), system.to_string()));
        }
        messages.push(("user".into(), prompt.to_string()));
    } else if let Some(list) = payload.get("messages").and_then(Value::as_array) {
        for m in list {
            let (Some(role), Some(content)) =
                (m.get("role").and_then(Value::as_str), m.get("content").and_then(Value::as_str))
            else {
                return Err(ArtifactError::bad_request("each message needs a text role and content"));
            };
            messages.push((role.to_string(), content.to_string()));
        }
    }
    let max_tokens = payload.get("max_tokens").and_then(Value::as_u64).map(|n| n.min(u32::MAX as u64) as u32);
    let shape: Vec<(String, usize)> = messages.iter().map(|(r, t)| (r.clone(), t.chars().count())).collect();
    let max_tokens = rules::check_ai_request(&shape, max_tokens)
        .map_err(|why| ArtifactError::unprocessable("invalid_ai_request", why))?;
    let temperature = payload
        .get("temperature")
        .and_then(Value::as_f64)
        .map(|t| t.clamp(0.0, 1.5) as f32);

    if !ai_rate_ok(&c.caller.id, &c.row.id) {
        return Err(ArtifactError::Api(ApiError::TooManyRequests(
            "This page is making too many AI calls. Wait a minute.".to_string(),
        )));
    }
    let viewer = c.caller.id.clone();
    let out = billed_completion(&state, &viewer, messages, max_tokens, temperature).await?;
    Ok(Json(out))
}

/// One non-streamed model call billed to `payer`: the same credit gate, free-tier
/// limit, pool checks and settlement as /v1/chat/completions. Returns
/// `{text, model, usage, finish_reason}`. Used by page artifacts (the viewer
/// pays) and by @gizzi comment replies (the commenter pays).
pub(crate) async fn billed_completion(
    state: &Arc<ApiState>,
    payer: &str,
    messages: Vec<(String, String)>,
    max_tokens: u32,
    temperature: Option<f32>,
) -> Result<Value> {
    if !state.model_router.is_enabled() {
        return Err(ArtifactError::Api(ApiError::ServiceUnavailable(
            "AI isn't available right now".to_string(),
        )));
    }

    let viewer = payer.to_string();
    let alias = ai_model();
    let balance = inference_settlement::credit_balance_row(&state.db, &viewer).await?;
    inference_settlement::check_inference_allowed(&state.db, &viewer, balance).await?;
    if balance.is_none() {
        if let Err(info) = state.free_inference_rate_limiter.check(&viewer).await {
            return Err(ArtifactError::Api(ApiError::TooManyRequests(format!(
                "Free AI limit reached. Try again in {}s or add credits.",
                info.reset_after.as_secs()
            ))));
        }
    }
    let prompt_chars: usize = messages.iter().map(|(_, t)| t.chars().count()).sum();
    let prices = state.model_router.retail_prices(&alias).await.map_err(ApiError::from)?;
    let pool = match state.model_router.provider_for_alias(&alias) {
        Some(provider) => state.inference_pool_service.pool_for_provider(provider).await?,
        None => None,
    };
    if let Some(pool) = pool.as_ref() {
        state.inference_pool_service.check_pool_available(pool).await?;
    }
    inference_pool::check_free_tier_pool(inference_pool::free_tier_pool_policy(), balance.is_none(), pool.as_ref())?;
    let pool_id = pool.map(|p| p.id);

    let request = ChatCompletionRequest {
        model: alias.clone(),
        messages: messages.into_iter().map(|(role, text)| Message::text(role, text)).collect(),
        temperature,
        max_tokens: Some(max_tokens),
        stream: Some(false),
        top_p: None,
        extra: Default::default(),
    };
    let response = state.model_router.chat_completions(request).await.map_err(ApiError::from)?;
    let response = inference_settlement::meter_json_response(
        &state.db, &viewer, &alias, pool_id.as_deref(), true, &prices, prompt_chars, response,
    )
    .await?;
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .map_err(|e| ApiError::Internal(format!("AI response unreadable: {e}")))?;
    let upstream: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if !status.is_success() {
        return Err(ArtifactError::coded(StatusCode::BAD_GATEWAY, "ai_failed", "The model couldn't answer"));
    }
    let text = upstream
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    Ok(json!({
        "text": text,
        "model": alias,
        "usage": upstream.get("usage").cloned().unwrap_or(Value::Null),
        "finish_reason": upstream.pointer("/choices/0/finish_reason").cloned().unwrap_or(Value::Null),
    }))
}

// ---------------------------------------------------------------------------
// Org admin: "Shared outside" review list
// ---------------------------------------------------------------------------

async fn shared_outside(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Result<Json<Value>> {
    let who = caller(&state, &headers).await?;
    let Some(org_id) = who.org_id.clone() else {
        return Err(ArtifactError::forbidden("Shared-outside review belongs to an organization"));
    };
    if !who.is_org_admin() {
        return Err(ArtifactError::forbidden("Only organization admins can review shared artifacts"));
    }
    let settings = org_settings(&state.db, &org_id).await?;
    let rows = sqlx::query_as::<_, (String, String, String, String, String, i64, chrono::DateTime<chrono::Utc>)>(
        "SELECT a.id, a.title, a.kind, a.owner_id, a.visibility, \
                (SELECT COUNT(*) FROM artifact_shares s WHERE s.artifact_id = a.id AND s.principal_type = 'email'), \
                a.updated_at \
         FROM artifacts a \
         WHERE a.org_id = $1 AND (a.visibility = 'link' OR EXISTS ( \
               SELECT 1 FROM artifact_shares s WHERE s.artifact_id = a.id AND s.principal_type = 'email')) \
         ORDER BY a.updated_at DESC LIMIT 200",
    )
    .bind(&org_id)
    .fetch_all(&state.db)
    .await?;
    let owners: Vec<String> = rows.iter().map(|r| r.3.clone()).collect();
    let names = super::profiles(&state.db, &owners).await;
    Ok(Json(json!({
        "items": rows.into_iter().map(|(id, title, kind, owner_id, visibility, invites, updated)| json!({
            "id": id,
            "title": title,
            "kind": kind,
            "owner_id": owner_id,
            "owner_name": names.get(&owner_id).and_then(|(n, _)| n.clone()),
            "by_link": visibility == "link",
            "outside_invites": invites,
            "allowed": settings.allowed_external.contains(&id),
            "updated_at": ts(updated),
        })).collect::<Vec<_>>(),
    })))
}

#[cfg(test)]
#[path = "artifact_runtime_tests.rs"]
mod tests;
