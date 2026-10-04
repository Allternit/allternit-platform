//! Events and signed webhook delivery for the Platform API.
//!
//! [`emit_event`] stores an event (`evt_…`) and queues one delivery per
//! matching endpoint of the project. [`deliver_due`] (run by the worker in
//! [`spawn_worker`]) POSTs each due delivery, signed:
//!
//! ```text
//! allternit-signature: t=<unix seconds>,v1=<hex HMAC-SHA256(secret, "<t>.<raw body>")>
//! ```
//!
//! A non-2xx answer or a network error retries with backoff (30 s, 2 min,
//! 10 min, 30 min, 1 h, then every 2 h) for 24 h, then the delivery is
//! `failed`. Every attempt re-checks the URL against [`check_url`] (https
//! only, no private, loopback or link-local addresses), so a DNS change can't
//! turn an endpoint into a request to our own network.

use std::net::IpAddr;
use std::time::Duration;

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use sqlx::PgPool;

use super::{new_id, PlatformError};

/// Event types a webhook can subscribe to. `*` subscribes to all of them.
pub const EVENT_TYPES: [&str; 3] = ["message.received", "message.status", "registration.updated"];

/// How long a delivery keeps retrying.
const RETRY_WINDOW_HOURS: i64 = 24;
const BATCH: i64 = 50;

/// Store `type` for the project and queue it for every endpoint that wants it.
/// Returns the event id.
pub async fn emit_event(db: &PgPool, project_id: &str, account_id: Option<&str>, kind: &str, data: Value) -> Result<String, sqlx::Error> {
    let id = new_id("evt_");
    let mut tx = db.begin().await?;
    sqlx::query("INSERT INTO platform_events (id, project_id, account_id, type, data) VALUES ($1, $2, $3, $4, $5)")
        .bind(&id)
        .bind(project_id)
        .bind(account_id)
        .bind(kind)
        .bind(&data)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO platform_webhook_deliveries (id, webhook_id, event_id) \
         SELECT 'whd_' || replace(gen_random_uuid()::text, '-', ''), w.id, $2 FROM platform_webhooks w \
         WHERE w.project_id = $1 AND w.deleted_at IS NULL AND ($3 = ANY(w.events) OR '*' = ANY(w.events)) \
         ON CONFLICT DO NOTHING",
    )
    .bind(project_id)
    .bind(&id)
    .bind(kind)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(id)
}

/// [`emit_event`] for a phone number, if it belongs to a Platform API project. No-op otherwise.
pub async fn emit_for_number(db: &PgPool, number_id: &str, kind: &str, data: Value) {
    let owner: Option<(Option<String>, Option<String>)> = sqlx::query_as("SELECT project_id, account_id FROM phone_numbers WHERE id = $1")
        .bind(number_id)
        .fetch_optional(db)
        .await
        .ok()
        .flatten();
    if let Some((Some(project), account)) = owner {
        if let Err(e) = emit_event(db, &project, account.as_deref(), kind, data).await {
            tracing::warn!(number = %number_id, "platform event {kind} not stored: {e}");
        }
    }
}

/// The JSON a webhook receives.
pub fn envelope(id: &str, kind: &str, created: DateTime<Utc>, project_id: &str, account_id: Option<&str>, data: &Value) -> Value {
    json!({ "id": id, "object": "event", "type": kind, "created": created.timestamp(), "project_id": project_id, "account_id": account_id, "data": data })
}

/// `t=<ts>,v1=<hex hmac>` over `"<ts>.<body>"`.
pub fn sign(secret: &str, ts: i64, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac accepts any key length");
    mac.update(ts.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    format!("t={ts},v1={}", hex::encode(mac.finalize().into_bytes()))
}

fn private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private() || v4.is_loopback() || v4.is_link_local() || v4.is_unspecified() || v4.is_broadcast() || v4.is_documentation()
                // 100.64.0.0/10 (carrier-grade NAT; also our mesh), 0.0.0.0/8
                || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xC0) == 64)
                || v4.octets()[0] == 0
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return private_ip(IpAddr::V4(v4));
            }
            let seg = v6.segments()[0];
            v6.is_loopback() || v6.is_unspecified() || (seg & 0xfe00) == 0xfc00 || (seg & 0xffc0) == 0xfe80
        }
    }
}

/// Refuse anything but a public https URL. Resolves the host now.
pub async fn check_url(raw: &str) -> Result<reqwest::Url, PlatformError> {
    let bad = |m: &str| PlatformError::invalid_request("invalid_url", m.to_string()).with_param("url");
    let url = reqwest::Url::parse(raw).map_err(|_| bad("url must be a valid absolute URL."))?;
    if url.scheme() != "https" {
        return Err(bad("url must use https."));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(bad("url must not carry credentials."));
    }
    let host = url.host_str().ok_or_else(|| bad("url needs a host."))?.to_string();
    let port = url.port_or_known_default().unwrap_or(443);
    let addrs: Vec<_> = tokio::net::lookup_host((host.as_str(), port)).await.map_err(|_| bad("url's host does not resolve."))?.collect();
    if addrs.is_empty() || addrs.iter().any(|a| private_ip(a.ip())) {
        return Err(bad("url must point to a public address."));
    }
    Ok(url)
}

/// Seconds until the next try after `attempts` failures.
pub fn backoff_secs(attempts: i32) -> i64 {
    match attempts {
        ..=1 => 30,
        2 => 120,
        3 => 600,
        4 => 1800,
        5 => 3600,
        _ => 7200,
    }
}

#[derive(sqlx::FromRow)]
struct Due {
    id: String,
    attempts: i32,
    url: String,
    secret: String,
    event_id: String,
    kind: String,
    project_id: String,
    account_id: Option<String>,
    data: Value,
    event_created: DateTime<Utc>,
    created_at: DateTime<Utc>,
}

/// One POST. `Ok(status)` for any HTTP answer, `Err` for a refused URL or a network error.
async fn post(client: &reqwest::Client, url: &str, secret: &str, body: &[u8]) -> Result<u16, String> {
    let url = check_url(url).await.map_err(|e| e.message)?;
    let ts = Utc::now().timestamp();
    let resp = client
        .post(url)
        .header("content-type", "application/json")
        .header("user-agent", "Allternit-Webhooks/1.0")
        .header("allternit-signature", sign(secret, ts, body))
        .body(body.to_vec())
        .send()
        .await
        .map_err(|e| if e.is_timeout() { "timed out".to_string() } else { "connection failed".to_string() })?;
    Ok(resp.status().as_u16())
}

pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("reqwest client builds")
}

/// Deliver up to one batch of due deliveries. Returns how many were attempted.
pub async fn deliver_due(db: &PgPool, client: &reqwest::Client) -> Result<usize, sqlx::Error> {
    let due: Vec<Due> = sqlx::query_as(
        "UPDATE platform_webhook_deliveries d SET next_attempt_at = now() + interval '5 minutes' \
         FROM platform_webhooks w, platform_events e \
         WHERE d.id IN (SELECT id FROM platform_webhook_deliveries WHERE state = 'pending' AND next_attempt_at <= now() ORDER BY next_attempt_at LIMIT $1 FOR UPDATE SKIP LOCKED) \
           AND w.id = d.webhook_id AND e.id = d.event_id \
         RETURNING d.id, d.attempts, w.url, w.secret, e.id AS event_id, e.type AS kind, e.project_id, e.account_id, e.data, e.created_at AS event_created, d.created_at",
    )
    .bind(BATCH)
    .fetch_all(db)
    .await?;
    for d in &due {
        let body = serde_json::to_vec(&envelope(&d.event_id, &d.kind, d.event_created, &d.project_id, d.account_id.as_deref(), &d.data)).unwrap_or_default();
        let outcome = post(client, &d.url, &d.secret, &body).await;
        let attempts = d.attempts + 1;
        match outcome {
            Ok(status) if (200..300).contains(&status) => {
                sqlx::query("UPDATE platform_webhook_deliveries SET state = 'delivered', attempts = $2, last_status = $3, last_error = NULL, delivered_at = now() WHERE id = $1")
                    .bind(&d.id)
                    .bind(attempts)
                    .bind(status as i32)
                    .execute(db)
                    .await?;
            }
            other => {
                let (status, error) = match other {
                    Ok(s) => (Some(s as i32), format!("HTTP {s}")),
                    Err(e) => (None, e),
                };
                let give_up = Utc::now() - d.created_at > chrono::Duration::hours(RETRY_WINDOW_HOURS);
                sqlx::query(
                    "UPDATE platform_webhook_deliveries SET attempts = $2, last_status = $3, last_error = $4, \
                     state = CASE WHEN $5 THEN 'failed' ELSE 'pending' END, \
                     next_attempt_at = now() + make_interval(secs => $6::float8) WHERE id = $1",
                )
                .bind(&d.id)
                .bind(attempts)
                .bind(status)
                .bind(&error)
                .bind(give_up)
                .bind(backoff_secs(attempts) as f64)
                .execute(db)
                .await?;
            }
        }
    }
    Ok(due.len())
}

/// Monthly number fees: one `number_*_month` usage row per live API number per
/// calendar month (idempotent per number and month, so re-runs are free).
pub async fn record_number_months(db: &PgPool) -> Result<u64, sqlx::Error> {
    let month = Utc::now().format("%Y-%m").to_string();
    let rows: Vec<(String, String, Option<String>, String)> = sqlx::query_as(
        "SELECT id, project_id, account_id, type FROM phone_numbers WHERE project_id IS NOT NULL AND released_at IS NULL AND simulated = false",
    )
    .fetch_all(db)
    .await?;
    let mut n = 0;
    for (id, project, account, kind) in rows {
        let meter = if kind == "toll_free" { "number_tollfree_month" } else { "number_local_month" };
        let event = super::UsageEvent {
            project_id: project,
            account_id: account,
            key_id: None,
            meter: meter.to_string(),
            quantity: 1.0,
            unit: Some("month".to_string()),
            ref_id: Some(id.clone()),
            idempotency: Some(format!("number:{id}:{month}")),
        };
        if super::record_usage(db, event).await.is_ok() {
            n += 1;
        }
    }
    Ok(n)
}

/// Background worker: webhook deliveries every 5 s, number months every 6 h.
/// Started only when the Platform API is switched on.
pub fn spawn_worker(db: PgPool) {
    tokio::spawn(async move {
        let client = http_client();
        let mut ticks: u64 = 0;
        loop {
            if ticks % (6 * 60 * 12) == 0 {
                if let Err(e) = record_number_months(&db).await {
                    tracing::warn!("platform number months: {e}");
                }
            }
            if let Err(e) = deliver_due(&db, &client).await {
                tracing::warn!("platform webhook delivery: {e}");
            }
            ticks = ticks.wrapping_add(1);
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });
}

