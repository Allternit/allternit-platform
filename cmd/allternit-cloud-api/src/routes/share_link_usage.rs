//! Share-link usage meter (Apps + MCP 2026, plan v3 lane 29, decision D8):
//! the 20 GB/month cap counts bytes OpenTunnel reports, never bytes a client
//! reports.
//!
//! `POST /api/v1/internal/share-links/usage` receives OpenTunnel's analytics
//! events (the relay's `ANALYTICS_URL`). Header `x-allternit-usage-secret` must
//! equal env `ALLTERNIT_SHARE_USAGE_SECRET` (32+ chars, constant-time compare);
//! no secret configured = 503, wrong = 401. No user session.
//!
//! UNVERIFIED event shape (OpenTunnel v0.6.2's real payload is not in this
//! repo; check against the relay's first real post before the train ships):
//! one event or `{ "events": [ ... ] }` (max 500), each
//! `{ id?, tunnel_id?, hostname, bytes_in?, bytes_out?, bytes?, window_start?, at? }`.
//! `hostname` is `<name>.live.<APP_DOMAIN>`; its first label is the link's
//! `subdomain`. bytes = `bytes`, else `bytes_in + bytes_out`. The idempotency key
//! is `id`, else `<tunnel_id or hostname>:<window_start or at>`; an event with
//! neither is skipped. `at`/`window_start` (RFC 3339) picks the UTC month, else now.
//!
//! Answer `{ accepted, duplicate, unmatched }`. A replayed event counts once.
//! Only live links are metered; an unknown hostname is `unmatched`.

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use chrono::{DateTime, Datelike, Utc};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::ApiState;

pub const SECRET_ENV: &str = "ALLTERNIT_SHARE_USAGE_SECRET";
pub const SECRET_HEADER: &str = "x-allternit-usage-secret";
const MAX_EVENTS: usize = 500;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new().route("/api/v1/internal/share-links/usage", post(receive))
}

fn fail(status: StatusCode, error: &str, message: &str) -> Response {
    (status, Json(json!({ "error": error, "message": message }))).into_response()
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0_u8, |d, (x, y)| d | (x ^ y)) == 0
}

pub fn period_of(t: DateTime<Utc>) -> String {
    format!("{:04}-{:02}", t.year(), t.month())
}

/// The month a link's bytes count against right now.
pub fn current_period() -> String {
    period_of(Utc::now())
}

struct Event {
    key: String,
    subdomain: String,
    bytes: i64,
    period: String,
}

fn parse_event(v: &Value) -> Option<Event> {
    let hostname = v.get("hostname").and_then(Value::as_str)?.trim().to_ascii_lowercase();
    let subdomain = hostname.split('.').next().filter(|s| !s.is_empty())?.to_string();
    let n = |k: &str| v.get(k).and_then(Value::as_i64).filter(|b| *b >= 0);
    let bytes = n("bytes").or_else(|| match (n("bytes_in"), n("bytes_out")) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(0).saturating_add(b.unwrap_or(0))),
    })?;
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
    let when = s("window_start").or_else(|| s("at"));
    let key = match s("id") {
        Some(id) => format!("id:{id}"),
        None => format!("{}:{}", s("tunnel_id").unwrap_or(&hostname), when?),
    };
    let period = when
        .and_then(|w| DateTime::parse_from_rfc3339(w).ok())
        .map(|t| period_of(t.with_timezone(&Utc)))
        .unwrap_or_else(current_period);
    Some(Event { key, subdomain, bytes, period })
}

async fn receive(State(state): State<Arc<ApiState>>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    let expected = std::env::var(SECRET_ENV).unwrap_or_default();
    if expected.len() < 32 {
        return fail(StatusCode::SERVICE_UNAVAILABLE, "not_configured", "Usage metering is not configured on this deployment.");
    }
    let given = headers.get(SECRET_HEADER).and_then(|v| v.to_str().ok()).unwrap_or_default();
    if !constant_time_eq(&expected, given) {
        return fail(StatusCode::UNAUTHORIZED, "unauthorized", "Invalid usage credential.");
    }
    let items: Vec<Value> = match body.get("events").and_then(Value::as_array) {
        Some(a) => a.clone(),
        None => vec![body],
    };
    if items.len() > MAX_EVENTS {
        return fail(StatusCode::BAD_REQUEST, "too_many_events", "Send up to 500 events per request.");
    }
    let (mut accepted, mut duplicate, mut unmatched) = (0, 0, 0);
    for item in &items {
        let Some(ev) = parse_event(item) else {
            unmatched += 1;
            continue;
        };
        match record(&state.db, &ev).await {
            Ok(Outcome::Counted) => accepted += 1,
            Ok(Outcome::Duplicate) => duplicate += 1,
            Ok(Outcome::NoLink) => unmatched += 1,
            Err(e) => {
                tracing::error!(error = %e, "share link usage write failed");
                return fail(StatusCode::INTERNAL_SERVER_ERROR, "internal", "Could not record usage; retry the batch.");
            }
        }
    }
    Json(json!({ "accepted": accepted, "duplicate": duplicate, "unmatched": unmatched })).into_response()
}

enum Outcome {
    Counted,
    Duplicate,
    NoLink,
}

/// One event, one transaction: claim the event key, then add the bytes. A
/// failure rolls both back so a retried batch counts it exactly once.
async fn record(db: &sqlx::PgPool, ev: &Event) -> Result<Outcome, sqlx::Error> {
    let mut tx = db.begin().await?;
    let link: Option<String> = sqlx::query_scalar(
        "SELECT id FROM share_links WHERE subdomain = $1 AND kind = 'live'",
    )
    .bind(&ev.subdomain)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(link) = link else { return Ok(Outcome::NoLink) };
    let claimed = sqlx::query(
        "INSERT INTO share_link_usage_events (event_id, link_id, bytes) VALUES ($1, $2, $3) \
         ON CONFLICT (event_id) DO NOTHING",
    )
    .bind(&ev.key)
    .bind(&link)
    .bind(ev.bytes)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if claimed == 0 {
        return Ok(Outcome::Duplicate);
    }
    let total: i64 = sqlx::query_scalar(
        "INSERT INTO share_link_usage (link_id, period, bytes) VALUES ($1, $2, $3) \
         ON CONFLICT (link_id, period) DO UPDATE SET bytes = share_link_usage.bytes + EXCLUDED.bytes, updated_at = now() \
         RETURNING bytes",
    )
    .bind(&link)
    .bind(&ev.period)
    .bind(ev.bytes)
    .fetch_one(&mut *tx)
    .await?;
    if ev.period == current_period() {
        sqlx::query("UPDATE share_links SET bytes_month = $2, last_seen_at = now() WHERE id = $1")
            .bind(&link)
            .bind(total)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(Outcome::Counted)
}

#[cfg(test)]
#[path = "share_link_usage_tests.rs"]
mod tests;
