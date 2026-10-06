//! `/v1/webhooks`: where a project's events go (scope `webhooks`).
//!
//! An endpoint has a public https `url`, the `events` it wants (or `["*"]`) and
//! a signing `secret` (`whsec_…`, shown once at creation). `signer` picks the
//! signature format: `allternit` (default, the `allternit-signature` header)
//! or `standard_webhooks` (`webhook-id` / `webhook-timestamp` /
//! `webhook-signature`, verifiable with any Standard Webhooks library; the
//! secret is then `whsec_` + base64 of 32 random bytes). The body is the same
//! event object either way. Deliveries are signed
//! and retried as described in [`super::events`]. A key bound to one account
//! can't manage endpoints (they are project-wide).

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json,
};
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;

use super::events::{check_url, EVENT_TYPES, SIGNER_ALLTERNIT, SIGNER_STANDARD};
use super::{build_page, new_id, ApiJson, ApiQuery, Page, PageParams, PlatformCaller, PlatformError, RouteTable};
use crate::ApiState;

const MAX_ENDPOINTS: i64 = 20;

pub fn register(table: RouteTable) -> RouteTable {
    table
        .add("/v1/webhooks", &["GET", "POST"], get(list_webhooks).post(create_webhook))
        .add("/v1/webhooks/:id", &["GET", "DELETE"], get(get_webhook).delete(delete_webhook))
        .add("/v1/webhooks/:id/test", &["POST"], post(test_webhook))
        .add("/v1/webhooks/:id/deliveries", &["GET"], get(list_deliveries))
        .add("/v1/webhooks/:id/deliveries/:delivery_id/redeliver", &["POST"], post(redeliver))
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Webhook {
    pub id: String,
    pub object: String,
    pub url: String,
    pub events: Vec<String>,
    pub description: Option<String>,
    pub signer: String,
    pub created_at: DateTime<Utc>,
}

const COLUMNS: &str = "id, 'webhook_endpoint'::text AS object, url, events, description, signer, created_at";

fn manage(caller: &PlatformCaller) -> Result<(), PlatformError> {
    caller.require("webhooks")?;
    caller.require_unbound()
}

fn validate_events(events: &[String]) -> Result<Vec<String>, PlatformError> {
    if events.is_empty() {
        return Err(PlatformError::invalid_request("missing_events", format!("events must list at least one of {} (or \"*\").", EVENT_TYPES.join(", "))).with_param("events"));
    }
    let mut out = Vec::new();
    for e in events {
        if e != "*" && !EVENT_TYPES.contains(&e.as_str()) {
            return Err(PlatformError::invalid_request("unknown_event", format!("Unknown event '{e}'. Known: {}.", EVENT_TYPES.join(", "))).with_param("events"));
        }
        if !out.contains(e) {
            out.push(e.clone());
        }
    }
    Ok(out)
}

#[derive(Debug, Deserialize)]
struct CreateBody {
    url: String,
    events: Vec<String>,
    description: Option<String>,
    /// `allternit` (default) | `standard_webhooks`.
    signer: Option<String>,
}

async fn create_webhook(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    ApiJson(body): ApiJson<CreateBody>,
) -> Result<(StatusCode, Json<Value>), PlatformError> {
    manage(&caller)?;
    let url = check_url(&body.url).await?;
    let events = validate_events(&body.events)?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM platform_webhooks WHERE project_id = $1 AND deleted_at IS NULL")
        .bind(&caller.project_id)
        .fetch_one(&state.db)
        .await?;
    if count >= MAX_ENDPOINTS {
        return Err(PlatformError::invalid_request("too_many_endpoints", format!("A project can have up to {MAX_ENDPOINTS} webhook endpoints.")));
    }
    let signer = match body.signer.as_deref() {
        None | Some(SIGNER_ALLTERNIT) => SIGNER_ALLTERNIT,
        Some(SIGNER_STANDARD) => SIGNER_STANDARD,
        Some(other) => {
            return Err(PlatformError::invalid_request("invalid_signer", format!("Unknown signer '{other}'. Use \"{SIGNER_ALLTERNIT}\" or \"{SIGNER_STANDARD}\".")).with_param("signer"))
        }
    };
    let secret = if signer == SIGNER_STANDARD {
        let mut raw = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut raw);
        format!("whsec_{}", base64::Engine::encode(&base64::engine::general_purpose::STANDARD, raw))
    } else {
        let mut raw = [0u8; 24];
        rand::thread_rng().fill_bytes(&mut raw);
        format!("whsec_{}", hex::encode(raw))
    };
    let description = body.description.map(|d| d.chars().take(200).collect::<String>());
    let hook = sqlx::query_as::<_, Webhook>(&format!(
        "INSERT INTO platform_webhooks (id, project_id, url, events, secret, description, signer) VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING {COLUMNS}"
    ))
    .bind(new_id("wh_"))
    .bind(&caller.project_id)
    .bind(url.as_str())
    .bind(&events)
    .bind(&secret)
    .bind(&description)
    .bind(signer)
    .fetch_one(&state.db)
    .await?;
    let mut v = serde_json::to_value(&hook).unwrap_or(Value::Null);
    v["secret"] = json!(secret);
    Ok((StatusCode::CREATED, Json(v)))
}

async fn list_webhooks(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    ApiQuery(page): ApiQuery<PageParams>,
) -> Result<Json<Page<Webhook>>, PlatformError> {
    caller.require("webhooks")?;
    let limit = page.limit()?;
    let (after_at, after_id) = match page.cursor()? {
        Some((at, id)) => (Some(at), Some(id)),
        None => (None, None),
    };
    let rows = sqlx::query_as::<_, Webhook>(&format!(
        "SELECT {COLUMNS} FROM platform_webhooks WHERE project_id = $1 AND deleted_at IS NULL \
           AND ($2::timestamptz IS NULL OR (created_at, id) > ($2, $3)) ORDER BY created_at, id LIMIT $4"
    ))
    .bind(&caller.project_id)
    .bind(after_at)
    .bind(after_id)
    .bind(limit + 1)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(build_page(rows, limit, |w| (w.created_at, w.id.clone()))))
}

async fn find(state: &ApiState, caller: &PlatformCaller, id: &str) -> Result<Webhook, PlatformError> {
    sqlx::query_as::<_, Webhook>(&format!("SELECT {COLUMNS} FROM platform_webhooks WHERE id = $1 AND project_id = $2 AND deleted_at IS NULL"))
        .bind(id)
        .bind(&caller.project_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| PlatformError::not_found("webhook_not_found", "No such webhook endpoint."))
}

async fn get_webhook(State(state): State<Arc<ApiState>>, caller: PlatformCaller, Path(id): Path<String>) -> Result<Json<Webhook>, PlatformError> {
    caller.require("webhooks")?;
    Ok(Json(find(&state, &caller, &id).await?))
}

async fn delete_webhook(State(state): State<Arc<ApiState>>, caller: PlatformCaller, Path(id): Path<String>) -> Result<Json<Value>, PlatformError> {
    manage(&caller)?;
    let hook = find(&state, &caller, &id).await?;
    sqlx::query("UPDATE platform_webhooks SET deleted_at = now() WHERE id = $1").bind(&hook.id).execute(&state.db).await?;
    // Nothing further goes to a deleted endpoint.
    sqlx::query("UPDATE platform_webhook_deliveries SET state = 'failed', last_error = 'endpoint deleted' WHERE webhook_id = $1 AND state = 'pending'")
        .bind(&hook.id)
        .execute(&state.db)
        .await?;
    Ok(Json(json!({ "id": hook.id, "object": "webhook_endpoint", "deleted": true })))
}

/// Queue a `webhook.test` event for this endpoint only.
async fn test_webhook(State(state): State<Arc<ApiState>>, caller: PlatformCaller, Path(id): Path<String>) -> Result<(StatusCode, Json<Value>), PlatformError> {
    manage(&caller)?;
    let hook = find(&state, &caller, &id).await?;
    // Only to this endpoint, whatever it subscribes to (so not through emit_event's fan-out).
    let event = new_id("evt_");
    sqlx::query("INSERT INTO platform_events (id, project_id, type, data) VALUES ($1, $2, 'webhook.test', $3)")
        .bind(&event)
        .bind(&caller.project_id)
        .bind(json!({ "webhook_id": hook.id }))
        .execute(&state.db)
        .await?;
    sqlx::query(
        "INSERT INTO platform_webhook_deliveries (id, webhook_id, event_id) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind(new_id("whd_"))
    .bind(&hook.id)
    .bind(&event)
    .execute(&state.db)
    .await?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "event_id": event, "webhook_id": hook.id, "queued": true }))))
}

#[derive(Debug, Serialize, FromRow)]
struct Delivery {
    id: String,
    object: String,
    event_id: String,
    event_type: String,
    state: String,
    attempts: i32,
    last_status: Option<i32>,
    last_error: Option<String>,
    next_attempt_at: DateTime<Utc>,
    delivered_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

async fn list_deliveries(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    Path(id): Path<String>,
    ApiQuery(page): ApiQuery<PageParams>,
) -> Result<Json<Page<Delivery>>, PlatformError> {
    caller.require("webhooks")?;
    let hook = find(&state, &caller, &id).await?;
    let limit = page.limit()?;
    let (after_at, after_id) = match page.cursor()? {
        Some((at, id)) => (Some(at), Some(id)),
        None => (None, None),
    };
    // Newest first reads best in a delivery log, but cursors are ascending everywhere else;
    // keep one rule: ascending by creation.
    let rows = sqlx::query_as::<_, Delivery>(
        "SELECT d.id, 'webhook_delivery'::text AS object, d.event_id, e.type AS event_type, d.state, d.attempts, d.last_status, d.last_error, \
                d.next_attempt_at, d.delivered_at, d.created_at \
         FROM platform_webhook_deliveries d JOIN platform_events e ON e.id = d.event_id \
         WHERE d.webhook_id = $1 AND ($2::timestamptz IS NULL OR (d.created_at, d.id) > ($2, $3)) ORDER BY d.created_at, d.id LIMIT $4",
    )
    .bind(&hook.id)
    .bind(after_at)
    .bind(after_id)
    .bind(limit + 1)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(build_page(rows, limit, |d| (d.created_at, d.id.clone()))))
}

/// Try a delivery again now (also revives a `failed` one for another 24 h).
async fn redeliver(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    Path((id, delivery_id)): Path<(String, String)>,
) -> Result<(StatusCode, Json<Value>), PlatformError> {
    manage(&caller)?;
    let hook = find(&state, &caller, &id).await?;
    let done = sqlx::query(
        "UPDATE platform_webhook_deliveries SET state = 'pending', next_attempt_at = now(), created_at = now() WHERE id = $1 AND webhook_id = $2",
    )
    .bind(&delivery_id)
    .bind(&hook.id)
    .execute(&state.db)
    .await?;
    if done.rows_affected() == 0 {
        return Err(PlatformError::not_found("delivery_not_found", "No such delivery for this endpoint."));
    }
    Ok((StatusCode::ACCEPTED, Json(json!({ "id": delivery_id, "queued": true }))))
}
