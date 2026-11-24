//! Allternit Meetings calendar sync and auto-join (plan step 2.7,
//! docs/meetings/CONTRACT.md sections 2, 4, 7 and "Change 2026-11-13: ICS
//! calendar links").
//!
//! - **Sync.** Every active calendar connection is synced server-side every
//!   5 minutes, so auto-join works with the user's runtime off. Google: one
//!   windowed `events.list` (now - 1 h .. now + 7 d, `singleEvents`). A Google
//!   `syncToken` can't be combined with a time window, and events that drift
//!   into the 7-day window never show up as changes, so the window is fetched
//!   whole each time (one request per connection). ICS: a full fetch with
//!   `If-None-Match` / `If-Modified-Since`, and an unconditional fetch at least
//!   hourly so recurring instances entering the window get expanded. Meeting
//!   links come from structured conference fields first, then location, then
//!   description (section 7 patterns). Events that vanish or are cancelled
//!   are removed while Gizzi has not joined them yet.
//! - **Auto-join.** The `meeting_join` pass (run from the scheduler tick)
//!   sends Gizzi 2 minutes before start when the per-meeting `auto_join`
//!   override or the user's rule says so, with the same checks as
//!   `POST /meetings/:id/sessions`, then moves due `scheduled` sessions to
//!   `launching`, where the pod launcher (`meeting_launcher`) picks them up.
//! - **Feed fetches** are SSRF-guarded: https only, every resolved address
//!   must be public, the connection is pinned to the checked address,
//!   redirects are re-checked hop by hop, and size and time are capped.

pub mod ics;

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration as StdDuration;

use chrono::{DateTime, Duration, Utc};
use futures::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::routes::meeting_links::{self, AutoJoinRule, ParsedLink};
use crate::routes::meeting_state::{EndReason, State as SessionState};
use crate::routes::meetings::{self, Caller, MeetingError, When};
use crate::ApiState;

/// Gizzi joins this long before the start (same as `POST ... when=start`).
pub(crate) const JOIN_LEAD: Duration = meetings::JOIN_LEAD;
/// Auto-join still fires this long after the start (server restart, a rule
/// switched on mid-call); later than that the user sends Gizzi by hand.
const LATE_JOIN_GRACE: Duration = Duration::minutes(15);
/// A meeting with no end time counts as an hour long.
const DEFAULT_LENGTH: Duration = Duration::hours(1);
const SYNC_EVERY_MINUTES: i64 = 5;
const SYNC_AHEAD: Duration = Duration::days(7);
const ICS_FULL_FETCH_EVERY: Duration = Duration::hours(1);
const MAX_FEED_BYTES: usize = 5 * 1024 * 1024;
const MAX_REDIRECTS: usize = 3;
const FEED_TIMEOUT: StdDuration = StdDuration::from_secs(20);
const GOOGLE_MAX_PAGES: usize = 10;

pub(crate) const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
pub(crate) const GOOGLE_REVOKE_URL: &str = "https://oauth2.googleapis.com/revoke";
const GOOGLE_EVENTS_URL: &str = "https://www.googleapis.com/calendar/v3/calendars/primary/events";

// ---------------------------------------------------------------------------
// Shared shapes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Attendee {
    pub email: Option<String>,
    pub name: Option<String>,
    pub response: Option<String>,
    pub is_self: bool,
}

/// One meeting occurrence from any calendar, ready to upsert.
#[derive(Debug, Clone)]
pub struct CalendarEvent {
    /// Google event id, or the ICS UID (plus `/<original start>` for an
    /// instance of a recurring event).
    pub external_id: String,
    pub title: String,
    pub starts_at: DateTime<Utc>,
    pub ends_at: Option<DateTime<Utc>>,
    pub link: ParsedLink,
    pub attendees: Vec<Attendee>,
    /// None when the event has no attendee list (never 1:1).
    pub attendee_count: Option<i32>,
    pub external_attendee_count: Option<i32>,
}

/// `&amp;` etc. in HTML event descriptions, so Zoom `pwd` links survive.
pub(crate) fn decode_entities(text: &str) -> String {
    text.replace("&amp;", "&").replace("&quot;", "\"").replace("&#39;", "'").replace("&lt;", "<").replace("&gt;", ">")
}

/// credential_cipher, like `provider_tokens`: plaintext only when
/// `ALLTERNIT_CREDENTIALS_KEY` is unset (main.rs warns; production refuses).
pub(crate) fn seal(state: &ApiState, plaintext: &str) -> Result<String, MeetingError> {
    match &state.credential_cipher {
        Some(cipher) => cipher.encrypt(plaintext).map_err(|e| {
            tracing::error!("meeting calendar: encrypt failed: {e}");
            MeetingError::internal()
        }),
        None => Ok(plaintext.to_string()),
    }
}

pub(crate) fn open(state: &ApiState, stored: &str) -> Option<String> {
    match &state.credential_cipher {
        Some(cipher) => cipher.decrypt(stored).map_err(|e| tracing::error!("meeting calendar: decrypt failed: {e}")).ok(),
        None => Some(stored.to_string()),
    }
}

pub(crate) struct GoogleConfig {
    pub client_id: String,
    pub client_secret: String,
}

impl GoogleConfig {
    /// `GOOGLE_CALENDAR_CLIENT_ID` / `GOOGLE_CALENDAR_CLIENT_SECRET` (local
    /// dev: Keychain item `allternit-google-calendar-oauth`).
    pub fn from_env() -> Option<Self> {
        let get = |k: &str| std::env::var(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        Some(GoogleConfig { client_id: get("GOOGLE_CALENDAR_CLIENT_ID")?, client_secret: get("GOOGLE_CALENDAR_CLIENT_SECRET")? })
    }
}

pub(crate) fn http() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(StdDuration::from_secs(15))
            .connect_timeout(StdDuration::from_secs(5))
            .user_agent("Allternit-Calendar/1.0")
            .build()
            .expect("calendar http client builds")
    })
}

fn is_missing_table(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(db) if db.code().as_deref() == Some("42P01"))
}

/// Production applies migrations by hand: until 130 is in, the background
/// passes log once instead of every minute.
fn tables_missing(error: &sqlx::Error) -> bool {
    static WARNED: AtomicBool = AtomicBool::new(false);
    if is_missing_table(error) {
        if !WARNED.swap(true, Ordering::Relaxed) {
            tracing::warn!("meetings: tables missing (migration 130 not applied); calendar sync and auto-join idle");
        }
        return true;
    }
    false
}

// ---------------------------------------------------------------------------
// SSRF-guarded feed fetch
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedError {
    InvalidUrl,
    Blocked,
    Http(u16),
    TooLarge,
    Timeout,
    Network,
}

impl FeedError {
    pub fn message(&self) -> String {
        match self {
            FeedError::InvalidUrl => "Paste the calendar's https (or webcal) link.".into(),
            FeedError::Blocked => "That link points to a private or local network address.".into(),
            FeedError::Http(code) => format!("The calendar server answered {code}. Check the link is still published."),
            FeedError::TooLarge => "That calendar is larger than 5 MB.".into(),
            FeedError::Timeout => "The calendar server took too long to answer.".into(),
            FeedError::Network => "Couldn't reach the calendar server.".into(),
        }
    }

    /// The feed is gone or now refused for good: the user must paste it again.
    fn is_permanent(&self) -> bool {
        matches!(self, FeedError::Blocked | FeedError::InvalidUrl | FeedError::Http(401 | 403 | 404 | 410))
    }
}

/// Trim and turn `webcal://` / `webcals://` into `https://`; only https with
/// a host is accepted.
pub fn normalise_feed_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > 2048 {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    let rewritten = if lower.starts_with("webcals://") {
        format!("https://{}", &trimmed[10..])
    } else if lower.starts_with("webcal://") {
        format!("https://{}", &trimmed[9..])
    } else {
        trimmed.to_string()
    };
    let url = url::Url::parse(&rewritten).ok()?;
    (url.scheme() == "https" && url.host_str().is_some_and(|h| !h.is_empty())).then(|| url.to_string())
}

/// Addresses a feed fetch may never reach: loopback, private, link-local,
/// CGNAT (tailnet), multicast, documentation, reserved, and IPv6 forms that
/// embed an IPv4 address.
pub fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => blocked_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return blocked_v4(v4);
            }
            let s = v6.segments();
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00 // unique local
                || (s[0] & 0xffc0) == 0xfe80 // link-local
                || (s[0] & 0xffc0) == 0xfec0 // site-local (deprecated)
                || (s[0] == 0x2001 && s[1] == 0x0db8) // documentation
                || (s[0] == 0x0064 && s[1] == 0xff9b) // NAT64
                || s[0] == 0x2002 // 6to4
                || (s[0] == 0x2001 && s[1] == 0) // Teredo
                || (s[0..6] == [0, 0, 0, 0, 0, 0]) // IPv4-compatible
        }
    }
}

fn blocked_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || o[0] == 0
        || (o[0] == 100 && (64..128).contains(&o[1])) // CGNAT / tailnet
        || (o[0] == 192 && o[1] == 0 && o[2] == 0) // IETF protocol assignments
        || (o[0] == 198 && (o[1] == 18 || o[1] == 19)) // benchmarking
        || o[0] >= 240 // reserved
}

/// Resolve `host:port` and refuse it unless every address is public.
async fn public_addr(host: &str, port: u16) -> Result<SocketAddr, FeedError> {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = host.parse::<IpAddr>() {
        return if is_blocked_ip(ip) { Err(FeedError::Blocked) } else { Ok(SocketAddr::new(ip, port)) };
    }
    let addrs: Vec<SocketAddr> = tokio::time::timeout(StdDuration::from_secs(5), tokio::net::lookup_host((host, port)))
        .await
        .map_err(|_| FeedError::Timeout)?
        .map_err(|_| FeedError::Network)?
        .collect();
    if addrs.is_empty() {
        return Err(FeedError::Network);
    }
    if addrs.iter().any(|a| is_blocked_ip(a.ip())) {
        return Err(FeedError::Blocked);
    }
    Ok(addrs[0])
}

#[derive(Debug, Default)]
pub struct FetchedFeed {
    pub body: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub not_modified: bool,
}

/// Fetch a calendar feed (`url` already normalised to https). `validators`
/// = (ETag, Last-Modified) from the last fetch for a conditional GET.
pub async fn fetch_feed(url: &str, validators: Option<(Option<&str>, Option<&str>)>) -> Result<FetchedFeed, FeedError> {
    tokio::time::timeout(FEED_TIMEOUT, fetch_feed_inner(url, validators)).await.map_err(|_| FeedError::Timeout)?
}

async fn fetch_feed_inner(url: &str, validators: Option<(Option<&str>, Option<&str>)>) -> Result<FetchedFeed, FeedError> {
    let mut current = url::Url::parse(url).map_err(|_| FeedError::InvalidUrl)?;
    for _hop in 0..=MAX_REDIRECTS {
        if current.scheme() != "https" {
            return Err(FeedError::InvalidUrl);
        }
        let host = current.host_str().ok_or(FeedError::InvalidUrl)?.to_string();
        let port = current.port_or_known_default().unwrap_or(443);
        let addr = public_addr(&host, port).await?;
        // Pin the connection to the address just checked (no DNS rebinding
        // between the check and the connect); TLS still verifies `host`.
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .resolve(host.trim_start_matches('[').trim_end_matches(']'), addr)
            .connect_timeout(StdDuration::from_secs(5))
            .timeout(StdDuration::from_secs(15))
            .user_agent("Allternit-Calendar/1.0")
            .build()
            .map_err(|_| FeedError::Network)?;
        let mut request = client.get(current.clone()).header("accept", "text/calendar, */*;q=0.5");
        if let Some((etag, last_modified)) = validators {
            if let Some(etag) = etag {
                request = request.header("if-none-match", etag);
            }
            if let Some(lm) = last_modified {
                request = request.header("if-modified-since", lm);
            }
        }
        let mut response = request.send().await.map_err(|e| if e.is_timeout() { FeedError::Timeout } else { FeedError::Network })?;
        let status = response.status();
        if status.is_redirection() && status != reqwest::StatusCode::NOT_MODIFIED {
            let location = response
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .ok_or(FeedError::Http(status.as_u16()))?;
            current = current.join(location).map_err(|_| FeedError::InvalidUrl)?;
            continue;
        }
        let header = |name: &str| response.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
        let (etag, last_modified) = (header("etag"), header("last-modified"));
        if status == reqwest::StatusCode::NOT_MODIFIED {
            return Ok(FetchedFeed { etag, last_modified, not_modified: true, ..Default::default() });
        }
        if !status.is_success() {
            return Err(FeedError::Http(status.as_u16()));
        }
        if response.content_length().is_some_and(|n| n as usize > MAX_FEED_BYTES) {
            return Err(FeedError::TooLarge);
        }
        let mut bytes: Vec<u8> = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| FeedError::Network)? {
            if bytes.len() + chunk.len() > MAX_FEED_BYTES {
                return Err(FeedError::TooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }
        return Ok(FetchedFeed { body: String::from_utf8_lossy(&bytes).into_owned(), etag, last_modified, not_modified: false });
    }
    Err(FeedError::Http(310))
}

// ---------------------------------------------------------------------------
// Connections
// ---------------------------------------------------------------------------

pub(crate) const CONNECTION_COLUMNS: &str = "id, user_id, provider, account_email, label, encrypted_refresh_token, \
    encrypted_access_token, access_expires_at, encrypted_feed_url, scopes, sync_token, status, last_synced_at, created_at";

#[derive(Debug, Clone, FromRow)]
pub(crate) struct ConnectionRow {
    pub id: Uuid,
    pub user_id: String,
    pub provider: String,
    pub account_email: Option<String>,
    pub label: Option<String>,
    pub encrypted_refresh_token: Option<String>,
    pub encrypted_access_token: Option<String>,
    pub access_expires_at: Option<DateTime<Utc>>,
    pub encrypted_feed_url: Option<String>,
    pub scopes: Vec<String>,
    pub sync_token: Option<String>,
    pub status: String,
    pub last_synced_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// The `CalendarConnection` JSON (types.ts). Never carries a token or the
/// feed URL.
pub(crate) fn connection_json(row: &ConnectionRow) -> Value {
    json!({
        "id": row.id,
        "provider": row.provider,
        "account_email": row.account_email,
        "label": row.label,
        "status": row.status,
        "scopes": row.scopes,
        "last_synced_at": row.last_synced_at.map(meetings::ts),
        "created_at": meetings::ts(row.created_at),
    })
}

/// ICS conditional-fetch validators, kept in `sync_token` as JSON.
#[derive(Debug, Default, serde::Serialize, Deserialize)]
pub(crate) struct IcsValidators {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub full_at: Option<DateTime<Utc>>,
}

impl IcsValidators {
    pub fn from_fetch(fetched: &FetchedFeed, now: DateTime<Utc>) -> Self {
        IcsValidators { etag: fetched.etag.clone(), last_modified: fetched.last_modified.clone(), full_at: Some(now) }
    }
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

#[derive(Debug)]
enum SyncError {
    /// The grant or feed is gone: status -> needs_reauth.
    NeedsReauth(String),
    /// Try again next cycle.
    Transient(String),
}

impl From<MeetingError> for SyncError {
    fn from(error: MeetingError) -> Self {
        SyncError::Transient(format!("{}: {}", error.code, error.message))
    }
}

impl From<sqlx::Error> for SyncError {
    fn from(error: sqlx::Error) -> Self {
        SyncError::Transient(format!("database: {error}"))
    }
}

/// Sync every active connection not synced in the last 5 minutes. Claimed
/// with `FOR UPDATE SKIP LOCKED` by bumping `last_synced_at`, so replicas
/// never sync one connection twice.
pub async fn sync_due_connections(state: &Arc<ApiState>) -> anyhow::Result<usize> {
    let claimed = sqlx::query_as::<_, ConnectionRow>(&format!(
        "UPDATE calendar_connections SET last_synced_at = now() WHERE id IN ( \
             SELECT id FROM calendar_connections \
             WHERE status = 'active' AND provider IN ('google', 'ics') \
               AND (last_synced_at IS NULL OR last_synced_at < now() - make_interval(mins => $1)) \
             ORDER BY last_synced_at ASC NULLS FIRST LIMIT 50 \
             FOR UPDATE SKIP LOCKED) \
         RETURNING {CONNECTION_COLUMNS}"
    ))
    .bind(SYNC_EVERY_MINUTES as i32)
    .fetch_all(&state.db)
    .await;
    let claimed = match claimed {
        Ok(rows) => rows,
        Err(error) if tables_missing(&error) => return Ok(0),
        Err(error) => return Err(error.into()),
    };
    let count = claimed.len();
    futures::stream::iter(claimed)
        .for_each_concurrent(8, |conn| async move { sync_connection(state, &conn, None).await })
        .await;
    Ok(count)
}

/// Sync one connection now (after connect). Errors are logged and stored as
/// status; the next cycle retries transient ones.
pub(crate) async fn sync_connection_by_id(state: &Arc<ApiState>, id: Uuid) {
    let row = sqlx::query_as::<_, ConnectionRow>(&format!(
        "UPDATE calendar_connections SET last_synced_at = now() WHERE id = $1 AND status = 'active' RETURNING {CONNECTION_COLUMNS}"
    ))
    .bind(id)
    .fetch_optional(&state.db)
    .await;
    match row {
        Ok(Some(conn)) => sync_connection(state, &conn, None).await,
        Ok(None) => {}
        Err(error) => tracing::warn!(%id, "meeting calendar: first sync skipped: {error}"),
    }
}

/// `prefetched`: an ICS body fetched moments ago (the connect route).
pub(crate) async fn sync_connection(state: &Arc<ApiState>, conn: &ConnectionRow, prefetched: Option<FetchedFeed>) {
    // Free plans can connect (contract) but Gizzi can't join for them:
    // don't spend provider calls until they upgrade.
    match crate::routes::meetings::meetings_plan(&state.db, &conn.user_id).await {
        Ok(plan) if plan == "free" => return,
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(id = %conn.id, "meeting calendar: plan lookup failed: {error}");
            return;
        }
    }
    let result = match conn.provider.as_str() {
        "google" => sync_google(state, conn).await,
        "ics" => sync_ics(state, conn, prefetched).await,
        _ => Ok(()),
    };
    match result {
        Ok(()) => {}
        Err(SyncError::NeedsReauth(why)) => {
            tracing::info!(id = %conn.id, provider = %conn.provider, "meeting calendar: needs reconnect: {why}");
            let _ = sqlx::query(
                "UPDATE calendar_connections SET status = 'needs_reauth', encrypted_access_token = NULL, updated_at = now() WHERE id = $1",
            )
            .bind(conn.id)
            .execute(&state.db)
            .await;
        }
        Err(SyncError::Transient(why)) => {
            tracing::warn!(id = %conn.id, provider = %conn.provider, "meeting calendar: sync failed: {why}")
        }
    }
}

async fn sync_ics(state: &Arc<ApiState>, conn: &ConnectionRow, prefetched: Option<FetchedFeed>) -> Result<(), SyncError> {
    let now = Utc::now();
    let fetched = match prefetched {
        Some(f) => f,
        None => {
            let url = conn
                .encrypted_feed_url
                .as_deref()
                .and_then(|s| open(state, s))
                .ok_or_else(|| SyncError::Transient("feed url unreadable".into()))?;
            let previous: IcsValidators =
                conn.sync_token.as_deref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default();
            let conditional = previous.full_at.is_some_and(|at| now - at < ICS_FULL_FETCH_EVERY);
            let validators = conditional.then(|| (previous.etag.as_deref(), previous.last_modified.as_deref()));
            match fetch_feed(&url, validators).await {
                Ok(f) if f.not_modified => return Ok(()),
                Ok(f) => f,
                Err(e) if e.is_permanent() => return Err(SyncError::NeedsReauth(e.message())),
                Err(e) => return Err(SyncError::Transient(e.message())),
            }
        }
    };
    let (from, to) = (now - Duration::hours(12), now + SYNC_AHEAD);
    let feed = ics::parse_feed(&fetched.body, from, to)
        .ok_or_else(|| SyncError::NeedsReauth("the link no longer returns a calendar".into()))?;
    apply_events(&state.db, conn, "calendar_ics", &feed.events, &[], Some((now, to))).await?;
    sqlx::query("UPDATE calendar_connections SET sync_token = $2, updated_at = now() WHERE id = $1")
        .bind(conn.id)
        .bind(IcsValidators::from_fetch(&fetched, now).to_json())
        .execute(&state.db)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Google
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct TokenResponse {
    pub access_token: Option<String>,
    pub expires_in: Option<i64>,
    pub refresh_token: Option<String>,
    pub scope: Option<String>,
    pub id_token: Option<String>,
    pub error: Option<String>,
}

async fn google_access_token(state: &ApiState, conn: &ConnectionRow) -> Result<String, SyncError> {
    if let (Some(sealed), Some(expires)) = (&conn.encrypted_access_token, conn.access_expires_at) {
        if expires > Utc::now() + Duration::seconds(60) {
            if let Some(token) = open(state, sealed) {
                return Ok(token);
            }
        }
    }
    let config = GoogleConfig::from_env().ok_or_else(|| SyncError::Transient("Google calendar client not configured".into()))?;
    let refresh = conn
        .encrypted_refresh_token
        .as_deref()
        .and_then(|s| open(state, s))
        .ok_or_else(|| SyncError::NeedsReauth("refresh token unreadable".into()))?;
    let response = http()
        .post(GOOGLE_TOKEN_URL)
        .form(&[
            ("client_id", config.client_id.as_str()),
            ("client_secret", config.client_secret.as_str()),
            ("refresh_token", refresh.as_str()),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await
        .map_err(|e| SyncError::Transient(format!("token refresh: {e}")))?;
    let status = response.status();
    let body: TokenResponse =
        response.json().await.map_err(|e| SyncError::Transient(format!("token refresh body: {e}")))?;
    if body.error.as_deref() == Some("invalid_grant") {
        return Err(SyncError::NeedsReauth("invalid_grant".into()));
    }
    let access = body
        .access_token
        .filter(|_| status.is_success())
        .ok_or_else(|| SyncError::Transient(format!("token refresh answered {status}")))?;
    let expires = Utc::now() + Duration::seconds(body.expires_in.unwrap_or(3600));
    let new_refresh = body.refresh_token.as_deref().map(|r| seal(state, r)).transpose()?;
    sqlx::query(
        "UPDATE calendar_connections SET encrypted_access_token = $2, access_expires_at = $3, \
         encrypted_refresh_token = COALESCE($4, encrypted_refresh_token), updated_at = now() WHERE id = $1",
    )
    .bind(conn.id)
    .bind(seal(state, &access)?)
    .bind(expires)
    .bind(new_refresh)
    .execute(&state.db)
    .await?;
    Ok(access)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GEventList {
    #[serde(default)]
    items: Vec<GEvent>,
    next_page_token: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GEvent {
    pub id: String,
    pub status: Option<String>,
    pub summary: Option<String>,
    pub start: Option<GTime>,
    pub end: Option<GTime>,
    pub hangout_link: Option<String>,
    pub conference_data: Option<GConference>,
    pub location: Option<String>,
    pub description: Option<String>,
    pub attendees: Option<Vec<GAttendee>>,
    pub event_type: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GTime {
    pub date_time: Option<String>,
    pub date: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GConference {
    #[serde(default)]
    pub entry_points: Vec<GEntryPoint>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GEntryPoint {
    pub entry_point_type: Option<String>,
    pub uri: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GAttendee {
    pub email: Option<String>,
    pub display_name: Option<String>,
    #[serde(rename = "self")]
    pub is_self: Option<bool>,
    pub resource: Option<bool>,
    pub response_status: Option<String>,
}

#[derive(Debug)]
pub(crate) enum GoogleItem {
    Meeting(CalendarEvent),
    /// Cancelled, or declined by the calendar's owner.
    Removed(String),
    /// All-day, no meeting link, out-of-office and the like.
    Skip,
}

const CONSUMER_DOMAINS: &[&str] =
    &["gmail.com", "googlemail.com", "outlook.com", "hotmail.com", "live.com", "icloud.com", "me.com", "yahoo.com"];

pub(crate) fn google_item(event: GEvent, account_email: &str) -> GoogleItem {
    let declined = event
        .attendees
        .as_deref()
        .unwrap_or_default()
        .iter()
        .any(|a| a.is_self == Some(true) && a.response_status.as_deref() == Some("declined"));
    if event.status.as_deref() == Some("cancelled") || declined {
        return GoogleItem::Removed(event.id);
    }
    if matches!(event.event_type.as_deref(), Some("outOfOffice" | "focusTime" | "workingLocation" | "birthday")) {
        return GoogleItem::Skip;
    }
    let parse = |t: &Option<GTime>| {
        t.as_ref()
            .and_then(|t| t.date_time.as_deref())
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc))
    };
    let Some(starts_at) = parse(&event.start) else { return GoogleItem::Skip }; // all-day
    let structured = event
        .conference_data
        .as_ref()
        .into_iter()
        .flat_map(|c| c.entry_points.iter())
        .filter(|e| e.entry_point_type.as_deref() == Some("video"))
        .filter_map(|e| e.uri.as_deref())
        .chain(event.hangout_link.as_deref())
        .find_map(meeting_links::parse_link);
    let link = structured.or_else(|| {
        [event.location.as_deref(), event.description.as_deref()]
            .into_iter()
            .flatten()
            .find_map(|text| meeting_links::find_in_text(&decode_entities(text)))
    });
    let Some(link) = link else { return GoogleItem::Skip };

    let attendees: Vec<Attendee> = event
        .attendees
        .unwrap_or_default()
        .into_iter()
        .filter(|a| a.resource != Some(true))
        .map(|a| Attendee {
            email: a.email.map(|e| e.to_ascii_lowercase()),
            name: a.display_name,
            response: a.response_status,
            is_self: a.is_self == Some(true),
        })
        .collect();
    let attendee_count = (!attendees.is_empty()).then(|| attendees.len() as i32);
    let domain = account_email.rsplit_once('@').map(|(_, d)| d.to_ascii_lowercase());
    let external_attendee_count = match domain {
        Some(d) if !CONSUMER_DOMAINS.contains(&d.as_str()) && attendee_count.is_some() => Some(
            attendees
                .iter()
                .filter(|a| !a.is_self)
                .filter(|a| a.email.as_deref().and_then(|e| e.rsplit_once('@')).is_some_and(|(_, ad)| ad != d))
                .count() as i32,
        ),
        _ => None,
    };
    GoogleItem::Meeting(CalendarEvent {
        external_id: event.id,
        title: event.summary.unwrap_or_default().trim().to_string(),
        starts_at,
        ends_at: parse(&event.end).filter(|e| *e > starts_at),
        link,
        attendees,
        attendee_count,
        external_attendee_count,
    })
}

async fn sync_google(state: &Arc<ApiState>, conn: &ConnectionRow) -> Result<(), SyncError> {
    let token = google_access_token(state, conn).await?;
    let now = Utc::now();
    let (time_min, time_max) = (now - Duration::hours(1), now + SYNC_AHEAD);
    let account = conn.account_email.clone().unwrap_or_default();
    let mut events = Vec::new();
    let mut removed = Vec::new();
    let mut page: Option<String> = None;
    let mut complete = false;
    for _ in 0..GOOGLE_MAX_PAGES {
        let mut query: Vec<(&str, String)> = vec![
            ("timeMin", time_min.to_rfc3339()),
            ("timeMax", time_max.to_rfc3339()),
            ("singleEvents", "true".into()),
            ("showDeleted", "true".into()),
            ("maxResults", "250".into()),
        ];
        if let Some(p) = &page {
            query.push(("pageToken", p.clone()));
        }
        let response = http()
            .get(GOOGLE_EVENTS_URL)
            .bearer_auth(&token)
            .query(&query)
            .send()
            .await
            .map_err(|e| SyncError::Transient(format!("events.list: {e}")))?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(SyncError::NeedsReauth("events.list 401".into()));
        }
        if !status.is_success() {
            return Err(SyncError::Transient(format!("events.list answered {status}")));
        }
        let list: GEventList = response.json().await.map_err(|e| SyncError::Transient(format!("events.list body: {e}")))?;
        for event in list.items {
            match google_item(event, &account) {
                GoogleItem::Meeting(m) => events.push(m),
                GoogleItem::Removed(id) => removed.push(id),
                GoogleItem::Skip => {}
            }
        }
        match list.next_page_token {
            Some(next) => page = Some(next),
            None => {
                complete = true;
                break;
            }
        }
    }
    // Only a complete listing can say an event is gone.
    let reconcile = complete.then_some((now, time_max));
    apply_events(&state.db, conn, "calendar_google", &events, &removed, reconcile).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Upsert and reconcile
// ---------------------------------------------------------------------------

fn attendees_json(attendees: &[Attendee]) -> Value {
    Value::Array(
        attendees
            .iter()
            .map(|a| json!({ "email": a.email, "name": a.name, "response": a.response, "self": a.is_self }))
            .collect(),
    )
}

/// Upsert `events` for a connection, remove `removed` ids, and when
/// `reconcile = (from, to)` remove this connection's rows starting in that
/// range that the calendar no longer lists. Removal never touches a meeting
/// Gizzi has joined or recapped (see [`remove_calendar_meeting`]).
async fn apply_events(
    db: &PgPool,
    conn: &ConnectionRow,
    source: &str,
    events: &[CalendarEvent],
    removed: &[String],
    reconcile: Option<(DateTime<Utc>, DateTime<Utc>)>,
) -> Result<(), SyncError> {
    let reply_mode = crate::routes::org_meeting_settings::load_user_settings(db, &conn.user_id).await?.default_reply_mode;
    let mut seen: HashSet<&str> = HashSet::new();
    for event in events {
        seen.insert(event.external_id.as_str());
        upsert_event(db, conn, source, event, &reply_mode).await?;
    }
    let mut gone: Vec<String> = removed.iter().filter(|id| !seen.contains(id.as_str())).cloned().collect();
    if let Some((from, to)) = reconcile {
        let listed: Vec<String> = sqlx::query_scalar(
            "SELECT external_event_id FROM meetings WHERE calendar_connection_id = $1 \
             AND external_event_id IS NOT NULL AND starts_at >= $2 AND starts_at < $3",
        )
        .bind(conn.id)
        .bind(from)
        .bind(to)
        .fetch_all(db)
        .await?;
        gone.extend(listed.into_iter().filter(|id| !seen.contains(id.as_str())));
    }
    for external_id in gone {
        let id: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM meetings WHERE calendar_connection_id = $1 AND external_event_id = $2")
                .bind(conn.id)
                .bind(&external_id)
                .fetch_optional(db)
                .await?;
        if let Some(id) = id {
            remove_calendar_meeting(db, id).await?;
        }
    }
    Ok(())
}

async fn upsert_event(db: &PgPool, conn: &ConnectionRow, source: &str, event: &CalendarEvent, reply_mode: &str) -> Result<(), SyncError> {
    let one_on_one = meeting_links::is_one_on_one(event.attendee_count);
    let attendees = attendees_json(&event.attendees);
    let existing: Option<(Uuid, Option<DateTime<Utc>>, String, Option<Uuid>)> = sqlx::query_as(
        "SELECT id, starts_at, url_key, latest_session_id FROM meetings WHERE calendar_connection_id = $1 AND external_event_id = $2",
    )
    .bind(conn.id)
    .bind(&event.external_id)
    .fetch_optional(db)
    .await?;
    if let Some((id, old_start, old_key, latest)) = existing {
        let updated = sqlx::query(
            "UPDATE meetings SET platform = $2, title = $3, join_url = $4, url_key = $5, starts_at = $6, ends_at = $7, \
                 attendee_count = $8, external_attendee_count = $9, is_one_on_one = $10, attendees = $11, updated_at = now() \
             WHERE id = $1 AND (platform, title, join_url, url_key, starts_at, ends_at, attendee_count, \
                 external_attendee_count, is_one_on_one, attendees) IS DISTINCT FROM \
                 ($2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        )
        .bind(id)
        .bind(event.link.platform.as_str())
        .bind(&event.title)
        .bind(&event.link.join_url)
        .bind(&event.link.url_key)
        .bind(event.starts_at)
        .bind(event.ends_at)
        .bind(event.attendee_count)
        .bind(event.external_attendee_count)
        .bind(one_on_one)
        .bind(&attendees)
        .execute(db)
        .await;
        match updated {
            Ok(_) => {}
            Err(error) if meetings::is_unique_violation(&error) => {
                tracing::debug!(%id, "meeting calendar: moved event collides with another row; kept as is");
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        }
        if let Some(sid) = latest {
            if old_key != event.link.url_key {
                // New call link: the not-yet-launched bot is for the old one.
                drop_scheduled_session(db, id, sid).await?;
            } else if old_start != Some(event.starts_at) {
                sqlx::query(
                    "UPDATE meeting_sessions SET scheduled_join_at = $2, updated_at = now() WHERE id = $1 AND state = 'scheduled'",
                )
                .bind(sid)
                .bind(event.starts_at - JOIN_LEAD)
                .execute(db)
                .await?;
            }
        }
        return Ok(());
    }

    let inserted = sqlx::query(
        "INSERT INTO meetings (user_id, platform, title, join_url, url_key, starts_at, ends_at, source, \
             calendar_connection_id, external_event_id, attendee_count, external_attendee_count, is_one_on_one, \
             reply_mode, attendees) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)",
    )
    .bind(&conn.user_id)
    .bind(event.link.platform.as_str())
    .bind(&event.title)
    .bind(&event.link.join_url)
    .bind(&event.link.url_key)
    .bind(event.starts_at)
    .bind(event.ends_at)
    .bind(source)
    .bind(conn.id)
    .bind(&event.external_id)
    .bind(event.attendee_count)
    .bind(event.external_attendee_count)
    .bind(one_on_one)
    .bind(reply_mode)
    .bind(&attendees)
    .execute(db)
    .await;
    match inserted {
        Ok(_) => Ok(()),
        // The user already has this call at this time: a pasted link is
        // adopted into the calendar row; another calendar's copy is left.
        Err(error) if meetings::is_unique_violation(&error) => {
            sqlx::query(
                "UPDATE meetings SET source = $4, calendar_connection_id = $5, external_event_id = $6, \
                     ends_at = COALESCE($7, ends_at), attendee_count = $8, external_attendee_count = $9, \
                     is_one_on_one = $10, attendees = $11, title = CASE WHEN title = '' THEN $12 ELSE title END, \
                     updated_at = now() \
                 WHERE user_id = $1 AND url_key = $2 AND starts_at = $3 AND calendar_connection_id IS NULL \
                   AND deleted_at IS NULL AND source IN ('link', 'chat', 'mcp')",
            )
            .bind(&conn.user_id)
            .bind(&event.link.url_key)
            .bind(event.starts_at)
            .bind(source)
            .bind(conn.id)
            .bind(&event.external_id)
            .bind(event.ends_at)
            .bind(event.attendee_count)
            .bind(event.external_attendee_count)
            .bind(one_on_one)
            .bind(&attendees)
            .bind(&event.title)
            .execute(db)
            .await?;
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

/// The call link of `meeting_id` changed: forget its not-yet-launched bot.
/// Its own `scheduled` session is deleted (other meetings attached to it
/// start over); a session owned by someone else's meeting is only detached.
async fn drop_scheduled_session(db: &PgPool, meeting_id: Uuid, sid: Uuid) -> Result<(), sqlx::Error> {
    let dropped: Option<Uuid> = sqlx::query_scalar(
        "DELETE FROM meeting_sessions WHERE id = $1 AND meeting_id = $2 AND state = 'scheduled' RETURNING id",
    )
    .bind(sid)
    .bind(meeting_id)
    .fetch_optional(db)
    .await?;
    let detach = sqlx::query(
        "UPDATE meetings SET latest_session_id = NULL, updated_at = now() \
         WHERE latest_session_id = $1 AND (id = $2 OR $3) \
           AND NOT EXISTS (SELECT 1 FROM meeting_sessions WHERE id = $1 AND state <> 'scheduled')",
    )
    .bind(sid)
    .bind(meeting_id)
    .bind(dropped.is_some())
    .execute(db);
    detach.await?;
    if dropped.is_some() {
        crate::routes::meeting_events::retire(sid);
    }
    Ok(())
}

/// Remove a calendar meeting the calendar dropped (cancelled, declined,
/// deleted, moved out of the window, or the calendar was disconnected).
/// Only while Gizzi never got past `scheduled` for it: a meeting with a live
/// bot or a recap stays. Returns whether it was removed.
pub(crate) async fn remove_calendar_meeting(db: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let scheduled: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM meeting_sessions WHERE meeting_id = $1 AND state = 'scheduled'")
            .bind(id)
            .fetch_all(db)
            .await?;
    let deleted = sqlx::query(
        "DELETE FROM meetings m WHERE m.id = $1 \
         AND NOT EXISTS (SELECT 1 FROM meeting_sessions s WHERE s.meeting_id = m.id AND s.state <> 'scheduled')",
    )
    .bind(id)
    .execute(db)
    .await?
    .rows_affected()
        > 0;
    if deleted && !scheduled.is_empty() {
        // The sessions went with the row (ON DELETE CASCADE); other users'
        // meetings attached to them start over.
        sqlx::query("UPDATE meetings SET latest_session_id = NULL, updated_at = now() WHERE latest_session_id = ANY($1)")
            .bind(&scheduled)
            .execute(db)
            .await?;
        for sid in scheduled {
            crate::routes::meeting_events::retire(sid);
        }
    }
    Ok(deleted)
}

// ---------------------------------------------------------------------------
// Auto-join (`meeting_join`)
// ---------------------------------------------------------------------------

/// Inside the join window: from `JOIN_LEAD` before the start until
/// `LATE_JOIN_GRACE` after it, and never once the meeting is over.
pub fn in_join_window(starts_at: DateTime<Utc>, ends_at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    let over = ends_at.unwrap_or(starts_at + DEFAULT_LENGTH);
    starts_at - JOIN_LEAD <= now && now < starts_at + LATE_JOIN_GRACE && now < over
}

/// The auto-join decision for one meeting: the per-meeting override wins,
/// else the user's rule (`all_except_1on1` skips 1:1s), inside the window.
pub fn should_auto_join(
    per_meeting: Option<bool>,
    rule: AutoJoinRule,
    one_on_one: bool,
    starts_at: DateTime<Utc>,
    ends_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> bool {
    in_join_window(starts_at, ends_at, now) && meeting_links::will_auto_join(per_meeting, rule, one_on_one)
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct JoinStats {
    pub scheduled: usize,
    pub launching: usize,
    pub no_capacity: usize,
    pub cancelled: usize,
}

#[derive(FromRow)]
struct Candidate {
    id: Uuid,
    user_id: String,
    auto_join: Option<bool>,
    is_one_on_one: bool,
    starts_at: DateTime<Utc>,
    ends_at: Option<DateTime<Utc>>,
}

/// The `meeting_join` run: (1) send Gizzi to meetings starting within 2
/// minutes that auto-join applies to (a `scheduled` session, same checks as
/// `POST /meetings/:id/sessions`: plan, org, guests, dedupe by `url_key`);
/// (2) move due `scheduled` sessions to `launching` after re-checking plan,
/// org, guests and capacity (over capacity -> ended `no_capacity`). The pod
/// launcher picks `launching` sessions up.
pub async fn meeting_join_tick(db: &PgPool, now: DateTime<Utc>) -> anyhow::Result<JoinStats> {
    let mut stats = JoinStats::default();
    let candidates = sqlx::query_as::<_, Candidate>(
        "SELECT m.id, m.user_id, m.auto_join, m.is_one_on_one, m.starts_at, m.ends_at \
         FROM meetings m LEFT JOIN meeting_sessions s ON s.id = m.latest_session_id \
         WHERE m.deleted_at IS NULL AND m.starts_at IS NOT NULL \
           AND m.starts_at <= $1 + make_interval(secs => $2) AND m.starts_at > $1 - make_interval(secs => $3) \
           AND m.auto_join IS DISTINCT FROM false AND s.id IS NULL \
         ORDER BY m.starts_at LIMIT 500",
    )
    .bind(now)
    .bind(JOIN_LEAD.num_seconds() as f64)
    .bind(LATE_JOIN_GRACE.num_seconds() as f64)
    .fetch_all(db)
    .await;
    let candidates = match candidates {
        Ok(rows) => rows,
        Err(error) if tables_missing(&error) => return Ok(stats),
        Err(error) => return Err(error.into()),
    };
    let mut rules: HashMap<String, AutoJoinRule> = HashMap::new();
    for c in candidates {
        let rule = match rules.get(&c.user_id) {
            Some(rule) => *rule,
            None => {
                let settings = crate::routes::org_meeting_settings::load_user_settings(db, &c.user_id)
                    .await
                    .map_err(|e| anyhow::anyhow!("{}", e.message))?;
                let rule = AutoJoinRule::parse(&settings.auto_join).unwrap_or(AutoJoinRule::Off);
                rules.insert(c.user_id.clone(), rule);
                rule
            }
        };
        if !should_auto_join(c.auto_join, rule, c.is_one_on_one, c.starts_at, c.ends_at, now) {
            continue;
        }
        let meeting = match meetings::fetch_meeting(db, c.id).await {
            Ok(m) => m,
            Err(_) => continue,
        };
        // No stored org role: the org's `enabled` switch is checked, roles
        // were checked when the user set the rule or override.
        let caller = Caller { id: c.user_id.clone(), org_id: meeting.org_id.clone(), org_role: None };
        match meetings::start_session(db, &caller, &meeting, When::Start, None).await {
            Ok(_) => stats.scheduled += 1,
            Err(error) => tracing::info!(meeting = %c.id, code = error.code, "meetings: auto-join skipped: {}", error.message),
        }
    }

    let due: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM meeting_sessions WHERE state = 'scheduled' AND scheduled_join_at <= $1 \
         ORDER BY scheduled_join_at LIMIT 500",
    )
    .bind(now)
    .fetch_all(db)
    .await?;
    for sid in due {
        match promote(db, sid, now).await {
            Ok(Promoted::Launching) => stats.launching += 1,
            Ok(Promoted::NoCapacity) => stats.no_capacity += 1,
            Ok(Promoted::Cancelled) => stats.cancelled += 1,
            Ok(Promoted::Skipped) => {}
            Err(error) => tracing::warn!(%sid, code = error.code, "meetings: join failed: {}", error.message),
        }
    }
    Ok(stats)
}

enum Promoted {
    Launching,
    NoCapacity,
    Cancelled,
    Skipped,
}

async fn promote(db: &PgPool, sid: Uuid, now: DateTime<Utc>) -> Result<Promoted, MeetingError> {
    let Some(session) = meetings::fetch_session(db, sid).await? else { return Ok(Promoted::Skipped) };
    if session.state() != SessionState::Scheduled {
        return Ok(Promoted::Skipped);
    }
    let owner: Option<(bool, Option<bool>, Option<DateTime<Utc>>, Option<DateTime<Utc>>)> =
        sqlx::query_as("SELECT deleted_at IS NOT NULL, auto_join, starts_at, ends_at FROM meetings WHERE id = $1")
            .bind(session.meeting_id)
            .fetch_optional(db)
            .await?;
    let end = |reason: EndReason| async move {
        match meetings::set_state(db, sid, SessionState::Scheduled, SessionState::Ended, Some(reason)).await {
            Ok(_) => Ok(if reason == EndReason::NoCapacity { Promoted::NoCapacity } else { Promoted::Cancelled }),
            Err(e) if e.code == "conflict" => Ok(Promoted::Skipped),
            Err(e) => Err(e),
        }
    };
    let Some((deleted, auto_join, starts_at, ends_at)) = owner else { return end(EndReason::LeftByUser).await };
    // Switched off or removed after it was scheduled.
    if deleted || auto_join == Some(false) {
        return end(EndReason::LeftByUser).await;
    }
    // Missed (the service was down past the meeting).
    if let Some(start) = starts_at {
        if now >= ends_at.unwrap_or(start + DEFAULT_LENGTH) {
            return end(EndReason::Error).await;
        }
    }
    let meeting = meetings::fetch_meeting(db, session.meeting_id).await?;
    let caller = Caller { id: session.user_id.clone(), org_id: session.org_id.clone(), org_role: None };
    // Same order as the route: plan, org, guests, capacity (dedupe happened
    // when the session was created: one per url_key).
    let plan = match meetings::require_paid(db, &caller).await {
        Ok(plan) => plan,
        Err(e) if e.status.is_client_error() => return end(EndReason::Error).await,
        Err(e) => return Err(e),
    };
    let org = match meetings::org_gate(db, meeting.org_id.as_deref(), None).await {
        Ok(org) => org,
        Err(e) if e.status.is_client_error() => return end(EndReason::Error).await,
        Err(e) => return Err(e),
    };
    if meeting.external_attendee_count.unwrap_or(0) > 0 && org.as_ref().is_some_and(|o| !o.allow_external_guests) {
        return end(EndReason::Error).await;
    }
    match meetings::check_capacity(db, &session.user_id, &plan).await {
        Ok(()) => {}
        Err(e) if e.code == "no_capacity" => return end(EndReason::NoCapacity).await,
        Err(e) => return Err(e),
    }
    match meetings::set_state(db, sid, SessionState::Scheduled, SessionState::Launching, None).await {
        // Hand-off: meeting_launcher's pickup loop launches `launching`
        // sessions (and ends them no_capacity if a cap filled meanwhile).
        Ok(_) => Ok(Promoted::Launching),
        Err(e) if e.code == "conflict" => Ok(Promoted::Skipped),
        Err(e) => Err(e),
    }
}

/// Background calendar sync, every minute (each connection at most every
/// 5 minutes). Started with the scheduler service.
pub fn start_calendar_sync_task(state: Arc<ApiState>) {
    if matches!(std::env::var("MEETINGS_CALENDAR_SYNC_ENABLED").as_deref(), Ok("0") | Ok("false")) {
        tracing::info!("meetings: calendar sync disabled (MEETINGS_CALENDAR_SYNC_ENABLED=0)");
        return;
    }
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(StdDuration::from_secs(60));
        loop {
            tick.tick().await;
            if let Err(error) = sync_due_connections(&state).await {
                tracing::warn!("meetings: calendar sync pass failed: {error}");
            }
        }
    });
}

#[cfg(test)]
mod tests;
