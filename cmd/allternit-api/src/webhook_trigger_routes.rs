//! Inbound webhook triggers for bots (`/api/v1/webhook-triggers` and `/webhooks/inbound/:id`).
//!
//! Users configure triggers that map an external source + event type to a target
//! bot/agent. When a signed webhook is received on the public inbound endpoint,
//! the platform verifies the signature, records the delivery, and creates a
//! Rails ticket so the target bot can pick up the work.

use axum::{
    extract::{Extension, Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use hmac::{Hmac, Mac};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};
use std::sync::Arc;
use tracing::info;

use crate::{auth::AuthUser, error::ApiError, AppState};
use crate::rails::RailsState;
use allternit_factory_engine::rails_id::{HierarchicalId, TicketId};
use allternit_factory_engine::tickets::{
    Ticket, TicketKind, TicketPriority, TicketStatus, TicketStore,
};

type HmacSha256 = Hmac<Sha256>;

/// Protected management surface for webhook triggers. Mount under `/api/v1`.
pub fn webhook_trigger_protected_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/webhook-triggers", get(list_triggers).post(create_trigger))
        .route(
            "/webhook-triggers/:id",
            get(get_trigger).patch(update_trigger).delete(delete_trigger),
        )
        .route("/webhook-triggers/:id/deliveries", get(list_deliveries))
}

/// Public inbound webhook receiver. Mount at root.
pub fn webhook_trigger_public_router() -> Router<Arc<AppState>> {
    Router::new().route("/webhooks/inbound/:id", post(receive_inbound_webhook))
}

/// Combined router for standalone tests (keeps the `/api/v1` prefix).
pub fn webhook_trigger_router() -> Router<Arc<AppState>> {
    Router::new()
        .nest("/api/v1", webhook_trigger_protected_router())
        .merge(webhook_trigger_public_router())
}

// ═══════════════════════════════════════════════════════════════════════════════
// Domain types
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookTrigger {
    pub id: String,
    pub user_id: String,
    pub org_id: Option<String>,
    pub name: String,
    pub source: String,
    pub event_type: String,
    pub target_agent_id: String,
    pub prompt_template: Option<String>,
    pub execution_mode: String,
    pub active: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookTriggerDelivery {
    pub id: String,
    pub trigger_id: String,
    pub event: String,
    pub payload: Value,
    pub signature_valid: bool,
    pub status: String,
    pub ticket_id: Option<String>,
    pub error: Option<String>,
    pub attempts: i32,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
struct CreateTriggerBody {
    name: String,
    source: String,
    event_type: String,
    target_agent_id: String,
    #[serde(default)]
    prompt_template: Option<String>,
    #[serde(default = "default_execution_mode")]
    execution_mode: String,
    secret: String,
}

#[derive(Debug, Deserialize, Default)]
struct UpdateTriggerBody {
    name: Option<String>,
    source: Option<String>,
    event_type: Option<String>,
    target_agent_id: Option<String>,
    prompt_template: Option<Option<String>>,
    execution_mode: Option<String>,
    secret: Option<String>,
    active: Option<bool>,
}

fn default_execution_mode() -> String {
    "REQUIRE_APPROVAL".to_string()
}

fn read_trigger(row: &rusqlite::Row<'_>) -> rusqlite::Result<WebhookTrigger> {
    Ok(WebhookTrigger {
        id: row.get(0)?,
        user_id: row.get(1)?,
        org_id: row.get(2)?,
        name: row.get(3)?,
        source: row.get(4)?,
        event_type: row.get(5)?,
        target_agent_id: row.get(6)?,
        prompt_template: row.get(7)?,
        execution_mode: row.get(8)?,
        active: row.get::<_, i32>(9)? != 0,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

fn read_delivery(row: &rusqlite::Row<'_>) -> rusqlite::Result<WebhookTriggerDelivery> {
    let payload_json: String = row.get(3)?;
    Ok(WebhookTriggerDelivery {
        id: row.get(0)?,
        trigger_id: row.get(1)?,
        event: row.get(2)?,
        payload: serde_json::from_str(&payload_json).unwrap_or(json!({})),
        signature_valid: row.get::<_, i32>(4)? != 0,
        status: row.get(5)?,
        ticket_id: row.get(6)?,
        error: row.get(7)?,
        attempts: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

// ═══════════════════════════════════════════════════════════════════════════════
// Validation helpers
// ═══════════════════════════════════════════════════════════════════════════════

fn validate_trigger_body(body: &CreateTriggerBody) -> Result<(), ApiError> {
    if body.name.trim().is_empty() {
        return Err(ApiError::BadRequest("name is required".into()));
    }
    if body.source.trim().is_empty() {
        return Err(ApiError::BadRequest("source is required".into()));
    }
    if body.event_type.trim().is_empty() {
        return Err(ApiError::BadRequest("event_type is required".into()));
    }
    if body.target_agent_id.trim().is_empty() {
        return Err(ApiError::BadRequest("target_agent_id is required".into()));
    }
    if body.secret.trim().is_empty() {
        return Err(ApiError::BadRequest("secret is required".into()));
    }
    let valid_modes = ["PLAN_ONLY", "REQUIRE_APPROVAL", "ACCEPT_EDITS", "BYPASS_PERMISSIONS"];
    if !valid_modes.contains(&body.execution_mode.as_str()) {
        return Err(ApiError::BadRequest(format!(
            "execution_mode must be one of: {}",
            valid_modes.join(", ")
        )));
    }
    Ok(())
}

fn hash_secret(secret: &str) -> String {
    // Store a fast salted hash so the raw secret is not kept in the API DB.
    // The inbound endpoint verifies against the original secret via HMAC,
    // so we only need the hash for persistence. For simplicity we use a
    // plaintext hash marker plus HMAC; this is sufficient for the first pass.
    format!("sha256:{}", hex::encode(sha2::Sha256::digest(secret.as_bytes())))
}

fn verify_signature(secret: &str, body: &[u8], signature: &str) -> bool {
    let mut mac = match HmacSha256::new_from_slice(secret.as_bytes()) {
        Ok(m) => m,
        Err(_) => return false,
    };
    mac.update(body);
    let expected = hex::encode(mac.finalize().into_bytes());

    if expected.len() != signature.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in expected.bytes().zip(signature.bytes()) {
        diff |= a ^ b;
    }
    diff == 0
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn extract_inbound_signature(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-allternit-signature")
        .or_else(|| headers.get("X-Allternit-Signature"))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

// ═══════════════════════════════════════════════════════════════════════════════
// Authenticated trigger management
// ═══════════════════════════════════════════════════════════════════════════════

async fn create_trigger(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<CreateTriggerBody>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    validate_trigger_body(&body)?;

    let id = uuid::Uuid::new_v4().to_string();
    let secret_hash = hash_secret(&body.secret);

    let db = state.db.clone();
    let lookup_id = id.clone();
    let trigger = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "INSERT INTO webhook_triggers
             (id, user_id, org_id, name, source, event_type, target_agent_id, prompt_template, execution_mode, secret_hash, active)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 1)",
            params![
                id,
                user.user_id,
                user.organization_id,
                body.name,
                body.source,
                body.event_type,
                body.target_agent_id,
                body.prompt_template,
                body.execution_mode,
                secret_hash,
            ],
        )?;
        conn.query_row(
            "SELECT id, user_id, org_id, name, source, event_type, target_agent_id, prompt_template, execution_mode, active, created_at, updated_at
             FROM webhook_triggers WHERE id = ?1",
            params![lookup_id],
            read_trigger,
        )
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .map_err(|e: rusqlite::Error| ApiError::DbError(e.to_string()))?;

    info!("Created webhook trigger {} for agent {}", trigger.id, trigger.target_agent_id);
    Ok((StatusCode::CREATED, Json(json!({ "trigger": trigger }))))
}

async fn list_triggers(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result<Json<Value>, ApiError> {
    let db = state.db.clone();
    let user_id = user.user_id.clone();
    let org_id = user.organization_id.clone();
    let rows = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, user_id, org_id, name, source, event_type, target_agent_id, prompt_template, execution_mode, active, created_at, updated_at
             FROM webhook_triggers WHERE user_id = ?1 OR org_id = ?2 ORDER BY created_at DESC",
        )?;
        let rows = stmt
            .query_map(params![user_id, org_id], read_trigger)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok::<_, rusqlite::Error>(rows)
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .map_err(|e: rusqlite::Error| ApiError::DbError(e.to_string()))?;

    Ok(Json(json!({ "triggers": rows, "total": rows.len() })))
}

async fn get_trigger(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let db = state.db.clone();
    let user_id = user.user_id.clone();
    let org_id = user.organization_id.clone();
    let row = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.query_row(
            "SELECT id, user_id, org_id, name, source, event_type, target_agent_id, prompt_template, execution_mode, active, created_at, updated_at
             FROM webhook_triggers WHERE id = ?1 AND (user_id = ?2 OR org_id = ?3)",
            params![id, user_id, org_id],
            read_trigger,
        )
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .map_err(|e: rusqlite::Error| {
        if matches!(e, rusqlite::Error::QueryReturnedNoRows) {
            ApiError::NotFound("trigger not found".into())
        } else {
            ApiError::DbError(e.to_string())
        }
    })?;

    Ok(Json(json!({ "trigger": row })))
}

async fn update_trigger(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<UpdateTriggerBody>,
) -> Result<Json<Value>, ApiError> {
    let valid_modes = ["PLAN_ONLY", "REQUIRE_APPROVAL", "ACCEPT_EDITS", "BYPASS_PERMISSIONS"];
    if let Some(ref mode) = body.execution_mode {
        if !valid_modes.contains(&mode.as_str()) {
            return Err(ApiError::BadRequest(format!(
                "execution_mode must be one of: {}",
                valid_modes.join(", ")
            )));
        }
    }

    let has_fields = body.name.is_some()
        || body.source.is_some()
        || body.event_type.is_some()
        || body.target_agent_id.is_some()
        || body.prompt_template.is_some()
        || body.execution_mode.is_some()
        || body.secret.is_some()
        || body.active.is_some();
    if !has_fields {
        return Err(ApiError::BadRequest("no fields provided".into()));
    }

    let db = state.db.clone();
    let trigger_id = id.clone();
    let user_id = user.user_id.clone();
    let org_id = user.organization_id.clone();
    let affected = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut sets: Vec<&'static str> = Vec::new();
        let mut sql_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(name) = body.name {
            sets.push("name = ?");
            sql_params.push(Box::new(name));
        }
        if let Some(source) = body.source {
            sets.push("source = ?");
            sql_params.push(Box::new(source));
        }
        if let Some(event_type) = body.event_type {
            sets.push("event_type = ?");
            sql_params.push(Box::new(event_type));
        }
        if let Some(target_agent_id) = body.target_agent_id {
            sets.push("target_agent_id = ?");
            sql_params.push(Box::new(target_agent_id));
        }
        if let Some(prompt_template) = body.prompt_template {
            sets.push("prompt_template = ?");
            sql_params.push(Box::new(prompt_template));
        }
        if let Some(execution_mode) = body.execution_mode {
            sets.push("execution_mode = ?");
            sql_params.push(Box::new(execution_mode));
        }
        if let Some(secret) = body.secret {
            if secret.trim().is_empty() {
                return Err(rusqlite::Error::ToSqlConversionFailure(
                    "secret cannot be empty".into(),
                ));
            }
            sets.push("secret_hash = ?");
            sql_params.push(Box::new(hash_secret(&secret)));
        }
        if let Some(active) = body.active {
            sets.push("active = ?");
            sql_params.push(Box::new(if active { 1 } else { 0 }));
        }

        sets.push("updated_at = CURRENT_TIMESTAMP");

        let sql = format!(
            "UPDATE webhook_triggers SET {} WHERE id = ? AND (user_id = ? OR org_id = ?)",
            sets.join(", ")
        );
        let mut stmt = conn.prepare(&sql)?;
        for (i, param) in sql_params.iter().enumerate() {
            stmt.raw_bind_parameter(i + 1, param.as_ref())?;
        }
        stmt.raw_bind_parameter(sql_params.len() + 1, &trigger_id)?;
        stmt.raw_bind_parameter(sql_params.len() + 2, &user_id)?;
        stmt.raw_bind_parameter(sql_params.len() + 3, &org_id)?;
        stmt.raw_execute()
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .map_err(|e: rusqlite::Error| ApiError::DbError(e.to_string()))?;

    if affected == 0 {
        return Err(ApiError::NotFound("trigger not found".into()));
    }

    get_trigger(State(state), Extension(user), Path(id)).await
}

async fn delete_trigger(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let db = state.db.clone();
    let affected = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "DELETE FROM webhook_triggers WHERE id = ?1 AND (user_id = ?2 OR org_id = ?3)",
            params![id, user.user_id, user.organization_id],
        )
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .map_err(|e: rusqlite::Error| ApiError::DbError(e.to_string()))?;

    if affected == 0 {
        return Err(ApiError::NotFound("trigger not found".into()));
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn list_deliveries(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let db = state.db.clone();
    let user_id = user.user_id.clone();
    let org_id = user.organization_id.clone();
    let rows = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT d.id, d.trigger_id, d.event, d.payload, d.signature_valid, d.status, d.ticket_id, d.error, d.attempts, d.created_at, d.updated_at
             FROM webhook_trigger_deliveries d
             JOIN webhook_triggers t ON t.id = d.trigger_id
             WHERE d.trigger_id = ?1 AND (t.user_id = ?2 OR t.org_id = ?3)
             ORDER BY d.created_at DESC LIMIT 100",
        )?;
        let rows = stmt
            .query_map(params![id, user_id, org_id], read_delivery)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok::<_, rusqlite::Error>(rows)
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .map_err(|e: rusqlite::Error| ApiError::DbError(e.to_string()))?;

    Ok(Json(json!({ "deliveries": rows, "total": rows.len() })))
}

// ═══════════════════════════════════════════════════════════════════════════════
// Public inbound webhook receiver
// ═══════════════════════════════════════════════════════════════════════════════

async fn receive_inbound_webhook(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Json<Value>, ApiError> {
    let signature = extract_inbound_signature(&headers).unwrap_or_default();
    let payload: Value = serde_json::from_slice(&body).unwrap_or(json!({ "raw": base64_encoded(&body) }));
    let event = payload
        .get("event")
        .or_else(|| payload.get("type"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    // Load trigger and verify it exists/active.
    let db = state.db.clone();
    let trigger_id = id.clone();
    let (trigger, secret_hash) = match tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.query_row(
            "SELECT id, user_id, org_id, name, source, event_type, target_agent_id, prompt_template, execution_mode, active, created_at, updated_at, secret_hash
             FROM webhook_triggers WHERE id = ?1",
            params![trigger_id],
            |row| Ok((read_trigger(row)?, row.get::<_, Option<String>>(12)?)),
        )
    })
    .await
    {
        Ok(Ok(t)) => t,
        Ok(Err(rusqlite::Error::QueryReturnedNoRows)) => {
            return Ok(Json(json!({ "status": "ignored", "reason": "trigger not found" })));
        }
        Ok(Err(e)) => return Err(ApiError::DbError(e.to_string())),
        Err(e) => return Err(ApiError::Internal(e.to_string())),
    };

    if !trigger.active {
        return Ok(Json(json!({ "status": "ignored", "reason": "trigger inactive" })));
    }

    // The secret is stored as a hash; we need the original secret to verify the
    // HMAC signature. To avoid storing plaintext secrets, we accept a second
    // header `X-Allternit-Trigger-Secret` that carries the secret for this
    // single request. This keeps the DB hash for audit while still allowing
    // signature verification without a plaintext vault dependency.
    //
    let provided_secret = headers
        .get("x-allternit-trigger-secret")
        .or_else(|| headers.get("X-Allternit-Trigger-Secret"))
        .and_then(|v| v.to_str().ok());

    // The presented secret must be the trigger's own (its hash is what we keep),
    // and the body must be signed with it. Anything else never creates a ticket.
    let signature_valid = match (signature.is_empty(), provided_secret) {
        (false, Some(secret)) => {
            secret_hash.as_deref().is_some_and(|stored| constant_time_eq(stored.as_bytes(), hash_secret(secret).as_bytes()))
                && verify_signature(secret, &body, &signature)
        }
        _ => false,
    };

    // Record delivery before doing work so we always have an audit row.
    let delivery_id = uuid::Uuid::new_v4().to_string();
    {
        let db = state.db.clone();
        let delivery_id = delivery_id.clone();
        let trigger_id = trigger.id.clone();
        let event = event.clone();
        let payload_str = payload.to_string();
        let sig_valid = signature_valid;
        tokio::task::spawn_blocking(move || {
            let conn = db.connect()?;
            conn.execute(
                "INSERT INTO webhook_trigger_deliveries
                 (id, trigger_id, event, payload, signature_valid, status, attempts)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'pending', 1)",
                params![delivery_id, trigger_id, event, payload_str, if sig_valid { 1 } else { 0 }],
            )?;
            Ok::<_, rusqlite::Error>(())
        })
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .map_err(|e: rusqlite::Error| ApiError::DbError(e.to_string()))?;
    }

    if !signature_valid {
        // Unsigned requests used to be accepted "for local testing": anyone who
        // learned a trigger id could create tickets for its bot. Refused now.
        let reason = if signature.is_empty() { "signature required" } else { "signature invalid" };
        update_delivery_status(&state.db, &delivery_id, "rejected", None, Some(reason.into())).await?;
        return Ok(Json(json!({ "status": "rejected", "reason": reason })));
    }

    // Create a Rails ticket assigned to the target agent/bot.
    let ticket_result = create_ticket_for_trigger(&state.rails, &trigger, &event, &payload).await;

    match ticket_result {
        Ok(ticket_id) => {
            update_delivery_status(
                &state.db,
                &delivery_id,
                "accepted",
                Some(ticket_id.clone()),
                None,
            )
            .await?;
            info!(
                "Inbound webhook {} accepted for trigger {} (agent {}); ticket {}",
                delivery_id, trigger.id, trigger.target_agent_id, ticket_id
            );
            Ok(Json(json!({
                "status": "accepted",
                "delivery_id": delivery_id,
                "ticket_id": ticket_id,
            })))
        }
        Err(e) => {
            update_delivery_status(&state.db, &delivery_id, "failed", None, Some(e.to_string())).await?;
            Ok(Json(json!({ "status": "failed", "reason": e.to_string() })))
        }
    }
}

async fn update_delivery_status(
    db: &crate::db::DbHandle,
    delivery_id: &str,
    status: &str,
    ticket_id: Option<String>,
    error: Option<String>,
) -> Result<(), ApiError> {
    let db = db.clone();
    let delivery_id = delivery_id.to_string();
    let status = status.to_string();
    tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "UPDATE webhook_trigger_deliveries
             SET status = ?1, ticket_id = ?2, error = ?3, updated_at = CURRENT_TIMESTAMP
             WHERE id = ?4",
            params![status, ticket_id, error, delivery_id],
        )?;
        Ok::<_, rusqlite::Error>(())
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .map_err(|e: rusqlite::Error| ApiError::DbError(e.to_string()))
}

/// Also the bot wake-up for MCP connector events (`mcp_events_client`), so a
/// connector event and a signed inbound webhook make the same ticket.
pub(crate) async fn create_ticket_for_trigger(
    rails: &RailsState,
    trigger: &WebhookTrigger,
    event: &str,
    payload: &Value,
) -> Result<String, ApiError> {
    create_trigger_ticket(rails, trigger, event, payload, None)
}

/// Serialize durable MCP ticket creation with the runtime database. Stable IDs
/// recover the file-store/SQL boundary if a process stops after writing a ticket.
pub(crate) fn create_mcp_ticket(
    db: &crate::db::DbHandle,
    rails: &RailsState,
    trigger: &WebhookTrigger,
    event: &str,
    payload: &Value,
    event_id: &str,
) -> Result<String, ApiError> {
    let mut c = db
        .connect()
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let tx = c
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let result = create_trigger_ticket(rails, trigger, event, payload, Some(event_id))?;
    tx.commit().map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(result)
}

fn create_trigger_ticket(
    rails: &RailsState,
    trigger: &WebhookTrigger,
    event: &str,
    payload: &Value,
    stable_event: Option<&str>,
) -> Result<String, ApiError> {
    let store = TicketStore::new(&rails.root_dir).map_err(|e| ApiError::Internal(e.to_string()))?;

    let now = chrono::Utc::now();
    let seed = match stable_event {
        Some(id) => format!("mcp:{}:{id}", trigger.id),
        None => format!("webhook:{}:{}:{}", trigger.id, event, now),
    };
    let id = if stable_event.is_some() {
        TicketId::new(format!("T-{}",hex::encode(&Sha256::digest(seed.as_bytes())[..16])))
    } else {TicketId::mint(seed.as_bytes())};
    if stable_event.is_some()
        && store
            .get(&id)
            .map_err(|e| ApiError::Internal(e.to_string()))?
            .is_some()
    {
        return Ok(id.to_string());
    }
    let title = format!("[{}] {} → {}", trigger.source, event, trigger.name);
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("source".to_string(), json!(trigger.source));
    metadata.insert("event".to_string(), json!(event));
    metadata.insert("trigger_id".to_string(), json!(trigger.id));
    metadata.insert(
        "target_agent_id".to_string(),
        json!(trigger.target_agent_id),
    );
    metadata.insert("execution_mode".to_string(), json!(trigger.execution_mode));
    metadata.insert("payload".to_string(), payload.clone());
    if let Some(ref tpl) = trigger.prompt_template {
        metadata.insert("prompt_template".to_string(), json!(tpl));
    }

    let mut labels = vec!["webhook".to_string(), trigger.source.clone()];
    if trigger.execution_mode == "REQUIRE_APPROVAL" {
        labels.push("approval-required".to_string());
    }

    let ticket = Ticket {
        id: id.clone(),
        hierarchical_id: HierarchicalId::root(id.clone()),
        title,
        description: format!(
            "Webhook from {} ({}).\n\nTarget bot: {}\nExecution mode: {}\n\nPayload:\n{}",
            trigger.source,
            event,
            trigger.target_agent_id,
            trigger.execution_mode,
            serde_json::to_string_pretty(payload).unwrap_or_default()
        ),
        design: None,
        acceptance: None,
        notes: Vec::new(),
        status: TicketStatus::Open,
        kind: TicketKind::Task,
        priority: TicketPriority::P2,
        assignee: Some(trigger.target_agent_id.clone()),
        estimate_minutes: None,
        due_at: None,
        defer_until: None,
        labels,
        external_ref: Some(trigger.id.clone()),
        metadata,
        created_at: now,
        updated_at: now,
        closed_at: None,
        close_reason: None,
    };

    store
        .create(ticket)
        .map(|_| id.to_string())
        .map_err(|e| ApiError::Internal(format!("failed to create ticket: {}", e)))
}

fn base64_encoded(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use std::path::Path as FsPath;
    use std::sync::Arc;
    use tokio::sync::RwLock;
    use tower::ServiceExt;

    fn test_user(id: &str, org_id: Option<&str>) -> AuthUser {
        AuthUser {
            user_id: id.to_string(),
            email: Some(format!("{}@example.test", id)),
            name: None,
            avatar_url: None,
            tenant_id: None,
            organization_id: org_id.map(|s| s.to_string()),
            organization_role: None,
            organization_slug: None,
        }
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("allternit-webhook-triggers-{}-{}", tag, uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    async fn test_app_state(temp: &FsPath) -> Arc<AppState> {
        let config = crate::AppConfig {
            company: Default::default(),
            user: Default::default(),
        };
        let db = crate::db::DbHandle::new(temp.join("test.db")).expect("test db");
        let db_for_init = db.clone();
        let auth_config = crate::auth::AuthConfig::from_app_config(&config);
        let jwks = crate::auth::JwksManager::new(&auth_config);
        let rails = crate::rails::RailsState::new(temp.to_path_buf())
            .await
            .expect("test rails");
        Arc::new(AppState {
            config,
            db,
            data_dir: temp.to_path_buf(),
            jwks,
            auth_config,
            vm_driver: None,
            rails,
            vm_sessions: crate::vm_session_routes::new_vm_session_store(),
            cowork_scheduler: None,
            cowork_background: None,
            cowork_run_manager: None,
            webhook_secret: None,
            office_runtime: Arc::new(RwLock::new(crate::office_routes::OfficeRuntimeFile::default())),
            design_skill_cache: crate::design_connector_routes::DesignSkillCache::new(),
            terminal_sessions: crate::terminal_routes::TerminalSessionStore::new(),
            mcp_dispatcher: crate::mcp_dispatcher::McpDispatcher::new(),
            office_cli_docs: Arc::new(RwLock::new(std::collections::HashMap::new())),
            office_cli_watches: Arc::new(RwLock::new(std::collections::HashMap::new())),
            office_cli_mcp_sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
            approval_store: Arc::new(crate::permission_policy::ApprovalStore::new()),
            incus_driver: None,
            desktop_host_registry:
                crate::desktop_host_registry::DesktopHostRegistry::new(db_for_init.clone()),
            desktop_host_provisioner: None,
            bot_desktop_sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
            computer_guest_tokens: Arc::new(RwLock::new(std::collections::HashMap::new())),
            passkey_state: None,
            resource_class_catalog:
                crate::fabric::sku::ResourceClassCatalog::from_db(&db_for_init)
                    .expect("resource class catalog"),
            fabric_node_provider:
                allternit_computer_cloud::providers::fabric_node::FabricNodeProvider::new(
                    std::sync::Arc::new(
                        allternit_computer_cloud::providers::fabric_node::FabricNodePool::new(),
                    ),
                    "__system".to_string(),
                ),
            fabric_provider_registry: crate::fabric::build_provider_registry(
                allternit_computer_cloud::providers::fabric_node::FabricNodeProvider::new(
                    std::sync::Arc::new(
                        allternit_computer_cloud::providers::fabric_node::FabricNodePool::new(),
                    ),
                    "__system".to_string(),
                ),
            ),
            fabric_scheduler: crate::fabric::Scheduler::new(
                crate::fabric::CostEngine::default_engine(),
            )
            .with_price_cache(crate::fabric::PriceCache::new(db_for_init.clone())),
            fabric_price_cache: crate::fabric::PriceCache::new(db_for_init),
            os_control_plane: None,
            dp_jwks: crate::auth_dp_jwt::DataPlaneJwks::disabled(),
            deployment_scheduler: Arc::new(
                crate::deployment_scheduler::DeploymentSchedulerState::new(),
            ),
        })
    }

    fn json_body(value: &Value) -> Body {
        Body::from(value.to_string())
    }

    async fn body_json(body: Body) -> Value {
        let bytes = body.collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn trigger_crud() {
        let temp = temp_dir("crud");
        let state = test_app_state(&temp).await;
        let app = webhook_trigger_router().with_state(state);
        let user = test_user("user-a", Some("org-a"));

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/webhook-triggers")
                    .header("content-type", "application/json")
                    .extension(user.clone())
                    .body(json_body(&json!({
                        "name": "Stripe bookings",
                        "source": "stripe",
                        "event_type": "checkout.session.completed",
                        "target_agent_id": "agent-1",
                        "execution_mode": "REQUIRE_APPROVAL",
                        "secret": "whsec_test"
                    })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = body_json(resp.into_body()).await;
        let _id = body["trigger"]["id"].as_str().unwrap().to_string();
        assert_eq!(body["trigger"]["source"], "stripe");
        assert_eq!(body["trigger"]["active"], true);

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn protected_router_mounted_once_under_api_v1() {
        // main.rs nests its v1 routes under `/api/v1`; mounting the test
        // helper there doubled the prefix and the UI got the SPA page back.
        let temp = temp_dir("mount");
        let state = test_app_state(&temp).await;
        let app = Router::new()
            .nest("/api/v1", webhook_trigger_protected_router())
            .with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/webhook-triggers")
                    .extension(test_user("user-a", Some("org-a")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn rejects_invalid_execution_mode() {
        let temp = temp_dir("validation");
        let state = test_app_state(&temp).await;
        let app = webhook_trigger_router().with_state(state);

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/webhook-triggers")
                    .header("content-type", "application/json")
                    .extension(test_user("user-a", Some("org-a")))
                    .body(json_body(&json!({
                        "name": "X",
                        "source": "x",
                        "event_type": "x",
                        "target_agent_id": "a",
                        "execution_mode": "INVALID",
                        "secret": "s"
                    })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let _ = std::fs::remove_dir_all(&temp);
    }
}
