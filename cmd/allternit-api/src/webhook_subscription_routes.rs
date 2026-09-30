//! Webhook subscription management (`/beta/webhooks`) and event delivery.
//!
//! Subscriptions are scoped to an organization. When a subscribed event
//! occurs, the API POSTs a signed JSON payload to the subscription URL and
//! records the delivery attempt in `webhook_deliveries` for observability and
//! retries.

use axum::{
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use hmac::{Hmac, Mac};
use rand::Rng;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

use crate::{auth::AuthUser, error::ApiError, AppState};

type HmacSha256 = Hmac<Sha256>;

/// Validated registry of webhook event types. Subscription create/update
/// rejects any event not listed here (400). The wildcard [`events::WILDCARD`]
/// matches every event at delivery time.
pub mod events {
    /// An agent row was created.
    pub const AGENT_CREATED: &str = "agent.created";
    /// An agent row was updated.
    pub const AGENT_UPDATED: &str = "agent.updated";
    /// An agent row was archived.
    pub const AGENT_ARCHIVED: &str = "agent.archived";
    /// A managed session was created.
    pub const SESSION_CREATED: &str = "session.created";
    /// An event was appended to a managed session.
    pub const SESSION_EVENT_CREATED: &str = "session.event_created";
    /// A managed session was archived.
    pub const SESSION_ARCHIVED: &str = "session.archived";
    /// A session budget was exceeded and usage was rejected.
    pub const SESSION_OVER_BUDGET: &str = "session.over_budget";
    /// A deployment run was created.
    pub const DEPLOYMENT_RUN_CREATED: &str = "deployment.run_created";
    /// A deployment run reached a terminal status.
    pub const DEPLOYMENT_RUN_UPDATED: &str = "deployment.run_updated";
    /// Organization credits were purchased.
    pub const BILLING_CREDIT_PURCHASE: &str = "billing.credit_purchase";
    /// An LLM gateway virtual key was created.
    pub const KEY_CREATED: &str = "key.created";
    /// An LLM gateway virtual key was revoked.
    pub const KEY_REVOKED: &str = "key.revoked";

    /// Every registered event type.
    pub const ALL: &[&str] = &[
        AGENT_CREATED,
        AGENT_UPDATED,
        AGENT_ARCHIVED,
        SESSION_CREATED,
        SESSION_EVENT_CREATED,
        SESSION_ARCHIVED,
        SESSION_OVER_BUDGET,
        DEPLOYMENT_RUN_CREATED,
        DEPLOYMENT_RUN_UPDATED,
        BILLING_CREDIT_PURCHASE,
        KEY_CREATED,
        KEY_REVOKED,
    ];

    /// Subscription wildcard matching every event type.
    pub const WILDCARD: &str = "*";

    pub fn is_registered(event: &str) -> bool {
        ALL.contains(&event)
    }
}

/// Event emitted when a new event is appended to a managed session.
pub const SESSION_EVENT: &str = events::SESSION_EVENT_CREATED;
/// Event emitted when a deployment run is updated to a terminal status.
pub const DEPLOYMENT_RUN_EVENT: &str = events::DEPLOYMENT_RUN_UPDATED;

/// Total delivery attempts per event: the initial attempt plus retries.
pub const MAX_ATTEMPTS: u32 = 3;
/// Env var overriding the retry backoff base, in seconds.
pub const RETRY_BASE_SECS_ENV: &str = "ALLTERNIT_WEBHOOK_RETRY_BASE_SECS";
/// Default retry backoff base when the env var is unset or unparseable.
/// Retry N sleeps `base * 4^(N-1)` ± jitter (base 5s → ~5s, then ~20s).
pub const DEFAULT_RETRY_BASE_SECS: u64 = 5;

/// Backoff schedule for webhook delivery retries.
#[derive(Debug, Clone, Copy)]
struct RetryPolicy {
    base: Duration,
    max_attempts: u32,
}

/// Parse a backoff-base value. Unparseable or absurd values (0 or huge)
/// fall back to [`DEFAULT_RETRY_BASE_SECS`].
fn parse_base_secs(raw: Option<&str>) -> u64 {
    raw.and_then(|s| s.parse::<u64>().ok())
        .filter(|v| (1..=3600).contains(v))
        .unwrap_or(DEFAULT_RETRY_BASE_SECS)
}

impl RetryPolicy {
    fn from_env() -> Self {
        Self {
            base: Duration::from_secs(parse_base_secs(
                std::env::var(RETRY_BASE_SECS_ENV).ok().as_deref(),
            )),
            max_attempts: MAX_ATTEMPTS,
        }
    }

    /// Delay before retry number `retry` (1-based): `base * 4^(retry-1)`
    /// scaled by ±25% jitter.
    fn backoff(&self, retry: u32) -> Duration {
        let multiplier = 4u64.pow(retry.saturating_sub(1).min(4));
        let nominal = self.base.saturating_mul(multiplier as u32);
        let jitter_pct = rand::thread_rng().gen_range(75..=125u64);
        nominal.mul_f64(jitter_pct as f64 / 100.0)
    }
}

/// How a single delivery attempt ended.
#[derive(Debug, PartialEq, Eq)]
enum AttemptOutcome {
    /// 2xx — delivered, stop.
    Delivered,
    /// 4xx — the endpoint rejected the payload, retrying cannot help.
    PermanentFailure,
    /// Connect error, timeout, or 5xx — worth retrying.
    TransientFailure,
}

fn classify_attempt(response_status: Option<i32>) -> AttemptOutcome {
    match response_status {
        Some(code) if (200..300).contains(&code) => AttemptOutcome::Delivered,
        Some(code) if (400..500).contains(&code) => AttemptOutcome::PermanentFailure,
        _ => AttemptOutcome::TransientFailure,
    }
}

pub fn webhook_subscription_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/beta/webhooks", get(list_subscriptions).post(create_subscription))
        .route(
            "/beta/webhooks/:id",
            get(get_subscription)
                .patch(update_subscription)
                .delete(delete_subscription),
        )
        .route("/beta/webhooks/:id/deliveries", get(list_deliveries))
}

#[derive(Debug, Deserialize)]
struct CreateSubscriptionBody {
    url: String,
    events: Vec<String>,
    secret: String,
}

#[derive(Debug, Deserialize, Default)]
struct UpdateSubscriptionBody {
    url: Option<String>,
    events: Option<Vec<String>>,
    secret: Option<String>,
    active: Option<bool>,
}

#[derive(Debug, Serialize)]
struct SubscriptionRow {
    id: String,
    org_id: String,
    url: String,
    events: Vec<String>,
    active: bool,
    created_at: String,
    updated_at: String,
}

fn read_subscription(row: &rusqlite::Row<'_>) -> rusqlite::Result<SubscriptionRow> {
    let events_json: String = row.get(3)?;
    Ok(SubscriptionRow {
        id: row.get(0)?,
        org_id: row.get(1)?,
        url: row.get(2)?,
        events: serde_json::from_str(&events_json).unwrap_or_default(),
        active: row.get::<_, i32>(4)? != 0,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

fn validate_subscription_body(url: &str, events: &[String], secret: &str) -> Result<(), ApiError> {
    if url.is_empty() || !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(ApiError::BadRequest("url must be an http/https URL".into()));
    }
    check_webhook_target(url, &internal_webhook_endpoints()).map_err(ApiError::BadRequest)?;
    validate_event_list(events)?;
    if secret.is_empty() {
        return Err(ApiError::BadRequest("secret is required".into()));
    }
    Ok(())
}

/// Validate a subscription event list against the registry. The wildcard `*`
/// is accepted but must be the only entry.
fn validate_event_list(events: &[String]) -> Result<(), ApiError> {
    if events.is_empty() || events.iter().any(|e| e.is_empty()) {
        return Err(ApiError::BadRequest("events must be a non-empty array of strings".into()));
    }
    for event in events {
        if event == events::WILDCARD {
            if events.len() > 1 {
                return Err(ApiError::BadRequest(
                    "wildcard \"*\" must be the only event".into(),
                ));
            }
            continue;
        }
        if !events::is_registered(event) {
            return Err(ApiError::BadRequest(format!("unknown event type: {event}")));
        }
    }
    Ok(())
}

fn require_org(user: &AuthUser) -> Result<String, ApiError> {
    user.organization_id
        .clone()
        .ok_or_else(|| ApiError::BadRequest("organization required".into()))
}

async fn create_subscription(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<CreateSubscriptionBody>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    validate_subscription_body(&body.url, &body.events, &body.secret)?;
    let org_id = require_org(&user)?;
    let id = uuid::Uuid::new_v4().to_string();
    let events_json =
        serde_json::to_string(&body.events).map_err(|e| ApiError::Internal(e.to_string()))?;

    let db = state.db.clone();
    let lookup_id = id.clone();
    let subscription = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "INSERT INTO webhook_subscriptions (id, org_id, url, events, secret, active)
             VALUES (?1, ?2, ?3, ?4, ?5, 1)",
            params![id, org_id, body.url, events_json, body.secret],
        )?;
        conn.query_row(
            "SELECT id, org_id, url, events, active, created_at, updated_at
             FROM webhook_subscriptions WHERE id = ?1",
            params![lookup_id],
            read_subscription,
        )
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .map_err(|e: rusqlite::Error| ApiError::DbError(e.to_string()))?;

    info!("Created webhook subscription {} for org {}", subscription.id, subscription.org_id);
    Ok((StatusCode::CREATED, Json(json!({"subscription": subscription}))))
}

async fn list_subscriptions(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result<Json<Value>, ApiError> {
    let org_id = require_org(&user)?;
    let db = state.db.clone();
    let rows = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, org_id, url, events, active, created_at, updated_at
             FROM webhook_subscriptions WHERE org_id = ?1 ORDER BY created_at DESC",
        )?;
        let rows = stmt
            .query_map(params![org_id], read_subscription)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok::<_, rusqlite::Error>(rows)
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .map_err(|e: rusqlite::Error| ApiError::DbError(e.to_string()))?;
    Ok(Json(json!({"subscriptions": rows, "total": rows.len()})))
}

async fn get_subscription(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let org_id = require_org(&user)?;
    let db = state.db.clone();
    let row = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.query_row(
            "SELECT id, org_id, url, events, active, created_at, updated_at
             FROM webhook_subscriptions WHERE id = ?1 AND org_id = ?2",
            params![id, org_id],
            read_subscription,
        )
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .map_err(|e: rusqlite::Error| {
        if matches!(e, rusqlite::Error::QueryReturnedNoRows) {
            ApiError::NotFound("subscription not found".into())
        } else {
            ApiError::DbError(e.to_string())
        }
    })?;
    Ok(Json(json!({"subscription": row})))
}

async fn update_subscription(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<UpdateSubscriptionBody>,
) -> Result<Json<Value>, ApiError> {
    let org_id = require_org(&user)?;

    if let Some(ref url) = body.url {
        if url.is_empty() || (!url.starts_with("http://") && !url.starts_with("https://")) {
            return Err(ApiError::BadRequest("url must be an http/https URL".into()));
        }
        check_webhook_target(url, &internal_webhook_endpoints()).map_err(ApiError::BadRequest)?;
    }
    if let Some(ref events) = body.events {
        validate_event_list(events)?;
    }
    if let Some(ref secret) = body.secret {
        if secret.is_empty() {
            return Err(ApiError::BadRequest("secret is required".into()));
        }
    }

    let has_fields = body.url.is_some()
        || body.events.is_some()
        || body.secret.is_some()
        || body.active.is_some();
    if !has_fields {
        return Err(ApiError::BadRequest("no fields provided".into()));
    }

    let db = state.db.clone();
    let lookup_id = id.clone();
    let affected = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut sets: Vec<&'static str> = Vec::new();
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(url) = body.url {
            sets.push("url = ?");
            params.push(Box::new(url));
        }
        if let Some(events) = body.events {
            sets.push("events = ?");
            params.push(Box::new(serde_json::to_string(&events).map_err(|e| {
                rusqlite::Error::ToSqlConversionFailure(Box::new(e))
            })?));
        }
        if let Some(secret) = body.secret {
            sets.push("secret = ?");
            params.push(Box::new(secret));
        }
        if let Some(active) = body.active {
            sets.push("active = ?");
            params.push(Box::new(if active { 1 } else { 0 }));
        }

        sets.push("updated_at = CURRENT_TIMESTAMP");

        let sql = format!(
            "UPDATE webhook_subscriptions SET {} WHERE id = ? AND org_id = ?",
            sets.join(", ")
        );
        let mut stmt = conn.prepare(&sql)?;
        for (i, param) in params.iter().enumerate() {
            stmt.raw_bind_parameter(i + 1, param.as_ref())?;
        }
        stmt.raw_bind_parameter(params.len() + 1, &id)?;
        stmt.raw_bind_parameter(params.len() + 2, &org_id)?;
        stmt.raw_execute()
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .map_err(|e: rusqlite::Error| ApiError::DbError(e.to_string()))?;

    if affected == 0 {
        return Err(ApiError::NotFound("subscription not found".into()));
    }

    let db = state.db.clone();
    let row = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.query_row(
            "SELECT id, org_id, url, events, active, created_at, updated_at
             FROM webhook_subscriptions WHERE id = ?1",
            params![lookup_id],
            read_subscription,
        )
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .map_err(|e: rusqlite::Error| ApiError::DbError(e.to_string()))?;
    Ok(Json(json!({"subscription": row})))
}

async fn delete_subscription(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let org_id = require_org(&user)?;
    let db = state.db.clone();
    let affected = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "DELETE FROM webhook_subscriptions WHERE id = ?1 AND org_id = ?2",
            params![id, org_id],
        )
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .map_err(|e: rusqlite::Error| ApiError::DbError(e.to_string()))?;

    if affected == 0 {
        return Err(ApiError::NotFound("subscription not found".into()));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /beta/webhooks/:id/deliveries?limit=&status=` — delivery log for one
/// subscription, newest first. Same auth/scope as the subscription CRUD: the
/// subscription must belong to the caller's organization. Pagination follows
/// the repo convention (`page` 1-based, `limit` default 50 max 100); `status`
/// filters to `pending` | `delivered` | `failed`.
#[derive(Debug, Deserialize)]
struct DeliveriesQuery {
    page: Option<u32>,
    limit: Option<u32>,
    status: Option<String>,
}

#[derive(Debug, Serialize)]
struct DeliveryRow {
    id: String,
    event_type: String,
    status: String,
    attempts: i64,
    response_status: Option<i32>,
    response_body: Option<String>,
    error: Option<String>,
    created_at: String,
    updated_at: String,
}

async fn list_deliveries(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Query(query): Query<DeliveriesQuery>,
) -> Result<Json<Value>, ApiError> {
    let org_id = require_org(&user)?;
    if let Some(ref status) = query.status {
        if !matches!(status.as_str(), "pending" | "delivered" | "failed") {
            return Err(ApiError::BadRequest(format!(
                "unsupported status filter: {status} (pending | delivered | failed)"
            )));
        }
    }
    let page = query.page.unwrap_or(1).max(1);
    let limit = query.limit.unwrap_or(50).clamp(1, 100);
    let offset = (page - 1) * limit;

    let db = state.db.clone();
    let status_filter = query.status.clone();
    let (rows, total) = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        // Same org scope as the CRUD handlers: an id that is not in the
        // caller's organization is indistinguishable from a missing one.
        let owned: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM webhook_subscriptions WHERE id = ?1 AND org_id = ?2)",
            params![id, org_id],
            |row| row.get(0),
        )?;
        if !owned {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }

        let (rows, total) = if let Some(ref status) = status_filter {
            let mut stmt = conn.prepare(
                "SELECT id, event, status, attempts, response_status, response_body, error,
                        created_at, updated_at
                 FROM webhook_deliveries
                 WHERE subscription_id = ?1 AND status = ?2
                 ORDER BY created_at DESC, id DESC
                 LIMIT ?3 OFFSET ?4",
            )?;
            let rows = stmt
                .query_map(params![id, status, limit, offset], read_delivery)?
                .collect::<Result<Vec<_>, _>>()?;
            let total: i64 = conn.query_row(
                "SELECT COUNT(*) FROM webhook_deliveries
                 WHERE subscription_id = ?1 AND status = ?2",
                params![id, status],
                |row| row.get(0),
            )?;
            (rows, total)
        } else {
            let mut stmt = conn.prepare(
                "SELECT id, event, status, attempts, response_status, response_body, error,
                        created_at, updated_at
                 FROM webhook_deliveries
                 WHERE subscription_id = ?1
                 ORDER BY created_at DESC, id DESC
                 LIMIT ?2 OFFSET ?3",
            )?;
            let rows = stmt
                .query_map(params![id, limit, offset], read_delivery)?
                .collect::<Result<Vec<_>, _>>()?;
            let total: i64 = conn.query_row(
                "SELECT COUNT(*) FROM webhook_deliveries WHERE subscription_id = ?1",
                params![id],
                |row| row.get(0),
            )?;
            (rows, total)
        };
        Ok::<_, rusqlite::Error>((rows, total))
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .map_err(|e: rusqlite::Error| {
        if matches!(e, rusqlite::Error::QueryReturnedNoRows) {
            ApiError::NotFound("subscription not found".into())
        } else {
            ApiError::DbError(e.to_string())
        }
    })?;

    Ok(Json(json!({
        "deliveries": rows,
        "total": total,
        "page": page,
        "limit": limit,
    })))
}

fn read_delivery(row: &rusqlite::Row<'_>) -> rusqlite::Result<DeliveryRow> {
    const MAX_RESPONSE_BODY_CHARS: usize = 500;
    let response_body: Option<String> = row.get(5)?;
    let response_body = response_body.map(|body| {
        let truncated: String = body.chars().take(MAX_RESPONSE_BODY_CHARS).collect();
        if body.chars().count() > MAX_RESPONSE_BODY_CHARS {
            format!("{truncated}…")
        } else {
            truncated
        }
    });
    Ok(DeliveryRow {
        id: row.get(0)?,
        event_type: row.get(1)?,
        status: row.get(2)?,
        attempts: row.get(3)?,
        response_status: row.get(4)?,
        response_body,
        error: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

/// Deliver a session event to all active subscriptions in the organization.
pub async fn deliver_session_event(
    state: Arc<AppState>,
    org_id: Option<&str>,
    session_id: &str,
    event: &Value,
) {
    let Some(org_id) = org_id else { return };
    let payload = json!({
        "event": SESSION_EVENT,
        "session_id": session_id,
        "event_data": event,
    });
    deliver_event(state, org_id, SESSION_EVENT, payload).await;
}

/// Deliver a deployment run update to all active subscriptions in the organization.
pub async fn deliver_deployment_run_update(
    state: Arc<AppState>,
    org_id: Option<&str>,
    deployment_id: &str,
    run: &Value,
) {
    let Some(org_id) = org_id else { return };
    let payload = json!({
        "event": DEPLOYMENT_RUN_EVENT,
        "deployment_id": deployment_id,
        "run": run,
    });
    deliver_event(state, org_id, DEPLOYMENT_RUN_EVENT, payload).await;
}

/// Deliver any registered event type with the standard envelope
/// `{id, type, created_at, data}` to all matching active subscriptions.
///
/// Fire-and-forget by design: every failure path logs a warning and returns
/// without error, so callers can await it inline after a successful request
/// without ever failing that request on webhook trouble. Unknown event types
/// are rejected with a warning (the registry constant should be used).
pub async fn deliver_registered_event(
    state: Arc<AppState>,
    org_id: Option<&str>,
    event_type: &str,
    data: Value,
) {
    let Some(org_id) = org_id else { return };
    if !events::is_registered(event_type) {
        warn!("refusing to deliver unregistered webhook event type: {event_type}");
        return;
    }
    let payload = json!({
        "id": uuid::Uuid::new_v4().to_string(),
        "type": event_type,
        "created_at": chrono::Utc::now().to_rfc3339(),
        "data": data,
    });
    deliver_event(state, org_id, event_type, payload).await;
}

async fn deliver_event(state: Arc<AppState>, org_id: &str, event: &str, payload: Value) {
    let db = state.db.clone();
    let org_id = org_id.to_string();
    let event = event.to_string();

    let subscriptions = match tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, url, secret, events FROM webhook_subscriptions
             WHERE org_id = ?1 AND active = 1",
        )?;
        let rows = stmt
            .query_map(params![org_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok::<_, rusqlite::Error>(rows)
    })
    .await
    {
        Ok(Ok(rows)) => rows,
        Ok(Err(e)) => {
            warn!("failed to load webhook subscriptions: {}", e);
            return;
        }
        Err(e) => {
            warn!("webhook subscription lookup panicked: {}", e);
            return;
        }
    };

    for (sub_id, url, secret, events_json) in subscriptions {
        let events: Vec<String> = serde_json::from_str(&events_json).unwrap_or_default();
        if !events.iter().any(|e| e == &event || e == "*") {
            continue;
        }

        let delivery_id = uuid::Uuid::new_v4().to_string();
        let payload_str = payload.to_string();
        let event_for_db = event.clone();
        let db = state.db.clone();
        let delivery_id_for_spawn = delivery_id.clone();
        if let Err(e) = tokio::task::spawn_blocking(move || {
            let conn = db.connect()?;
            conn.execute(
                "INSERT INTO webhook_deliveries (id, subscription_id, event, payload, status, attempts)
                 VALUES (?1, ?2, ?3, ?4, 'pending', 0)",
                params![delivery_id_for_spawn, sub_id, event_for_db, payload_str],
            )?;
            Ok::<_, rusqlite::Error>(())
        })
        .await
        {
            warn!("failed to record pending webhook delivery: {}", e);
            continue;
        }

        let state = state.clone();
        let payload = payload.clone();
        tokio::spawn(async move {
            attempt_delivery(state, delivery_id, url, secret, payload, RetryPolicy::from_env())
                .await;
        });
    }
}

/// Env var listing internal endpoints (`host:port`, comma separated) that
/// webhook deliveries may reach even though they are not public. Operator
/// configured only; a subscription URL can never add to it.
const INTERNAL_WEBHOOK_ENDPOINTS_ENV: &str = "ALLTERNIT_WEBHOOK_INTERNAL_ENDPOINTS";

#[cfg(test)]
static TEST_INTERNAL_ENDPOINTS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

fn internal_webhook_endpoints() -> Vec<String> {
    let mut out: Vec<String> = std::env::var(INTERNAL_WEBHOOK_ENDPOINTS_ENV)
        .map(|v| {
            v.split(',')
                .map(|e| e.trim().to_ascii_lowercase())
                .filter(|e| !e.is_empty())
                .collect()
        })
        .unwrap_or_default();
    #[cfg(test)]
    out.extend(TEST_INTERNAL_ENDPOINTS.lock().unwrap().iter().cloned());
    out
}

fn is_allowed_internal(url: &reqwest::Url, allow: &[String]) -> bool {
    let Some(host) = url.host_str() else { return false };
    let Some(port) = url.port_or_known_default() else { return false };
    let key = format!("{}:{}", host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase(), port);
    allow.iter().any(|a| a == &key)
}

/// Destination policy for a webhook URL: http(s) only, and no literal
/// loopback / private / link-local (cloud metadata) host unless it is an
/// explicitly configured internal endpoint. Domain names are vetted at
/// connect time by [`allternit_commrails::egress::PublicOnlyResolver`].
fn check_webhook_target(raw: &str, allow: &[String]) -> Result<(), String> {
    let url = reqwest::Url::parse(raw).map_err(|_| "url is not a valid URL".to_string())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("url must be an http/https URL".into());
    }
    let host = url.host_str().ok_or_else(|| "url has no host".to_string())?;
    if is_allowed_internal(&url, allow) {
        return Ok(());
    }
    if allternit_commrails::egress::host_is_forbidden_literal(host) {
        return Err("url points to a local or private address".into());
    }
    Ok(())
}

/// Client for one delivery. Redirects are off (a hop would leave the vetted
/// destination). Public destinations dial only addresses `PublicOnlyResolver`
/// approved; a configured internal endpoint uses the default resolver.
fn webhook_client(url: &str, allow: &[String]) -> Result<reqwest::Client, String> {
    let parsed = reqwest::Url::parse(url).map_err(|_| "url is not a valid URL".to_string())?;
    let mut b = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none());
    if !is_allowed_internal(&parsed, allow) {
        b = b.dns_resolver(Arc::new(allternit_commrails::egress::PublicOnlyResolver));
    }
    b.build().map_err(|e| e.to_string())
}

/// Perform up to `policy.max_attempts` HTTP attempts for one delivery row,
/// sleeping exponential-backoff + jitter between transient failures. The
/// existing `webhook_deliveries` row is updated in place: `attempts` counts
/// completed attempts, `status` is `pending` while retries remain, then
/// `delivered` or `failed`.
///
/// Shutdown semantics: retries live in spawned tokio tasks, so a process stop
/// mid-retry abandons them — the row is left `pending` and is NOT resumed on
/// restart (the event is gone; redelivery would need a durable queue).
async fn attempt_delivery(
    state: Arc<AppState>,
    delivery_id: String,
    url: String,
    secret: String,
    payload: Value,
    policy: RetryPolicy,
) {
    let body_bytes = match serde_json::to_vec(&payload) {
        Ok(bytes) => bytes,
        Err(e) => {
            warn!("failed to serialize webhook payload: {}", e);
            record_delivery_result(
                state,
                delivery_id,
                1,
                "failed",
                None,
                None,
                Some(e.to_string()),
            )
            .await;
            return;
        }
    };
    let signature = sign_payload(&secret, &body_bytes);

    let allow = internal_webhook_endpoints();
    let client = match check_webhook_target(&url, &allow).and_then(|_| webhook_client(&url, &allow)) {
        Ok(c) => c,
        Err(e) => {
            warn!("webhook delivery refused for {delivery_id}: {e}");
            record_delivery_result(
                state,
                delivery_id,
                1,
                "failed",
                None,
                None,
                Some(format!("blocked destination: {e}")),
            )
            .await;
            return;
        }
    };

    for attempt in 1..=policy.max_attempts {
        let result = client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("X-Allternit-Signature", &signature)
            .body(body_bytes.clone())
            .send()
            .await;

        let (response_status, response_body, error) = match result {
            Ok(resp) => {
                let code = resp.status().as_u16() as i32;
                let body_text = resp.text().await.unwrap_or_default();
                (Some(code), Some(body_text), None)
            }
            Err(e) => (None, None, Some(e.to_string())),
        };

        let outcome = classify_attempt(response_status);
        let terminal = outcome != AttemptOutcome::TransientFailure
            || attempt >= policy.max_attempts;
        let status = match outcome {
            AttemptOutcome::Delivered => "delivered",
            _ if terminal => "failed",
            _ => "pending",
        };
        record_delivery_result(
            state.clone(),
            delivery_id.clone(),
            attempt,
            status,
            response_status,
            response_body,
            error,
        )
        .await;

        if terminal {
            return;
        }
        let delay = policy.backoff(attempt);
        info!(
            delivery_id = %delivery_id,
            attempt,
            "webhook delivery transient failure; retrying in {delay:?}"
        );
        tokio::time::sleep(delay).await;
    }
}

/// Persist the outcome of one attempt onto the delivery row. Response bodies
/// are truncated to [`MAX_RESPONSE_BODY_CHARS`] so a chatty endpoint cannot
/// bloat the table.
async fn record_delivery_result(
    state: Arc<AppState>,
    delivery_id: String,
    attempts: u32,
    status: &str,
    response_status: Option<i32>,
    response_body: Option<String>,
    error: Option<String>,
) {
    const MAX_RESPONSE_BODY_CHARS: usize = 500;
    let db = state.db.clone();
    let status = status.to_string();
    let response_body = response_body.map(|body| {
        let truncated: String = body.chars().take(MAX_RESPONSE_BODY_CHARS).collect();
        if body.chars().count() > MAX_RESPONSE_BODY_CHARS {
            format!("{truncated}…")
        } else {
            truncated
        }
    });
    if let Err(e) = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "UPDATE webhook_deliveries
             SET status = ?1, response_status = ?2, response_body = ?3, error = ?4,
                 attempts = ?5, updated_at = CURRENT_TIMESTAMP
             WHERE id = ?6",
            params![status, response_status, response_body, error, attempts, delivery_id],
        )?;
        Ok::<_, rusqlite::Error>(())
    })
    .await
    {
        warn!("failed to record webhook delivery result: {}", e);
    }
}

fn sign_payload(secret: &str, body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(body);
    hex::encode(mac.finalize().into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use std::collections::HashMap;
    use std::path::Path as FsPath;
    use std::sync::{Arc, Mutex};
    use tokio::sync::RwLock;
    use tower::ServiceExt;

    #[derive(Clone, Debug)]
    pub(crate) struct ReceivedRequest {
        pub(crate) signature: Option<String>,
        pub(crate) body: Value,
    }

    pub(crate) fn org_user(id: &str, org_id: &str) -> AuthUser {
        AuthUser {
            user_id: id.to_string(),
            email: Some(format!("{}@example.test", id)),
            name: None,
            avatar_url: None,
            tenant_id: None,
            organization_id: Some(org_id.to_string()),
            organization_role: None,
            organization_slug: None,
        }
    }

    /// Insert a subscription row directly (most emitter tests don't need the
    /// HTTP create path exercised — that's covered by the CRUD tests).
    pub(crate) fn insert_subscription(
        state: &Arc<AppState>,
        org_id: &str,
        url: &str,
        events: &[&str],
        secret: &str,
    ) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let conn = state.db.connect().unwrap();
        conn.execute(
            "INSERT INTO webhook_subscriptions (id, org_id, url, events, secret, active)
             VALUES (?1, ?2, ?3, ?4, ?5, 1)",
            params![
                id,
                org_id,
                url,
                serde_json::to_string(&events).unwrap(),
                secret
            ],
        )
        .unwrap();
        id
    }

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
        let dir = std::env::temp_dir().join(format!(
            "allternit-webhooks-{}-{}",
            tag,
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    async fn test_app_state(temp: &FsPath) -> Arc<AppState> {
        let config = crate::AppConfig {
            company: crate::config::CompanyConfig {
                internal_service_token: Some("webhook-test-internal-token".to_string()),
                ..Default::default()
            },
            user: Default::default(),
        };
        let db = crate::db::DbHandle::new(temp.join("test.db")).expect("test db");
        let auth_config = crate::auth::AuthConfig::from_app_config(&config);
        let jwks = crate::auth::JwksManager::new(&auth_config);
        let rails = crate::rails::RailsState::new(temp.join("rails"))
            .await
            .expect("test rails");
        let desktop_host_registry = crate::desktop_host_registry::DesktopHostRegistry::new(db.clone());
        Arc::new(AppState {
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
            office_runtime: Arc::new(RwLock::new(
                crate::office_routes::OfficeRuntimeFile::default(),
            )),
            design_skill_cache: crate::design_connector_routes::DesignSkillCache::new(),
            #[cfg(unix)]
            terminal_sessions: crate::terminal_routes::TerminalSessionStore::new(),
            mcp_dispatcher: crate::mcp_dispatcher::McpDispatcher::new(),
            office_cli_docs: Arc::new(RwLock::new(HashMap::new())),
            office_cli_watches: Arc::new(RwLock::new(HashMap::new())),
            office_cli_mcp_sessions: Arc::new(RwLock::new(HashMap::new())),
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

    pub(crate) async fn start_receiver(
        received: Arc<Mutex<Vec<ReceivedRequest>>>,
    ) -> String {
        use axum::{extract::State, http::HeaderMap, routing::post, Router};

        async fn capture(
            State(received): State<Arc<Mutex<Vec<ReceivedRequest>>>>,
            headers: HeaderMap,
            body: Body,
        ) -> StatusCode {
            let bytes = body.collect().await.unwrap().to_bytes();
            let value = serde_json::from_slice(&bytes).unwrap_or(json!({}));
            received.lock().unwrap().push(ReceivedRequest {
                signature: headers
                    .get("X-Allternit-Signature")
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string()),
                body: value,
            });
            StatusCode::OK
        }

        let app = Router::new()
            .route("/webhook", post(capture))
            .with_state(received);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        TEST_INTERNAL_ENDPOINTS.lock().unwrap().push(format!("127.0.0.1:{port}"));
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://127.0.0.1:{}/webhook", port)
    }

    fn verify_signature(secret: &str, body: &Value, signature: &str) -> bool {
        let bytes = serde_json::to_vec(body).unwrap();
        sign_payload(secret, &bytes) == signature
    }

    #[tokio::test]
    async fn subscription_crud() {
        let temp = temp_dir("crud");
        let state = test_app_state(&temp).await;
        let app = webhook_subscription_router().with_state(state);
        let org = "org-crud";

        // Create
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/beta/webhooks")
                    .header("content-type", "application/json")
                    .extension(test_user("user-a", Some(org)))
                    .body(json_body(&json!({
                        "url": "https://example.test/hook",
                        "events": [SESSION_EVENT],
                        "secret": "s3cret"
                    })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = body_json(resp.into_body()).await;
        let id = body["subscription"]["id"].as_str().unwrap().to_string();
        assert_eq!(body["subscription"]["url"], "https://example.test/hook");
        assert_eq!(body["subscription"]["events"], json!([SESSION_EVENT]));
        assert_eq!(body["subscription"]["active"], true);

        // List
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/beta/webhooks")
                    .extension(test_user("user-a", Some(org)))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["subscriptions"].as_array().unwrap().len(), 1);

        // Other org cannot see
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/beta/webhooks")
                    .extension(test_user("user-b", Some("other-org")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["subscriptions"].as_array().unwrap().len(), 0);

        // Get
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/beta/webhooks/{}", id))
                    .extension(test_user("user-a", Some(org)))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Update
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/beta/webhooks/{}", id))
                    .header("content-type", "application/json")
                    .extension(test_user("user-a", Some(org)))
                    .body(json_body(&json!({
                        "active": false,
                        "events": [DEPLOYMENT_RUN_EVENT]
                    })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["subscription"]["active"], false);
        assert_eq!(body["subscription"]["events"], json!([DEPLOYMENT_RUN_EVENT]));

        // Delete
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/beta/webhooks/{}", id))
                    .extension(test_user("user-a", Some(org)))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn signed_delivery_to_matching_subscriptions() {
        let temp = temp_dir("delivery");
        let state = test_app_state(&temp).await;
        let app = webhook_subscription_router().with_state(state.clone());
        let org = "org-delivery";
        let secret = "delivery-secret";

        let received = Arc::new(Mutex::new(Vec::new()));
        let url = start_receiver(received.clone()).await;

        // Create subscription
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/beta/webhooks")
                    .header("content-type", "application/json")
                    .extension(test_user("user-a", Some(org)))
                    .body(json_body(&json!({
                        "url": url,
                        "events": [SESSION_EVENT],
                        "secret": secret
                    })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        // Create an inactive subscription to verify filtering
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/beta/webhooks")
                    .header("content-type", "application/json")
                    .extension(test_user("user-a", Some(org)))
                    .body(json_body(&json!({
                        "url": url,
                        "events": [SESSION_EVENT],
                        "secret": secret
                    })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let inactive_id = body_json(resp.into_body()).await["subscription"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        // Deactivate the second subscription
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/beta/webhooks/{}", inactive_id))
                    .header("content-type", "application/json")
                    .extension(test_user("user-a", Some(org)))
                    .body(json_body(&json!({"active": false})))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Trigger delivery
        let event = json!({"type": "thinking_delta", "delta": "hello"});
        deliver_session_event(state.clone(), Some(org), "session-1", &event).await;

        // Wait for the spawned HTTP delivery task.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        let received = received.lock().unwrap();
        assert_eq!(received.len(), 1);
        let req = &received[0];
        assert_eq!(req.body["event"], SESSION_EVENT);
        assert_eq!(req.body["session_id"], "session-1");
        assert_eq!(req.body["event_data"], event);
        let signature = req.signature.as_ref().unwrap();
        assert!(verify_signature(secret, &req.body, signature));

        // A delivery row should have been recorded.
        let conn = state.db.connect().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM webhook_deliveries WHERE status = 'delivered'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        let _ = std::fs::remove_dir_all(&temp);
    }

    async fn wait_for_received(
        received: &Arc<Mutex<Vec<ReceivedRequest>>>,
        n: usize,
    ) -> Vec<ReceivedRequest> {
        for _ in 0..100 {
            let got = received.lock().unwrap().clone();
            if got.len() >= n {
                return got;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("timed out waiting for {n} webhook requests");
    }

    /// Stub receiver that returns `status` for the first `fails` requests,
    /// then 200 OK.
    async fn start_flaky_receiver(
        received: Arc<Mutex<Vec<ReceivedRequest>>>,
        fails: Arc<std::sync::atomic::AtomicUsize>,
        status: StatusCode,
    ) -> String {
        use axum::{extract::State, http::HeaderMap, routing::post, Router};

        async fn capture(
            State((received, fails, status)): State<(
                Arc<Mutex<Vec<ReceivedRequest>>>,
                Arc<std::sync::atomic::AtomicUsize>,
                StatusCode,
            )>,
            headers: HeaderMap,
            body: Body,
        ) -> StatusCode {
            let bytes = body.collect().await.unwrap().to_bytes();
            let value = serde_json::from_slice(&bytes).unwrap_or(json!({}));
            received.lock().unwrap().push(ReceivedRequest {
                signature: headers
                    .get("X-Allternit-Signature")
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string()),
                body: value,
            });
            if fails.fetch_sub(1, std::sync::atomic::Ordering::SeqCst) > 0 {
                status
            } else {
                StatusCode::OK
            }
        }

        let app = Router::new()
            .route("/webhook", post(capture))
            .with_state((received, fails, status));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        TEST_INTERNAL_ENDPOINTS.lock().unwrap().push(format!("127.0.0.1:{port}"));
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://127.0.0.1:{}/webhook", port)
    }

    fn insert_pending_delivery(
        state: &Arc<AppState>,
        subscription_id: &str,
        event: &str,
        payload: &Value,
    ) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let conn = state.db.connect().unwrap();
        conn.execute(
            "INSERT INTO webhook_deliveries (id, subscription_id, event, payload, status, attempts)
             VALUES (?1, ?2, ?3, ?4, 'pending', 0)",
            params![id, subscription_id, event, payload.to_string()],
        )
        .unwrap();
        id
    }

    fn delivery_row(state: &Arc<AppState>, delivery_id: &str) -> (String, i64) {
        let conn = state.db.connect().unwrap();
        conn.query_row(
            "SELECT status, attempts FROM webhook_deliveries WHERE id = ?1",
            params![delivery_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .unwrap()
    }

    #[test]
    fn retry_backoff_base_parsing_and_clamps() {
        assert_eq!(parse_base_secs(Some("1")), 1);
        assert_eq!(parse_base_secs(None), DEFAULT_RETRY_BASE_SECS);
        assert_eq!(parse_base_secs(Some("junk")), DEFAULT_RETRY_BASE_SECS);
        assert_eq!(parse_base_secs(Some("0")), DEFAULT_RETRY_BASE_SECS);
        assert_eq!(parse_base_secs(Some("99999")), DEFAULT_RETRY_BASE_SECS);
    }

    #[test]
    fn retry_backoff_grows_and_jitters() {
        let policy = RetryPolicy {
            base: Duration::from_secs(10),
            max_attempts: MAX_ATTEMPTS,
        };
        let first = policy.backoff(1);
        let second = policy.backoff(2);
        // ~10s and ~40s nominal, ±25% jitter.
        assert!(first >= Duration::from_millis(7500) && first <= Duration::from_millis(12500));
        assert!(second >= Duration::from_millis(30000) && second <= Duration::from_millis(50000));
        assert!(second > first);
    }

    #[test]
    fn attempt_classification() {
        assert_eq!(classify_attempt(Some(200)), AttemptOutcome::Delivered);
        assert_eq!(classify_attempt(Some(299)), AttemptOutcome::Delivered);
        assert_eq!(classify_attempt(Some(400)), AttemptOutcome::PermanentFailure);
        assert_eq!(classify_attempt(Some(422)), AttemptOutcome::PermanentFailure);
        assert_eq!(classify_attempt(Some(500)), AttemptOutcome::TransientFailure);
        assert_eq!(classify_attempt(Some(503)), AttemptOutcome::TransientFailure);
        assert_eq!(classify_attempt(None), AttemptOutcome::TransientFailure);
    }

    #[tokio::test]
    async fn retry_succeeds_after_transient_failures() {
        let temp = temp_dir("retry-success");
        let state = test_app_state(&temp).await;
        let org = "org-retry-ok";
        let secret = "retry-secret";

        let received = Arc::new(Mutex::new(Vec::new()));
        let fails = Arc::new(std::sync::atomic::AtomicUsize::new(2));
        let url = start_flaky_receiver(received.clone(), fails, StatusCode::INTERNAL_SERVER_ERROR).await;
        let sub_id = insert_subscription(&state, org, &url, &[events::AGENT_CREATED], secret);
        let delivery_id = insert_pending_delivery(
            &state,
            &sub_id,
            events::AGENT_CREATED,
            &json!({"id": "evt-1"}),
        );

        attempt_delivery(
            state.clone(),
            delivery_id.clone(),
            url,
            secret.to_string(),
            json!({"id": "evt-1"}),
            RetryPolicy {
                base: Duration::from_millis(10),
                max_attempts: MAX_ATTEMPTS,
            },
        )
        .await;

        assert_eq!(delivery_row(&state, &delivery_id), ("delivered".to_string(), 3));
        assert_eq!(received.lock().unwrap().len(), 3);

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn retry_permanent_fail_on_4xx_without_retry() {
        let temp = temp_dir("retry-4xx");
        let state = test_app_state(&temp).await;
        let org = "org-retry-4xx";
        let secret = "retry-secret";

        let received = Arc::new(Mutex::new(Vec::new()));
        let fails = Arc::new(std::sync::atomic::AtomicUsize::new(usize::MAX));
        let url = start_flaky_receiver(received.clone(), fails, StatusCode::BAD_REQUEST).await;
        let sub_id = insert_subscription(&state, org, &url, &[events::AGENT_CREATED], secret);
        let delivery_id = insert_pending_delivery(
            &state,
            &sub_id,
            events::AGENT_CREATED,
            &json!({"id": "evt-1"}),
        );

        attempt_delivery(
            state.clone(),
            delivery_id.clone(),
            url,
            secret.to_string(),
            json!({"id": "evt-1"}),
            RetryPolicy {
                base: Duration::from_millis(10),
                max_attempts: MAX_ATTEMPTS,
            },
        )
        .await;

        assert_eq!(delivery_row(&state, &delivery_id), ("failed".to_string(), 1));
        assert_eq!(received.lock().unwrap().len(), 1);

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn retry_exhausts_attempts_on_persistent_failure() {
        let temp = temp_dir("retry-exhaust");
        let state = test_app_state(&temp).await;
        let org = "org-retry-exhaust";
        let secret = "retry-secret";

        // Nothing listens on this port: every attempt is a connect error.
        let dead_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let dead_port = dead_listener.local_addr().unwrap().port();
        drop(dead_listener);
        TEST_INTERNAL_ENDPOINTS.lock().unwrap().push(format!("127.0.0.1:{dead_port}"));
        let url = format!("http://127.0.0.1:{dead_port}/webhook");
        let sub_id = insert_subscription(&state, org, &url, &[events::AGENT_CREATED], secret);
        let delivery_id = insert_pending_delivery(
            &state,
            &sub_id,
            events::AGENT_CREATED,
            &json!({"id": "evt-1"}),
        );

        attempt_delivery(
            state.clone(),
            delivery_id.clone(),
            url,
            secret.to_string(),
            json!({"id": "evt-1"}),
            RetryPolicy {
                base: Duration::from_millis(10),
                max_attempts: MAX_ATTEMPTS,
            },
        )
        .await;

        assert_eq!(delivery_row(&state, &delivery_id), ("failed".to_string(), 3));

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn registered_event_envelope_and_signature() {
        let temp = temp_dir("envelope");
        let state = test_app_state(&temp).await;
        let org = "org-envelope";
        let secret = "envelope-secret";

        let received = Arc::new(Mutex::new(Vec::new()));
        let url = start_receiver(received.clone()).await;
        insert_subscription(&state, org, &url, &[events::AGENT_CREATED], secret);

        deliver_registered_event(
            state.clone(),
            Some(org),
            events::AGENT_CREATED,
            json!({"agent_id": "a-1", "name": "Helper"}),
        )
        .await;

        let got = wait_for_received(&received, 1).await;
        let body = &got[0].body;
        assert_eq!(body["type"], events::AGENT_CREATED);
        assert!(body["id"].as_str().is_some());
        assert!(body["created_at"].as_str().is_some());
        assert_eq!(body["data"], json!({"agent_id": "a-1", "name": "Helper"}));
        let signature = got[0].signature.as_ref().unwrap();
        assert!(verify_signature(secret, body, signature));

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn wildcard_subscription_receives_all_events() {
        let temp = temp_dir("wildcard");
        let state = test_app_state(&temp).await;
        let org = "org-wildcard";
        let secret = "wildcard-secret";

        let received = Arc::new(Mutex::new(Vec::new()));
        let url = start_receiver(received.clone()).await;
        insert_subscription(&state, org, &url, &[events::WILDCARD], secret);

        deliver_registered_event(
            state.clone(),
            Some(org),
            events::KEY_REVOKED,
            json!({"key_id": "k-1"}),
        )
        .await;
        deliver_session_event(state.clone(), Some(org), "s-1", &json!({"type": "thinking_delta"}))
            .await;

        let got = wait_for_received(&received, 2).await;
        let types: Vec<&str> = got
            .iter()
            .map(|r| match r.body["type"].as_str().or_else(|| r.body["event"].as_str()) {
                Some(t) => t,
                None => panic!("unexpected webhook body: {:?}", r.body),
            })
            .collect();
        assert!(types.contains(&events::KEY_REVOKED));
        assert!(types.contains(&SESSION_EVENT));

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn unregistered_event_type_is_not_delivered() {
        let temp = temp_dir("unregistered");
        let state = test_app_state(&temp).await;

        let received = Arc::new(Mutex::new(Vec::new()));
        let url = start_receiver(received.clone()).await;
        insert_subscription(&state, "org-x", &url, &[events::WILDCARD], "s");

        deliver_registered_event(
            state.clone(),
            Some("org-x"),
            "totally.made_up",
            json!({}),
        )
        .await;

        // Nothing should arrive even though the subscription is a wildcard.
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(received.lock().unwrap().is_empty());

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn rejects_unknown_and_malformed_event_types() {
        let temp = temp_dir("registry-validation");
        let state = test_app_state(&temp).await;
        let app = webhook_subscription_router().with_state(state);
        let org = "org-registry";

        // Unknown event type on create.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/beta/webhooks")
                    .header("content-type", "application/json")
                    .extension(test_user("user-a", Some(org)))
                    .body(json_body(&json!({
                        "url": "https://example.test/hook",
                        "events": ["agent.created", "bogus.event"],
                        "secret": "s"
                    })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // Wildcard alone is accepted…
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/beta/webhooks")
                    .header("content-type", "application/json")
                    .extension(test_user("user-a", Some(org)))
                    .body(json_body(&json!({
                        "url": "https://example.test/hook",
                        "events": ["*"],
                        "secret": "s"
                    })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        // …but wildcard mixed with other events is rejected.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/beta/webhooks")
                    .header("content-type", "application/json")
                    .extension(test_user("user-a", Some(org)))
                    .body(json_body(&json!({
                        "url": "https://example.test/hook",
                        "events": ["*", "agent.created"],
                        "secret": "s"
                    })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // Unknown event type on update.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/beta/webhooks")
                    .header("content-type", "application/json")
                    .extension(test_user("user-a", Some(org)))
                    .body(json_body(&json!({
                        "url": "https://example.test/hook",
                        "events": ["agent.created"],
                        "secret": "s"
                    })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let id = body_json(resp.into_body()).await["subscription"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/beta/webhooks/{id}"))
                    .header("content-type", "application/json")
                    .extension(test_user("user-a", Some(org)))
                    .body(json_body(&json!({"events": ["nope.nope"]})))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn deliveries_endpoint_lists_filters_and_scopes() {
        let temp = temp_dir("deliveries-list");
        let state = test_app_state(&temp).await;
        let app = webhook_subscription_router().with_state(state.clone());
        let org = "org-deliveries";
        let secret = "deliveries-secret";

        let received = Arc::new(Mutex::new(Vec::new()));
        let url = start_receiver(received).await;

        // Create the subscription over HTTP, then record deliveries both via a
        // real (successful) delivery and by inserting failed rows directly.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/beta/webhooks")
                    .header("content-type", "application/json")
                    .extension(test_user("user-a", Some(org)))
                    .body(json_body(&json!({
                        "url": url,
                        "events": [events::AGENT_CREATED],
                        "secret": secret
                    })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let sub_id = body_json(resp.into_body()).await["subscription"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        deliver_registered_event(
            state.clone(),
            Some(org),
            events::AGENT_CREATED,
            json!({"agent_id": "a-1"}),
        )
        .await;
        // Wait for the delivered row so created_at ordering is deterministic.
        tokio::time::sleep(Duration::from_millis(400)).await;

        let conn = state.db.connect().unwrap();
        for (event, status) in [
            ("agent.created", "failed"),
            ("agent.created", "pending"),
        ] {
            conn.execute(
                "INSERT INTO webhook_deliveries (id, subscription_id, event, payload, status, attempts)
                 VALUES (?1, ?2, ?3, '{}', ?4, 1)",
                params![uuid::Uuid::new_v4().to_string(), sub_id, event, status],
            )
            .unwrap();
        }
        drop(conn);

        // Full list, newest first.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/beta/webhooks/{sub_id}/deliveries"))
                    .extension(test_user("user-a", Some(org)))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["total"], 3);
        assert_eq!(body["deliveries"].as_array().unwrap().len(), 3);

        // Status filter.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/beta/webhooks/{sub_id}/deliveries?status=failed"))
                    .extension(test_user("user-a", Some(org)))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["total"], 1);
        let rows = body["deliveries"].as_array().unwrap();
        assert_eq!(rows[0]["status"], "failed");
        assert_eq!(rows[0]["event_type"], "agent.created");
        assert!(rows[0]["id"].as_str().is_some());
        assert!(rows[0]["created_at"].as_str().is_some());
        assert!(rows[0]["updated_at"].as_str().is_some());

        // Bad status filter → 400.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/beta/webhooks/{sub_id}/deliveries?status=bogus"))
                    .extension(test_user("user-a", Some(org)))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // Limit pagination.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/beta/webhooks/{sub_id}/deliveries?limit=2"))
                    .extension(test_user("user-a", Some(org)))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["deliveries"].as_array().unwrap().len(), 2);
        assert_eq!(body["total"], 3);

        // Other org cannot read the delivery log (404, same as CRUD scope).
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/beta/webhooks/{sub_id}/deliveries"))
                    .extension(test_user("user-b", Some("other-org")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // Unknown subscription → 404.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/beta/webhooks/nope/deliveries")
                    .extension(test_user("user-a", Some(org)))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn response_body_is_truncated_in_delivery_log() {
        let temp = temp_dir("truncate");
        let state = test_app_state(&temp).await;

        let long_body = "x".repeat(1200);
        let sub_id = uuid::Uuid::new_v4().to_string();
        let delivery_id = uuid::Uuid::new_v4().to_string();
        let conn = state.db.connect().unwrap();
        conn.execute(
            "INSERT INTO webhook_subscriptions (id, org_id, url, events, secret, active)
             VALUES (?1, 'org-t', 'https://example.test/h', '[\"agent.created\"]', 's', 1)",
            params![sub_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO webhook_deliveries (id, subscription_id, event, payload, status,
                                             response_status, response_body, attempts)
             VALUES (?1, ?2, 'agent.created', '{}', 'failed', 500, ?3, 3)",
            params![delivery_id, sub_id, long_body],
        )
        .unwrap();
        drop(conn);

        let app = webhook_subscription_router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/beta/webhooks/{sub_id}/deliveries?status=failed"))
                    .extension(test_user("user-a", Some("org-t")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_json(resp.into_body()).await;
        let row = &body["deliveries"].as_array().unwrap()[0];
        let text = row["response_body"].as_str().unwrap();
        assert!(text.chars().count() <= 502, "body should be truncated to ~500 chars");
        assert!(text.ends_with('…'));

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn agent_lifecycle_emits_webhooks() {
        let temp = temp_dir("agent-emitter");
        let state = test_app_state(&temp).await;
        let app = crate::agent_routes::agent_router().with_state(state.clone());
        let org = "org-agent-emitter";
        let secret = "agent-secret";

        let received = Arc::new(Mutex::new(Vec::new()));
        let url = start_receiver(received.clone()).await;
        insert_subscription(
            &state,
            org,
            &url,
            &[
                events::AGENT_CREATED,
                events::AGENT_UPDATED,
                events::AGENT_ARCHIVED,
            ],
            secret,
        );

        // Create (checklist-valid body).
        let body = json!({
            "name": "Webhook Agent",
            "description": "Emitter test agent",
            "type": "worker",
            "model": "kimi-k2",
            "provider": "allternit",
            "harness_config": {"mode": "local"},
            "enabled_modes": ["chat"],
            "trust_tier": "standard",
            "tools": ["web_search"],
        });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/agents")
                    .header("content-type", "application/json")
                    .extension(org_user("user-a", org))
                    .body(json_body(&body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let agent_id = body_json(resp.into_body()).await["agent"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        // Update (PATCH).
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/agents/{agent_id}"))
                    .header("content-type", "application/json")
                    .extension(org_user("user-a", org))
                    .body(json_body(&json!({"name": "Webhook Agent v2"})))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Archive.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/agents/{agent_id}/archive"))
                    .extension(org_user("user-a", org))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let got = wait_for_received(&received, 3).await;
        let by_type: HashMap<&str, &Value> = got
            .iter()
            .map(|r| (r.body["type"].as_str().unwrap(), &r.body))
            .collect();
        assert_eq!(by_type[events::AGENT_CREATED]["data"]["agent_id"], json!(agent_id));
        assert_eq!(by_type[events::AGENT_CREATED]["data"]["name"], json!("Webhook Agent"));
        assert_eq!(by_type[events::AGENT_UPDATED]["data"]["name"], json!("Webhook Agent v2"));
        assert_eq!(by_type[events::AGENT_ARCHIVED]["data"]["agent_id"], json!(agent_id));
        for req in &got {
            assert!(verify_signature(secret, &req.body, req.signature.as_ref().unwrap()));
        }

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn session_create_archive_and_over_budget_emit_webhooks() {
        let temp = temp_dir("session-emitter");
        let state = test_app_state(&temp).await;
        let sessions = crate::beta_session_routes::beta_session_router().with_state(state.clone());
        let cloud = crate::cloud_agents_routes::cloud_agents_router().with_state(state.clone());
        let org = "org-session-emitter";
        let secret = "session-secret";

        let received = Arc::new(Mutex::new(Vec::new()));
        let url = start_receiver(received.clone()).await;
        insert_subscription(
            &state,
            org,
            &url,
            &[events::SESSION_CREATED, events::SESSION_ARCHIVED, events::SESSION_OVER_BUDGET],
            secret,
        );

        // session.created via the cloud session endpoint.
        let resp = cloud
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions")
                    .header("content-type", "application/json")
                    .extension(org_user("user-a", org))
                    .body(json_body(&json!({
                        "computer": {"kind": "none"},
                        "metadata": {},
                        "budget": {"max_tokens": 10}
                    })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let session_id = body_json(resp.into_body()).await["session"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        // session.over_budget: append usage beyond max_tokens.
        let resp = sessions
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/beta/sessions/{session_id}/events"))
                    .header("content-type", "application/json")
                    .extension(org_user("user-a", org))
                    .body(json_body(&json!({
                        "type": "thinking_delta",
                        "data": {},
                        "usage": {"tokens": 100, "turns": 0, "tool_calls": 0}
                    })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let append_body = body_json(resp.into_body()).await;
        assert_eq!(append_body["accepted"], false);

        // session.archived via the cloud archive endpoint.
        let resp = cloud
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/sessions/{session_id}/archive"))
                    .extension(org_user("user-a", org))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let got = wait_for_received(&received, 3).await;
        let by_type: HashMap<&str, &Value> = got
            .iter()
            .map(|r| (r.body["type"].as_str().unwrap(), &r.body))
            .collect();
        assert_eq!(by_type[events::SESSION_CREATED]["data"]["session_id"], json!(session_id));
        assert_eq!(by_type[events::SESSION_ARCHIVED]["data"]["session_id"], json!(session_id));
        let over = &by_type[events::SESSION_OVER_BUDGET]["data"];
        assert_eq!(over["session_id"], json!(session_id));
        assert_eq!(over["event"]["type"], "budget_exceeded");

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn deployment_trigger_emits_run_created() {
        let temp = temp_dir("deployment-emitter");
        let state = test_app_state(&temp).await;
        let app = crate::beta_deployment_routes::beta_deployment_router().with_state(state.clone());
        let org = "org-deployment-emitter";
        let secret = "deployment-secret";

        let received = Arc::new(Mutex::new(Vec::new()));
        let url = start_receiver(received.clone()).await;
        insert_subscription(&state, org, &url, &[events::DEPLOYMENT_RUN_CREATED], secret);

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/beta/deployments")
                    .header("content-type", "application/json")
                    .extension(org_user("user-a", org))
                    .body(json_body(&json!({
                        "agent_id": "agent-1",
                        "cron": "0 9 * * *",
                        "metadata": {}
                    })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let deployment_id = body_json(resp.into_body()).await["deployment"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/beta/deployments/{deployment_id}/runs"))
                    .extension(org_user("user-a", org))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let run_id = body_json(resp.into_body()).await["id"]
            .as_str()
            .unwrap()
            .to_string();

        let got = wait_for_received(&received, 1).await;
        assert_eq!(got[0].body["type"], events::DEPLOYMENT_RUN_CREATED);
        assert_eq!(got[0].body["data"]["deployment_id"], json!(deployment_id));
        assert_eq!(got[0].body["data"]["run_id"], json!(run_id));
        assert_eq!(got[0].body["data"]["triggered_by"], "manual");

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn stripe_webhook_grant_emits_billing_webhook() {
        let temp = temp_dir("billing-emitter");
        let state = test_app_state(&temp).await;
        let app = crate::fabric_credits_routes::router().with_state(state.clone());
        let org = "org-billing-emitter";
        let secret = "billing-secret";

        // Seed the org the internal (cloud-api Stripe webhook) grant targets.
        let conn = state.db.connect().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO organizations (id, name) VALUES (?1, 'Test Org')",
            params![org],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO users (id, email) VALUES (?1, ?2)",
            params!["user-a", "user-a@test.local"],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO organization_members (id, organization_id, user_id, role)
             VALUES (?1, ?2, ?3, 'owner')",
            params![format!("{org}:user-a"), org, "user-a"],
        )
        .unwrap();
        drop(conn);

        let received = Arc::new(Mutex::new(Vec::new()));
        let url = start_receiver(received.clone()).await;
        insert_subscription(&state, org, &url, &[events::BILLING_CREDIT_PURCHASE], secret);

        // The cloud-api webhook path: internal service token + synthetic
        // internal identity, org and Stripe-event idempotency key in the body.
        let mut internal_user = org_user("user-a", org);
        internal_user.user_id = crate::auth::INTERNAL_SERVICE_USER_ID.to_string();
        internal_user.organization_id = None;
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/credits/grant")
                    .header("content-type", "application/json")
                    .header("x-allternit-internal-token", "webhook-test-internal-token")
                    .extension(internal_user)
                    .body(json_body(&json!({
                        "amount_cents": 2500,
                        "organization_id": org,
                        "idempotency_key": "stripe-evt_emit_1",
                        "reference_id": "evt_emit_1"
                    })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let got = wait_for_received(&received, 1).await;
        assert_eq!(got[0].body["type"], events::BILLING_CREDIT_PURCHASE);
        assert_eq!(got[0].body["data"]["amount_cents"], 2500);
        assert_eq!(got[0].body["data"]["organization_id"], json!(org));
        assert_eq!(got[0].body["data"]["method"], "stripe");

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn key_create_and_revoke_emit_webhooks() {
        let temp = temp_dir("key-emitter");
        let state = test_app_state(&temp).await;
        let app = crate::llm_gateway::gateway_keys_router().with_state(state.clone());
        let org = "org-key-emitter";
        let secret = "key-secret";

        let received = Arc::new(Mutex::new(Vec::new()));
        let url = start_receiver(received.clone()).await;
        insert_subscription(
            &state,
            org,
            &url,
            &[events::KEY_CREATED, events::KEY_REVOKED],
            secret,
        );
        // llm_virtual_keys.user_id is a FK into users.
        let conn = state.db.connect().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO users (id, email) VALUES (?1, ?2)",
            params!["user-a", "user-a@example.test"],
        )
        .unwrap();
        drop(conn);

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/gateway/keys")
                    .header("content-type", "application/json")
                    .extension(org_user("user-a", org))
                    .body(json_body(&json!({"name": "CI key"})))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let body = body_json(resp.into_body()).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let key_id = body["id"]
            .as_str()
            .unwrap()
            .to_string();

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/gateway/keys/{key_id}"))
                    .extension(org_user("user-a", org))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let got = wait_for_received(&received, 2).await;
        let by_type: HashMap<&str, &Value> = got
            .iter()
            .map(|r| (r.body["type"].as_str().unwrap(), &r.body))
            .collect();
        assert_eq!(by_type[events::KEY_CREATED]["data"]["key_id"], json!(key_id));
        assert_eq!(by_type[events::KEY_CREATED]["data"]["name"], json!("CI key"));
        assert!(by_type[events::KEY_CREATED]["data"]["key"].is_null(), "plaintext key must not be in the webhook");
        assert_eq!(by_type[events::KEY_REVOKED]["data"]["key_id"], json!(key_id));

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn request_succeeds_even_when_webhook_endpoint_is_down() {
        let temp = temp_dir("emitter-down");
        let state = test_app_state(&temp).await;
        let app = crate::agent_routes::agent_router().with_state(state.clone());
        let org = "org-emitter-down";

        // Subscription pointing at a port with nothing listening.
        let dead_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let dead_port = dead_listener.local_addr().unwrap().port();
        drop(dead_listener);
        TEST_INTERNAL_ENDPOINTS.lock().unwrap().push(format!("127.0.0.1:{dead_port}"));
        let url = format!("http://127.0.0.1:{dead_port}/webhook");
        insert_subscription(&state, org, &url, &[events::AGENT_CREATED], "s");

        let body = json!({
            "name": "Resilient Agent",
            "description": "Webhook target is down",
            "type": "worker",
            "model": "kimi-k2",
            "provider": "allternit",
            "harness_config": {"mode": "local"},
            "enabled_modes": ["chat"],
            "trust_tier": "standard",
            "tools": ["web_search"],
        });
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/agents")
                    .header("content-type", "application/json")
                    .extension(org_user("user-a", org))
                    .body(json_body(&body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn rejects_invalid_subscription_body() {
        let temp = temp_dir("validation");
        let state = test_app_state(&temp).await;
        let app = webhook_subscription_router().with_state(state);

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/beta/webhooks")
                    .header("content-type", "application/json")
                    .extension(test_user("user-a", Some("org")))
                    .body(json_body(&json!({
                        "url": "ftp://example.test/hook",
                        "events": [SESSION_EVENT],
                        "secret": "s"
                    })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/beta/webhooks")
                    .header("content-type", "application/json")
                    .extension(test_user("user-a", Some("org")))
                    .body(json_body(&json!({
                        "url": "https://example.test/hook",
                        "events": [],
                        "secret": "s"
                    })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn webhook_target_refuses_metadata_and_private_literals() {
        for u in [
            "http://169.254.169.254/latest/meta-data",
            "http://10.0.0.5/hook",
            "http://192.168.1.1/hook",
            "http://127.0.0.1:8013/hook",
            "http://[::1]/hook",
            "http://localhost/hook",
        ] {
            assert!(check_webhook_target(u, &[]).is_err(), "{u} must be refused");
        }
        assert!(check_webhook_target("https://hooks.example.com/x", &[]).is_ok());
        assert!(check_webhook_target("ftp://example.com/x", &[]).is_err());
    }

    #[test]
    fn webhook_target_allows_only_configured_internal_endpoint() {
        let allow = vec!["127.0.0.1:7717".to_string()];
        assert!(check_webhook_target("http://127.0.0.1:7717/hook", &allow).is_ok());
        assert!(check_webhook_target("http://127.0.0.1:7718/hook", &allow).is_err());
        assert!(check_webhook_target("http://169.254.169.254:7717/hook", &allow).is_err());
    }

    #[tokio::test]
    async fn webhook_create_rejects_metadata_url() {
        assert!(validate_subscription_body(
            "http://169.254.169.254/latest",
            &[events::AGENT_CREATED.to_string()],
            "s"
        )
        .is_err());
    }

    #[tokio::test]
    async fn webhook_client_refuses_private_name_resolution_but_dials_allowlisted() {
        let client = webhook_client("http://localhost:1/x", &[]).unwrap();
        let err = client.post("http://localhost:1/x").send().await.unwrap_err();
        assert!(err.to_string().len() > 0);
        assert!(webhook_client("http://localhost:1/x", &["localhost:1".to_string()]).is_ok());
    }
}
