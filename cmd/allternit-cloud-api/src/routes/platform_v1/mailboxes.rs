//! `/v1/mailboxes`: email addresses for a project's accounts and agents (scope `mail`).
//!
//! A mailbox is an address on Allternit Mail (mx.allternit.com, domain
//! `ALLTERNIT_BOT_EMAIL_DOMAIN`, default `bots.allternit.com`). cloud-api makes
//! it with the Allternit Mail admin key, the same way `bot_email` does for a
//! runtime's bots, and points a `message.inbound` webhook at a `platform_mail`
//! relay route (`/channels/in/<key>`). Inbound mail is kept in
//! `platform_mail_messages` with the one-time codes and sign-in links found in
//! it (the `email_otp` extractor hosted agents use) and sent to the developer
//! as the `mail.received` webhook.
//!
//! * `POST /v1/mailboxes {account_id?, agent_id?, local_part?, display_name?}`
//! * `GET /v1/mailboxes`, `GET|DELETE /v1/mailboxes/{id}`
//! * `GET /v1/mailboxes/{id}/messages`, `GET /v1/mailboxes/{id}/messages/{message_id}`
//! * `POST /v1/mailboxes/{id}/otp {wait_seconds?, since?}`: the newest code,
//!   waiting up to 60 s for one to arrive.
//! * `POST /v1/mailboxes/{id}/send {to, subject, text?, html?}` (live projects;
//!   Allternit Mail may hold it for review: `status: pending_approval`).
//!
//! Billing: card on file (sandbox too), spend cap on live projects, and one
//! `mailbox_month` ($2) per live mailbox per month. A mailbox inside an
//! identity is covered by the identity's bundle instead.
//!
//! [`MailHost`] is the seam: production talks to Allternit Mail; tests layer a
//! fake as an `Extension<Arc<dyn MailHost>>`. Unset mail env → 503 `mail_not_configured`.

use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Extension, Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, PgPool};

use super::caller::ProjectEnv;
use super::{build_page, new_id, projects, record_usage, ApiJson, ApiQuery, Page, PageParams, PlatformCaller, PlatformError, RouteTable, UsageEvent};
use crate::routes::bot_email::{self, MailConfig};
use crate::ApiState;

/// The one-time code / magic link extractor of hosted agents' mail, shared as is.
#[path = "../../../../allternit-api/src/email_otp.rs"]
#[allow(dead_code)]
pub(crate) mod email_otp;

pub const SCOPE: &str = "mail";
pub const METER: &str = "mailbox_month";
/// Relay route provider for platform mailboxes (`channel_inbound`).
pub const ROUTE_PROVIDER: &str = "platform_mail";
pub const MAX_OTP_WAIT_SECS: u64 = 60;
const MAX_SUBJECT: usize = 998;
const MAX_BODY: usize = 256 * 1024;

pub fn register(table: RouteTable) -> RouteTable {
    table
        .add("/v1/mailboxes", &["GET", "POST"], get(list_mailboxes).post(create))
        .add("/v1/mailboxes/:id", &["GET", "DELETE"], get(get_mailbox).delete(remove))
        .add("/v1/mailboxes/:id/messages", &["GET"], get(list_messages))
        .add("/v1/mailboxes/:id/messages/:mid", &["GET"], get(get_message))
        .add("/v1/mailboxes/:id/otp", &["POST"], post(otp))
        .add("/v1/mailboxes/:id/send", &["POST"], post(send))
}

// ---------------------------------------------------------------------------
// Allternit Mail seam
// ---------------------------------------------------------------------------

/// What Allternit Mail answered for a new mailbox.
#[derive(Debug, Clone)]
pub struct CreatedMailbox {
    pub mail_mailbox_id: String,
    pub address: String,
    pub webhook_id: Option<String>,
    pub webhook_secret: Option<String>,
}

#[async_trait]
pub trait MailHost: Send + Sync {
    /// `local@domain` (`-2`, `-3`… when taken) plus a `message.inbound` webhook to `webhook_url`.
    async fn create(&self, local: &str, display_name: Option<&str>, webhook_url: &str) -> Result<CreatedMailbox, PlatformError>;
    /// Remove the webhook and the mailbox (best effort).
    async fn delete(&self, mail_mailbox_id: &str, webhook_id: Option<&str>);
    /// Send as the mailbox. Answers Allternit Mail's `{messageId, jobId?, status}`.
    async fn send(&self, mail_mailbox_id: &str, from: &str, body: &SendBody, idempotency: &str) -> Result<Value, PlatformError>;
    /// The full message (`textBody`, `htmlBody`), when the webhook carried only a snippet.
    async fn fetch(&self, provider_message_id: &str) -> Option<Value>;
}

fn not_configured() -> PlatformError {
    PlatformError::service_unavailable("mail_not_configured", "Mail is not configured on this deployment.")
}

fn mail_failed(what: &str, status: Option<u16>, detail: &str) -> PlatformError {
    tracing::warn!("platform mail: {what} failed ({status:?}): {detail}");
    PlatformError::api_error("mail_service_error", format!("Allternit Mail refused to {what}."))
}

pub struct ProdMailHost;

#[async_trait]
impl MailHost for ProdMailHost {
    async fn create(&self, local: &str, display_name: Option<&str>, webhook_url: &str) -> Result<CreatedMailbox, PlatformError> {
        let cfg = MailConfig::from_env().ok_or_else(not_configured)?;
        let domains = bot_email::call(&cfg, reqwest::Method::GET, "/api/domains", None).await.map_err(|(s, d)| mail_failed("list domains", s, &d))?;
        let list = domains.get("domains").and_then(Value::as_array).cloned().or_else(|| domains.as_array().cloned()).unwrap_or_default();
        let domain_id = list
            .iter()
            .find(|d| d.get("hostname").and_then(Value::as_str).is_some_and(|h| h.eq_ignore_ascii_case(&cfg.domain)))
            .and_then(|d| d.get("id").and_then(Value::as_str))
            .map(str::to_string)
            .ok_or_else(|| mail_failed("find the mail domain", None, &cfg.domain))?;
        let mut made: Option<Value> = None;
        for n in 0..10 {
            let name = if n == 0 { local.to_string() } else { format!("{local}-{}", n + 1) };
            match bot_email::call(&cfg, reqwest::Method::POST, "/api/mailboxes", Some(json!({ "domainId": domain_id, "localPart": name, "displayName": display_name }))).await {
                Ok(v) => {
                    made = Some(v);
                    break;
                }
                Err((Some(409), _)) => continue,
                Err((s, d)) => return Err(mail_failed("create the mailbox", s, &d)),
            }
        }
        let made = made.ok_or_else(|| PlatformError::conflict("address_taken", "That address and its numbered variants are taken. Pick another local_part.").with_param("local_part"))?;
        let mb = made.get("mailbox").unwrap_or(&made);
        let mail_mailbox_id = mb.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
        let address = mb
            .get("address")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("{}@{}", mb.get("localPart").and_then(Value::as_str).unwrap_or(local), cfg.domain));
        let hook = match bot_email::call(&cfg, reqwest::Method::POST, "/api/webhooks", Some(json!({ "url": webhook_url, "events": ["message.inbound"], "mailboxId": mail_mailbox_id }))).await {
            Ok(v) => v,
            Err((s, d)) => {
                let _ = bot_email::call(&cfg, reqwest::Method::DELETE, &format!("/api/mailboxes/{mail_mailbox_id}"), None).await;
                return Err(mail_failed("create the mailbox webhook", s, &d));
            }
        };
        Ok(CreatedMailbox {
            mail_mailbox_id,
            address,
            webhook_id: hook.get("id").and_then(Value::as_str).map(str::to_string),
            webhook_secret: hook.get("secret").and_then(Value::as_str).map(str::to_string),
        })
    }

    async fn delete(&self, mail_mailbox_id: &str, webhook_id: Option<&str>) {
        let Some(cfg) = MailConfig::from_env() else { return };
        if let Some(id) = webhook_id {
            let _ = bot_email::call(&cfg, reqwest::Method::DELETE, &format!("/api/webhooks/{id}"), None).await;
        }
        let _ = bot_email::call(&cfg, reqwest::Method::DELETE, &format!("/api/mailboxes/{mail_mailbox_id}"), None).await;
    }

    async fn send(&self, mail_mailbox_id: &str, from: &str, body: &SendBody, idempotency: &str) -> Result<Value, PlatformError> {
        let cfg = MailConfig::from_env().ok_or_else(not_configured)?;
        let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(15)).build().map_err(|e| mail_failed("send", None, &e.to_string()))?;
        let resp = client
            .post(format!("{}/api/v1/send", cfg.base))
            .bearer_auth(&cfg.admin_key)
            .header("Idempotency-Key", idempotency)
            .json(&json!({ "from": from, "to": body.to, "subject": body.subject, "text": body.text, "html": body.html, "mailboxId": mail_mailbox_id }))
            .send()
            .await
            .map_err(|e| mail_failed("send", None, &e.to_string()))?;
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        if !(200..300).contains(&status) {
            return Err(mail_failed("send", Some(status), &text));
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    async fn fetch(&self, provider_message_id: &str) -> Option<Value> {
        let cfg = MailConfig::from_env()?;
        bot_email::call(&cfg, reqwest::Method::GET, &format!("/api/v1/messages/{provider_message_id}"), None).await.ok()
    }
}

pub type MailExt = Option<Extension<Arc<dyn MailHost>>>;

pub fn host_for(layered: MailExt) -> Arc<dyn MailHost> {
    match layered {
        Some(Extension(h)) => h,
        None => Arc::new(ProdMailHost),
    }
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, FromRow)]
pub struct MailboxRow {
    pub id: String,
    pub account_id: String,
    pub agent_id: Option<String>,
    pub address: String,
    pub display_name: Option<String>,
    pub mail_mailbox_id: String,
    pub route_id: Option<String>,
    pub webhook_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

const COLUMNS: &str = "id, account_id, agent_id, address, display_name, mail_mailbox_id, route_id, webhook_id, created_at";

impl MailboxRow {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id, "object": "mailbox", "account_id": self.account_id, "agent_id": self.agent_id,
            "address": self.address, "display_name": self.display_name, "created_at": self.created_at,
        })
    }
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct MailMessage {
    pub id: String,
    pub mailbox_id: String,
    #[sqlx(rename = "from_address")]
    pub from: String,
    #[sqlx(rename = "to_address")]
    pub to: String,
    pub subject: Option<String>,
    pub snippet: Option<String>,
    pub codes: Value,
    pub links: Value,
    pub is_verification: bool,
    pub received_at: DateTime<Utc>,
}

const MESSAGE_COLUMNS: &str = "id, mailbox_id, from_address, to_address, subject, snippet, codes, links, is_verification, received_at";

impl MailMessage {
    pub fn to_json(&self) -> Value {
        let mut v = serde_json::to_value(self).unwrap_or(Value::Null);
        v["object"] = json!("mail_message");
        v
    }
}

/// A live mailbox of this project the key may see, or 404.
pub async fn api_mailbox(db: &PgPool, caller: &PlatformCaller, id: &str) -> Result<MailboxRow, PlatformError> {
    let account = caller.account_filter(None)?;
    sqlx::query_as::<_, MailboxRow>(&format!(
        "SELECT {COLUMNS} FROM platform_mailboxes WHERE id = $1 AND project_id = $2 AND deleted_at IS NULL AND ($3::text IS NULL OR account_id = $3)"
    ))
    .bind(id)
    .bind(&caller.project_id)
    .bind(&account)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| PlatformError::not_found("mailbox_not_found", "No such mailbox."))
}

/// The account a new mailbox or identity belongs to: the agent's when one is
/// named (and it must agree with `account_id`), else `account_id` or the key's
/// bound account. Checked against the project.
pub(crate) async fn resolve_account(state: &ApiState, caller: &PlatformCaller, account_id: Option<&str>, agent_id: Option<&str>) -> Result<(String, Option<super::agents::Agent>), PlatformError> {
    let agent = match agent_id {
        Some(id) => Some(super::agents::fetch_visible(state, caller, id).await.map_err(|e| e.with_param("agent_id"))?),
        None => None,
    };
    let requested = account_id.or(agent.as_ref().map(|a| a.account_id.as_str()));
    let account = caller
        .account_filter(requested)?
        .ok_or_else(|| PlatformError::invalid_request("missing_account_id", "account_id is required (or an agent_id, or a key bound to one account).").with_param("account_id"))?;
    if let Some(a) = &agent {
        if a.account_id != account {
            return Err(PlatformError::invalid_request("account_mismatch", "The agent belongs to a different account.").with_param("agent_id"));
        }
    }
    projects::account_in_project(&state.db, &caller.project_id, &account).await?;
    Ok((account, agent))
}

/// Card on file (sandbox too), and live projects under their spend cap.
pub(crate) async fn billable(state: &ApiState, caller: &PlatformCaller) -> Result<(), PlatformError> {
    if caller.project_env == ProjectEnv::Sandbox {
        super::billing::require_payment_method(&state.db, &caller.project_id).await
    } else {
        super::spend_allowed(&state.db, &caller.project_id).await
    }
}

/// Make the mailbox: relay route, the address on Allternit Mail, its webhook,
/// then the row. Records no usage (callers bill it their own way). A failure
/// after the address exists removes it again.
pub(crate) async fn provision(
    state: &ApiState,
    caller: &PlatformCaller,
    host: &dyn MailHost,
    account: &str,
    agent: Option<&super::agents::Agent>,
    local_part: Option<&str>,
    display_name: Option<&str>,
) -> Result<MailboxRow, PlatformError> {
    let id = new_id("mbx_");
    let local = bot_email::clean_local_part(local_part.or(agent.map(|a| a.name.as_str())).unwrap_or("agent"));
    let key = crate::routes::channel_inbound::new_key();
    let route_id = uuid::Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO channel_inbound_routes (id, key_hash, user_id, runtime_id, provider, label) VALUES ($1, $2, $3, '', $4, $5)")
        .bind(&route_id)
        .bind(crate::routes::channel_inbound::sha256_hex(&key))
        .bind(&caller.owner_user_id)
        .bind(ROUTE_PROVIDER)
        .bind(&id)
        .execute(&state.db)
        .await?;
    let webhook_url = format!("{}/channels/in/{}", crate::routes::channel_inbound::public_base(), key);
    let made = match host.create(&local, display_name, &webhook_url).await {
        Ok(m) => m,
        Err(e) => {
            revoke_route(&state.db, &route_id).await;
            return Err(e);
        }
    };
    let sealed = match (&made.webhook_secret, state.credential_cipher.as_deref()) {
        (Some(secret), Some(cipher)) => cipher.encrypt(secret).ok(),
        _ => None,
    };
    let inserted = sqlx::query(
        "INSERT INTO platform_mailboxes (id, project_id, account_id, agent_id, address, display_name, mail_mailbox_id, route_id, webhook_id, webhook_secret_sealed) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(&id)
    .bind(&caller.project_id)
    .bind(account)
    .bind(agent.map(|a| a.id.as_str()))
    .bind(&made.address)
    .bind(display_name)
    .bind(&made.mail_mailbox_id)
    .bind(&route_id)
    .bind(&made.webhook_id)
    .bind(&sealed)
    .execute(&state.db)
    .await;
    if let Err(e) = inserted {
        host.delete(&made.mail_mailbox_id, made.webhook_id.as_deref()).await;
        revoke_route(&state.db, &route_id).await;
        return Err(e.into());
    }
    api_mailbox(&state.db, caller, &id).await
}

async fn revoke_route(db: &PgPool, route_id: &str) {
    let _ = sqlx::query("UPDATE channel_inbound_routes SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL").bind(route_id).execute(db).await;
}

/// Remove the address on Allternit Mail, revoke the relay route, mark the row deleted.
pub(crate) async fn teardown(state: &ApiState, host: &dyn MailHost, mailbox: &MailboxRow) -> Result<(), PlatformError> {
    host.delete(&mailbox.mail_mailbox_id, mailbox.webhook_id.as_deref()).await;
    if let Some(route) = &mailbox.route_id {
        revoke_route(&state.db, route).await;
    }
    sqlx::query("UPDATE platform_mailboxes SET deleted_at = now() WHERE id = $1").bind(&mailbox.id).execute(&state.db).await?;
    Ok(())
}

fn month_event(caller: &PlatformCaller, account: &str, meter: &str, ref_id: &str, prefix: &str) -> UsageEvent {
    let month = Utc::now().format("%Y-%m");
    UsageEvent {
        project_id: caller.project_id.clone(),
        account_id: Some(account.to_string()),
        key_id: Some(caller.key_id.clone()),
        meter: meter.into(),
        quantity: 1.0,
        unit: Some("month".into()),
        ref_id: Some(ref_id.to_string()),
        idempotency: Some(format!("{prefix}:{ref_id}:{month}")),
    }
}

/// One `meter` row for this month, live projects only (sandbox usage isn't charged).
pub(crate) async fn record_month(db: &PgPool, caller: &PlatformCaller, account: &str, meter: &str, ref_id: &str, prefix: &str) {
    if caller.project_env == ProjectEnv::Live {
        let _ = record_usage(db, month_event(caller, account, meter, ref_id, prefix)).await;
    }
}

/// Monthly mailbox fees: one `mailbox_month` per live mailbox of a live project
/// that isn't inside an identity (idempotent per mailbox and month).
pub async fn record_mailbox_months(db: &PgPool) -> Result<u64, sqlx::Error> {
    let month = Utc::now().format("%Y-%m").to_string();
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT m.id, m.project_id, m.account_id FROM platform_mailboxes m JOIN platform_projects p ON p.id = m.project_id \
         WHERE m.deleted_at IS NULL AND p.archived_at IS NULL AND p.env = 'live' \
           AND NOT EXISTS (SELECT 1 FROM platform_identities i WHERE i.mailbox_id = m.id AND i.deleted_at IS NULL)",
    )
    .fetch_all(db)
    .await?;
    let mut n = 0;
    for (id, project, account) in rows {
        let event = UsageEvent {
            project_id: project,
            account_id: Some(account),
            key_id: None,
            meter: METER.to_string(),
            quantity: 1.0,
            unit: Some("month".to_string()),
            ref_id: Some(id.clone()),
            idempotency: Some(format!("mailbox:{id}:{month}")),
        };
        if matches!(record_usage(db, event).await, Ok(Some(_))) {
            n += 1;
        }
    }
    Ok(n)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateBody {
    account_id: Option<String>,
    agent_id: Option<String>,
    local_part: Option<String>,
    display_name: Option<String>,
}

async fn create(State(state): State<Arc<ApiState>>, caller: PlatformCaller, layered: MailExt, ApiJson(body): ApiJson<CreateBody>) -> Result<(StatusCode, Json<Value>), PlatformError> {
    caller.require(SCOPE)?;
    if body.display_name.as_deref().is_some_and(|d| d.len() > 100 || d.chars().any(char::is_control)) {
        return Err(PlatformError::invalid_request("invalid_display_name", "display_name must be at most 100 characters.").with_param("display_name"));
    }
    let (account, agent) = resolve_account(&state, &caller, body.account_id.as_deref(), body.agent_id.as_deref()).await?;
    billable(&state, &caller).await?;
    let host = host_for(layered);
    let mailbox = provision(&state, &caller, host.as_ref(), &account, agent.as_ref(), body.local_part.as_deref(), body.display_name.as_deref()).await?;
    record_month(&state.db, &caller, &account, METER, &mailbox.id, "mailbox").await;
    Ok((StatusCode::CREATED, Json(mailbox.to_json())))
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    #[serde(flatten)]
    page: PageParams,
    account_id: Option<String>,
    agent_id: Option<String>,
}

async fn list_mailboxes(State(state): State<Arc<ApiState>>, caller: PlatformCaller, ApiQuery(q): ApiQuery<ListQuery>) -> Result<Json<Page<Value>>, PlatformError> {
    caller.require(SCOPE)?;
    let limit = q.page.limit()?;
    let (after_at, after_id) = match q.page.cursor()? {
        Some((at, id)) => (Some(at), Some(id)),
        None => (None, None),
    };
    let account = caller.account_filter(q.account_id.as_deref())?;
    let rows = sqlx::query_as::<_, MailboxRow>(&format!(
        "SELECT {COLUMNS} FROM platform_mailboxes WHERE project_id = $1 AND deleted_at IS NULL AND ($2::text IS NULL OR account_id = $2) \
           AND ($3::text IS NULL OR agent_id = $3) AND ($4::timestamptz IS NULL OR (created_at, id) > ($4, $5)) ORDER BY created_at, id LIMIT $6"
    ))
    .bind(&caller.project_id)
    .bind(&account)
    .bind(&q.agent_id)
    .bind(after_at)
    .bind(after_id)
    .bind(limit + 1)
    .fetch_all(&state.db)
    .await?;
    let page = build_page(rows, limit, |m| (m.created_at, m.id.clone()));
    Ok(Json(Page { data: page.data.iter().map(MailboxRow::to_json).collect(), has_more: page.has_more, next_cursor: page.next_cursor }))
}

async fn get_mailbox(State(state): State<Arc<ApiState>>, caller: PlatformCaller, Path(id): Path<String>) -> Result<Json<Value>, PlatformError> {
    caller.require(SCOPE)?;
    Ok(Json(api_mailbox(&state.db, &caller, &id).await?.to_json()))
}

async fn remove(State(state): State<Arc<ApiState>>, caller: PlatformCaller, layered: MailExt, Path(id): Path<String>) -> Result<Json<Value>, PlatformError> {
    caller.require(SCOPE)?;
    let mailbox = api_mailbox(&state.db, &caller, &id).await?;
    if super::identities::mailbox_in_identity(&state.db, &mailbox.id).await? {
        return Err(PlatformError::conflict("mailbox_in_identity", "This mailbox belongs to an identity. DELETE /v1/identities/{id} removes it with its number."));
    }
    teardown(&state, host_for(layered).as_ref(), &mailbox).await?;
    Ok(Json(json!({ "id": mailbox.id, "object": "mailbox", "deleted": true })))
}

#[derive(Debug, Deserialize)]
struct MessagesQuery {
    #[serde(flatten)]
    page: PageParams,
    /// `true`: only messages that look like verification email.
    verification: Option<bool>,
}

/// Newest first.
pub(crate) async fn messages_page(db: &PgPool, mailbox_id: &str, page: &PageParams, verification_only: bool) -> Result<Page<Value>, PlatformError> {
    let limit = page.limit()?;
    let (after_at, after_id) = match page.cursor()? {
        Some((at, id)) => (Some(at), Some(id)),
        None => (None, None),
    };
    let rows = sqlx::query_as::<_, MailMessage>(&format!(
        "SELECT {MESSAGE_COLUMNS} FROM platform_mail_messages WHERE mailbox_id = $1 AND ($2 = false OR is_verification) \
           AND ($3::timestamptz IS NULL OR (received_at, id) < ($3, $4)) ORDER BY received_at DESC, id DESC LIMIT $5"
    ))
    .bind(mailbox_id)
    .bind(verification_only)
    .bind(after_at)
    .bind(after_id)
    .bind(limit + 1)
    .fetch_all(db)
    .await?;
    let page = build_page(rows, limit, |m| (m.received_at, m.id.clone()));
    Ok(Page { data: page.data.iter().map(MailMessage::to_json).collect(), has_more: page.has_more, next_cursor: page.next_cursor })
}

async fn list_messages(State(state): State<Arc<ApiState>>, caller: PlatformCaller, Path(id): Path<String>, ApiQuery(q): ApiQuery<MessagesQuery>) -> Result<Json<Page<Value>>, PlatformError> {
    caller.require(SCOPE)?;
    let mailbox = api_mailbox(&state.db, &caller, &id).await?;
    Ok(Json(messages_page(&state.db, &mailbox.id, &q.page, q.verification.unwrap_or(false)).await?))
}

/// One message with its `text` and `html` bodies (fetched from Allternit Mail
/// once, when the webhook carried only a snippet).
pub(crate) async fn read_message(state: &ApiState, host: &dyn MailHost, mailbox_id: &str, message_id: &str) -> Result<Value, PlatformError> {
    let row: Option<(Option<String>, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT provider_message_id, text_body, html_body FROM platform_mail_messages WHERE id = $1 AND mailbox_id = $2")
            .bind(message_id)
            .bind(mailbox_id)
            .fetch_optional(&state.db)
            .await?;
    let Some((provider_id, mut text, mut html)) = row else {
        return Err(PlatformError::not_found("message_not_found", "No such message."));
    };
    if text.is_none() && html.is_none() {
        if let Some(full) = match provider_id.as_deref() {
            Some(pid) => host.fetch(pid).await,
            None => None,
        } {
            text = full.get("textBody").and_then(Value::as_str).map(str::to_string);
            html = full.get("htmlBody").and_then(Value::as_str).map(str::to_string);
            let _ = sqlx::query("UPDATE platform_mail_messages SET text_body = $1, html_body = $2 WHERE id = $3").bind(&text).bind(&html).bind(message_id).execute(&state.db).await;
        }
    }
    let m = sqlx::query_as::<_, MailMessage>(&format!("SELECT {MESSAGE_COLUMNS} FROM platform_mail_messages WHERE id = $1"))
        .bind(message_id)
        .fetch_one(&state.db)
        .await?;
    let mut v = m.to_json();
    v["text"] = json!(text);
    v["html"] = json!(html);
    Ok(v)
}

async fn get_message(State(state): State<Arc<ApiState>>, caller: PlatformCaller, layered: MailExt, Path((id, mid)): Path<(String, String)>) -> Result<Json<Value>, PlatformError> {
    caller.require(SCOPE)?;
    let mailbox = api_mailbox(&state.db, &caller, &id).await?;
    Ok(Json(read_message(&state, host_for(layered).as_ref(), &mailbox.id, &mid).await?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OtpBody {
    /// How long to wait for a code to arrive (0–60 s, default 0).
    pub wait_seconds: Option<u64>,
    /// Only mail received after this time (default: the last 10 minutes).
    pub since: Option<DateTime<Utc>>,
}

/// The newest message with a code (or sign-in link) received after `since`.
async fn latest_code(db: &PgPool, mailbox_id: &str, since: DateTime<Utc>) -> Result<Option<MailMessage>, sqlx::Error> {
    sqlx::query_as::<_, MailMessage>(&format!(
        "SELECT {MESSAGE_COLUMNS} FROM platform_mail_messages WHERE mailbox_id = $1 AND received_at > $2 \
           AND (jsonb_array_length(codes) > 0 OR jsonb_array_length(links) > 0) ORDER BY received_at DESC, id DESC LIMIT 1"
    ))
    .bind(mailbox_id)
    .bind(since)
    .fetch_optional(db)
    .await
}

/// Poll for the newest one-time code: `{found, code, codes, links, message}`.
pub(crate) async fn read_otp(db: &PgPool, mailbox_id: &str, body: &OtpBody) -> Result<Value, PlatformError> {
    let wait = body.wait_seconds.unwrap_or(0);
    if wait > MAX_OTP_WAIT_SECS {
        return Err(PlatformError::invalid_request("invalid_wait", format!("wait_seconds must be between 0 and {MAX_OTP_WAIT_SECS}.")).with_param("wait_seconds"));
    }
    let since = body.since.unwrap_or_else(|| Utc::now() - chrono::Duration::minutes(10));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(wait);
    loop {
        if let Some(m) = latest_code(db, mailbox_id, since).await? {
            let code = m.codes.get(0).cloned().unwrap_or(Value::Null);
            return Ok(json!({ "object": "mail_otp", "found": true, "code": code, "codes": m.codes, "links": m.links, "message": m.to_json() }));
        }
        if std::time::Instant::now() >= deadline {
            return Ok(json!({ "object": "mail_otp", "found": false, "code": null, "codes": [], "links": [], "message": null }));
        }
        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
    }
}

async fn otp(State(state): State<Arc<ApiState>>, caller: PlatformCaller, Path(id): Path<String>, body: Option<ApiJson<OtpBody>>) -> Result<Json<Value>, PlatformError> {
    caller.require(SCOPE)?;
    let mailbox = api_mailbox(&state.db, &caller, &id).await?;
    let body = body.map(|ApiJson(b)| b).unwrap_or_default();
    Ok(Json(read_otp(&state.db, &mailbox.id, &body).await?))
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SendBody {
    pub to: String,
    pub subject: String,
    pub text: Option<String>,
    pub html: Option<String>,
}

fn plain_address(raw: &str) -> bool {
    let s = raw.trim();
    match s.split_once('@') {
        Some((l, d)) => !l.is_empty() && d.contains('.') && !s.chars().any(|c| c.is_whitespace() || c.is_control() || c == ',' || c == '<' || c == '>'),
        None => false,
    }
}

async fn send(State(state): State<Arc<ApiState>>, caller: PlatformCaller, layered: MailExt, headers: HeaderMap, Path(id): Path<String>, ApiJson(body): ApiJson<SendBody>) -> Result<Response, PlatformError> {
    caller.require(SCOPE)?;
    let mailbox = api_mailbox(&state.db, &caller, &id).await?;
    if caller.project_env == ProjectEnv::Sandbox {
        return Err(PlatformError::invalid_request("sandbox_project", "Sandbox projects receive mail but don't send it. Use a live project."));
    }
    if !plain_address(&body.to) {
        return Err(PlatformError::invalid_request("invalid_to", "to must be one plain email address.").with_param("to"));
    }
    if body.subject.trim().is_empty() || body.subject.len() > MAX_SUBJECT || body.subject.contains(['\r', '\n']) {
        return Err(PlatformError::invalid_request("invalid_subject", "subject is required, one line, at most 998 characters.").with_param("subject"));
    }
    if body.text.is_none() && body.html.is_none() {
        return Err(PlatformError::invalid_request("missing_body", "Send text, html, or both.").with_param("text"));
    }
    if body.text.as_ref().map_or(0, String::len) + body.html.as_ref().map_or(0, String::len) > MAX_BODY {
        return Err(PlatformError::invalid_request("body_too_large", "text and html together must be at most 256 KB."));
    }
    super::spend_allowed(&state.db, &caller.project_id).await?;
    let idem = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(|k| format!("platform:{}:{k}", caller.key_id))
        .unwrap_or_else(|| new_id("send_"));
    let out = host_for(layered).send(&mailbox.mail_mailbox_id, &mailbox.address, &body, &idem).await?;
    let status = out.get("status").and_then(Value::as_str).unwrap_or("queued").to_string();
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({ "object": "mail_send", "mailbox_id": mailbox.id, "from": mailbox.address, "to": body.to.trim(), "message_id": out.get("messageId"), "status": status })),
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// Inbound (from `channel_inbound`, provider `platform_mail`)
// ---------------------------------------------------------------------------

fn signature_ok(secret: &str, headers: &HeaderMap, body: &[u8]) -> bool {
    use hmac::{Hmac, Mac};
    let Some(sig) = headers.get("x-email-platform-signature").and_then(|v| v.to_str().ok()) else { return false };
    let Ok(provided) = hex::decode(sig.trim()) else { return false };
    let Ok(mut mac) = <Hmac<sha2::Sha256> as Mac>::new_from_slice(secret.as_bytes()) else { return false };
    mac.update(body);
    mac.verify_slice(&provided).is_ok()
}

/// An Allternit Mail `message.inbound` webhook on a mailbox's relay route:
/// check its signature (when the secret is kept), store the message with its
/// codes and links, emit `mail.received`. Repeats of one message are kept once.
pub async fn inbound(state: &ApiState, route_id: &str, headers: &HeaderMap, body: &[u8]) -> Response {
    match inbound_inner(state, route_id, headers, body).await {
        Ok(status) => status.into_response(),
        Err(e) => {
            tracing::error!(route = %route_id, "platform inbound mail not kept: {e:?}");
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
    }
}

async fn inbound_inner(state: &ApiState, route_id: &str, headers: &HeaderMap, body: &[u8]) -> Result<StatusCode, sqlx::Error> {
    let db = &state.db;
    let row: Option<(String, String, String, Option<String>)> =
        sqlx::query_as("SELECT id, project_id, account_id, webhook_secret_sealed FROM platform_mailboxes WHERE route_id = $1 AND deleted_at IS NULL")
            .bind(route_id)
            .fetch_optional(db)
            .await?;
    let Some((mailbox_id, project_id, account_id, sealed)) = row else { return Ok(StatusCode::ACCEPTED) };
    if let (Some(sealed), Some(cipher)) = (sealed.as_deref(), state.credential_cipher.as_deref()) {
        if let Ok(secret) = cipher.decrypt(sealed) {
            if !signature_ok(&secret, headers, body) {
                return Ok(StatusCode::UNAUTHORIZED);
            }
        }
    }
    let payload: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    if payload.get("type").and_then(Value::as_str) != Some("message.inbound") {
        return Ok(StatusCode::ACCEPTED);
    }
    let Some(m) = store_inbound(db, &mailbox_id, &payload["data"]).await? else { return Ok(StatusCode::OK) };
    if let Err(e) = super::events::emit_event(db, &project_id, Some(&account_id), "mail.received", json!({ "message": m.to_json() })).await {
        tracing::warn!(mailbox = %mailbox_id, "mail.received not stored: {e}");
    }
    Ok(StatusCode::OK)
}

/// Keep one inbound message (`data` of `message.inbound`). `None` when it was
/// already kept (a webhook retry).
pub(crate) async fn store_inbound(db: &PgPool, mailbox_id: &str, data: &Value) -> Result<Option<MailMessage>, sqlx::Error> {
    let s = |k: &str| data.get(k).and_then(Value::as_str);
    let text = s("textBody");
    let html = s("htmlBody");
    let found = email_otp::extract(s("subject"), text.or(s("snippet")), html);
    let id = new_id("mail_");
    sqlx::query_as::<_, MailMessage>(&format!(
        "INSERT INTO platform_mail_messages (id, mailbox_id, provider_message_id, from_address, to_address, subject, snippet, text_body, html_body, codes, links, is_verification) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) \
         ON CONFLICT (mailbox_id, provider_message_id) WHERE provider_message_id IS NOT NULL DO NOTHING RETURNING {MESSAGE_COLUMNS}"
    ))
    .bind(&id)
    .bind(mailbox_id)
    .bind(s("messageId"))
    .bind(s("from").unwrap_or(""))
    .bind(s("to").or(s("mailbox")).unwrap_or(""))
    .bind(s("subject"))
    .bind(s("snippet"))
    .bind(text)
    .bind(html)
    .bind(json!(found.codes))
    .bind(json!(found.links))
    .bind(found.is_verification)
    .fetch_optional(db)
    .await
}

// ---------------------------------------------------------------------------
// MCP tools (scope `mail`), served by `mcp_edge` for project keys
// ---------------------------------------------------------------------------

/// The mail tools a project key with the `mail` scope gets on `/mcp`.
pub const MCP_TOOLS: [&str; 5] = ["mailbox_create", "mailbox_list", "mail_list", "mail_read", "mail_read_otp"];

/// `tools/list` entries for [`MCP_TOOLS`].
pub fn mcp_tool_defs() -> Vec<Value> {
    let id = json!({ "type": "string", "description": "Mailbox id (mbx_…)." });
    vec![
        json!({ "name": "mailbox_create", "description": "Create an email address for an account or a hosted agent. $2 a month on live projects.",
            "inputSchema": { "type": "object", "properties": { "account_id": { "type": "string" }, "agent_id": { "type": "string" }, "local_part": { "type": "string", "description": "The part before @." }, "display_name": { "type": "string" } }, "additionalProperties": false } }),
        json!({ "name": "mailbox_list", "description": "List the project's mailboxes.",
            "inputSchema": { "type": "object", "properties": { "account_id": { "type": "string" }, "agent_id": { "type": "string" } }, "additionalProperties": false } }),
        json!({ "name": "mail_list", "description": "List a mailbox's messages, newest first, with the one-time codes and sign-in links found in each.",
            "inputSchema": { "type": "object", "properties": { "mailbox_id": id, "limit": { "type": "integer", "minimum": 1, "maximum": 100 }, "verification": { "type": "boolean" } }, "required": ["mailbox_id"], "additionalProperties": false } }),
        json!({ "name": "mail_read", "description": "Read one message with its text and HTML body.",
            "inputSchema": { "type": "object", "properties": { "mailbox_id": id, "message_id": { "type": "string" } }, "required": ["mailbox_id", "message_id"], "additionalProperties": false } }),
        json!({ "name": "mail_read_otp", "description": "The newest one-time code in a mailbox, waiting up to 60 seconds for one to arrive.",
            "inputSchema": { "type": "object", "properties": { "mailbox_id": id, "wait_seconds": { "type": "integer", "minimum": 0, "maximum": 60 }, "since": { "type": "string", "format": "date-time" } }, "required": ["mailbox_id"], "additionalProperties": false } }),
    ]
}

/// Run one mail tool as `caller`. `Err` is the `/v1` error's message.
pub async fn mcp_call(state: &ApiState, caller: &PlatformCaller, host: &dyn MailHost, name: &str, args: &Value) -> Result<Value, PlatformError> {
    caller.require(SCOPE)?;
    let s = |k: &str| args.get(k).and_then(Value::as_str);
    let need = |k: &str| s(k).ok_or_else(|| PlatformError::invalid_request("missing_argument", format!("{k} is required.")).with_param(k));
    match name {
        "mailbox_create" => {
            let (account, agent) = resolve_account(state, caller, s("account_id"), s("agent_id")).await?;
            billable(state, caller).await?;
            let m = provision(state, caller, host, &account, agent.as_ref(), s("local_part"), s("display_name")).await?;
            record_month(&state.db, caller, &account, METER, &m.id, "mailbox").await;
            Ok(m.to_json())
        }
        "mailbox_list" => {
            let account = caller.account_filter(s("account_id"))?;
            let rows = sqlx::query_as::<_, MailboxRow>(&format!(
                "SELECT {COLUMNS} FROM platform_mailboxes WHERE project_id = $1 AND deleted_at IS NULL AND ($2::text IS NULL OR account_id = $2) \
                   AND ($3::text IS NULL OR agent_id = $3) ORDER BY created_at, id LIMIT 100"
            ))
            .bind(&caller.project_id)
            .bind(&account)
            .bind(s("agent_id"))
            .fetch_all(&state.db)
            .await?;
            Ok(json!({ "data": rows.iter().map(MailboxRow::to_json).collect::<Vec<_>>() }))
        }
        "mail_list" => {
            let m = api_mailbox(&state.db, caller, need("mailbox_id")?).await?;
            let page = PageParams { limit: args.get("limit").and_then(Value::as_i64), after: None };
            Ok(serde_json::to_value(messages_page(&state.db, &m.id, &page, args.get("verification").and_then(Value::as_bool).unwrap_or(false)).await?).unwrap_or(Value::Null))
        }
        "mail_read" => {
            let m = api_mailbox(&state.db, caller, need("mailbox_id")?).await?;
            read_message(state, host, &m.id, need("message_id")?).await
        }
        "mail_read_otp" => {
            let m = api_mailbox(&state.db, caller, need("mailbox_id")?).await?;
            let since = match s("since") {
                Some(raw) => Some(raw.parse::<DateTime<Utc>>().map_err(|_| PlatformError::invalid_request("invalid_since", "since must be an RFC 3339 time.").with_param("since"))?),
                None => None,
            };
            read_otp(&state.db, &m.id, &OtpBody { wait_seconds: args.get("wait_seconds").and_then(Value::as_u64), since }).await
        }
        _ => Err(PlatformError::not_found("unknown_tool", format!("No tool named '{name}'."))),
    }
}
