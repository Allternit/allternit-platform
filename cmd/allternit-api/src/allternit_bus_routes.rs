//! Production AllternitBus messaging and autonomous bot primitives.
//!
//! Mounted under `/api/v1` by main.rs. All state is persisted in SQLite and
//! secrets are encrypted at rest via `token_crypto`.

use axum::{
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::{info, warn};

use crate::auth::AuthUser;
use crate::AppState;

pub fn allternit_bus_router() -> Router<Arc<AppState>> {
    // NOTE (2026-09-14 de-duplication): this router used to also register
    // /agents/:agent_id/secrets/:key, /agents/:agent_id/secrets/resolve,
    // /agents/:agent_id/identity, /agents/:agent_id/identity/phone, and
    // /photon/sessions/:session_id/bridge. Those paths are still served, but
    // from photon_routes::photon_router (handlers byte-identical to the ones
    // that used to live here). Registering them in both routers made axum
    // panic with "Overlapping method route" at startup.
    Router::new()
        // AllternitBus inbox (route paths kept for backward compatibility)
        .route("/photon/agents/:agent_id/inbox", post(send_message))
        .route("/photon/agents/:agent_id/inbox", get(get_inbox))
        // Connectors (this file holds the maintained handler — the
        // connector resolution test mounts this router)
        .route(
            "/agents/:agent_id/connectors/resolve",
            post(resolve_agent_connectors),
        )
        // Identity channels (mailflare-aware provision_email lives here)
        .route("/agents/:agent_id/identity/email", post(provision_email))
}

/// Public webhook surface for inbound Photon.codes messages.
/// Mounted on the public router in main.rs because it is called server-to-server
/// by Photon and cannot carry a Clerk session.
pub fn allternit_bus_webhook_router() -> Router<Arc<AppState>> {
    Router::new().route("/webhooks/photon", post(receive_inbound_message))
}

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

/// Verify the agent exists and is owned by the requesting user.
fn require_agent_owner(
    state: &AppState,
    user: &AuthUser,
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
        Some(owner_id) if owner_id == user.user_id => Ok(()),
        Some(_) => Err(err(StatusCode::FORBIDDEN, "forbidden", "Agent does not belong to user")),
        None => Err(err(StatusCode::NOT_FOUND, "not_found", "Agent not found")),
    }
}

// ============================================================================
// AllternitBus inbox
// ============================================================================

#[derive(Debug, Deserialize)]
struct SendMessageRequest {
    from: String,
    content: String,
    surface: Option<String>,
}

#[derive(Debug, Serialize)]
struct AllternitBusMessage {
    id: String,
    from: String,
    to: String,
    content: String,
    surface: Option<String>,
    created_at: String,
}

#[derive(Debug, Deserialize)]
struct InboxQuery {
    since: Option<String>,
    limit: Option<usize>,
}

async fn send_message(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(agent_id): Path<String>,
    Json(req): Json<SendMessageRequest>,
) -> Result<Response, ApiError> {
    require_agent_owner(&state, &user, &agent_id)?;

    let msg_id = uuid::Uuid::new_v4().to_string();
    let created_at = chrono::Utc::now().to_rfc3339();

    tokio::task::spawn_blocking({
        let db = state.db.clone();
        let msg_id = msg_id.clone();
        let created_at = created_at.clone();
        let from = req.from.clone();
        let content = req.content.clone();
        let surface = req.surface.clone();
        let agent_id = agent_id.clone();
        move || {
            let conn = db.connect().map_err(internal)?;
            conn.execute(
                "INSERT INTO agent_photon_inbox (id, agent_id, from_id, to_id, content, surface, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    msg_id,
                    agent_id,
                    from,
                    agent_id,
                    content,
                    surface,
                    created_at
                ],
            )
            .map_err(internal)?;
            Ok::<_, ApiError>(())
        }
    })
    .await
    .map_err(|e| internal(e))??;

    info!(agent_id = %agent_id, msg_id = %msg_id, "AllternitBus message delivered");
    Ok((StatusCode::ACCEPTED, Json(json!({ "id": msg_id, "status": "delivered" }))).into_response())
}

async fn get_inbox(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(agent_id): Path<String>,
    Query(query): Query<InboxQuery>,
) -> Result<Response, ApiError> {
    require_agent_owner(&state, &user, &agent_id)?;

    let messages = tokio::task::spawn_blocking({
        let db = state.db.clone();
        let agent_id = agent_id.clone();
        let since = query.since.clone();
        let limit = query.limit.unwrap_or(100).min(500);
        move || {
            let conn = db.connect().map_err(internal)?;
            let mut stmt = conn.prepare(
                "SELECT id, from_id, to_id, content, surface, created_at
                 FROM agent_photon_inbox
                 WHERE agent_id = ?1
                   AND (?2 IS NULL OR created_at > ?2)
                 ORDER BY created_at DESC
                 LIMIT ?3",
            ).map_err(internal)?;
            let rows = stmt
                .query_map(params![agent_id, since, limit], |row| {
                    Ok(AllternitBusMessage {
                        id: row.get(0)?,
                        from: row.get(1)?,
                        to: row.get(2)?,
                        content: row.get(3)?,
                        surface: row.get(4)?,
                        created_at: row.get(5)?,
                    })
                })
                .map_err(internal)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(internal)?;
            Ok::<_, ApiError>(rows)
        }
    })
    .await
    .map_err(|e| internal(e))??;

    Ok(Json(json!({ "agent_id": agent_id, "messages": messages })).into_response())
}

// ============================================================================
// Inbound AllternitBus webhook
// ============================================================================

#[derive(Debug, Deserialize, Clone)]
struct AllternitBusWebhookPayload {
    from: String,
    to: String,
    body: String,
    channel: String,
    message_id: String,
}

async fn receive_inbound_message(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<AllternitBusWebhookPayload>,
) -> Result<Response, ApiError> {
    let to = payload.to.clone();
    let from = payload.from.clone();

    let routed = tokio::task::spawn_blocking({
        let db = state.db.clone();
        let payload = payload.clone();
        move || {
            let conn = db.connect().map_err(internal)?;
            let agent_id: Option<String> = conn
                .query_row(
                    "SELECT agent_id FROM agent_identity_channels WHERE phone_provider = 'photon' AND phone_number = ?1",
                    params![payload.to],
                    |row| row.get(0),
                )
                .optional()
                .map_err(internal)?;
            let agent_id = match agent_id {
                Some(id) => id,
                None => return Ok::<_, ApiError>(None),
            };
            conn.execute(
                "INSERT OR IGNORE INTO agent_photon_inbox (id, agent_id, from_id, to_id, content, surface, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    payload.message_id,
                    agent_id,
                    payload.from,
                    payload.to,
                    payload.body,
                    payload.channel,
                    chrono::Utc::now().to_rfc3339()
                ],
            )
            .map_err(internal)?;
            Ok::<_, ApiError>(Some(agent_id))
        }
    })
    .await
    .map_err(|e| internal(e))??;

    match routed {
        Some(agent_id) => {
            info!(agent_id = %agent_id, from = %from, "AllternitBus inbound webhook routed");
        }
        None => {
            warn!(to = %to, "AllternitBus inbound webhook received but no matching agent phone channel");
        }
    }

    Ok((StatusCode::ACCEPTED, Json(json!({ "status": "accepted" }))).into_response())
}

/// Dispatch an outbound SMS/message via Photon.codes Cloud Messaging REST API.
pub async fn send_photon_outbound_message(
    project_id: &str,
    project_secret: &str,
    to_phone: &str,
    body: &str,
) -> Result<Value, String> {
    let client = reqwest::Client::new();
    let url = "https://api.photon.codes/v1/messages";
    let resp = client
        .post(url)
        .header("Authorization", format!("Bearer {}", project_secret))
        .header("Content-Type", "application/json")
        .json(&json!({
            "projectId": project_id,
            "to": to_phone,
            "body": body,
        }))
        .send()
        .await
        .map_err(|e| format!("Photon API error: {}", e))?;

    if resp.status().is_success() {
        let val = resp.json::<Value>().await.unwrap_or_else(|_| json!({"status": "sent"}));
        Ok(val)
    } else {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        Err(format!("Photon API rejected message ({}): {}", status, text))
    }
}

// ============================================================================
// Connector credential resolution
// ============================================================================

#[derive(Debug, Deserialize, Clone)]
struct ConnectorBindingInput {
    connector_id: String,
    provider: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default)]
    autonomous: bool,
    #[serde(default)]
    allowed_actions: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct ResolveConnectorsRequest {
    bindings: Vec<ConnectorBindingInput>,
}

#[derive(Debug, Serialize)]
struct ResolvedConnectorCredential {
    connector_id: String,
    provider: String,
    key: String,
    value: String,
    source: &'static str,
}

/// Marker for a binding whose connection holds no unsealable token in
/// `connector_connections`: sidecar-backed connectors (runtime access goes
/// through the MCP proxy, `via: "mcp"`) and the `allternit-mail` connector
/// (per-agent mailbox, key sealed in `agent_identity_channels`,
/// `via: "agent_email"`). Additive alongside `credentials` — env-token
/// resolution for rust_native rows is unchanged.
#[derive(Debug, Serialize)]
struct ResolvedConnectorConnection {
    connector_id: String,
    backend: String,
    connected: bool,
    via: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    address: Option<String>,
}

#[derive(Debug, Serialize)]
struct ResolveConnectorsResponse {
    credentials: Vec<ResolvedConnectorCredential>,
    #[serde(default)]
    connections: Vec<ResolvedConnectorConnection>,
    missing: Vec<String>,
    errors: Vec<String>,
}

async fn resolve_agent_connectors(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(agent_id): Path<String>,
    Json(req): Json<ResolveConnectorsRequest>,
) -> Result<Response, ApiError> {
    require_agent_owner(&state, &user, &agent_id)?;

    let result = tokio::task::spawn_blocking({
        let db = state.db.clone();
        let agent_id = agent_id.clone();
        let user_id = user.user_id.clone();
        let bindings = req.bindings.clone();
        move || {
            let conn = db.connect().map_err(internal)?;
            let mut credentials = Vec::new();
            let mut connections = Vec::new();
            let mut missing = Vec::new();
            let mut errors = Vec::new();

            for b in bindings {
                let mut resolved = false;

                // 1. Lookup owned connector connection.
                let row: Option<(Option<String>, Option<String>, String)> = conn
                    .query_row(
                        "SELECT access_token, refresh_token, COALESCE(backend, 'rust_native') FROM connector_connections
                         WHERE connector_id = ?1 AND user_id = ?2 AND status = 'connected'
                         ORDER BY updated_at DESC LIMIT 1",
                        params![b.connector_id, user_id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .optional()
                    .map_err(internal)?;

                match row {
                    // Sidecar-backed index row: tokens are NULL by design (the
                    // sidecar is the vault). Resolve to an explicit MCP marker
                    // instead of silently falling through to `missing`.
                    Some((_, _, ref backend)) if backend == "open_connector" => {
                        connections.push(ResolvedConnectorConnection {
                            connector_id: b.connector_id.clone(),
                            backend: backend.clone(),
                            connected: true,
                            via: "mcp",
                            address: None,
                        });
                        resolved = true;
                    }
                    // Allternit Mail: the secret is the per-agent mailflare key
                    // sealed in agent_identity_channels, never a user token.
                    Some((_, _, ref backend)) if backend == "allternit_native" => {
                        let address: Option<String> = conn
                            .query_row(
                                "SELECT email_address FROM agent_identity_channels
                                 WHERE agent_id = ?1 AND email_provider = 'mailflare'",
                                params![agent_id],
                                |row| row.get(0),
                            )
                            .optional()
                            .map_err(internal)?;
                        connections.push(ResolvedConnectorConnection {
                            connector_id: b.connector_id.clone(),
                            backend: backend.clone(),
                            connected: true,
                            via: "agent_email",
                            address,
                        });
                        resolved = true;
                    }
                    Some((access_token, refresh_token, _)) => {
                        if let Some(token) = access_token {
                            let plain = crate::token_crypto::open(&token);
                            if !plain.is_empty() {
                                credentials.push(ResolvedConnectorCredential {
                                    connector_id: b.connector_id.clone(),
                                    provider: b.provider.clone(),
                                    key: format!("{}_ACCESS_TOKEN", env_key(&b.provider)),
                                    value: plain,
                                    source: "connector_connections",
                                });
                                resolved = true;
                            }
                        }
                        if let Some(token) = refresh_token {
                            let plain = crate::token_crypto::open(&token);
                            if !plain.is_empty() {
                                credentials.push(ResolvedConnectorCredential {
                                    connector_id: b.connector_id.clone(),
                                    provider: b.provider.clone(),
                                    key: format!("{}_REFRESH_TOKEN", env_key(&b.provider)),
                                    value: plain,
                                    source: "connector_connections",
                                });
                            }
                        }
                    }
                    None => {}
                }

                // 2. Fallback to legacy allternit_vault_credentials.
                if !resolved {
                    let sealed: Option<String> = conn
                        .query_row(
                            "SELECT encrypted_value FROM allternit_vault_credentials
                             WHERE user_id = ?1 AND provider = ?2 AND agent_id = ?3
                               AND revoked_at IS NULL
                               AND (expires_at IS NULL OR expires_at > CURRENT_TIMESTAMP)
                             ORDER BY updated_at DESC LIMIT 1",
                            params![user_id, b.provider, agent_id],
                            |row| row.get(0),
                        )
                        .optional()
                        .map_err(internal)?;

                    if let Some(value) = sealed {
                        let plain = crate::token_crypto::open(&value);
                        if !plain.is_empty() {
                            credentials.push(ResolvedConnectorCredential {
                                connector_id: b.connector_id.clone(),
                                provider: b.provider.clone(),
                                key: format!("{}_TOKEN", env_key(&b.provider)),
                                value: plain,
                                source: "allternit_vault",
                            });
                            resolved = true;
                        }
                    }
                }

                if !resolved {
                    missing.push(format!("{} ({})", b.label, b.provider));
                }
            }

            Ok::<_, ApiError>(ResolveConnectorsResponse {
                credentials,
                connections,
                missing,
                errors,
            })
        }
    })
    .await
    .map_err(|e| internal(e))??;

    Ok(Json(result).into_response())
}

fn env_key(provider: &str) -> String {
    provider.to_ascii_uppercase().replace('-', "_")
}

// ============================================================================
// Identity channels
// ============================================================================

#[derive(Debug, Serialize)]
struct ProvisionEmailResponse {
    address: String,
    provider: &'static str,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProvisionEmailBody {
    /// The part before the `@`. Defaults to a slug of the bot's name.
    local_part: Option<String>,
}

async fn provision_email(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(agent_id): Path<String>,
    body: Option<Json<ProvisionEmailBody>>,
) -> Result<Response, ApiError> {
    require_agent_owner(&state, &user, &agent_id)?;
    let requested = body.and_then(|Json(b)| b.local_part);

    // When mailflare is configured, provision a real mailbox + scoped API key.
    // Otherwise fall back to the legacy mint-only behavior below.
    if let Some(client) = crate::mailflare_client::MailflareClient::from_env() {
        let address =
            provision_email_mailflare(&state, &user.user_id, &agent_id, client, requested.as_deref())
                .await?;
        return Ok(Json(ProvisionEmailResponse { address, provider: "mailflare" }).into_response());
    }
    // No local admin key: Allternit's cloud provisions the mailbox for this runtime.
    if crate::mailflare_client::brokered_available() {
        let address = provision_email_brokered(&state, &user.user_id, &agent_id, requested.as_deref()).await?;
        return Ok(Json(ProvisionEmailResponse { address, provider: "mailflare" }).into_response());
    }

    let domain = std::env::var("ALLTERNIT_BOT_EMAIL_DOMAIN")
        .ok()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            err(
                StatusCode::NOT_IMPLEMENTED,
                "email_domain_not_configured",
                "ALLTERNIT_BOT_EMAIL_DOMAIN is not configured.",
            )
        })?;

    let (bot_name, existing_address) = {
        let conn = state.db.connect().map_err(internal)?;
        (
            agent_display_name(&conn, &agent_id),
            crate::agent_email_routes::lookup_email_channel(&conn, &agent_id)
                .map_err(internal)?
                .map(|c| c.address),
        )
    };
    // The address is stable once created: re-provisioning keeps it.
    let address = match existing_address {
        Some(address) => address,
        None => {
            let mut chosen = None;
            for local in local_part_candidates(requested.as_deref(), &bot_name, &agent_id)? {
                let candidate = format!("{local}@{domain}");
                if !email_address_taken(&state, &candidate, &agent_id)? {
                    chosen = Some(candidate);
                    break;
                }
            }
            chosen.ok_or_else(|| {
                err(
                    StatusCode::CONFLICT,
                    "email_local_part_taken",
                    "That address is already taken.",
                )
            })?
        }
    };

    tokio::task::spawn_blocking({
        let db = state.db.clone();
        let agent_id = agent_id.clone();
        let user_id = user.user_id.clone();
        let address = address.clone();
        move || {
            let conn = db.connect().map_err(internal)?;
            let id = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO agent_identity_channels (id, agent_id, user_id, email_address, email_provider, email_send_enabled, email_receive_enabled, updated_at)
                 VALUES (?1, ?2, ?3, ?4, 'commrails', 1, 1, CURRENT_TIMESTAMP)
                 ON CONFLICT(agent_id) DO UPDATE SET
                     email_address = excluded.email_address,
                     email_provider = excluded.email_provider,
                     email_send_enabled = excluded.email_send_enabled,
                     email_receive_enabled = excluded.email_receive_enabled,
                     updated_at = CURRENT_TIMESTAMP",
                params![id, agent_id, user_id, address],
            )
            .map_err(internal)?;
            Ok::<_, ApiError>(())
        }
    })
    .await
    .map_err(|e| internal(e))??;

    Ok(Json(ProvisionEmailResponse { address, provider: "commrails" }).into_response())
}

/// Provision a real mailflare mailbox for the agent: resolve the domain id,
/// create the mailbox (+ Cloudflare routing rule), mint a mailbox-scoped
/// send+read API key, seal it, and persist the channel row. On any failure no
/// half-written channel row is left behind; a mailbox we created is deleted
/// again best-effort. Returns the provisioned address. Shared by the
/// `POST /agents/:id/identity/email` route and the `allternit-mail` connector
/// connect path.
/// Provision the bot's mailbox through cloud-api (`POST /api/v1/runtime-devices/me/bot-email/mailboxes`,
/// this runtime's device credential). The cloud creates the mailbox, a key scoped to it and a webhook
/// that delivers only its mail to this runtime; the key and the webhook secret are sealed here.
pub(crate) async fn provision_email_brokered(
    state: &Arc<AppState>,
    user_id: &str,
    agent_id: &str,
    requested_local_part: Option<&str>,
) -> Result<String, ApiError> {
    let existing = {
        let conn = state.db.connect().map_err(internal)?;
        crate::agent_email_routes::lookup_email_channel(&conn, agent_id).map_err(internal)?
    };
    if let Some(channel) = existing {
        if channel.mailbox_id.is_some() && channel.api_key_sealed.is_some() {
            return Ok(channel.address);
        }
    }
    let bot_name = {
        let conn = state.db.connect().map_err(internal)?;
        agent_display_name(&conn, agent_id)
    };
    let local = local_part_candidates(requested_local_part, &bot_name, agent_id)?.into_iter().next().unwrap_or_else(|| sanitize_local_part(agent_id));
    let bearer = crate::phone_sync::runtime_bearer()
        .ok_or_else(|| err(StatusCode::CONFLICT, "runtime_not_paired", "Sign this computer in to your Allternit account to give bots email."))?;
    let resp = reqwest::Client::new()
        .post(format!("{}/api/v1/runtime-devices/me/bot-email/mailboxes", crate::phone_sync::cloud_base().trim_end_matches('/')))
        .bearer_auth(bearer)
        .timeout(std::time::Duration::from_secs(30))
        .json(&serde_json::json!({ "agentId": agent_id, "localPart": local, "displayName": bot_name }))
        .send()
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, "cloud_unreachable", e.to_string()))?;
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
    if status == StatusCode::SERVICE_UNAVAILABLE {
        return Err(err(StatusCode::NOT_IMPLEMENTED, "email_domain_not_configured", "Bot email isn't available on Allternit's cloud yet."));
    }
    if !status.is_success() {
        let msg = body.get("message").or_else(|| body.get("error")).and_then(|v| v.as_str()).unwrap_or("the cloud refused to create the mailbox").to_string();
        return Err(err(StatusCode::BAD_GATEWAY, "mailbox_not_created", msg));
    }
    let s = |k: &str| body.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let (address, mailbox_id, api_key, webhook_secret, mail_url) = (s("address"), s("mailboxId"), s("apiKey"), s("webhookSecret"), s("mailUrl"));
    if address.is_empty() || mailbox_id.is_empty() || api_key.is_empty() || mail_url.is_empty() {
        return Err(err(StatusCode::BAD_GATEWAY, "mailbox_not_created", "the cloud's answer was incomplete"));
    }
    let sealed_key = crate::token_crypto::seal(&api_key);
    let sealed_secret = (!webhook_secret.is_empty()).then(|| crate::token_crypto::seal(&webhook_secret));
    let conn = state.db.connect().map_err(internal)?;
    let id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO agent_identity_channels (id, agent_id, user_id, email_address, email_provider, email_send_enabled, email_receive_enabled, email_mailbox_id, email_api_key_sealed, email_webhook_secret_sealed, email_mail_url, updated_at)
         VALUES (?1, ?2, ?3, ?4, 'mailflare', 1, 1, ?5, ?6, ?7, ?8, CURRENT_TIMESTAMP)
         ON CONFLICT(agent_id) DO UPDATE SET
             email_address = excluded.email_address,
             email_provider = excluded.email_provider,
             email_send_enabled = excluded.email_send_enabled,
             email_receive_enabled = excluded.email_receive_enabled,
             email_mailbox_id = excluded.email_mailbox_id,
             email_api_key_sealed = excluded.email_api_key_sealed,
             email_webhook_secret_sealed = excluded.email_webhook_secret_sealed,
             email_mail_url = excluded.email_mail_url,
             updated_at = CURRENT_TIMESTAMP",
        params![id, agent_id, user_id, address, mailbox_id, sealed_key, sealed_secret, mail_url],
    )
    .map_err(internal)?;
    info!(agent_id = %agent_id, address = %address, "agent-email: mailbox provisioned through the cloud");
    Ok(address)
}

pub(crate) async fn provision_email_mailflare(
    state: &Arc<AppState>,
    user_id: &str,
    agent_id: &str,
    client: crate::mailflare_client::MailflareClient,
    requested_local_part: Option<&str>,
) -> Result<String, ApiError> {
    // Idempotent re-provision: an existing, fully-configured mailflare channel
    // is returned as-is.
    let existing = {
        let conn = state.db.connect().map_err(internal)?;
        crate::agent_email_routes::lookup_email_channel(&conn, agent_id).map_err(internal)?
    };
    if let Some(channel) = existing {
        if channel.mailbox_id.is_some() && channel.api_key_sealed.is_some() {
            return Ok(channel.address);
        }
    }

    let domain_id = client.resolve_domain_id().await.map_err(|e| {
        err(
            StatusCode::BAD_GATEWAY,
            "mailflare_domain_unresolved",
            e.to_string(),
        )
    })?;

    let bot_name = {
        let conn = state.db.connect().map_err(internal)?;
        agent_display_name(&conn, agent_id)
    };
    let mut found = None;
    for local_part in local_part_candidates(requested_local_part, &bot_name, agent_id)? {
        let address = format!("{}@{}", local_part, client.config().domain);
        if email_address_taken(state, &address, agent_id)? {
            continue;
        }
        match client
            .create_mailbox(&domain_id, &local_part, Some(&bot_name))
            .await
        {
            Ok(mailbox) => {
                found = Some((mailbox.id, mailbox.address, true));
                break;
            }
            Err(e) if e.status == Some(StatusCode::CONFLICT) => {
                // Only the legacy agent-id address can be an orphan of this
                // agent (a previous provisioning whose channel row was lost) —
                // adopt it. Any other taken name belongs to someone else: move
                // on to the next candidate.
                if local_part != sanitize_local_part(agent_id) {
                    continue;
                }
                let mailboxes = client.list_mailboxes().await.map_err(|e| {
                    err(StatusCode::BAD_GATEWAY, "mailflare_error", e.to_string())
                })?;
                found = mailboxes
                    .into_iter()
                    .find(|m| m.address().eq_ignore_ascii_case(&address))
                    .map(|m| (m.id.clone(), m.address(), false));
                if found.is_none() {
                    return Err(err(
                        StatusCode::CONFLICT,
                        "mailflare_mailbox_conflict",
                        format!("Mailbox {address} already exists but could not be resolved."),
                    ));
                }
                break;
            }
            Err(e) => return Err(err(StatusCode::BAD_GATEWAY, "mailflare_error", e.to_string())),
        }
    }
    let (mailbox_id, address, created_here) = found.ok_or_else(|| {
        err(
            StatusCode::CONFLICT,
            "email_local_part_taken",
            "That address is already taken.",
        )
    })?;

    // Mint the per-agent mailbox-scoped key; on failure roll back the mailbox
    // we just created (the key itself cannot be revoked via the admin key, so
    // a later DB failure leaves only an orphaned, mailbox-scoped key — noted
    // in the audit log line below).
    let key = match client
        .create_scoped_key(
            &format!("agent:{agent_id}"),
            &["send", "read"],
            std::slice::from_ref(&mailbox_id),
        )
        .await
    {
        Ok(key) => key,
        Err(e) => {
            if created_here {
                if let Err(del) = client.delete_mailbox(&mailbox_id).await {
                    warn!(error = %del, mailbox_id = %mailbox_id, "agent-email: rollback mailbox deletion failed");
                }
            }
            return Err(err(
                StatusCode::BAD_GATEWAY,
                "mailflare_key_creation_failed",
                e.to_string(),
            ));
        }
    };
    let sealed_key = crate::token_crypto::seal(&key.key);

    let persisted = tokio::task::spawn_blocking({
        let db = state.db.clone();
        let agent_id = agent_id.to_string();
        let user_id = user_id.to_string();
        let address = address.clone();
        let mailbox_id = mailbox_id.clone();
        move || {
            let conn = db.connect().map_err(internal)?;
            let id = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO agent_identity_channels (id, agent_id, user_id, email_address, email_provider, email_send_enabled, email_receive_enabled, email_mailbox_id, email_api_key_sealed, updated_at)
                 VALUES (?1, ?2, ?3, ?4, 'mailflare', 1, 1, ?5, ?6, CURRENT_TIMESTAMP)
                 ON CONFLICT(agent_id) DO UPDATE SET
                     email_address = excluded.email_address,
                     email_provider = excluded.email_provider,
                     email_send_enabled = excluded.email_send_enabled,
                     email_receive_enabled = excluded.email_receive_enabled,
                     email_mailbox_id = excluded.email_mailbox_id,
                     email_api_key_sealed = excluded.email_api_key_sealed,
                     updated_at = CURRENT_TIMESTAMP",
                params![id, agent_id, user_id, address, mailbox_id, sealed_key],
            )
            .map_err(internal)?;
            Ok::<_, ApiError>(())
        }
    })
    .await
    .map_err(|e| internal(e))?;

    if let Err(e) = persisted {
        if created_here {
            if let Err(del) = client.delete_mailbox(&mailbox_id).await {
                warn!(error = %del, mailbox_id = %mailbox_id, "agent-email: rollback mailbox deletion failed");
            }
        }
        return Err(e);
    }

    info!(agent_id = %agent_id, address = %address, "agent-email: mailflare mailbox provisioned");
    Ok(address)
}

/// The bot's name, or the id when it has none.
fn agent_display_name(conn: &rusqlite::Connection, agent_id: &str) -> String {
    conn.query_row("SELECT name FROM agents WHERE id = ?1", params![agent_id], |row| {
        row.get::<_, String>(0)
    })
    .ok()
    .map(|n| n.trim().to_string())
    .filter(|n| !n.is_empty())
    .unwrap_or_else(|| agent_id.to_string())
}

const LOCAL_PART_MIN: usize = 3;
const LOCAL_PART_MAX: usize = 40;
const MAX_LOCAL_PART_ATTEMPTS: usize = 50;
const RESERVED_LOCAL_PARTS: [&str; 10] = [
    "abuse", "admin", "administrator", "hostmaster", "noreply", "no-reply", "postmaster",
    "root", "support", "webmaster",
];

/// A readable local part from a bot's name: lowercase a-z, 0-9 and single
/// hyphens, 3-40 chars.
pub(crate) fn slugify_local_part(name: &str) -> String {
    let mut slug = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let mut slug = slug.trim_matches('-').to_string();
    slug.truncate(LOCAL_PART_MAX);
    let mut slug = slug.trim_end_matches('-').to_string();
    if slug.is_empty() {
        slug = "bot".to_string();
    }
    while slug.len() < LOCAL_PART_MIN {
        slug.push('x');
    }
    slug
}

/// Validate a caller-chosen local part: lowercase a-z, 0-9 and hyphens, 3-40
/// chars, no leading/trailing hyphen, not a role address.
pub(crate) fn validate_local_part(local: &str) -> Result<(), ApiError> {
    let ok = (LOCAL_PART_MIN..=LOCAL_PART_MAX).contains(&local.len())
        && local
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !local.starts_with('-')
        && !local.ends_with('-')
        && !local.contains("--");
    if !ok {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "invalid_local_part",
            "Use 3-40 lowercase letters, numbers and single hyphens.",
        ));
    }
    if RESERVED_LOCAL_PARTS.contains(&local) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "reserved_local_part",
            "That address is reserved.",
        ));
    }
    Ok(())
}

/// Local parts to try, in order. A requested one is tried alone; otherwise the
/// bot-name slug, then `-2`, `-3`, ... on collision.
pub(crate) fn local_part_candidates(
    requested: Option<&str>,
    bot_name: &str,
    agent_id: &str,
) -> Result<Vec<String>, ApiError> {
    if let Some(local) = requested.map(str::trim).filter(|l| !l.is_empty()) {
        let local = local.to_lowercase();
        validate_local_part(&local)?;
        return Ok(vec![local]);
    }
    let base = slugify_local_part(bot_name);
    let mut out = vec![base.clone()];
    for n in 2..(MAX_LOCAL_PART_ATTEMPTS + 2) {
        let suffix = format!("-{n}");
        let mut stem = base.clone();
        stem.truncate(LOCAL_PART_MAX - suffix.len());
        out.push(format!("{}{suffix}", stem.trim_end_matches('-')));
    }
    // A name that cannot be told apart from reserved words falls back to the id.
    if RESERVED_LOCAL_PARTS.contains(&base.as_str()) {
        out = vec![sanitize_local_part(agent_id)];
    }
    Ok(out)
}

/// Whether another agent already holds this address.
fn email_address_taken(
    state: &Arc<AppState>,
    address: &str,
    agent_id: &str,
) -> Result<bool, ApiError> {
    let conn = state.db.connect().map_err(internal)?;
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM agent_identity_channels WHERE lower(email_address) = lower(?1) AND agent_id != ?2",
            params![address, agent_id],
            |row| row.get(0),
        )
        .map_err(internal)?;
    Ok(n > 0)
}

fn sanitize_local_part(value: &str) -> String {
    value
        .to_lowercase()
        .replace(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_' && c != '.', "-")
        .replace("..", ".")
}

#[cfg(test)]
mod email_local_part_tests {
    use super::*;
    use crate::mailflare_client::{MailflareClient, MailflareConfig};
    use axum::routing::{get, post};
    use std::sync::Mutex;

    #[test]
    fn slug_is_lowercase_hyphenated_and_bounded() {
        assert_eq!(slugify_local_part("Ledger Bot"), "ledger-bot");
        assert_eq!(slugify_local_part("  Dana's  Helper!! "), "dana-s-helper");
        assert_eq!(slugify_local_part("Ünï"), "nxx");
        assert_eq!(slugify_local_part("é"), "bot");
        assert_eq!(slugify_local_part("ab"), "abx");
        let long = slugify_local_part(&"a".repeat(80));
        assert_eq!(long.len(), 40);
        let hyphen_edge = slugify_local_part(&format!("{} b", "a".repeat(39)));
        assert!(!hyphen_edge.ends_with('-') && hyphen_edge.len() <= 40);
        for s in ["Ledger Bot", "x", "9 lives", "***"] {
            assert!(validate_local_part(&slugify_local_part(s)).is_ok(), "{s}");
        }
    }

    #[test]
    fn candidates_add_numeric_suffixes_within_the_limit() {
        let c = local_part_candidates(None, "Ledger", "agent-1").unwrap();
        assert_eq!(&c[..3], ["ledger", "ledger-2", "ledger-3"]);
        let long = local_part_candidates(None, &"a".repeat(60), "agent-1").unwrap();
        assert!(long.iter().all(|l| l.len() <= 40 && validate_local_part(l).is_ok()));
        assert_eq!(local_part_candidates(Some(" Sales-Bot "), "x", "a").unwrap(), ["sales-bot"]);
        assert_eq!(local_part_candidates(Some("   "), "Ledger", "a").unwrap()[0], "ledger");
        assert_eq!(local_part_candidates(None, "Admin", "agent-1").unwrap(), ["agent-1"]);
    }

    #[test]
    fn invalid_local_parts_are_rejected() {
        for bad in ["ab", "-abc", "abc-", "a--b", "has space", "dot.ted", "under_score", &"a".repeat(41)] {
            assert_eq!(validate_local_part(bad).unwrap_err().0, StatusCode::BAD_REQUEST, "{bad}");
        }
        assert_eq!(validate_local_part("postmaster").unwrap_err().1 .0["error"], "reserved_local_part");
    }

    #[derive(Default)]
    struct Fake {
        /// Local parts mailflare already has (answers 409).
        taken: Mutex<Vec<String>>,
        created: Mutex<Vec<Value>>,
    }

    async fn fake_mailflare(fake: Arc<Fake>) -> String {
        let f = fake.clone();
        let app = axum::Router::new()
            .route(
                "/api/domains",
                get(|| async { Json(json!({"domains": [{"id": "dom-bus", "hostname": "bus.test"}]})) }),
            )
            .route(
                "/api/mailboxes",
                post(move |Json(body): Json<Value>| {
                    let f = f.clone();
                    async move {
                        let local = body["localPart"].as_str().unwrap().to_string();
                        if f.taken.lock().unwrap().contains(&local) {
                            return (StatusCode::CONFLICT, Json(json!({"error": "exists"})));
                        }
                        f.created.lock().unwrap().push(body);
                        (StatusCode::OK, Json(json!({"id": format!("mb-{local}"), "address": format!("{local}@bus.test")})))
                    }
                }),
            )
            .route(
                "/api/api-keys",
                post(|| async { Json(json!({"id": "k1", "key": "ep_key"})) }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        url
    }

    fn client(url: String) -> MailflareClient {
        MailflareClient::new(MailflareConfig {
            base_url: url,
            admin_key: "ep_admin".into(),
            domain: "bus.test".into(),
            webhook_secret: None,
            brokered: false,
        })
    }

    fn add_agent(state: &Arc<AppState>, id: &str, name: &str) {
        state
            .db
            .connect()
            .unwrap()
            .execute(
                "INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES (?1, 'u1', ?2, 'm', 'p', 1, '{}')",
                params![id, name],
            )
            .unwrap();
    }

    #[tokio::test]
    async fn provisioning_uses_the_bot_name_and_stays_stable() {
        let temp = tempfile::tempdir().unwrap();
        let state = crate::test_helpers::app_state(temp.path()).await;
        let fake = Arc::new(Fake::default());
        let url = fake_mailflare(fake.clone()).await;
        add_agent(&state, "agent-a", "Ledger Bot");
        add_agent(&state, "agent-b", "Ledger Bot");
        add_agent(&state, "agent-c", "Casey");

        let a = provision_email_mailflare(&state, "u1", "agent-a", client(url.clone()), None).await.unwrap();
        assert_eq!(a, "ledger-bot@bus.test");
        assert_eq!(fake.created.lock().unwrap()[0]["displayName"], "Ledger Bot");

        // A second bot with the same name gets -2; the first keeps its address.
        let b = provision_email_mailflare(&state, "u1", "agent-b", client(url.clone()), None).await.unwrap();
        assert_eq!(b, "ledger-bot-2@bus.test");
        let a_again = provision_email_mailflare(&state, "u1", "agent-a", client(url.clone()), Some("other-name")).await.unwrap();
        assert_eq!(a_again, a, "address is stable once created");

        // A name mailflare already has (not ours) is skipped, not adopted.
        fake.taken.lock().unwrap().push("casey".into());
        let c = provision_email_mailflare(&state, "u1", "agent-c", client(url.clone()), None).await.unwrap();
        assert_eq!(c, "casey-2@bus.test");
    }

    #[tokio::test]
    async fn requested_local_part_is_honored_or_refused() {
        let temp = tempfile::tempdir().unwrap();
        let state = crate::test_helpers::app_state(temp.path()).await;
        let fake = Arc::new(Fake::default());
        let url = fake_mailflare(fake.clone()).await;
        add_agent(&state, "agent-a", "Ledger");
        add_agent(&state, "agent-b", "Other");

        let bad = provision_email_mailflare(&state, "u1", "agent-a", client(url.clone()), Some("A B")).await.unwrap_err();
        assert_eq!(bad.0, StatusCode::BAD_REQUEST);
        let a = provision_email_mailflare(&state, "u1", "agent-a", client(url.clone()), Some("Billing-Desk")).await.unwrap();
        assert_eq!(a, "billing-desk@bus.test");
        // Another bot cannot take it: a requested name never gets a suffix.
        let taken = provision_email_mailflare(&state, "u1", "agent-b", client(url.clone()), Some("billing-desk")).await.unwrap_err();
        assert_eq!((taken.0, taken.1 .0["error"].as_str()), (StatusCode::CONFLICT, Some("email_local_part_taken")));
        assert!(state.db.connect().unwrap()
            .query_row("SELECT COUNT(*) FROM agent_identity_channels WHERE agent_id = 'agent-b'", [], |r| r.get::<_, i64>(0)).unwrap() == 0);
    }
}
