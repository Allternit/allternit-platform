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
    extract::{Extension, State},
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
use allternit_commrails::{MailImportance, TypedMessage};

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
}

/// Public webhook surface for mailflare inbound messages. Server-to-server —
/// no Clerk session exists, so requests are authenticated by the HMAC
/// signature instead (same shape as the Slack/Photon webhooks).
pub fn agent_email_webhook_router() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/agent-email/inbound", post(receive_inbound_email))
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
}

/// Look up the agent's mailflare email channel. Returns `None` when the agent
/// has no mailflare-backed email (including legacy `commrails` rows).
pub fn lookup_email_channel(
    conn: &rusqlite::Connection,
    agent_id: &str,
) -> rusqlite::Result<Option<AgentEmailChannel>> {
    conn.query_row(
        "SELECT email_address, email_send_enabled, email_receive_enabled,
                email_mailbox_id, email_api_key_sealed
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

/// Approval-gated outbound send, shared by the REST route and the internal MCP
/// `allternit_mail.send` tool. Enforces agent ownership by `user_id`, records
/// the outbound row, and returns the same JSON payload either way.
pub(crate) async fn send_email_for_user(
    state: &Arc<AppState>,
    user_id: &str,
    req: SendAgentEmailRequest,
) -> Result<Value, ApiError> {
    require_agent_owner_id(state, user_id, &req.agent_id)?;

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

    let client = MailflareClient::from_env().ok_or_else(|| {
        err(
            StatusCode::NOT_IMPLEMENTED,
            "mailflare_not_configured",
            "ALLTERNIT_MAILFLARE_URL/ALLTERNIT_MAILFLARE_ADMIN_KEY are not configured.",
        )
    })?;

    let channel = {
        let conn = state.db.connect().map_err(internal)?;
        lookup_email_channel(&conn, &req.agent_id)
            .map_err(internal)?
            .ok_or_else(|| {
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
    let api_key = open_channel_key(&channel)?;

    // Record first so the row exists even when the mailflare call fails.
    let outbound_id = uuid::Uuid::new_v4().to_string();
    let idempotency_key = format!("agent-email:{outbound_id}");
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
                 (id, agent_id, user_id, thread_id, idempotency_key, to_address, subject, snippet, status, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending_approval', CURRENT_TIMESTAMP)",
            params![
                outbound_id,
                req.agent_id,
                user_id,
                thread_id,
                idempotency_key,
                req.to,
                req.subject,
                snippet
            ],
        )
        .map_err(internal)?;
    }

    let send_result = client
        .send(
            &api_key,
            &SendEmailRequest {
                from: &channel.address,
                to: &req.to,
                subject: &req.subject,
                text: req.text.as_deref(),
                html: req.html.as_deref(),
                mailbox_id: &mailbox_id,
            },
            &idempotency_key,
        )
        .await;

    let send_response = match send_result {
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

    let client = match MailflareClient::from_env() {
        Some(client) => client,
        None => {
            return EmailDecisionOutcome::Failed("mailflare is not configured".to_string());
        }
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
            let receipt = allternit_commrails::ReceiptRecord {
                receipt_id: allternit_commrails::core::ids::create_receipt_id(),
                run_id: format!("agent-email-outbound:{outbound_id}"),
                step: None,
                tool: "agent-email".to_string(),
                tool_version: None,
                inputs_ref: None,
                outputs_ref: provider_message_id.clone(),
                exit: Some(allternit_commrails::core::types::ReceiptExit {
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
    body: axum::body::Bytes,
) -> Response {
    let config = match crate::mailflare_client::MailflareConfig::from_env() {
        Some(config) => config,
        None => {
            warn!("agent-email webhook received but mailflare is not configured; rejecting");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "mailflare_not_configured"})),
            )
                .into_response();
        }
    };
    let Some(secret) = config.webhook_secret.clone() else {
        warn!("agent-email webhook received but ALLTERNIT_MAILFLARE_WEBHOOK_SECRET is not set; rejecting");
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

    // Resolve the receiving agent by the `to` address.
    let resolved = {
        let conn = match state.db.connect() {
            Ok(conn) => conn,
            Err(e) => return internal(e).into_response(),
        };
        conn.query_row(
            "SELECT agent_id, email_receive_enabled FROM agent_identity_channels
             WHERE LOWER(email_address) = LOWER(?1)",
            params![to],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i32>(1)? != 0)),
        )
        .optional()
    };
    let (agent_id, receive_enabled) = match resolved {
        Ok(Some(row)) => row,
        Ok(None) => {
            // Unknown recipient — acknowledge (202) so mailflare does not retry.
            info!(to = %to, "agent-email: inbound for unknown recipient; acknowledged");
            return (StatusCode::ACCEPTED, Json(json!({"accepted": true, "delivered": false})))
                .into_response();
        }
        Err(e) => return internal(e).into_response(),
    };
    if !receive_enabled {
        info!(agent_id = %agent_id, "agent-email: inbound for agent with receive disabled; acknowledged");
        return (StatusCode::ACCEPTED, Json(json!({"accepted": true, "delivered": false})))
            .into_response();
    }

    // Persist the webhook payload, then bridge into Rails Mail so the external
    // email appears as a typed message in the agent's inbound email thread.
    let inbound_id = uuid::Uuid::new_v4().to_string();
    {
        let conn = match state.db.connect() {
            Ok(conn) => conn,
            Err(e) => return internal(e).into_response(),
        };
        if let Err(e) = conn.execute(
            "INSERT INTO agent_email_inbound
                 (id, agent_id, provider_message_id, from_address, to_address, subject, snippet, text_body, headers_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
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
            ],
        ) {
            return internal(e).into_response();
        }
    }

    // The email is also a thread for the bot (P6.2): one per conversation
    // (sender + subject without Re:/Fwd:), so a reply continues it.
    {
        let db = state.db.clone();
        let agent = agent_id.clone();
        let from_s = from.to_string();
        let subject_s = subject.unwrap_or("(no subject)").to_string();
        let body_s = text_body.or(snippet).unwrap_or("(no body)").to_string();
        tokio::spawn(async move {
            let rt = crate::thread_routes::GizziRuntime { db: db.clone() };
            let key = format!("email:{}:{}", from_s.to_lowercase(), crate::thread_routes::conversation_subject(&subject_s));
            let text = format!("[email from {from_s}] Subject: {subject_s}\n\n{body_s}");
            match crate::thread_routes::channel_thread(&db, &rt, &agent, "email", &key, &subject_s, &body_s).await {
                Ok(session) => {
                    if let Err(e) = crate::agent_session_routes::send_bot_turn(&db, &session, &agent, &text).await {
                        warn!(error = %e, agent_id = %agent, "agent-email: the email thread's turn failed");
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
/// revoked-by-mailbox-deletion only. Never fails the caller.
pub async fn revoke_agent_mailbox(agent_id: &str, db: &crate::db::DbHandle) {
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
            return;
        }
    };
    let Ok(Some(Some(mailbox_id))) = mailbox_id else {
        return;
    };
    let Some(client) = MailflareClient::from_env() else {
        return;
    };
    match client.delete_mailbox(&mailbox_id).await {
        Ok(()) => info!(agent_id = %agent_id, mailbox_id = %mailbox_id, "agent-email: mailbox deleted"),
        Err(e) => warn!(error = %e, agent_id = %agent_id, mailbox_id = %mailbox_id, "agent-email: mailbox deletion failed"),
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
