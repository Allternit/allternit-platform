//! Bot email for every runtime: cloud-api provisions a bot's mailbox on
//! Allternit Mail on behalf of the runtime that owns the bot.
//!
//! Allternit Mail (the `allternit-agent-mail` worker) runs once, owned by the
//! platform. Only cloud-api holds its admin key (`ALLTERNIT_MAILFLARE_URL`,
//! `ALLTERNIT_MAILFLARE_ADMIN_KEY`, domain `ALLTERNIT_BOT_EMAIL_DOMAIN`,
//! default `bots.allternit.com`). A runtime authenticates with its own device
//! credential and gets back what it needs to run that one mailbox:
//!
//! * `POST /api/v1/runtime-devices/me/bot-email/mailboxes {agentId, localPart, displayName?}`
//!   1. an `email` relay route to this runtime (`/channels/in/<key>`),
//!   2. the mailbox (`local@domain`; `-2`, `-3`… when the name is taken),
//!   3. a `send`+`read` key scoped to that mailbox,
//!   4. a webhook for `message.inbound` scoped to that mailbox, pointed at the
//!      relay route, with its own signing secret,
//!
//!   and answers `201 {address, mailboxId, apiKey, webhookSecret, mailUrl}` once.
//!   Calling again for the same bot returns 409 `already_provisioned`.
//! * `DELETE /api/v1/runtime-devices/me/bot-email/mailboxes/:mailbox_id` undoes
//!   it (webhook, key, mailbox, route).
//!
//! * `PUT /api/v1/runtime-devices/me/bot-email/domains/:domain` adds a customer
//!   domain (support@acme.com) for the device's user and answers the DNS records
//!   to add; `GET …/domains` lists the user's domains with a live record check;
//!   `DELETE …/domains/:domain` removes one that has no bot mailboxes left. The
//!   domain is served by Allternit's mail host (mx.allternit.com, services/mail-relay),
//!   which Allternit Mail reaches through `/api/v1/relay/domains/:host`. Ownership is
//!   `bot_email_domains` (one user per domain).
//! * The mailbox provision takes `domain` to put the bot on a verified customer domain.
//!
//! Unset env → 503 `bot_email_not_configured` (nothing changes for anyone).

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};

use super::channel_inbound::{new_key, public_base, sha256_hex};
use super::runtime_pairing::{device_token_from_headers, runtime_device_for_token};
use crate::{ApiError, ApiState};

pub const DEFAULT_DOMAIN: &str = "bots.allternit.com";

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/runtime-devices/me/bot-email/mailboxes", post(provision))
        .route("/api/v1/runtime-devices/me/bot-email/mailboxes/:mailbox_id", delete(teardown))
        .route("/api/v1/runtime-devices/me/bot-email/domains", get(list_domains))
        .route("/api/v1/runtime-devices/me/bot-email/domains/:domain", put(add_domain).delete(remove_domain))
}

#[derive(Clone)]
pub struct MailConfig {
    pub base: String,
    pub admin_key: String,
    pub domain: String,
}

impl MailConfig {
    pub fn from_env() -> Option<Self> {
        let get = |k: &str| std::env::var(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        Some(Self {
            base: get("ALLTERNIT_MAILFLARE_URL")?.trim_end_matches('/').to_string(),
            admin_key: get("ALLTERNIT_MAILFLARE_ADMIN_KEY")?,
            domain: get("ALLTERNIT_BOT_EMAIL_DOMAIN").unwrap_or_else(|| DEFAULT_DOMAIN.to_string()),
        })
    }
}

fn not_configured() -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": "bot_email_not_configured" }))).into_response()
}

fn mail_error(what: &str, status: Option<u16>, detail: &str) -> Response {
    tracing::warn!("bot email: {what} failed ({status:?}): {detail}");
    (StatusCode::BAD_GATEWAY, Json(json!({ "error": "mail_service_error", "message": format!("Allternit Mail refused to {what}.") }))).into_response()
}

/// One call to Allternit Mail with the admin key. `Err((status, body))`.
async fn call(cfg: &MailConfig, method: reqwest::Method, path: &str, body: Option<Value>) -> Result<Value, (Option<u16>, String)> {
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(15)).build().map_err(|e| (None, e.to_string()))?;
    let mut req = client.request(method, format!("{}{}", cfg.base, path)).bearer_auth(&cfg.admin_key);
    if let Some(b) = body {
        req = req.json(&b);
    }
    let resp = req.send().await.map_err(|e| (None, e.to_string()))?;
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    if !(200..300).contains(&status) {
        return Err((Some(status), text));
    }
    Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
}

/// Lowercase letters, digits, dots and dashes; trimmed; at most 48 characters.
pub fn clean_local_part(raw: &str) -> String {
    let mut out = String::new();
    for c in raw.trim().to_ascii_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if (c == '-' || c == '.' || c == '_' || c.is_whitespace()) && !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out: String = out.trim_matches('-').chars().take(48).collect();
    if out.is_empty() { "bot".into() } else { out }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProvisionBody {
    agent_id: String,
    local_part: Option<String>,
    display_name: Option<String>,
    /// A verified customer domain of this user; the platform bot domain when absent.
    domain: Option<String>,
}

/// `acme.com`, lower-cased and without a trailing dot; `None` unless it is a
/// plain domain name that isn't Allternit's own.
pub fn clean_domain(raw: &str) -> Option<String> {
    let d = raw.trim().trim_end_matches('.').to_ascii_lowercase();
    let labels: Vec<&str> = d.split('.').collect();
    let ok = d.len() <= 253
        && labels.len() >= 2
        && labels.iter().all(|l| !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && !l.ends_with('-') && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        && labels.last().is_some_and(|tld| tld.len() >= 2 && tld.bytes().all(|b| b.is_ascii_alphabetic()));
    (ok && d != "allternit.com" && !d.ends_with(".allternit.com")).then_some(d)
}

/// The user's claim on a domain: `Ok(true)` theirs, `Ok(false)` free, `Err` someone else's.
async fn domain_claim(db: &sqlx::PgPool, user_id: &str, domain: &str) -> Result<Option<bool>, ApiError> {
    let owner: Option<(String,)> = sqlx::query_as("SELECT user_id FROM bot_email_domains WHERE domain = $1").bind(domain).fetch_optional(db).await?;
    Ok(match owner {
        None => Some(false),
        Some((u,)) if u == user_id => Some(true),
        Some(_) => None,
    })
}

fn domain_taken() -> Response {
    (StatusCode::CONFLICT, Json(json!({ "error": "domain_taken", "message": "That domain is already used for bot email on another Allternit account." }))).into_response()
}

/// Allternit Mail's answer for a domain, as `{domain, state, records}` for the app.
fn domain_json(v: &Value) -> Value {
    let verified = v.get("verified").and_then(Value::as_bool).unwrap_or(false);
    json!({
        "domain": v.get("domain").cloned().unwrap_or(Value::Null),
        "state": if verified { "verified" } else { "pending" },
        "records": v.get("records").cloned().unwrap_or_else(|| json!([])),
    })
}

async fn add_domain(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(domain): Path<String>) -> Result<Response, ApiError> {
    let Some(cfg) = MailConfig::from_env() else { return Ok(not_configured()) };
    let (_, user_id) = device(&state, &headers).await?;
    let Some(domain) = clean_domain(&domain) else {
        return Err(ApiError::BadRequest("Enter a domain like acme.com or mail.acme.com.".into()));
    };
    match domain_claim(&state.db, &user_id, &domain).await? {
        None => return Ok(domain_taken()),
        Some(true) => {}
        Some(false) => {
            sqlx::query("INSERT INTO bot_email_domains (domain, user_id) VALUES ($1, $2) ON CONFLICT (domain) DO NOTHING").bind(&domain).bind(&user_id).execute(&state.db).await?;
            if domain_claim(&state.db, &user_id, &domain).await? != Some(true) {
                return Ok(domain_taken());
            }
        }
    }
    match call(&cfg, reqwest::Method::PUT, &format!("/api/v1/relay/domains/{domain}"), None).await {
        Ok(v) => Ok(Json(domain_json(&v)).into_response()),
        Err((Some(503), _)) => Ok((StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": "custom_domains_unavailable", "message": "Company domains for bot email aren't switched on yet." }))).into_response()),
        Err((s, d)) => Ok(mail_error("add the domain", s, &d)),
    }
}

/// The user's domains, each checked live; a domain that passes is marked verified.
async fn list_domains(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Result<Response, ApiError> {
    let Some(cfg) = MailConfig::from_env() else { return Ok(not_configured()) };
    let (_, user_id) = device(&state, &headers).await?;
    let rows: Vec<(String,)> = sqlx::query_as("SELECT domain FROM bot_email_domains WHERE user_id = $1 ORDER BY created_at").bind(&user_id).fetch_all(&state.db).await?;
    let mut out = Vec::new();
    for (domain,) in rows {
        match call(&cfg, reqwest::Method::GET, &format!("/api/v1/relay/domains/{domain}"), None).await {
            Ok(v) => {
                let d = domain_json(&v);
                if d["state"] == "verified" {
                    sqlx::query("UPDATE bot_email_domains SET verified_at = COALESCE(verified_at, now()) WHERE domain = $1").bind(&domain).execute(&state.db).await?;
                }
                out.push(d);
            }
            Err((s, d)) => {
                tracing::warn!("bot email: check {domain} failed ({s:?}): {d}");
                out.push(json!({ "domain": domain, "state": "unknown", "records": [] }));
            }
        }
    }
    Ok(Json(json!({ "domains": out })).into_response())
}

async fn remove_domain(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(domain): Path<String>) -> Result<Response, ApiError> {
    let Some(cfg) = MailConfig::from_env() else { return Ok(not_configured()) };
    let (_, user_id) = device(&state, &headers).await?;
    let Some(domain) = clean_domain(&domain) else { return Err(ApiError::NotFound("Domain not found".into())) };
    if domain_claim(&state.db, &user_id, &domain).await? != Some(true) {
        return Err(ApiError::NotFound("Domain not found".into()));
    }
    let in_use: Option<(String,)> = sqlx::query_as("SELECT address FROM bot_email_mailboxes WHERE user_id = $1 AND deleted_at IS NULL AND lower(address) LIKE $2 LIMIT 1")
        .bind(&user_id)
        .bind(format!("%@{domain}"))
        .fetch_optional(&state.db)
        .await?;
    if let Some((address,)) = in_use {
        return Ok((StatusCode::CONFLICT, Json(json!({ "error": "domain_in_use", "message": format!("{address} still uses this domain. Move or remove that bot's address first.") }))).into_response());
    }
    match call(&cfg, reqwest::Method::DELETE, &format!("/api/v1/relay/domains/{domain}"), None).await {
        Ok(_) | Err((Some(404), _)) => {}
        Err((s, d)) => return Ok(mail_error("remove the domain", s, &d)),
    }
    sqlx::query("DELETE FROM bot_email_domains WHERE domain = $1 AND user_id = $2").bind(&domain).bind(&user_id).execute(&state.db).await?;
    Ok(Json(json!({ "domain": domain, "deleted": true })).into_response())
}

async fn device(state: &ApiState, headers: &HeaderMap) -> Result<(String, String), ApiError> {
    let token = device_token_from_headers(headers).ok_or_else(|| ApiError::Unauthorized("Runtime credential required".to_string()))?;
    let d = runtime_device_for_token(&state.db, token, None).await?;
    Ok((d.id, d.user_id))
}

async fn provision(State(state): State<Arc<ApiState>>, headers: HeaderMap, Json(body): Json<ProvisionBody>) -> Result<Response, ApiError> {
    let Some(cfg) = MailConfig::from_env() else { return Ok(not_configured()) };
    let (runtime_id, user_id) = device(&state, &headers).await?;
    if body.agent_id.trim().is_empty() {
        return Err(ApiError::BadRequest("agentId is required".into()));
    }
    let existing: Option<(String,)> = sqlx::query_as("SELECT address FROM bot_email_mailboxes WHERE runtime_id = $1 AND agent_id = $2 AND deleted_at IS NULL")
        .bind(&runtime_id)
        .bind(&body.agent_id)
        .fetch_optional(&state.db)
        .await?;
    if let Some((address,)) = existing {
        return Ok((StatusCode::CONFLICT, Json(json!({ "error": "already_provisioned", "address": address }))).into_response());
    }

    // A customer domain must be this user's and verified.
    let mail_domain = match body.domain.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
        None => cfg.domain.clone(),
        Some(raw) => {
            let Some(d) = clean_domain(raw) else { return Err(ApiError::BadRequest("domain is not a valid domain name".into())) };
            let verified: Option<(Option<chrono::DateTime<chrono::Utc>>,)> =
                sqlx::query_as("SELECT verified_at FROM bot_email_domains WHERE domain = $1 AND user_id = $2").bind(&d).bind(&user_id).fetch_optional(&state.db).await?;
            match verified {
                Some((Some(_),)) => d,
                Some((None,)) => return Ok((StatusCode::CONFLICT, Json(json!({ "error": "domain_not_verified", "message": "That domain's DNS records haven't all checked yet." }))).into_response()),
                None => return Err(ApiError::NotFound("Domain not found".into())),
            }
        }
    };

    // The domain's id on Allternit Mail.
    let domains = match call(&cfg, reqwest::Method::GET, "/api/domains", None).await {
        Ok(v) => v,
        Err((s, d)) => return Ok(mail_error("list domains", s, &d)),
    };
    let list = domains.get("domains").and_then(Value::as_array).cloned().or_else(|| domains.as_array().cloned()).unwrap_or_default();
    let Some(domain_id) = list
        .iter()
        .find(|d| d.get("hostname").and_then(Value::as_str).is_some_and(|h| h.eq_ignore_ascii_case(&mail_domain)))
        .and_then(|d| d.get("id").and_then(Value::as_str))
        .map(str::to_string)
    else {
        return Ok(mail_error("find the bot mail domain", None, &mail_domain));
    };

    // 1. The relay route to this runtime (the URL is only ever given to Allternit Mail).
    let key = new_key();
    let route_id = uuid::Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO channel_inbound_routes (id, key_hash, user_id, runtime_id, provider, label) VALUES ($1, $2, $3, $4, 'email', $5)")
        .bind(&route_id)
        .bind(sha256_hex(&key))
        .bind(&user_id)
        .bind(&runtime_id)
        .bind(format!("bot email {}", body.agent_id))
        .execute(&state.db)
        .await?;
    let relay_url = format!("{}/channels/in/{}", public_base(), key);
    let undo_route = |db: sqlx::PgPool, id: String| async move {
        let _ = sqlx::query("UPDATE channel_inbound_routes SET revoked_at = now() WHERE id = $1").bind(id).execute(&db).await;
    };

    // 2. The mailbox; a taken name gets a numeric suffix.
    let base = clean_local_part(body.local_part.as_deref().unwrap_or(&body.agent_id));
    let mut mailbox: Option<Value> = None;
    for n in 0..10 {
        let local = if n == 0 { base.clone() } else { format!("{base}-{}", n + 1) };
        match call(&cfg, reqwest::Method::POST, "/api/mailboxes", Some(json!({ "domainId": domain_id, "localPart": local, "displayName": body.display_name }))).await {
            Ok(v) => {
                mailbox = Some(v);
                break;
            }
            Err((Some(409), _)) => continue,
            Err((s, d)) => {
                undo_route(state.db.clone(), route_id.clone()).await;
                return Ok(mail_error("create the mailbox", s, &d));
            }
        }
    }
    let Some(mailbox) = mailbox else {
        undo_route(state.db.clone(), route_id.clone()).await;
        return Ok((StatusCode::CONFLICT, Json(json!({ "error": "address_taken", "message": "That address and its numbered variants are taken. Pick another name." }))).into_response());
    };
    let mb = mailbox.get("mailbox").unwrap_or(&mailbox);
    let mailbox_id = mb.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
    let address = mb
        .get("address")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("{}@{}", mb.get("localPart").and_then(Value::as_str).unwrap_or(&base), mail_domain));
    let undo_mailbox = |cfg: MailConfig, id: String| async move {
        let _ = call(&cfg, reqwest::Method::DELETE, &format!("/api/mailboxes/{id}"), None).await;
    };

    // 3. A key that can only send and read as this mailbox.
    let key_resp = match call(&cfg, reqwest::Method::POST, "/api/api-keys", Some(json!({ "name": format!("bot {}", body.agent_id), "scopes": ["send", "read"], "mailboxIds": [mailbox_id] }))).await {
        Ok(v) => v,
        Err((s, d)) => {
            undo_mailbox(cfg.clone(), mailbox_id.clone()).await;
            undo_route(state.db.clone(), route_id.clone()).await;
            return Ok(mail_error("create the mailbox key", s, &d));
        }
    };
    let api_key = key_resp.get("key").or_else(|| key_resp.get("token")).and_then(Value::as_str).unwrap_or_default().to_string();
    let api_key_id = key_resp.get("id").and_then(Value::as_str).map(str::to_string);

    // 4. Inbound mail for this mailbox only, to this runtime only.
    let hook = match call(&cfg, reqwest::Method::POST, "/api/webhooks", Some(json!({ "url": relay_url, "events": ["message.inbound"], "mailboxId": mailbox_id }))).await {
        Ok(v) => v,
        Err((s, d)) => {
            undo_mailbox(cfg.clone(), mailbox_id.clone()).await;
            undo_route(state.db.clone(), route_id.clone()).await;
            return Ok(mail_error("create the mailbox webhook", s, &d));
        }
    };
    let webhook_id = hook.get("id").and_then(Value::as_str).map(str::to_string);
    let webhook_secret = hook.get("secret").and_then(Value::as_str).unwrap_or_default().to_string();

    sqlx::query(
        "INSERT INTO bot_email_mailboxes (mailbox_id, user_id, runtime_id, agent_id, address, route_id, webhook_id, api_key_id) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(&mailbox_id)
    .bind(&user_id)
    .bind(&runtime_id)
    .bind(&body.agent_id)
    .bind(&address)
    .bind(&route_id)
    .bind(&webhook_id)
    .bind(&api_key_id)
    .execute(&state.db)
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "address": address, "mailboxId": mailbox_id, "apiKey": api_key, "webhookSecret": webhook_secret, "mailUrl": cfg.base, "provider": "mailflare" })),
    )
        .into_response())
}

async fn teardown(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(mailbox_id): Path<String>) -> Result<Response, ApiError> {
    let Some(cfg) = MailConfig::from_env() else { return Ok(not_configured()) };
    let (runtime_id, _) = device(&state, &headers).await?;
    let row: Option<(Option<String>, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT route_id, webhook_id, api_key_id FROM bot_email_mailboxes WHERE mailbox_id = $1 AND runtime_id = $2 AND deleted_at IS NULL")
            .bind(&mailbox_id)
            .bind(&runtime_id)
            .fetch_optional(&state.db)
            .await?;
    let Some((route_id, webhook_id, api_key_id)) = row else {
        return Err(ApiError::NotFound("Mailbox not found".into()));
    };
    if let Some(id) = webhook_id {
        let _ = call(&cfg, reqwest::Method::DELETE, &format!("/api/webhooks/{id}"), None).await;
    }
    if let Some(id) = api_key_id {
        let _ = call(&cfg, reqwest::Method::DELETE, &format!("/api/api-keys/{id}"), None).await;
    }
    let _ = call(&cfg, reqwest::Method::DELETE, &format!("/api/mailboxes/{mailbox_id}"), None).await;
    if let Some(id) = route_id {
        let _ = sqlx::query("UPDATE channel_inbound_routes SET revoked_at = now() WHERE id = $1").bind(id).execute(&state.db).await;
    }
    sqlx::query("UPDATE bot_email_mailboxes SET deleted_at = now() WHERE mailbox_id = $1").bind(&mailbox_id).execute(&state.db).await?;
    Ok(Json(json!({ "mailboxId": mailbox_id, "deleted": true })).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_parts_are_clean() {
        assert_eq!(clean_local_part("Dana's  Front Desk!"), "danas-front-desk");
        assert_eq!(clean_local_part("  --Ops.Bot_2 "), "ops-bot-2");
        assert_eq!(clean_local_part("✨"), "bot");
        assert_eq!(clean_local_part(&"a".repeat(80)).len(), 48);
    }

    #[test]
    fn customer_domains_are_plain_names_and_never_allternits() {
        assert_eq!(clean_domain(" Mail.Acme.COM. ").as_deref(), Some("mail.acme.com"));
        assert_eq!(clean_domain("acme.co.uk").as_deref(), Some("acme.co.uk"));
        for bad in ["acme", "-acme.com", "acme-.com", "ac me.com", "acme.c0m", "allternit.com", "bots.allternit.com", "a@acme.com", ""] {
            assert_eq!(clean_domain(bad), None, "{bad}");
        }
    }

    #[tokio::test]
    async fn a_domain_belongs_to_one_user() {
        let db = crate::routes::test_support::test_pool().await;
        sqlx::query("CREATE TABLE bot_email_domains (domain text PRIMARY KEY, user_id text NOT NULL, created_at timestamptz NOT NULL DEFAULT now(), verified_at timestamptz)").execute(&db).await.unwrap();
        let d = format!("t{}.example", uuid::Uuid::new_v4().simple());
        assert_eq!(domain_claim(&db, "u1", &d).await.unwrap(), Some(false));
        sqlx::query("INSERT INTO bot_email_domains (domain, user_id) VALUES ($1, 'u1')").bind(&d).execute(&db).await.unwrap();
        assert_eq!(domain_claim(&db, "u1", &d).await.unwrap(), Some(true));
        assert_eq!(domain_claim(&db, "u2", &d).await.unwrap(), None, "someone else's domain");
    }

}
