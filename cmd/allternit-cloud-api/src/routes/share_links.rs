//! Share-link registry (Apps + MCP 2026, plan v3 lane 7; table `share_links`,
//! migration 161). One row per hosted or live link the owner made.
//!
//! Routes (signed in; every link belongs to its creator, nobody else sees or
//! changes it; someone else's id is 404). Every error body is `{ error, message }`.
//!
//! * `POST /api/v1/share-links`
//!   `{ kind: hosted|live, subject: artifact|port|mcp, artifact_id?, runtime_device_id?,
//!      label?, subdomain?, target_port?, auth_mode?: public|signed_in|org,
//!      app_kind?: artifact|vendor, vendor_app_id? }` -> 201 link
//! * `GET /api/v1/share-links[?include_revoked=true]` -> `{ links: [link] }`, newest first
//! * `DELETE /api/v1/share-links/:id` -> `{ revoked: true, link }` (soft: the row stays
//!   for the abuse history; a killed link stays killed)
//! * `POST /api/v1/share-links/:id/pause` `{ paused?: bool }` (default true; false resumes) -> link
//! * `POST /api/v1/share-links/:id/heartbeat` -> link, `last_seen_at` = now. A revoked
//!   or killed link answers 410 so the tunnel client stops serving it.
//!
//! A link is `{ id, kind, subject, artifact_id, runtime_device_id, label, subdomain,
//! target_port, auth_mode, status, abuse_flag, bytes_month, app_kind, vendor_app_id,
//! url, created_at, last_seen_at }`. `abuse_note` is admin-only and never returned.
//! `url` (see [`link_url`]): hosted = `https://<subdomain>.<APP_DOMAIN>`, or
//! `https://ai.allternit.com/app/<subdomain>` when `APP_DOMAIN` is unset; live =
//! `https://<subdomain>.live.<APP_DOMAIN>`, `null` when `APP_DOMAIN` is unset.
//!
//! Rules here: `live` needs one of the caller's own runtime devices; `port`
//! needs `live` and a `target_port`; `artifact` needs an artifact the caller
//! owns; `vendor` needs a vendor app record visible to the caller. Subdomains
//! are DNS labels (3-63 of `a-z 0-9 -`, no hyphen at either end), minted at
//! random (16 chars) when not given, unique across every link (409 when taken).
//!
//! Lane 8 rules:
//! * Caps (D8), live links only: at most [`MAX_ACTIVE_LIVE_LINKS`] active per
//!   account and [`MAX_LIVE_BYTES_MONTH`] served a month (`bytes_month`, summed
//!   over the account's live links). Over either: 429
//!   `{ error: "cap_reached", message, cap: "links"|"bytes", used, limit }`, on
//!   create and on resume.
//! * Kill (admin, `admin_share_links`): a killed link never serves again (410
//!   `killed` on heartbeat and resume), and the artifact or vendor app it shared
//!   can't get a new link (410 `killed` on create). A flag alone only marks the
//!   link for review.
//! * Names (D23): no `allternit` or `gizzi` anywhere, no other vendor brand as a
//!   word or prefix, and no infrastructure names ([`reserved_name`]) -> 400
//!   `reserved_subdomain`.

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use std::sync::Arc;

use crate::routes::paired_computers::{signed_in_caller, Caller};
use crate::routes::platform_v1::new_id;
use crate::{ApiError, ApiState};

pub const KINDS: [&str; 2] = ["hosted", "live"];
pub const SUBJECTS: [&str; 3] = ["artifact", "port", "mcp"];
pub const AUTH_MODES: [&str; 3] = ["public", "signed_in", "org"];
pub const APP_KINDS: [&str; 2] = ["artifact", "vendor"];
pub const MAX_LABEL: usize = 80;
pub const MINTED_SUBDOMAIN_LEN: usize = 16;
/// D8 caps (recommended, Eoj 2026-11-24): live links per account and bytes a month.
pub const MAX_ACTIVE_LIVE_LINKS: i64 = 3;
pub const MAX_LIVE_BYTES_MONTH: i64 = 20_000_000_000;
/// Path-mode origin for hosted links when `APP_DOMAIN` is unset.
pub const WORKSPACE_ORIGIN: &str = "https://ai.allternit.com";

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/share-links", get(list_links).post(create_link))
        .route("/api/v1/share-links/:id", axum::routing::delete(delete_link))
        .route("/api/v1/share-links/:id/pause", post(pause_link))
        .route("/api/v1/share-links/:id/heartbeat", post(heartbeat_link))
}

fn fail(status: StatusCode, error: &str, message: &str) -> Response {
    (status, Json(json!({ "error": error, "message": message }))).into_response()
}

fn db_fail(e: sqlx::Error) -> Response {
    tracing::warn!("share links: {e}");
    fail(
        StatusCode::INTERNAL_SERVER_ERROR,
        "db_error",
        "Something went wrong saving that link. Try again in a moment.",
    )
}

fn not_found() -> Response {
    fail(StatusCode::NOT_FOUND, "not_found", "That link was not found.")
}

fn is_unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(d) if d.code().as_deref() == Some("23505"))
}

fn rfc3339(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

// ------------------------------------------------------------- the rows ----

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ShareLinkRow {
    pub id: String,
    pub user_id: String,
    pub org_id: Option<String>,
    pub kind: String,
    pub subject: String,
    pub artifact_id: Option<String>,
    pub runtime_device_id: Option<String>,
    pub label: Option<String>,
    pub subdomain: String,
    pub target_port: Option<i32>,
    pub auth_mode: String,
    pub status: String,
    pub abuse_flag: bool,
    pub bytes_month: i64,
    pub app_kind: Option<String>,
    pub vendor_app_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: Option<DateTime<Utc>>,
}

pub(crate) const COLS: &str = "id, user_id, org_id, kind, subject, artifact_id, runtime_device_id, label, \
    subdomain, target_port, auth_mode, status, abuse_flag, bytes_month, app_kind, vendor_app_id, \
    created_at, last_seen_at";

impl ShareLinkRow {
    pub fn json(&self) -> Value {
        json!({
            "id": self.id,
            "kind": self.kind,
            "subject": self.subject,
            "artifact_id": self.artifact_id,
            "runtime_device_id": self.runtime_device_id,
            "label": self.label,
            "subdomain": self.subdomain,
            "target_port": self.target_port,
            "auth_mode": self.auth_mode,
            "status": self.status,
            "abuse_flag": self.abuse_flag,
            "bytes_month": self.bytes_month,
            "app_kind": self.app_kind,
            "vendor_app_id": self.vendor_app_id,
            "url": link_url(&self.kind, &self.subdomain),
            "created_at": rfc3339(self.created_at),
            "last_seen_at": self.last_seen_at.map(rfc3339),
        })
    }
}

// ------------------------------------------------------------ the URLs ----

/// `APP_DOMAIN` (e.g. `allternitugc.app`), normalized like the client's
/// `appDomain()`: no scheme, no path, no dots at the ends, lower case. `None`
/// when unset or not a domain.
pub fn app_domain() -> Option<String> {
    let raw = std::env::var("APP_DOMAIN").ok()?;
    let d = raw.trim().to_ascii_lowercase();
    let d = d.strip_prefix("https://").or_else(|| d.strip_prefix("http://")).unwrap_or(d.as_str());
    let d = d.split('/').next().unwrap_or_default().trim_matches('.');
    let labels: Vec<&str> = d.split('.').collect();
    let ok = labels.len() >= 2
        && labels.iter().all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && l.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                && !l.starts_with('-')
                && !l.ends_with('-')
        })
        && labels.last().is_some_and(|tld| tld.len() >= 2 && tld.bytes().all(|b| b.is_ascii_lowercase()));
    ok.then(|| d.to_string())
}

/// The public URL of a link (same rule as the client's `appLink` / `liveLink`).
pub fn link_url(kind: &str, subdomain: &str) -> Option<String> {
    match (kind, app_domain()) {
        ("live", Some(domain)) => Some(format!("https://{subdomain}.live.{domain}")),
        ("live", None) => None,
        (_, Some(domain)) => Some(format!("https://{subdomain}.{domain}")),
        (_, None) => Some(format!("{WORKSPACE_ORIGIN}/app/{subdomain}")),
    }
}

// ---------------------------------------------------------- the checks ----

/// Our own names: refused anywhere in a link name.
const OWN_BRANDS: [&str; 2] = ["allternit", "gizzi"];
/// Other vendors' brands: refused as a word (`my-claude`) or a prefix (`claudeai`).
const VENDOR_BRANDS: [&str; 22] = [
    "anthropic", "claude", "openai", "chatgpt", "gpt", "google", "gmail", "gemini", "apple",
    "icloud", "microsoft", "copilot", "github", "facebook", "instagram", "whatsapp", "amazon",
    "aws", "paypal", "stripe", "cloudflare", "grok",
];
/// Short or common vendor words: refused only as a whole word.
const VENDOR_WORDS: [&str; 3] = ["meta", "xai", "twitter"];
/// Infrastructure and support names: refused as the whole name.
const RESERVED_EXACT: [&str; 24] = [
    "admin", "api", "www", "mail", "live", "app", "apps", "auth", "login", "signin", "signup",
    "account", "billing", "support", "help", "status", "docs", "security", "root", "smtp",
    "cdn", "static", "assets", "dashboard",
];

/// D23: true when a (lower-cased) link name is ours, a vendor's or infrastructure's.
pub fn reserved_name(label: &str) -> bool {
    if RESERVED_EXACT.contains(&label) || OWN_BRANDS.iter().any(|b| label.contains(b)) {
        return true;
    }
    let words: Vec<&str> = label.split('-').filter(|w| !w.is_empty()).collect();
    words.iter().any(|w| VENDOR_WORDS.contains(w))
        || VENDOR_BRANDS.iter().any(|b| label.starts_with(b) || words.iter().any(|w| w == b))
}

/// A DNS label: 3-63 of `a-z 0-9 -`, no hyphen at either end, not a reserved
/// name. Lowercases. `Err((code, message))`: `invalid_subdomain` or
/// `reserved_subdomain`.
pub fn check_subdomain(raw: &str) -> Result<String, (&'static str, &'static str)> {
    let s = raw.trim().to_ascii_lowercase();
    let ok = (3..=63).contains(&s.len())
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !s.starts_with('-')
        && !s.ends_with('-');
    if !ok {
        return Err((
            "invalid_subdomain",
            "A link name is 3 to 63 lowercase letters, digits or hyphens, and can't start or end with a hyphen.",
        ));
    }
    if reserved_name(&s) {
        return Err((
            "reserved_subdomain",
            "That link name is reserved. Names can't use Allternit, Gizzi or another company's brand.",
        ));
    }
    Ok(s)
}

/// A random DNS-safe label of `len` chars: a letter first, then `a-z 0-9`.
/// Shared with `artifacts_v2` for `app_slug`.
pub fn mint_label(len: usize) -> String {
    const LETTERS: &[u8] = b"abcdefghijklmnopqrstuvwxyz";
    const ALNUM: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut out = String::with_capacity(len);
    while out.len() < len {
        let b = rand::random::<u8>();
        let set = if out.is_empty() { LETTERS } else { ALNUM };
        // Rejection sampling keeps every character equally likely.
        let limit = 256 - (256 % set.len());
        if (b as usize) < limit {
            out.push(set[b as usize % set.len()] as char);
        }
    }
    out
}

fn one_of(value: &str, allowed: &[&str]) -> bool {
    allowed.contains(&value)
}

fn cap_reached(cap: &str, used: i64, limit: i64, message: &str) -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        Json(json!({ "error": "cap_reached", "message": message, "cap": cap, "used": used, "limit": limit })),
    )
        .into_response()
}

/// D8 caps, live links only (hosted links are served by our app host and
/// metered elsewhere): [`MAX_ACTIVE_LIVE_LINKS`] active per account and
/// [`MAX_LIVE_BYTES_MONTH`] served this month over every live link the account
/// made (revoking a link doesn't give its bytes back). Checked before a live
/// link is made or resumed; 429 `cap_reached` over either.
async fn enforce_caps(db: &PgPool, caller: &Caller, kind: &str) -> Result<(), Response> {
    if kind != "live" {
        return Ok(());
    }
    let (active, bytes): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*) FILTER (WHERE status = 'active')::BIGINT, COALESCE(SUM(bytes_month), 0)::BIGINT \
         FROM share_links WHERE user_id = $1 AND kind = 'live'",
    )
    .bind(&caller.user_id)
    .fetch_one(db)
    .await
    .map_err(db_fail)?;
    if active >= MAX_ACTIVE_LIVE_LINKS {
        return Err(cap_reached(
            "links",
            active,
            MAX_ACTIVE_LIVE_LINKS,
            "You already have 3 live links on. Pause or delete one first.",
        ));
    }
    if bytes >= MAX_LIVE_BYTES_MONTH {
        return Err(cap_reached(
            "bytes",
            bytes,
            MAX_LIVE_BYTES_MONTH,
            "Your live links served 20 GB this month. They start again next month.",
        ));
    }
    Ok(())
}

fn killed() -> Response {
    fail(StatusCode::GONE, "killed", "This link was turned off by Allternit and can't be used again.")
}

/// A killed link is gone for good: the owner can't resume or heartbeat it
/// (410). A flag alone only marks it for review; an admin kill stops it.
fn check_kill(row: &ShareLinkRow) -> Result<(), Response> {
    if row.status == "killed" {
        return Err(killed());
    }
    Ok(())
}

/// True when an admin killed a link to this artifact or vendor app: it can't
/// be shared again (a new link would route around the kill).
pub(crate) async fn subject_killed(
    db: &PgPool,
    artifact_id: Option<&str>,
    vendor_app_id: Option<&str>,
) -> Result<bool, sqlx::Error> {
    if artifact_id.is_none() && vendor_app_id.is_none() {
        return Ok(false);
    }
    let hit: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM share_links WHERE status = 'killed' \
           AND (($1::TEXT IS NOT NULL AND artifact_id = $1) OR ($2::TEXT IS NOT NULL AND vendor_app_id = $2)) \
         LIMIT 1",
    )
    .bind(artifact_id)
    .bind(vendor_app_id)
    .fetch_optional(db)
    .await?;
    Ok(hit.is_some())
}

// ---------------------------------------------------------------- CRUD ----

async fn load(db: &PgPool, user_id: &str, id: &str) -> Result<Option<ShareLinkRow>, sqlx::Error> {
    sqlx::query_as(&format!("SELECT {COLS} FROM share_links WHERE id = $1 AND user_id = $2"))
        .bind(id)
        .bind(user_id)
        .fetch_optional(db)
        .await
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateRequest {
    kind: String,
    subject: String,
    artifact_id: Option<String>,
    runtime_device_id: Option<String>,
    label: Option<String>,
    subdomain: Option<String>,
    target_port: Option<i64>,
    auth_mode: Option<String>,
    app_kind: Option<String>,
    vendor_app_id: Option<String>,
}

fn bad(error: &str, message: &str) -> Response {
    fail(StatusCode::BAD_REQUEST, error, message)
}

async fn create_link(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let caller = signed_in_caller(&state, &headers).await?;
    let req: CreateRequest = match serde_json::from_slice(&body) {
        Ok(req) => req,
        Err(_) => return Ok(bad("invalid_body", "Send kind, subject and the link's fields as JSON.")),
    };
    if !one_of(&req.kind, &KINDS) {
        return Ok(bad("invalid_kind", "kind is hosted or live."));
    }
    if !one_of(&req.subject, &SUBJECTS) {
        return Ok(bad("invalid_subject", "subject is artifact, port or mcp."));
    }
    let auth_mode = req.auth_mode.clone().unwrap_or_else(|| "public".into());
    if !one_of(&auth_mode, &AUTH_MODES) {
        return Ok(bad("invalid_auth_mode", "auth_mode is public, signed_in or org."));
    }
    if auth_mode == "org" && caller.organization_id.is_none() {
        return Ok(bad("no_org", "Only someone in an organization can make an org-only link."));
    }
    if let Some(app_kind) = req.app_kind.as_deref() {
        if !one_of(app_kind, &APP_KINDS) {
            return Ok(bad("invalid_app_kind", "app_kind is artifact or vendor."));
        }
        if app_kind == "artifact" && req.subject != "artifact" {
            return Ok(bad("invalid_app_kind", "An artifact app link shares an artifact."));
        }
        if app_kind == "vendor" && req.vendor_app_id.is_none() {
            return Ok(bad("vendor_app_required", "A vendor app link needs vendor_app_id."));
        }
    }
    if req.vendor_app_id.is_some() && req.app_kind.as_deref() != Some("vendor") {
        return Ok(bad("invalid_app_kind", "vendor_app_id goes with app_kind vendor."));
    }
    let label = req.label.as_deref().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string);
    if label.as_ref().is_some_and(|l| l.chars().count() > MAX_LABEL || l.chars().any(char::is_control)) {
        return Ok(bad("invalid_label", "A label can be up to 80 characters."));
    }
    let target_port = match req.target_port {
        None => None,
        Some(p) if (1..=65535).contains(&p) => Some(p as i32),
        Some(_) => return Ok(bad("invalid_port", "target_port is 1 to 65535.")),
    };
    if req.subject == "port" && (req.kind != "live" || target_port.is_none()) {
        return Ok(bad("invalid_port", "A port link is a live link with a target_port."));
    }
    if req.kind == "hosted" && (req.runtime_device_id.is_some() || target_port.is_some()) {
        return Ok(bad("invalid_kind", "A hosted link runs on Allternit, not on a computer or port."));
    }
    if req.kind == "live" && req.runtime_device_id.is_none() {
        return Ok(bad("device_required", "A live link needs the computer it runs on (runtime_device_id)."));
    }
    if req.subject == "artifact" && req.artifact_id.is_none() {
        return Ok(bad("artifact_required", "An artifact link needs artifact_id."));
    }
    if req.subject != "artifact" && req.artifact_id.is_some() {
        return Ok(bad("invalid_subject", "artifact_id goes with subject artifact."));
    }

    // Everything the link points at must be the caller's (someone else's = 404).
    if let Some(artifact_id) = req.artifact_id.as_deref() {
        let owned: Option<i32> =
            sqlx::query_scalar("SELECT 1 FROM artifacts WHERE id = $1 AND owner_id = $2")
                .bind(artifact_id)
                .bind(&caller.user_id)
                .fetch_optional(&state.db)
                .await?;
        if owned.is_none() {
            return Ok(fail(StatusCode::NOT_FOUND, "not_found", "That artifact was not found."));
        }
    }
    if let Some(device_id) = req.runtime_device_id.as_deref() {
        let owned: Option<i32> =
            sqlx::query_scalar("SELECT 1 FROM runtime_devices WHERE id = $1 AND user_id = $2")
                .bind(device_id)
                .bind(&caller.user_id)
                .fetch_optional(&state.db)
                .await?;
        if owned.is_none() {
            return Ok(fail(StatusCode::NOT_FOUND, "not_found", "That computer was not found."));
        }
    }
    if let Some(vendor_app_id) = req.vendor_app_id.as_deref() {
        let visible: Option<i32> = sqlx::query_scalar(
            "SELECT 1 FROM vendor_apps WHERE id = $1 \
               AND (owner_user_id = $2 OR (owner_org_id IS NOT NULL AND owner_org_id = $3))",
        )
        .bind(vendor_app_id)
        .bind(&caller.user_id)
        .bind(caller.organization_id.as_deref())
        .fetch_optional(&state.db)
        .await?;
        if visible.is_none() {
            return Ok(fail(StatusCode::NOT_FOUND, "not_found", "That app was not found."));
        }
    }
    let given_subdomain = match req.subdomain.as_deref() {
        Some(raw) => match check_subdomain(raw) {
            Ok(s) => Some(s),
            Err((code, msg)) => return Ok(bad(code, msg)),
        },
        None => None,
    };
    match subject_killed(&state.db, req.artifact_id.as_deref(), req.vendor_app_id.as_deref()).await {
        Ok(true) => return Ok(killed()),
        Ok(false) => {}
        Err(e) => return Ok(db_fail(e)),
    }
    if let Err(res) = enforce_caps(&state.db, &caller, &req.kind).await {
        return Ok(res);
    }

    // A minted name retries on the (vanishingly rare) clash; a chosen one is 409.
    for _ in 0..3 {
        let subdomain = match given_subdomain.clone() {
            Some(s) => s,
            // A random name that happens to start with a brand is redrawn.
            None => loop {
                let s = mint_label(MINTED_SUBDOMAIN_LEN);
                if !reserved_name(&s) {
                    break s;
                }
            },
        };
        let inserted: Result<ShareLinkRow, sqlx::Error> = sqlx::query_as(&format!(
            "INSERT INTO share_links (id, user_id, org_id, kind, subject, artifact_id, \
                 runtime_device_id, label, subdomain, target_port, auth_mode, app_kind, vendor_app_id) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13) RETURNING {COLS}"
        ))
        .bind(new_id("shl_"))
        .bind(&caller.user_id)
        .bind(caller.organization_id.as_deref())
        .bind(&req.kind)
        .bind(&req.subject)
        .bind(req.artifact_id.as_deref())
        .bind(req.runtime_device_id.as_deref())
        .bind(label.as_deref())
        .bind(&subdomain)
        .bind(target_port)
        .bind(&auth_mode)
        .bind(req.app_kind.as_deref())
        .bind(req.vendor_app_id.as_deref())
        .fetch_one(&state.db)
        .await;
        match inserted {
            Ok(row) => return Ok((StatusCode::CREATED, Json(row.json())).into_response()),
            Err(e) if is_unique_violation(&e) && given_subdomain.is_some() => {
                return Ok(fail(
                    StatusCode::CONFLICT,
                    "subdomain_taken",
                    "That link name is taken. Pick another.",
                ));
            }
            Err(e) if is_unique_violation(&e) => continue,
            Err(e) => return Ok(db_fail(e)),
        }
    }
    Ok(db_fail(sqlx::Error::RowNotFound))
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    include_revoked: Option<bool>,
}

async fn list_links(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> Result<Response, ApiError> {
    let caller = signed_in_caller(&state, &headers).await?;
    let rows: Vec<ShareLinkRow> = match sqlx::query_as(&format!(
        "SELECT {COLS} FROM share_links WHERE user_id = $1 AND ($2 OR status <> 'revoked') \
         ORDER BY created_at DESC, id DESC"
    ))
    .bind(&caller.user_id)
    .bind(q.include_revoked.unwrap_or(false))
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => rows,
        Err(e) => return Ok(db_fail(e)),
    };
    let links: Vec<Value> = rows.iter().map(ShareLinkRow::json).collect();
    Ok(Json(json!({ "links": links })).into_response())
}

async fn delete_link(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let caller = signed_in_caller(&state, &headers).await?;
    // Soft delete: the row stays for abuse history; killed stays killed.
    let row: Option<ShareLinkRow> = match sqlx::query_as(&format!(
        "UPDATE share_links SET status = CASE WHEN status = 'killed' THEN status ELSE 'revoked' END \
         WHERE id = $1 AND user_id = $2 RETURNING {COLS}"
    ))
    .bind(&id)
    .bind(&caller.user_id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(row) => row,
        Err(e) => return Ok(db_fail(e)),
    };
    match row {
        Some(row) => Ok(Json(json!({ "revoked": true, "link": row.json() })).into_response()),
        None => Ok(not_found()),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PauseRequest {
    paused: Option<bool>,
}

async fn pause_link(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Response, ApiError> {
    let caller = signed_in_caller(&state, &headers).await?;
    let req: PauseRequest = if body.iter().all(u8::is_ascii_whitespace) {
        PauseRequest::default()
    } else {
        match serde_json::from_slice(&body) {
            Ok(req) => req,
            Err(_) => return Ok(bad("invalid_body", "Send { \"paused\": true } or { \"paused\": false }.")),
        }
    };
    let paused = req.paused.unwrap_or(true);
    let Some(row) = load(&state.db, &caller.user_id, &id).await? else {
        return Ok(not_found());
    };
    if let Err(res) = check_kill(&row) {
        return Ok(res);
    }
    if row.status == "revoked" {
        return Ok(fail(StatusCode::GONE, "revoked", "This link was deleted. Make a new one."));
    }
    if !paused && row.status == "paused" {
        if let Err(res) = enforce_caps(&state.db, &caller, &row.kind).await {
            return Ok(res);
        }
    }
    let next = if paused { "paused" } else { "active" };
    let row: ShareLinkRow = sqlx::query_as(&format!(
        "UPDATE share_links SET status = $3 WHERE id = $1 AND user_id = $2 RETURNING {COLS}"
    ))
    .bind(&row.id)
    .bind(&caller.user_id)
    .bind(next)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(row.json()).into_response())
}

async fn heartbeat_link(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let caller = signed_in_caller(&state, &headers).await?;
    let Some(row) = load(&state.db, &caller.user_id, &id).await? else {
        return Ok(not_found());
    };
    if let Err(res) = check_kill(&row) {
        return Ok(res);
    }
    if row.status == "revoked" {
        return Ok(fail(StatusCode::GONE, "revoked", "This link was deleted. Stop serving it."));
    }
    let row: ShareLinkRow = sqlx::query_as(&format!(
        "UPDATE share_links SET last_seen_at = now() WHERE id = $1 AND user_id = $2 RETURNING {COLS}"
    ))
    .bind(&row.id)
    .bind(&caller.user_id)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(row.json()).into_response())
}

#[cfg(test)]
#[path = "share_links_tests.rs"]
mod tests;
