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
//! only, no private, loopback or link-local addresses) and connects to exactly
//! the address that check resolved (TLS still names the host), so a DNS
//! change can't turn an endpoint into a request to our own network.
//!
//! ## One queue, two endpoint kinds, two signers (migration 057)
//!
//! The same queue carries every outbound Allternit event
//! (`routes::allternit_events`). An endpoint row has:
//!
//! * `kind` — `platform_webhook` (a project's `/v1/webhooks` endpoint; body =
//!   [`envelope`], exactly as before) or `mcp_subscription` (an MCP Events
//!   subscription made at the mcp.allternit.com edge; body =
//!   `{eventId, name, timestamp, data, cursor}` plus an
//!   `X-MCP-Subscription-Id` header).
//! * `signer` — `allternit` (the `allternit-signature` above; every 051
//!   endpoint) or `standard_webhooks` (`webhook-id` = event id, stable across
//!   retries; `webhook-timestamp`; `webhook-signature: v1,<base64>` keyed by
//!   the decoded `whsec_` secret, dual-signed with the previous secret during
//!   a rotation window).
//!
//! MCP subscriptions additionally: never get a body over 256 KiB (failed, not
//! sent); a `410 Gone` ends the subscription and a `413` fails the delivery
//! without retry; an expired (`refresh_before` passed) or ended subscription
//! gets nothing further. Platform API endpoints keep the 051 retry rules.

use std::net::IpAddr;
use std::time::Duration;

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use sqlx::PgPool;

use super::{new_id, PlatformError};

/// Event types a webhook can subscribe to. `*` subscribes to all of them.
/// Defined by the one registry (`routes::allternit_events`).
pub const EVENT_TYPES: [&str; 6] = crate::routes::allternit_events::PLATFORM_EVENTS;

/// Endpoint kinds (`platform_webhooks.kind`).
pub const KIND_PLATFORM: &str = "platform_webhook";
pub const KIND_MCP: &str = "mcp_subscription";
/// Signers (`platform_webhooks.signer`).
pub const SIGNER_ALLTERNIT: &str = "allternit";
pub const SIGNER_STANDARD: &str = "standard_webhooks";

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
    // The MCP protocol crate's rule also covers multicast, benchmarking,
    // reserved and documentation ranges; either saying "private" refuses.
    if !mcp_protocol::events::is_public_ip(&ip) {
        return true;
    }
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
    check_url_resolved(raw).await.map(|(url, _)| url)
}

/// [`check_url`], also returning the vetted address to connect to, so the
/// request can't be re-resolved to somewhere else between check and connect.
pub async fn check_url_resolved(raw: &str) -> Result<(reqwest::Url, std::net::SocketAddr), PlatformError> {
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
    Ok((url, addrs[0]))
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
    endpoint_id: String,
    endpoint_kind: String,
    signer: String,
    url: String,
    secret: String,
    previous_secret: Option<String>,
    previous_secret_until: Option<DateTime<Utc>>,
    refresh_before: Option<DateTime<Utc>>,
    endpoint_ended: bool,
    event_id: String,
    kind: String,
    project_id: Option<String>,
    account_id: Option<String>,
    data: Value,
    event_created: DateTime<Utc>,
    occurred_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

/// Why a POST produced no HTTP answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PostError {
    /// The URL failed [`check_url`] (scheme, credentials, DNS, private address).
    Refused(String),
    Timeout,
    Connect,
    Tls,
    Other,
}

impl PostError {
    pub fn message(&self) -> String {
        match self {
            PostError::Refused(m) => m.clone(),
            PostError::Timeout => "timed out".into(),
            PostError::Connect | PostError::Tls | PostError::Other => "connection failed".into(),
        }
    }
}

/// One POST to a vetted address, with exactly `headers` (callers set
/// `content-type`). `Ok((status, body))` for any HTTP answer.
/// The connection goes to the address [`check_url_resolved`] approved (TLS
/// still verifies the hostname), redirects are never followed, 10 s timeout.
pub async fn post_vetted(url: &str, headers: &[(String, String)], body: &[u8], max_reply: usize) -> Result<(u16, Vec<u8>), PostError> {
    let (url, addr) = check_url_resolved(url).await.map_err(|e| PostError::Refused(e.message))?;
    let mut builder = reqwest::Client::builder().timeout(Duration::from_secs(10)).redirect(reqwest::redirect::Policy::none());
    if let Some(host) = url.host_str() {
        if host.parse::<IpAddr>().is_err() {
            builder = builder.resolve(host, addr);
        }
    }
    let client = builder.build().map_err(|_| PostError::Other)?;
    let mut req = client.post(url);
    for (k, v) in headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let resp = req.body(body.to_vec()).send().await.map_err(|e| {
        if e.is_timeout() {
            PostError::Timeout
        } else if format!("{e:?}").to_ascii_lowercase().contains("certificate") || format!("{e:?}").to_ascii_lowercase().contains("tls") {
            PostError::Tls
        } else if e.is_connect() {
            PostError::Connect
        } else {
            PostError::Other
        }
    })?;
    let status = resp.status().as_u16();
    let mut out = Vec::new();
    if max_reply > 0 {
        let mut stream = resp;
        while let Ok(Some(chunk)) = stream.chunk().await {
            out.extend_from_slice(&chunk);
            if out.len() > max_reply {
                out.truncate(max_reply);
                break;
            }
        }
    }
    Ok((status, out))
}

/// Standard Webhooks headers for one message: `webhook-id`,
/// `webhook-timestamp`, `webhook-signature` (one `v1,` per key).
pub fn standard_headers(keys: &[&[u8]], msg_id: &str, ts: i64, body: &[u8]) -> Vec<(String, String)> {
    use mcp_protocol::webhooks as sw;
    vec![
        (sw::HEADER_ID.to_string(), msg_id.to_string()),
        (sw::HEADER_TIMESTAMP.to_string(), ts.to_string()),
        (sw::HEADER_SIGNATURE.to_string(), sw::sign_all(keys, msg_id, ts, body)),
    ]
}

/// The delivery body for an endpoint kind (one envelope builder per kind).
fn body_for(d: &Due) -> Vec<u8> {
    let v = if d.endpoint_kind == KIND_MCP {
        let at = d.occurred_at.unwrap_or(d.event_created);
        mcp_protocol::events::event_envelope(&d.event_id, &d.kind, &at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true), d.data.clone(), None)
    } else {
        envelope(&d.event_id, &d.kind, d.event_created, d.project_id.as_deref().unwrap_or_default(), d.account_id.as_deref(), &d.data)
    };
    serde_json::to_vec(&v).unwrap_or_default()
}

/// The signing headers for an endpoint's signer (one header builder per signer).
fn headers_for(d: &Due, ts: i64, body: &[u8]) -> Result<Vec<(String, String)>, String> {
    let mut headers = vec![("content-type".to_string(), "application/json".to_string()), ("user-agent".to_string(), "Allternit-Webhooks/1.0".to_string())];
    if d.signer == SIGNER_STANDARD {
        let key = mcp_protocol::webhooks::parse_secret(&d.secret).map_err(|e| format!("endpoint secret unusable: {e}"))?;
        let previous = d
            .previous_secret
            .as_deref()
            .filter(|_| d.previous_secret_until.is_some_and(|until| until > Utc::now()))
            .and_then(|p| mcp_protocol::webhooks::parse_secret(p).ok());
        let mut keys: Vec<&[u8]> = vec![&key];
        if let Some(p) = &previous {
            keys.push(p);
        }
        headers.extend(standard_headers(&keys, &d.event_id, ts, body));
    } else {
        headers.push(("allternit-signature".to_string(), sign(&d.secret, ts, body)));
    }
    if d.endpoint_kind == KIND_MCP {
        headers.push((mcp_protocol::events::HEADER_SUBSCRIPTION_ID.to_string(), d.endpoint_id.clone()));
    }
    Ok(headers)
}

pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("reqwest client builds")
}

async fn finish_failed(db: &PgPool, id: &str, attempts: i32, status: Option<i32>, error: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE platform_webhook_deliveries SET state = 'failed', attempts = $2, last_status = $3, last_error = $4 WHERE id = $1")
        .bind(id)
        .bind(attempts)
        .bind(status)
        .bind(error)
        .execute(db)
        .await?;
    Ok(())
}

/// End an MCP subscription: nothing further is delivered to it.
pub async fn deactivate_endpoint(db: &PgPool, endpoint_id: &str, reason: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE platform_webhooks SET deleted_at = COALESCE(deleted_at, now()), deactivated_reason = $2, updated_at = now() WHERE id = $1")
        .bind(endpoint_id)
        .bind(reason)
        .execute(db)
        .await?;
    sqlx::query("UPDATE platform_webhook_deliveries SET state = 'failed', last_error = $2 WHERE webhook_id = $1 AND state = 'pending'")
        .bind(endpoint_id)
        .bind(format!("subscription ended: {reason}"))
        .execute(db)
        .await?;
    Ok(())
}

/// Sends one delivery. Production is [`HttpPoster`]; tests record instead.
#[async_trait::async_trait]
pub trait Poster: Send + Sync {
    async fn post(&self, url: &str, headers: &[(String, String)], body: &[u8]) -> Result<(u16, Vec<u8>), PostError>;
}

/// [`post_vetted`]: SSRF-checked, address-pinned, no redirects, 10 s.
pub struct HttpPoster;

#[async_trait::async_trait]
impl Poster for HttpPoster {
    async fn post(&self, url: &str, headers: &[(String, String)], body: &[u8]) -> Result<(u16, Vec<u8>), PostError> {
        post_vetted(url, headers, body, 0).await
    }
}

/// Deliver up to one batch of due deliveries. Returns how many were attempted.
/// The client argument is kept for callers; each POST builds its own
/// address-pinned client (see [`post_vetted`]).
pub async fn deliver_due(db: &PgPool, _client: &reqwest::Client) -> Result<usize, sqlx::Error> {
    deliver_due_with(db, &HttpPoster).await
}

/// [`deliver_due`] through any [`Poster`].
pub async fn deliver_due_with(db: &PgPool, poster: &dyn Poster) -> Result<usize, sqlx::Error> {
    let due: Vec<Due> = sqlx::query_as(
        "UPDATE platform_webhook_deliveries d SET next_attempt_at = now() + interval '5 minutes' \
         FROM platform_webhooks w, platform_events e \
         WHERE d.id IN (SELECT id FROM platform_webhook_deliveries WHERE state = 'pending' AND next_attempt_at <= now() ORDER BY next_attempt_at LIMIT $1 FOR UPDATE SKIP LOCKED) \
           AND w.id = d.webhook_id AND e.id = d.event_id \
         RETURNING d.id, d.attempts, w.id AS endpoint_id, w.kind AS endpoint_kind, w.signer, w.url, w.secret, w.previous_secret, w.previous_secret_until, \
                   w.refresh_before, (w.deleted_at IS NOT NULL) AS endpoint_ended, \
                   e.id AS event_id, e.type AS kind, e.project_id, e.account_id, e.data, e.created_at AS event_created, e.occurred_at, d.created_at",
    )
    .bind(BATCH)
    .fetch_all(db)
    .await?;
    for d in &due {
        let mcp = d.endpoint_kind == KIND_MCP;
        let attempts = d.attempts + 1;
        if mcp && d.endpoint_ended {
            finish_failed(db, &d.id, d.attempts, None, "subscription ended").await?;
            continue;
        }
        if mcp && d.refresh_before.is_some_and(|r| r <= Utc::now()) {
            finish_failed(db, &d.id, d.attempts, None, "subscription expired").await?;
            continue;
        }
        let body = body_for(d);
        if mcp && body.len() > mcp_protocol::events::MAX_EVENT_BYTES {
            finish_failed(db, &d.id, d.attempts, None, "payload too large (over 256 KiB)").await?;
            continue;
        }
        let headers = match headers_for(d, Utc::now().timestamp(), &body) {
            Ok(h) => h,
            Err(e) => {
                finish_failed(db, &d.id, d.attempts, None, &e).await?;
                continue;
            }
        };
        let outcome = poster.post(&d.url, &headers, &body).await;
        match outcome {
            Ok((status, _)) if (200..300).contains(&status) => {
                sqlx::query("UPDATE platform_webhook_deliveries SET state = 'delivered', attempts = $2, last_status = $3, last_error = NULL, delivered_at = now() WHERE id = $1")
                    .bind(&d.id)
                    .bind(attempts)
                    .bind(status as i32)
                    .execute(db)
                    .await?;
            }
            Ok((410, _)) if mcp => {
                finish_failed(db, &d.id, attempts, Some(410), "HTTP 410").await?;
                deactivate_endpoint(db, &d.endpoint_id, "gone").await?;
            }
            Ok((413, _)) if mcp => {
                finish_failed(db, &d.id, attempts, Some(413), "HTTP 413").await?;
            }
            other => {
                let (status, error) = match other {
                    Ok((s, _)) => (Some(s as i32), format!("HTTP {s}")),
                    Err(e) => (None, e.message()),
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
/// Started when the Platform API is switched on.
pub fn spawn_worker(db: PgPool) {
    spawn_worker_with(db, true);
}

/// The delivery worker alone (no number fees): what the MCP edge needs when
/// the Platform API is off. Start exactly one of the two.
pub fn spawn_delivery_worker(db: PgPool) {
    spawn_worker_with(db, false);
}

fn spawn_worker_with(db: PgPool, number_months: bool) {
    tokio::spawn(async move {
        let client = http_client();
        let mut ticks: u64 = 0;
        loop {
            if number_months && ticks % (6 * 60 * 12) == 0 {
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

