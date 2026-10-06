//! Agent email via the vendored mailflare service.
//!
//! Authenticated surface (mounted under `/api/v1`):
//! - `POST /agent-email/send` — approval-gated outbound send for an agent.
//! - `GET  /agent-email/status` — operator diagnostics (configured/domain/reachable).
//!
//! Public surface (mounted on the public router in main.rs, HMAC-verified):
//! - `POST /api/v1/agent-email/inbound` — mailflare `message.inbound` webhook.
//!
//! The Rails Mail review path (`POST /api/rails/mail/decide`) approves/rejects
//! pending outbound email through `decide_outbound_for_thread`, which is a
//! no-op for threads that have no pending outbound email row.

use axum::{
    extract::{Extension, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use hmac::{Hmac, Mac};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::Sha256;
use std::sync::Arc;
use tracing::{info, warn};

use crate::auth::AuthUser;
use crate::mailflare_client::{MailflareClient, SendEmailRequest};
use crate::AppState;
use allternit_factory_engine::{MailImportance, TypedMessage};

type HmacSha256 = Hmac<Sha256>;

type ApiError = (StatusCode, Json<Value>);

fn err(status: StatusCode, code: &str, message: impl Into<String>) -> ApiError {
    (
        status,
        Json(json!({"error": code, "message": message.into()})),
    )
}

fn internal(error: impl std::fmt::Display) -> ApiError {
    err(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal_error",
        error.to_string(),
    )
}

pub fn agent_email_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/agent-email/send", post(send_agent_email))
        .route("/agent-email/status", get(agent_email_status))
        .route("/agent-email/domains", get(email_domains_list))
        .route("/agent-email/domains/:domain", axum::routing::put(email_domain_add).delete(email_domain_remove))
}

/// One customer-domain call: through the local admin key when this runtime has
/// one, otherwise through Allternit's cloud with this runtime's device credential
/// (the cloud records which account owns the domain). Answers what the service
/// answered, so the app sees `{domain, state, records}` or `{domains}`.
async fn email_domain_call(method: reqwest::Method, domain: Option<&str>) -> Response {
    if let Some(client) = crate::mailflare_client::MailflareClient::from_env() {
        let Some(domain) = domain else {
            // A runtime with its own admin key has no per-account list; the app asks per domain.
            return Json(json!({ "domains": [] })).into_response();
        };
        return match client.relay_domain(method.clone(), domain).await {
            Ok(v) if method == reqwest::Method::DELETE => Json(json!({ "domain": domain, "deleted": true, "detail": v })).into_response(),
            Ok(v) => {
                let verified = v.get("verified").and_then(Value::as_bool).unwrap_or(false);
                Json(json!({ "domain": v.get("domain").cloned().unwrap_or(json!(domain)), "state": if verified { "verified" } else { "pending" }, "records": v.get("records").cloned().unwrap_or(json!([])) })).into_response()
            }
            Err(e) => err(e.status.unwrap_or(StatusCode::BAD_GATEWAY), "email_domain_error", e.message).into_response(),
        };
    }
    let Some(bearer) = crate::phone_sync::runtime_bearer() else {
        return err(StatusCode::CONFLICT, "runtime_not_paired", "Sign this computer in to your Allternit account to use a company domain.").into_response();
    };
    let base = crate::phone_sync::cloud_base();
    let url = match domain {
        Some(d) => format!("{}/api/v1/runtime-devices/me/bot-email/domains/{}", base.trim_end_matches('/'), urlencoding::encode(d)),
        None => format!("{}/api/v1/runtime-devices/me/bot-email/domains", base.trim_end_matches('/')),
    };
    match reqwest::Client::new().request(method, url).bearer_auth(bearer).timeout(std::time::Duration::from_secs(45)).send().await {
        Ok(r) => {
            let status = StatusCode::from_u16(r.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let body: Value = r.json().await.unwrap_or(Value::Null);
            (status, Json(body)).into_response()
        }
        Err(e) => err(StatusCode::BAD_GATEWAY, "cloud_unreachable", e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
struct DomainQuery {
    domain: Option<String>,
}

/// `GET /agent-email/domains[?domain=acme.com]`: the account's company domains, each checked
/// live. A runtime with its own admin key keeps no per-account list, so it checks `domain`.
async fn email_domains_list(Extension(_user): Extension<AuthUser>, axum::extract::Query(q): axum::extract::Query<DomainQuery>) -> Response {
    let one = q.domain.map(|d| d.trim().to_ascii_lowercase()).filter(|d| !d.is_empty());
    if crate::mailflare_client::MailflareClient::from_env().is_some() {
        if let Some(d) = one {
            let r = email_domain_call(reqwest::Method::GET, Some(&d)).await;
            if !r.status().is_success() {
                return r;
            }
            let body = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap_or_default();
            let v: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            return Json(json!({ "domains": [v] })).into_response();
        }
    }
    email_domain_call(reqwest::Method::GET, None).await
}

/// `PUT /agent-email/domains/:domain`: add a company domain; answers the DNS records to add.
async fn email_domain_add(Extension(_user): Extension<AuthUser>, Path(domain): Path<String>) -> Response {
    email_domain_call(reqwest::Method::PUT, Some(domain.trim())).await
}

/// `DELETE /agent-email/domains/:domain`: remove a company domain no bot uses.
async fn email_domain_remove(Extension(_user): Extension<AuthUser>, Path(domain): Path<String>) -> Response {
    email_domain_call(reqwest::Method::DELETE, Some(domain.trim())).await
}

/// Public webhook surface for mailflare inbound messages. Server-to-server —
/// no Clerk session exists, so requests are authenticated by the HMAC
/// signature instead (same shape as the Slack/Photon webhooks).
pub fn agent_email_webhook_router() -> Router<Arc<AppState>> {
    agent_email_webhook_router_with(crate::relay_auth::process_secret())
}

/// The inbound webhook reaches the runtime only through cloud-api's relay, so
/// it needs the relay signature ([`RelayedAuth`]) on top of mailflare's HMAC.
pub fn agent_email_webhook_router_with(secret: Arc<dyn crate::relay_auth::RelaySecret>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/agent-email/inbound", post(receive_inbound_email))
        .layer(crate::relay_auth::secret_layer(secret))
}

/// Verify the agent exists and is owned by the given user (same contract
/// as `allternit_bus_routes::require_agent_owner`). Takes a bare user id so
/// callers that authenticate outside the Clerk middleware (the internal MCP
/// surface authenticates via device token or the internal service token and
/// carries only a user id) share the exact same check.
pub(crate) fn require_agent_owner_id(
    state: &AppState,
    user_id: &str,
    agent_id: &str,
) -> Result<(), ApiError> {
    let conn = state.db.connect().map_err(internal)?;
    let owner: Option<String> = conn
        .query_row(
            "SELECT user_id FROM agents WHERE id = ?1",
            params![agent_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(internal)?;
    match owner {
        Some(owner_id) if owner_id == user_id => Ok(()),
        Some(_) => Err(err(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Agent does not belong to user",
        )),
        None => Err(err(StatusCode::NOT_FOUND, "not_found", "Agent not found")),
    }
}

/// An agent's mailflare email channel row.
pub struct AgentEmailChannel {
    pub address: String,
    pub send_enabled: bool,
    pub receive_enabled: bool,
    pub mailbox_id: Option<String>,
    /// Sealed per-agent mailflare API key (`token_crypto::seal` format).
    pub api_key_sealed: Option<String>,
    /// How the bot answers inbound email (`approve` | `auto_known` | `auto`).
    pub reply_mode: String,
    /// JSON array of sender domains that may be answered without approval.
    pub reply_allowlist: Option<String>,
    /// Per-bot kill switch: when false, turns still run but no reply is sent.
    pub reply_enabled: bool,
    /// The mailbox's domain passed outbound verification; required for `auto`.
    pub domain_verified: bool,
    /// Cloud-brokered mailboxes: the sealed per-mailbox webhook secret and the
    /// mail service URL the mailbox lives on.
    pub webhook_secret_sealed: Option<String>,
    pub mail_url: Option<String>,
}

impl AgentEmailChannel {
    /// The client to act on this mailbox with: the local admin config when this
    /// runtime has one, else the cloud-brokered client for the mailbox's URL.
    pub fn client(&self) -> Option<MailflareClient> {
        MailflareClient::from_env().or_else(|| self.mail_url.as_deref().map(|u| MailflareClient::new(crate::mailflare_client::MailflareConfig::brokered(u))))
    }
}

/// Look up the agent's mailflare email channel. Returns `None` when the agent
/// has no mailflare-backed email (including legacy `commrails` rows).
pub fn lookup_email_channel(
    conn: &rusqlite::Connection,
    agent_id: &str,
) -> rusqlite::Result<Option<AgentEmailChannel>> {
    conn.query_row(
        "SELECT email_address, email_send_enabled, email_receive_enabled,
                email_mailbox_id, email_api_key_sealed,
                email_reply_mode, email_reply_allowlist, email_reply_enabled,
                email_domain_verified, email_webhook_secret_sealed, email_mail_url
         FROM agent_identity_channels
         WHERE agent_id = ?1 AND email_provider = 'mailflare'",
        params![agent_id],
        |row| {
            Ok(AgentEmailChannel {
                address: row.get(0)?,
                send_enabled: row.get::<_, i32>(1)? != 0,
                receive_enabled: row.get::<_, i32>(2)? != 0,
                mailbox_id: row.get(3)?,
                api_key_sealed: row.get(4)?,
                reply_mode: row.get::<_, Option<String>>(5)?.unwrap_or_else(|| "approve".into()),
                reply_allowlist: row.get(6)?,
                reply_enabled: row.get::<_, Option<i32>>(7)?.unwrap_or(1) != 0,
                domain_verified: row.get::<_, Option<i32>>(8)?.unwrap_or(0) != 0,
                webhook_secret_sealed: row.get(9)?,
                mail_url: row.get(10)?,
            })
        },
    )
    .optional()
}

/// Open the agent's sealed mailflare API key (fail-closed like token_crypto).
fn open_channel_key(channel: &AgentEmailChannel) -> Result<String, ApiError> {
    let sealed = channel.api_key_sealed.as_deref().ok_or_else(|| {
        err(
            StatusCode::CONFLICT,
            "email_key_missing",
            "Agent has no mailflare API key; re-provision the email channel.",
        )
    })?;
    let key = crate::token_crypto::open(sealed);
    if key.is_empty() {
        return Err(err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "email_key_unreadable",
            "Agent mailflare API key could not be decrypted.",
        ));
    }
    Ok(key)
}

// ============================================================================
// Outbound send (approval-gated)
// ============================================================================

#[derive(Debug, Deserialize)]
pub(crate) struct SendAgentEmailRequest {
    pub(crate) agent_id: String,
    pub(crate) to: String,
    pub(crate) subject: String,
    pub(crate) text: Option<String>,
    pub(crate) html: Option<String>,
}

async fn send_agent_email(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(req): Json<SendAgentEmailRequest>,
) -> Result<Response, ApiError> {
    Ok(Json(send_email_for_user(&state, &user.user_id, req).await?).into_response())
}

/// Reply-path extras on top of a plain outbound send. `skip_approval` delivers
/// through the allternit-api admin key with mailflare's approval gate bypassed
/// — only the bot-reply path may set it, and only when the reply plan says so.
#[derive(Default)]
struct SendEmailExtra<'a> {
    /// Inbound row this send answers (marks the outbound row for the reply caps).
    reply_inbound_id: Option<&'a str>,
    /// Threading/auto-reply headers mailflare allow-lists.
    headers: Option<&'a std::collections::HashMap<String, String>>,
    /// Deliver without human approval (admin key only).
    skip_approval: bool,
    /// Files to attach (already size-checked by `channel_files::parse`).
    attachments: &'a [crate::channel_files::ChannelFile],
}

/// Approval-gated outbound send, shared by the REST route and the internal MCP
/// `allternit_mail.send` tool. Enforces agent ownership by `user_id`, records
/// the outbound row, and returns the same JSON payload either way.
/// [`send_email_for_user`] with files attached (a bot starting an email thread).
pub(crate) async fn send_email_with_files(
    state: &Arc<AppState>,
    user_id: &str,
    req: SendAgentEmailRequest,
    attachments: &[crate::channel_files::ChannelFile],
) -> Result<Value, ApiError> {
    send_email_inner(state, user_id, req, SendEmailExtra { attachments, ..SendEmailExtra::default() }).await
}

/// An Allternit notice to the owner's own address (a Factory approval request
/// or its outcome): delivered without mailflare's review because it goes only
/// to the bot owner's account email. The owner's autonomy level still applies.
pub(crate) async fn send_owner_notice(
    state: &Arc<AppState>,
    user_id: &str,
    req: SendAgentEmailRequest,
) -> Result<Value, ApiError> {
    send_email_inner(state, user_id, req, SendEmailExtra { skip_approval: true, ..SendEmailExtra::default() }).await
}

pub(crate) async fn send_email_for_user(
    state: &Arc<AppState>,
    user_id: &str,
    req: SendAgentEmailRequest,
) -> Result<Value, ApiError> {
    send_email_inner(state, user_id, req, SendEmailExtra::default()).await
}

async fn send_email_inner(
    state: &Arc<AppState>,
    user_id: &str,
    req: SendAgentEmailRequest,
    extra: SendEmailExtra<'_>,
) -> Result<Value, ApiError> {
    require_agent_owner_id(state, user_id, &req.agent_id)?;
    // Autonomy (the owner's per-place level) can only tighten here: a hold means mailflare's review.
    let autonomy = crate::autonomy::email_eval(&state.db, user_id, &req.agent_id, &req.to);
    let held = autonomy.as_ref().is_some_and(|e| e.held());
    let extra = SendEmailExtra { skip_approval: extra.skip_approval && !held, ..extra };

    if req.subject.trim().is_empty() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "subject is required",
        ));
    }
    if req.text.is_none() && req.html.is_none() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "text or html body is required",
        ));
    }

    let channel = {
        let conn = state.db.connect().map_err(internal)?;
        lookup_email_channel(&conn, &req.agent_id)
            .map_err(internal)?
            .ok_or_else(|| {
                // No address, and this computer can't make one either: say email is off here.
                if crate::mailflare_client::MailflareConfig::from_env().is_none() && !crate::mailflare_client::brokered_available() {
                    return err(
                        StatusCode::NOT_IMPLEMENTED,
                        "mailflare_not_configured",
                        "Allternit Mail is not configured on this runtime.",
                    );
                }
                err(
                    StatusCode::CONFLICT,
                    "email_not_provisioned",
                    "Agent has no mailflare email channel; provision one first.",
                )
            })?
    };
    if !channel.send_enabled {
        return Err(err(
            StatusCode::FORBIDDEN,
            "email_send_disabled",
            "Outbound email is disabled for this agent.",
        ));
    }
    let mailbox_id = channel.mailbox_id.clone().ok_or_else(|| {
        err(
            StatusCode::CONFLICT,
            "email_mailbox_missing",
            "Agent email channel has no mailflare mailbox id; re-provision.",
        )
    })?;
    let client = channel.client().ok_or_else(|| {
        err(
            StatusCode::NOT_IMPLEMENTED,
            "mailflare_not_configured",
            "Allternit Mail is not configured on this runtime.",
        )
    })?;
    let brokered = client.config().brokered;
    // Direct (unreviewed) replies: with the local admin key, mailflare honors
    // skipApproval. A cloud-brokered runtime has only the bot's own key: it
    // sends (mailflare holds it) and then approves its own send below, since
    // this runtime's review and autonomy already decided it may go.
    let api_key = if extra.skip_approval && !brokered {
        client.config().admin_key.clone()
    } else {
        open_channel_key(&channel)?
    };

    // Record first so the row exists even when the mailflare call fails.
    let outbound_id = uuid::Uuid::new_v4().to_string();
    let idempotency_key = match extra.reply_inbound_id {
        Some(inbound_id) => format!("agent-email-reply:{inbound_id}"),
        None => format!("agent-email:{outbound_id}"),
    };
    let thread_id = format!("mail:email-out-{outbound_id}");
    let snippet: String = req
        .text
        .as_deref()
        .or(req.html.as_deref())
        .unwrap_or("")
        .chars()
        .take(200)
        .collect();
    {
        let conn = state.db.connect().map_err(internal)?;
        conn.execute(
            "INSERT INTO agent_email_outbound
                 (id, agent_id, user_id, thread_id, idempotency_key, to_address, subject, snippet, status, updated_at, reply_inbound_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending_approval', CURRENT_TIMESTAMP, ?9)",
            params![
                outbound_id,
                req.agent_id,
                user_id,
                thread_id,
                idempotency_key,
                req.to,
                req.subject,
                snippet,
                extra.reply_inbound_id
            ],
        )
        .map_err(internal)?;
    }

    let mail_attachments: Vec<crate::mailflare_client::MailAttachment> = {
        use base64::Engine as _;
        extra
            .attachments
            .iter()
            .map(|f| crate::mailflare_client::MailAttachment { filename: f.filename.clone(), mime: f.mime.clone(), content_base64: base64::engine::general_purpose::STANDARD.encode(&f.data) })
            .collect()
    };
    let send_result = client
        .send_with_options(
            &api_key,
            &SendEmailRequest {
                from: &channel.address,
                to: &req.to,
                subject: &req.subject,
                text: req.text.as_deref(),
                html: req.html.as_deref(),
                mailbox_id: &mailbox_id,
                headers: extra.headers,
                attachments: &mail_attachments,
            },
            &idempotency_key,
            extra.skip_approval,
        )
        .await;

    let mut send_response = match send_result {
        Ok(response) => response,
        Err(e) => {
            mark_outbound_failed(state, &outbound_id, &e.to_string());
            return Err(err(
                StatusCode::BAD_GATEWAY,
                "mailflare_send_failed",
                e.to_string(),
            ));
        }
    };

    if brokered && extra.skip_approval && send_response.status == "pending_approval" {
        if let Some(job) = send_response.job_id.clone() {
            match client.approve(&api_key, &job).await {
                Ok(_) => send_response.status = "queued".into(),
                Err(e) => warn!(error = %e, outbound_id = %outbound_id, "agent-email: self-approve failed; left for review"),
            }
        }
    }

    let conn = state.db.connect().map_err(internal)?;
    if send_response.status == "pending_approval" {
        conn.execute(
            "UPDATE agent_email_outbound SET job_id = ?1, message_id = ?2, updated_at = CURRENT_TIMESTAMP WHERE id = ?3",
            params![send_response.job_id, send_response.message_id, outbound_id],
        )
        .map_err(internal)?;

        // Surface a human review card on the per-send thread.
        if let Err(e) = state.rails.mail.ensure_thread(&thread_id).await {
            warn!(error = %e, thread_id = %thread_id, "agent-email: ensure_thread failed");
        }
        if let Err(e) = state
            .rails
            .mail
            .request_review(
                &thread_id,
                &outbound_id,
                &format!("agent-email-outbound:{outbound_id}"),
            )
            .await
        {
            warn!(error = %e, thread_id = %thread_id, "agent-email: request_review failed");
        }

        info!(agent_id = %req.agent_id, to = %req.to, "agent-email: outbound pending approval");
        if let Some(e) = autonomy.as_ref().filter(|e| e.held()) {
            let (outcome, reason) = match &e.verdict {
                crate::autonomy::Verdict::Draft { reason } => ("drafted", reason.clone()),
                crate::autonomy::Verdict::Hold { reason } => ("held", reason.clone()),
                _ => ("held", String::new()),
            };
            let act = crate::autonomy::Action { owner: user_id, bot_id: &req.agent_id, channel: "email", persons: vec![req.to.clone()], action: "message", amount_cents: 0 };
            crate::autonomy::finish(&state.db, &act, autonomy.as_ref(), outcome, &outbound_id, &format!("{}\n{}", req.subject, snippet), &reason, Some(&thread_id), json!({ "outboundId": outbound_id, "subject": req.subject }));
        }
        return Ok(json!({
            "status": "pending_approval",
            "id": outbound_id,
            "thread": thread_id,
            "jobId": send_response.job_id,
            "messageId": send_response.message_id,
        }));
    }

    // REQUIRE_SEND_APPROVAL disabled on the worker — the send went straight
    // to the provider queue.
    conn.execute(
        "UPDATE agent_email_outbound SET status = 'sent', job_id = ?1, message_id = ?2, provider_message_id = ?2, updated_at = CURRENT_TIMESTAMP WHERE id = ?3",
        params![send_response.job_id, send_response.message_id, outbound_id],
    )
    .map_err(internal)?;
    {
        let act = crate::autonomy::Action { owner: user_id, bot_id: &req.agent_id, channel: "email", persons: vec![req.to.clone()], action: "message", amount_cents: 0 };
        crate::autonomy::finish(&state.db, &act, autonomy.as_ref(), "sent", &outbound_id, &format!("{}\n{}", req.subject, snippet), "", None, json!({ "outboundId": outbound_id, "subject": req.subject }));
    }
    Ok(json!({
        "status": "sent",
        "id": outbound_id,
        "messageId": send_response.message_id,
    }))
}

fn mark_outbound_failed(state: &AppState, outbound_id: &str, error: &str) {
    if let Ok(conn) = state.db.connect() {
        if let Err(e) = conn.execute(
            "UPDATE agent_email_outbound SET status = 'failed', error = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
            params![error, outbound_id],
        ) {
            warn!(error = %e, "agent-email: failed to mark outbound failed");
        }
    }
}

// ============================================================================
// Bot reply to an inbound turn (the reason this rail exists)
// ============================================================================

/// Record what the reply did on the inbound row ('sent', 'pending_approval',
/// 'failed', 'turn_failed', 'disabled', 'skipped') so a conversation's email
/// trail is explainable from the database alone.
fn set_reply_status(db: &crate::db::DbHandle, inbound_id: &str, status: &str) {
    if let Ok(conn) = db.connect() {
        if let Err(e) = conn.execute(
            "UPDATE agent_email_inbound SET reply_status = ?1 WHERE id = ?2",
            params![status, inbound_id],
        ) {
            warn!(error = %e, inbound_id = %inbound_id, "agent-email: failed to record reply status");
        }
    }
}

fn env_cap(name: &str, default: i64) -> i64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(default)
}

/// Reply caps: at most N bot replies per sender per hour and M per bot per
/// day. Counts every outbound row marked as a reply, whatever its status, so
/// approval-pending replies also count toward the limit.
fn over_reply_caps(conn: &rusqlite::Connection, agent_id: &str, to: &str) -> bool {
    let max_sender_hour = env_cap("ALLTERNIT_BOT_EMAIL_MAX_PER_SENDER_HOUR", 5);
    let max_bot_day = env_cap("ALLTERNIT_BOT_EMAIL_MAX_PER_BOT_DAY", 200);
    let sender_count: i64 = conn
        .query_row(
            "SELECT count(*) FROM agent_email_outbound
             WHERE agent_id = ?1 AND reply_inbound_id IS NOT NULL
               AND LOWER(to_address) = LOWER(?2)
               AND created_at > datetime('now', '-1 hour')",
            params![agent_id, to],
            |row| row.get(0),
        )
        .unwrap_or(0);
    let bot_count: i64 = conn
        .query_row(
            "SELECT count(*) FROM agent_email_outbound
             WHERE agent_id = ?1 AND reply_inbound_id IS NOT NULL
               AND created_at > datetime('now', '-1 day')",
            params![agent_id],
            |row| row.get(0),
        )
        .unwrap_or(0);
    sender_count >= max_sender_hour || bot_count >= max_bot_day
}

/// Whether the sender already has any email thread with this bot (the
/// `auto_known` mode trusts these senders without approval).
fn sender_has_thread(conn: &rusqlite::Connection, agent_id: &str, from_lower: &str) -> bool {
    let prefix = format!("email:{from_lower}:");
    conn.query_row(
        "SELECT count(*) FROM bot_threads
         WHERE bot_id = ?1 AND status NOT IN ('done', 'failed')
           AND substr(json_extract(origin, '$.channelKey'), 1, length(?2)) = ?2",
        params![agent_id, prefix],
        |row| row.get::<_, i64>(0),
    )
    .unwrap_or(0)
        > 0
}

/// Email the bot's turn answer back to the sender. Runs after `send_bot_turn`
/// returns `Ok(reply)`; the thread's conversation continues by email from
/// here. Approval policy (mode, allowlist, domain verification, caps, kill
/// switch) is re-read here so a settings change mid-conversation applies to
/// the next reply, not the next restart.
async fn send_reply_for_turn(
    state: &Arc<AppState>,
    db: &crate::db::DbHandle,
    agent_id: &str,
    inbound_id: &str,
    from: &str,
    subject: &str,
    turn_body: &str,
    reply: &str,
) -> Result<(), String> {
    use crate::agent_email_reply::{
        build_references, build_reply_text, decide_reply_plan, parse_allowlist, quote_excerpt,
        reply_recipient, reply_subject, sender_domain, ReplyMode, ReplyPlan,
    };

    let (channel, owner, in_reply_to, email_references, reply_to) = {
        let conn = db.connect().map_err(|e| e.to_string())?;
        let channel = lookup_email_channel(&conn, agent_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "agent email channel missing".to_string())?;
        let owner: String = conn
            .query_row(
                "SELECT user_id FROM agents WHERE id = ?1",
                params![agent_id],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        let (in_reply_to, email_references, reply_to): (Option<String>, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT in_reply_to, email_references, reply_to FROM agent_email_inbound WHERE id = ?1",
                params![inbound_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|e| e.to_string())?;
        (channel, owner, in_reply_to, email_references, reply_to)
    };

    // Per-bot kill switch: the turn ran, the reply does not go out.
    if !channel.reply_enabled {
        info!(agent_id = %agent_id, inbound_id = %inbound_id, "agent-email: reply disabled by kill switch");
        set_reply_status(db, inbound_id, "disabled");
        return Ok(());
    }

    let mode = ReplyMode::parse(&channel.reply_mode).unwrap_or(ReplyMode::Approve);
    let domain = sender_domain(from);
    let allowlist = parse_allowlist(channel.reply_allowlist.as_deref());
    let domain_allowed = allowlist.iter().any(|d| d == &domain);
    let (sender_known, over_cap) = {
        let conn = db.connect().map_err(|e| e.to_string())?;
        (
            sender_has_thread(&conn, agent_id, &from.to_lowercase()),
            over_reply_caps(&conn, agent_id, from),
        )
    };
    if over_cap {
        info!(
            agent_id = %agent_id,
            from = %from,
            "agent-email: reply cap reached; reply falls back to approval"
        );
    }
    let mut plan = decide_reply_plan(mode, channel.domain_verified, sender_known, domain_allowed, over_cap);
    // An explicit autonomy level for this place overrides the reply mode: Send and tell me /
    // Act within limits answer directly (never past the reply caps or on an unverified domain).
    if let Some(e) = crate::autonomy::email_eval(db, &owner, agent_id, &reply_recipient(from, reply_to.as_deref())) {
        if !e.held() && !over_cap && channel.domain_verified {
            plan = ReplyPlan::Direct;
        }
    }
    info!(
        agent_id = %agent_id,
        from = %from,
        mode = mode.as_str(),
        plan = match plan { ReplyPlan::Direct => "direct", ReplyPlan::Approval => "approval" },
        "agent-email: sending bot reply"
    );

    // Threading: answer to the sender's Message-ID, carry their References
    // chain, and mark the message as an automatic reply so responders and
    // other bots' guards can tell.
    let mut headers = std::collections::HashMap::new();
    if let Some(message_id) = in_reply_to.as_deref() {
        headers.insert("In-Reply-To".to_string(), message_id.to_string());
    }
    if let Some(references) = build_references(email_references.as_deref(), in_reply_to.as_deref()) {
        headers.insert("References".to_string(), references);
    }
    headers.insert("Auto-Submitted".to_string(), "auto-replied".to_string());

    let body = build_reply_text(reply, &quote_excerpt(turn_body, 500));
    let req = SendAgentEmailRequest {
        agent_id: agent_id.to_string(),
        to: reply_recipient(from, reply_to.as_deref()),
        subject: reply_subject(subject),
        text: Some(body),
        html: None,
    };
    let result = send_email_inner(
        state,
        &owner,
        req,
        SendEmailExtra {
            reply_inbound_id: Some(inbound_id),
            headers: Some(&headers),
            skip_approval: plan == ReplyPlan::Direct,
            ..SendEmailExtra::default()
        },
    )
    .await
    .map_err(|(_, Json(message))| message.to_string())?;
    let status = result
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("sent")
        .to_string();
    set_reply_status(db, inbound_id, &status);
    Ok(())
}

// ============================================================================
// Decide wiring (Rails Mail review → mailflare approve/reject)
// ============================================================================

/// Outcome of applying a review decision to a pending outbound email.
pub enum EmailDecisionOutcome {
    /// The thread has no pending outbound email — ordinary review thread.
    NotEmailThread,
    /// The pending outbound was approved/rejected with mailflare.
    Applied,
    /// The thread maps to an outbound email but mailflare could not action it.
    Failed(String),
}

/// Hook called from the Rails Mail decide path after a `ReviewDecision` was
/// recorded. When the thread belongs to a pending outbound email, approve or
/// reject the mailflare job with the agent's key and update the record +
/// receipt. No-op for ordinary threads.
pub async fn decide_outbound_for_thread(
    state: &AppState,
    thread_id: &str,
    approved: bool,
) -> EmailDecisionOutcome {
    let row = {
        let conn = match state.db.connect() {
            Ok(conn) => conn,
            Err(e) => return EmailDecisionOutcome::Failed(e.to_string()),
        };
        conn.query_row(
            "SELECT id, agent_id, job_id, status FROM agent_email_outbound WHERE thread_id = ?1",
            params![thread_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
    };
    let (outbound_id, agent_id, job_id, status) = match row {
        Ok(Some(row)) => row,
        Ok(None) => return EmailDecisionOutcome::NotEmailThread,
        Err(e) => return EmailDecisionOutcome::Failed(e.to_string()),
    };
    if status != "pending_approval" {
        // Already actioned — treat as no-op so repeat decisions don't error.
        return EmailDecisionOutcome::Applied;
    }
    let Some(job_id) = job_id else {
        return EmailDecisionOutcome::Failed(
            "pending outbound email has no mailflare job id".to_string(),
        );
    };

    let channel = match state
        .db
        .connect()
        .and_then(|conn| lookup_email_channel(&conn, &agent_id))
    {
        Ok(Some(channel)) => channel,
        Ok(None) => {
            return EmailDecisionOutcome::Failed(
                "agent email channel missing".to_string(),
            );
        }
        Err(e) => return EmailDecisionOutcome::Failed(e.to_string()),
    };
    // Approve/reject use the bot's own key, so a cloud-brokered runtime can decide too.
    let client = match channel.client() {
        Some(client) => client,
        None => {
            return EmailDecisionOutcome::Failed("mailflare is not configured".to_string());
        }
    };
    let api_key = match open_channel_key(&channel) {
        Ok(key) => key,
        Err((_, Json(message))) => {
            return EmailDecisionOutcome::Failed(message.to_string());
        }
    };

    let decision = if approved { "accepted" } else { "rejected" };
    let applied = if approved {
        client.approve(&api_key, &job_id).await.map(|message_id| {
            (
                "sent",
                if message_id.is_empty() {
                    None
                } else {
                    Some(message_id)
                },
            )
        })
    } else {
        client.reject(&api_key, &job_id).await.map(|_| ("rejected", None))
    };

    match applied {
        Ok((new_status, provider_message_id)) => {
            if let Ok(conn) = state.db.connect() {
                if let Err(e) = conn.execute(
                    "UPDATE agent_email_outbound SET status = ?1, provider_message_id = COALESCE(?2, provider_message_id), error = NULL, updated_at = CURRENT_TIMESTAMP WHERE id = ?3",
                    params![new_status, provider_message_id, outbound_id],
                ) {
                    warn!(error = %e, "agent-email: failed to update outbound after decision");
                }
            }
            // Ledger receipt for the provider-side action (same pattern as the
            // mail_share receipt write in rails/mod.rs).
            let receipt = allternit_factory_engine::ReceiptRecord {
                receipt_id: allternit_factory_engine::core::ids::create_receipt_id(),
                run_id: format!("agent-email-outbound:{outbound_id}"),
                step: None,
                tool: "agent-email".to_string(),
                tool_version: None,
                inputs_ref: None,
                outputs_ref: provider_message_id.clone(),
                exit: Some(allternit_factory_engine::core::types::ReceiptExit {
                    code: Some(0),
                    summary: Some(format!("mailflare {decision}: job {job_id}")),
                }),
                input_tokens: None,
                output_tokens: None,
                total_tokens: None,
            };
            if let Err(e) = state.rails.receipts.write_receipt(&receipt) {
                warn!(error = %e, "agent-email: receipt write after decision failed");
            }
            info!(outbound_id = %outbound_id, decision = decision, "agent-email: outbound decision applied");
            EmailDecisionOutcome::Applied
        }
        Err(e) => {
            if let Ok(conn) = state.db.connect() {
                let _ = conn.execute(
                    "UPDATE agent_email_outbound SET error = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
                    params![e.to_string(), outbound_id],
                );
            }
            warn!(error = %e, outbound_id = %outbound_id, "agent-email: mailflare decision call failed");
            EmailDecisionOutcome::Failed(e.to_string())
        }
    }
}

// ============================================================================
// Inbound webhook
// ============================================================================

/// Verify mailflare's `X-Email-Platform-Signature`: lowercase hex HMAC-SHA256
/// of the raw request body with the shared webhook secret.
fn verify_mailflare_signature(
    secret: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<(), String> {
    let signature = headers
        .get("x-email-platform-signature")
        .and_then(|v| v.to_str().ok())
        .ok_or("missing x-email-platform-signature header")?;
    let provided = hex::decode(signature.trim()).map_err(|_| "signature is not hex")?;
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).map_err(|_| "invalid secret length")?;
    mac.update(body);
    mac.verify_slice(&provided)
        .map_err(|_| "x-email-platform-signature mismatch".to_string())
}

async fn receive_inbound_email(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    relayed: crate::relay_auth::RelayedAuth,
) -> Response {
    let body = relayed.body.clone();
    // Which mailbox the event names (not trusted until the signature checks out).
    let claimed: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let claimed_data = claimed.get("data").cloned().unwrap_or(Value::Null);
    let claimed_mailbox = claimed_data.get("mailboxId").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let claimed_address = claimed_data
        .get("mailbox")
        .and_then(|v| v.as_str())
        .or_else(|| claimed_data.get("to").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string();
    // A cloud-brokered mailbox has its own webhook secret; otherwise the runtime-wide one.
    let resolved = {
        let conn = match state.db.connect() {
            Ok(conn) => conn,
            Err(e) => return internal(e).into_response(),
        };
        conn.query_row(
            "SELECT agent_id, email_receive_enabled, email_webhook_secret_sealed FROM agent_identity_channels
             WHERE email_provider = 'mailflare' AND ((?1 <> '' AND email_mailbox_id = ?1) OR LOWER(email_address) = LOWER(?2))
             ORDER BY (email_mailbox_id = ?1) DESC LIMIT 1",
            params![claimed_mailbox, claimed_address],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i32>(1)? != 0, row.get::<_, Option<String>>(2)?)),
        )
        .optional()
    };
    let resolved = match resolved {
        Ok(r) => r,
        Err(e) => return internal(e).into_response(),
    };
    let channel_secret = resolved
        .as_ref()
        .and_then(|(_, _, sealed)| sealed.as_deref())
        .map(crate::token_crypto::open)
        .filter(|s| !s.is_empty());
    let env_secret = crate::mailflare_client::MailflareConfig::from_env().and_then(|c| c.webhook_secret);
    let Some(secret) = channel_secret.or(env_secret) else {
        warn!("agent-email webhook received but no signing secret is known for it; rejecting");
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": "webhook_secret_not_configured"})),
        )
            .into_response();
    };

    if let Err(e) = verify_mailflare_signature(&secret, &headers, &body) {
        warn!("agent-email webhook signature verification failed: {e}");
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "invalid_signature"})),
        )
            .into_response();
    }

    let payload: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            warn!("agent-email webhook JSON parse error: {e}");
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid_json"})),
            )
                .into_response();
        }
    };

    if payload.get("type").and_then(|v| v.as_str()) != Some("message.inbound") {
        // Unknown/other event type — acknowledge so mailflare stops retrying.
        return (StatusCode::ACCEPTED, Json(json!({"ignored": true}))).into_response();
    }
    let data = payload.get("data").cloned().unwrap_or(Value::Null);
    let to = data.get("to").and_then(|v| v.as_str()).unwrap_or("");
    let from = data.get("from").and_then(|v| v.as_str()).unwrap_or("");
    let subject = data.get("subject").and_then(|v| v.as_str());
    let snippet = data.get("snippet").and_then(|v| v.as_str());
    let text_body = data.get("textBody").and_then(|v| v.as_str());
    let provider_message_id = data.get("messageId").and_then(|v| v.as_str());
    let mail_headers = crate::agent_email_reply::InboundMailHeaders::from_data(&data);

    let (agent_id, receive_enabled) = match resolved {
        Some((agent, enabled, _)) => (agent, enabled),
        None => {
            // Unknown recipient — acknowledge (202) so mailflare does not retry.
            info!(to = %to, "agent-email: inbound for unknown recipient; acknowledged");
            return (StatusCode::ACCEPTED, Json(json!({"accepted": true, "delivered": false})))
                .into_response();
        }
    };
    if let Err((status, body)) = require_agent_owner_id(&state, &relayed.owner, &agent_id) {
        return (status, body).into_response();
    }
    if !receive_enabled {
        info!(agent_id = %agent_id, "agent-email: inbound for agent with receive disabled; acknowledged");
        return (StatusCode::ACCEPTED, Json(json!({"accepted": true, "delivered": false})))
            .into_response();
    }

    // Loop guards run before any turn or reply: auto-responses, list traffic,
    // daemon addresses, reference bombs, bot-to-bot mail, and DMARC failures
    // must never reach the model or bounce back out. The sender is checked
    // against every bot's address so two bots can't ping-pong.
    let sender_is_bot = {
        let conn = match state.db.connect() {
            Ok(conn) => conn,
            Err(e) => return internal(e).into_response(),
        };
        match conn.query_row(
            "SELECT count(*) FROM agent_identity_channels
             WHERE email_address IS NOT NULL AND LOWER(email_address) = LOWER(?1)",
            params![from],
            |row| row.get::<_, i64>(0),
        ) {
            Ok(count) => count > 0,
            Err(e) => return internal(e).into_response(),
        }
    };
    let guard = crate::agent_email_reply::guard_reason(from, &mail_headers, sender_is_bot);
    if let Some(reason) = guard {
        info!(
            agent_id = %agent_id,
            from = %from,
            guard = reason.as_str(),
            "agent-email: inbound skipped by loop guard"
        );
    }

    // A Factory approval answer from the owner (`approve <node> <code>`) is
    // consumed here; it never becomes a bot turn or an inbox thread.
    if guard.is_none()
        && crate::factory_approvals_channels::email_answer(&state, &relayed.owner, &agent_id, from, subject, text_body.or(snippet).unwrap_or(""), provider_message_id.unwrap_or("")).await
    {
        return (StatusCode::ACCEPTED, Json(json!({"accepted": true, "delivered": false, "factoryApproval": true}))).into_response();
    }

    // Persist the webhook payload (raw body and threading headers), then
    // bridge into Rails Mail so the external email appears as a typed message
    // in the agent's inbound email thread.
    let inbound_id = uuid::Uuid::new_v4().to_string();
    {
        let conn = match state.db.connect() {
            Ok(conn) => conn,
            Err(e) => return internal(e).into_response(),
        };
        if let Err(e) = conn.execute(
            "INSERT INTO agent_email_inbound
                 (id, agent_id, provider_message_id, from_address, to_address, subject, snippet, text_body, headers_json,
                  in_reply_to, email_references, reply_to, auth_results, guard_reason, reply_status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                inbound_id,
                agent_id,
                provider_message_id,
                from,
                to,
                subject,
                snippet,
                text_body,
                data.get("headers").map(|h| h.to_string()),
                mail_headers.message_id,
                mail_headers.references,
                mail_headers.reply_to,
                mail_headers.auth_results,
                guard.map(|g| g.as_str()),
                guard.map(|_| "skipped"),
            ],
        ) {
            return internal(e).into_response();
        }
    }

    // The email is also a thread for the bot (P6.2): one per conversation
    // (sender + subject without Re:/Fwd:), so a reply continues it. The bot
    // reads the de-quoted body; the reply (when enabled and no guard fired)
    // is emailed back to the sender after the turn.
    if guard.is_none() {
        let db = state.db.clone();
        let app = state.clone();
        let agent = agent_id.clone();
        let inbound = inbound_id.clone();
        let from_s = from.to_string();
        let subject_s = subject.unwrap_or("(no subject)").to_string();
        let body_s = text_body.or(snippet).unwrap_or("(no body)").to_string();
        tokio::spawn(async move {
            let stripped = crate::agent_email_reply::strip_quoted_history(&body_s);
            let turn_body = if stripped.is_empty() { body_s.clone() } else { stripped };
            let rt = crate::thread_routes::GizziRuntime { db: db.clone() };
            let key = format!("email:{}:{}", from_s.to_lowercase(), crate::thread_routes::conversation_subject(&subject_s));
            let text = format!("[email from {from_s}] Subject: {subject_s}\n\n{turn_body}");
            match crate::thread_routes::channel_thread(&db, &rt, &agent, "email", &key, &subject_s, &body_s).await {
                Ok(session) => {
                    match crate::agent_session_routes::send_bot_turn(&db, &session, &agent, &text).await {
                        Ok(reply) => {
                            if let Err(e) = send_reply_for_turn(
                                &app,
                                &db,
                                &agent,
                                &inbound,
                                &from_s,
                                &subject_s,
                                &turn_body,
                                &reply,
                            )
                            .await
                            {
                                warn!(error = %e, agent_id = %agent, "agent-email: reply send failed");
                                set_reply_status(&db, &inbound, "failed");
                            }
                        }
                        Err(e) => {
                            warn!(error = %e, agent_id = %agent, "agent-email: the email thread's turn failed");
                            set_reply_status(&db, &inbound, "turn_failed");
                        }
                    }
                }
                Err(e) => warn!(error = %e, agent_id = %agent, "agent-email: couldn't start the email thread"),
            }
        });
    }

    let thread_id = format!(
        "mail:email-in-{}",
        agent_id
            .to_lowercase()
            .replace(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_' && c != '.', "-")
    );
    if let Err(e) = state.rails.mail.ensure_thread(&thread_id).await {
        warn!(error = %e, thread_id = %thread_id, "agent-email: ensure_thread failed");
    }
    let body_text = text_body
        .or(snippet)
        .unwrap_or("(no body)")
        .to_string();
    match state
        .rails
        .mail
        .send_typed_message(
            &thread_id,
            TypedMessage {
                from_agent: from.to_string(),
                to_agents: vec![agent_id.clone()],
                subject: subject.map(str::to_string),
                importance: MailImportance::Normal,
                ack_required: false,
                body: body_text,
            },
        )
        .await
    {
        Ok(message_id) => {
            info!(agent_id = %agent_id, from = %from, "agent-email: inbound bridged to rails mail");
            (
                StatusCode::OK,
                Json(json!({
                    "accepted": true,
                    "delivered": true,
                    "thread": thread_id,
                    "messageId": message_id,
                })),
            )
                .into_response()
        }
        Err(e) => {
            // 5xx so mailflare retries the delivery.
            warn!(error = %e, agent_id = %agent_id, "agent-email: rails mail bridge failed");
            internal(e).into_response()
        }
    }
}

// ============================================================================
// Status
// ============================================================================

async fn agent_email_status(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
) -> Response {
    Json(agent_email_status_value().await).into_response()
}

/// Rail diagnostics shared by `GET /agent-email/status`, the `allternit-mail`
/// connector connect/status paths, and the MCP `allternit_mail.status` tool.
pub(crate) async fn agent_email_status_value() -> Value {
    let config = match crate::mailflare_client::MailflareConfig::from_env() {
        Some(config) => config,
        None if crate::mailflare_client::brokered_available() => {
            // Mailboxes are provisioned through Allternit's cloud for this runtime.
            return json!({
                "configured": true,
                "mode": "cloud",
                "domain": std::env::var("ALLTERNIT_BOT_EMAIL_DOMAIN").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| crate::mailflare_client::DEFAULT_BOT_EMAIL_DOMAIN.to_string()),
            });
        }
        None => {
            return json!({
                "configured": false,
            });
        }
    };
    let client = MailflareClient::new(config.clone());
    // Cheap reachability probe: GET /api/domains also validates the admin key.
    let reachable = client.list_domains().await.is_ok();
    json!({
        "configured": true,
        "domain": config.domain,
        "baseUrl": config.base_url,
        "webhookSecretSet": config.webhook_secret.is_some(),
        "reachable": reachable,
    })
}

// ============================================================================
// Internal MCP surface (`allternit_mail.*` tools)
// ============================================================================

/// Tool descriptors merged into `tools/list` on the internal connectors MCP
/// endpoint (`/internal/connectors/mcp`). Names use `allternit_mail.` (snake —
/// the sidecar's own tool names are snake_case too); the catalog entry
/// `allternit-mail` advertises the same names.
pub(crate) fn mail_mcp_tools() -> Value {
    json!([
        {
            "name": "allternit_mail.send",
            "title": "Send email",
            "description": "Send an outbound email from an agent's own Allternit Mail address. Approval-gated: the send is queued for human review before delivery; the response carries the review thread id.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_id": { "type": "string", "minLength": 1 },
                    "to": { "type": "string", "minLength": 1 },
                    "subject": { "type": "string", "minLength": 1 },
                    "text": { "type": "string" },
                    "html": { "type": "string" }
                },
                "required": ["agent_id", "to", "subject"],
                "additionalProperties": false
            }
        },
        {
            "name": "allternit_mail.status",
            "title": "Get mail status",
            "description": "Read the Allternit Mail rail status (configured/reachable) and, when agent_id is passed, the agent's provisioned email address.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_id": { "type": "string" }
                },
                "additionalProperties": false
            }
        }
    ])
}

/// Execute one `allternit_mail.*` MCP tool call. `user_id` is the identity the
/// internal MCP endpoint already authenticated (device-token owner or the
/// caller-asserted `x-allternit-user-id` under the internal service token) —
/// ownership of `agent_id` is enforced against it exactly like the REST route.
/// Returns the tool's JSON payload, or the same `(status, error-json)` pair
/// the REST surface would have produced.
pub(crate) async fn call_mail_mcp_tool(
    state: &Arc<AppState>,
    user_id: &str,
    name: &str,
    args: Value,
) -> Result<Value, ApiError> {
    match name {
        "allternit_mail.send" => {
            let req: SendAgentEmailRequest = serde_json::from_value(args).map_err(|e| {
                err(
                    StatusCode::BAD_REQUEST,
                    "invalid_request",
                    format!("invalid allternit_mail.send arguments: {e}"),
                )
            })?;
            send_email_for_user(state, user_id, req).await
        }
        "allternit_mail.status" => {
            let agent_id = args
                .get("agent_id")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let mut out = agent_email_status_value().await;
            if let Some(agent_id) = agent_id {
                require_agent_owner_id(state, user_id, &agent_id)?;
                let channel = {
                    let conn = state.db.connect().map_err(internal)?;
                    lookup_email_channel(&conn, &agent_id).map_err(internal)?
                };
                out.as_object_mut().map(|o| {
                    o.insert("agent_id".to_string(), json!(agent_id));
                    o.insert(
                        "channel".to_string(),
                        match channel {
                            Some(c) => json!({
                                "provisioned": true,
                                "address": c.address,
                                "sendEnabled": c.send_enabled,
                                "receiveEnabled": c.receive_enabled,
                            }),
                            None => json!({ "provisioned": false }),
                        },
                    );
                });
            }
            Ok(out)
        }
        other => Err(err(
            StatusCode::BAD_REQUEST,
            "unknown_tool",
            format!("unknown allternit_mail tool: {other}"),
        )),
    }
}

// ============================================================================
// Revocation (best-effort mailbox teardown when an agent is deleted)
// ============================================================================

/// Best-effort mailflare teardown for a deleted/disabled agent: delete the
/// mailbox (removes the Cloudflare routing rule and disables it). mailflare
/// has no admin-scope key-revoke endpoint yet, so the per-agent key is left
/// revoked-by-mailbox-deletion only. Never fails the caller; returns false
/// only when a mailbox existed and could not be removed.
pub async fn revoke_agent_mailbox(agent_id: &str, db: &crate::db::DbHandle) -> bool {
    let mailbox_id = match db.connect() {
        Ok(conn) => conn
            .query_row(
                "SELECT email_mailbox_id FROM agent_identity_channels
                 WHERE agent_id = ?1 AND email_provider = 'mailflare'",
                params![agent_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional(),
        Err(e) => {
            warn!(error = %e, agent_id = %agent_id, "agent-email: revocation lookup failed");
            return false;
        }
    };
    let Ok(Some(Some(mailbox_id))) = mailbox_id else {
        return true;
    };
    let Some(client) = MailflareClient::from_env() else {
        // Cloud-brokered: the cloud holds the admin key and removes the mailbox, its key,
        // its webhook and the relay route.
        if let Some(bearer) = crate::phone_sync::runtime_bearer() {
            let url = format!("{}/api/v1/runtime-devices/me/bot-email/mailboxes/{}", crate::phone_sync::cloud_base().trim_end_matches('/'), mailbox_id);
            match reqwest::Client::new().delete(url).bearer_auth(bearer).timeout(std::time::Duration::from_secs(20)).send().await {
                Ok(r) if r.status().is_success() || r.status() == StatusCode::NOT_FOUND => {
                    info!(agent_id = %agent_id, mailbox_id = %mailbox_id, "agent-email: mailbox removed through the cloud");
                    return true;
                }
                Ok(r) => warn!(status = %r.status(), agent_id = %agent_id, "agent-email: cloud mailbox removal refused"),
                Err(e) => warn!(error = %e, agent_id = %agent_id, "agent-email: cloud mailbox removal failed"),
            }
        }
        return false;
    };
    match client.delete_mailbox(&mailbox_id).await {
        Ok(()) => {
            info!(agent_id = %agent_id, mailbox_id = %mailbox_id, "agent-email: mailbox deleted");
            true
        }
        Err(e) => {
            warn!(error = %e, agent_id = %agent_id, mailbox_id = %mailbox_id, "agent-email: mailbox deletion failed");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// HMAC vector computed with `hmac.new(b"test-webhook-secret", body, sha256)`.
    #[test]
    fn mailflare_signature_verifies_and_rejects() {
        let body = br#"{"type":"message.inbound","data":{"to":"a@b.c"}}"#;
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-email-platform-signature",
            "94d5e916b54889570d66168f1e84ab9111d04d38b91b5c09272561acdc2dd9bb"
                .parse()
                .unwrap(),
        );
        assert!(verify_mailflare_signature("test-webhook-secret", &headers, body).is_ok());

        // Wrong secret, tampered body, missing header, non-hex all reject.
        assert!(verify_mailflare_signature("wrong-secret", &headers, body).is_err());
        assert!(verify_mailflare_signature("test-webhook-secret", &headers, b"{}").is_err());
        assert!(verify_mailflare_signature("test-webhook-secret", &HeaderMap::new(), body).is_err());
        let mut bad_headers = HeaderMap::new();
        bad_headers.insert("x-email-platform-signature", "zz".parse().unwrap());
        assert!(verify_mailflare_signature("test-webhook-secret", &bad_headers, body).is_err());
    }
}
