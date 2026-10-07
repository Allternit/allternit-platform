//! Channels for an end customer's agent (spec §4 Channels). Scope `channels`.
//!
//! * `POST   /v1/accounts/{id}/channels/{kind}/connect` `{agent_id, return_url?, local_part?}`
//! * `GET    /v1/accounts/{id}/channels` (`?agent_id=` filter)
//! * `GET    /v1/accounts/{id}/channels/{channel_id}`
//! * `DELETE /v1/accounts/{id}/channels/{channel_id}`
//!
//! Connection records live in `platform_channel_connections` (migration 075). That table is
//! the one to read for an agent's live channels: `status = 'connected' AND
//! deleted_at IS NULL`, `kind`, `external_id` (Slack team id, email address).
//!
//! Kinds and how they connect:
//! * `email`: the agent gets its own address on Allternit Mail right away (the
//!   runtime provisions it through cloud-api's bot-email broker). Answers
//!   `status: connected` with the address.
//! * `slack`: answers `status: pending` and a `connect_url` (Slack's OAuth
//!   consent page for Allternit's shared Slack app). The end customer opens it;
//!   Slack's callback (`routes::slack_app`) finishes the row through
//!   [`finish_slack`] and sends the browser to `return_url` with
//!   `?channel_id=…&status=connected|failed`.
//! * `discord`, `teams`, `telegram`, `whatsapp`: `400 channel_kind_unavailable`.
//!   They need Allternit's production app for that vendor, and the account
//!   binding for them is not offered on the Platform API yet.
//!
//! A kind whose production app credentials are missing on this deployment is
//! also `channel_kind_unavailable` (never a broken link). One vendor place
//! (a Slack team, an address) belongs to one account in a project, so an inbound
//! message always has exactly one account.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Extension, Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, PgPool};

use super::{
    agents,
    conversations::ensure_agent,
    hosting::{host_for, runtime_owner, AgentHost},
    twin::visible_account,
    build_page, new_id, ApiJson, ApiQuery, Page, PageParams, PlatformCaller, PlatformError, RouteTable,
};
use crate::ApiState;

/// Every kind the API names (spec §4).
pub const KINDS: [&str; 6] = ["email", "slack", "discord", "teams", "telegram", "whatsapp"];
/// The prefix of a Slack OAuth `state` subject that belongs to a Platform API channel.
pub const SLACK_STATE_PREFIX: &str = "platform-channel:";

pub fn register(table: RouteTable) -> RouteTable {
    table
        .add("/v1/accounts/:id/channels", &["GET"], get(list_channels))
        .add("/v1/accounts/:id/channels/:kind/connect", &["POST"], post(connect))
        .add("/v1/accounts/:id/channels/:channel_id", &["GET", "DELETE"], get(get_channel).delete(delete_channel))
}

type HostExt = Option<Extension<Arc<dyn AgentHost>>>;

/// Which vendor apps this deployment has. Production reads the environment;
/// tests layer their own as an `Extension<Arc<ChannelApps>>`.
#[derive(Clone, Default)]
pub struct ChannelApps {
    pub slack: Option<crate::routes::slack_app::SlackAppConfig>,
    pub email: bool,
}

impl ChannelApps {
    pub fn from_env() -> Self {
        Self {
            slack: crate::routes::slack_app::app_config(),
            email: crate::routes::bot_email::MailConfig::from_env().is_some(),
        }
    }
}

fn apps_for(layered: Option<Extension<Arc<ChannelApps>>>) -> Arc<ChannelApps> {
    match layered {
        Some(Extension(a)) => a,
        None => Arc::new(ChannelApps::from_env()),
    }
}

fn unavailable(kind: &str, why: &str) -> PlatformError {
    PlatformError::invalid_request("channel_kind_unavailable", format!("{kind} channels aren't available on the Platform API yet: {why}")).with_param("kind")
}

/// `Ok` when `kind` can be connected here, else `channel_kind_unavailable` in plain words.
pub fn check_kind(apps: &ChannelApps, kind: &str) -> Result<(), PlatformError> {
    match kind {
        "email" if apps.email => Ok(()),
        "email" => Err(unavailable(kind, "Allternit Mail isn't configured on this deployment.")),
        "slack" if apps.slack.is_some() => Ok(()),
        "slack" => Err(unavailable(kind, "Allternit's production Slack app isn't set up yet.")),
        "discord" | "teams" | "telegram" | "whatsapp" => Err(unavailable(kind, "connecting them for an end-customer account is not offered yet.")),
        _ => Err(PlatformError::invalid_request("invalid_channel_kind", format!("kind must be one of {}.", KINDS.join(", "))).with_param("kind")),
    }
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Channel {
    pub id: String,
    #[sqlx(skip)]
    pub object: &'static str,
    pub account_id: String,
    pub agent_id: String,
    pub kind: String,
    pub status: String,
    pub external_id: Option<String>,
    pub display_name: Option<String>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub connected_at: Option<DateTime<Utc>>,
}

const COLUMNS: &str = "id, account_id, agent_id, kind, status, external_id, display_name, error, created_at, connected_at";

fn obj(mut c: Channel) -> Channel {
    c.object = "channel";
    c
}

async fn fetch(db: &PgPool, project: &str, account: &str, id: &str) -> Result<Channel, PlatformError> {
    sqlx::query_as::<_, Channel>(&format!(
        "SELECT {COLUMNS} FROM platform_channel_connections WHERE id = $1 AND project_id = $2 AND account_id = $3 AND deleted_at IS NULL"
    ))
    .bind(id)
    .bind(project)
    .bind(account)
    .fetch_optional(db)
    .await?
    .map(obj)
    .ok_or_else(|| PlatformError::not_found("channel_not_found", "No such channel."))
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    #[serde(flatten)]
    page: PageParams,
    agent_id: Option<String>,
}

async fn list_channels(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    Path(id): Path<String>,
    ApiQuery(q): ApiQuery<ListQuery>,
) -> Result<Json<Page<Channel>>, PlatformError> {
    caller.require("channels")?;
    let account = visible_account(&state.db, &caller, &id).await?;
    let limit = q.page.limit()?;
    let cursor = q.page.cursor()?;
    let rows = sqlx::query_as::<_, Channel>(&format!(
        "SELECT {COLUMNS} FROM platform_channel_connections WHERE project_id = $1 AND account_id = $2 AND deleted_at IS NULL \
         AND ($3::text IS NULL OR agent_id = $3) AND ($4::timestamptz IS NULL OR (created_at, id) > ($4, $5)) \
         ORDER BY created_at, id LIMIT $6"
    ))
    .bind(&caller.project_id)
    .bind(&account)
    .bind(&q.agent_id)
    .bind(cursor.as_ref().map(|c| c.0))
    .bind(cursor.as_ref().map(|c| c.1.clone()).unwrap_or_default())
    .bind(limit + 1)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(build_page(rows.into_iter().map(obj).collect(), limit, |c| (c.created_at, c.id.clone()))))
}

async fn get_channel(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    Path((id, channel)): Path<(String, String)>,
) -> Result<Json<Channel>, PlatformError> {
    caller.require("channels")?;
    let account = visible_account(&state.db, &caller, &id).await?;
    Ok(Json(fetch(&state.db, &caller.project_id, &account, &channel).await?))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConnectBody {
    agent_id: String,
    return_url: Option<String>,
    local_part: Option<String>,
}

/// `https://…` (or `http://localhost…` for local development), at most 2 KB.
fn check_return_url(raw: &str) -> Result<String, PlatformError> {
    let bad = || PlatformError::invalid_request("invalid_return_url", "return_url must be an https URL.").with_param("return_url");
    let url = reqwest::Url::parse(raw.trim()).map_err(|_| bad())?;
    let local = url.scheme() == "http" && matches!(url.host_str(), Some("localhost") | Some("127.0.0.1"));
    if raw.len() > 2048 || !(url.scheme() == "https" || local) || url.host_str().is_none() {
        return Err(bad());
    }
    Ok(url.to_string())
}

fn check_local_part(raw: &str) -> Result<String, PlatformError> {
    let s = raw.trim().to_lowercase();
    let ok = (1..=40).contains(&s.len())
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_')
        && !s.starts_with('.')
        && !s.ends_with('.');
    if ok {
        Ok(s)
    } else {
        Err(PlatformError::invalid_request("invalid_local_part", "local_part must be 1 to 40 letters, digits, dots, dashes or underscores.").with_param("local_part"))
    }
}

fn hash(s: &str) -> String {
    crate::services::api_keys::hash_token(s)
}

/// Map a runtime refusal to the `/v1` error format.
fn runtime_refused(status: u16, v: &Value) -> PlatformError {
    let code = v["error"].as_str().unwrap_or("channel_connect_failed");
    let message = v["message"].as_str().unwrap_or("The hosted runtime couldn't connect the channel.");
    match status {
        409 => PlatformError::conflict(code, message),
        400 | 422 => PlatformError::invalid_request(code, message),
        _ => PlatformError::api_error("channel_connect_failed", message),
    }
}

async fn connect(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    layered: HostExt,
    apps: Option<Extension<Arc<ChannelApps>>>,
    Path((id, kind)): Path<(String, String)>,
    ApiJson(body): ApiJson<ConnectBody>,
) -> Result<(StatusCode, Json<Value>), PlatformError> {
    caller.require("channels")?;
    let account = visible_account(&state.db, &caller, &id).await?;
    let apps = apps_for(apps);
    check_kind(&apps, &kind)?;
    let agent = agents::fetch_visible(&state, &caller, &body.agent_id).await.map_err(|e| e.with_param("agent_id"))?;
    if agent.account_id != account {
        return Err(PlatformError::not_found("agent_not_found", "No such agent in this account.").with_param("agent_id"));
    }
    let return_url = body.return_url.as_deref().map(check_return_url).transpose()?;
    let live: i64 = sqlx::query_scalar("SELECT count(*) FROM platform_channel_connections WHERE agent_id = $1 AND kind = $2 AND deleted_at IS NULL AND status <> 'failed'")
        .bind(&agent.id)
        .bind(&kind)
        .fetch_one(&state.db)
        .await?;
    if live > 0 {
        return Err(PlatformError::conflict("channel_already_connected", format!("This agent already has a {kind} channel. Delete it first.")));
    }

    match kind.as_str() {
        "email" => {
            let local = body.local_part.as_deref().map(check_local_part).transpose()?;
            let host = host_for(&state, layered);
            let rt = ensure_agent(&state, host.as_ref(), &caller, &agent.id).await?;
            let channel_id = new_id("ch_");
            insert_pending(&state.db, &channel_id, &caller.project_id, &account, &agent.id, "email", return_url.as_deref(), None).await?;
            let res = host
                .call(&rt, "PUT", &format!("/api/v1/platform/channels/{channel_id}"), &json!({ "kind": "email", "agentId": agent.id, "accountId": account, "localPart": local }))
                .await;
            let address = match res {
                Ok((status, v)) if status < 300 && v["address"].as_str().is_some() => v["address"].as_str().unwrap_or_default().to_string(),
                Ok((status, v)) => {
                    discard(&state.db, &channel_id).await?;
                    return Err(runtime_refused(status, &v));
                }
                Err(e) => {
                    discard(&state.db, &channel_id).await?;
                    return Err(e);
                }
            };
            sqlx::query(
                "UPDATE platform_channel_connections SET status = 'connected', external_id = $2, display_name = $2, connected_at = NOW(), updated_at = NOW() WHERE id = $1",
            )
            .bind(&channel_id)
            .bind(&address)
            .execute(&state.db)
            .await?;
            let ch = fetch(&state.db, &caller.project_id, &account, &channel_id).await?;
            Ok((StatusCode::CREATED, Json(json!(ch))))
        }
        "slack" => {
            let cfg = apps.slack.as_ref().ok_or_else(|| unavailable("slack", "Allternit's production Slack app isn't set up yet."))?;
            let channel_id = new_id("ch_");
            let nonce = hex::encode(rand::random::<[u8; 16]>());
            insert_pending(&state.db, &channel_id, &caller.project_id, &account, &agent.id, "slack", return_url.as_deref(), Some(&hash(&nonce))).await?;
            let subject = format!("{SLACK_STATE_PREFIX}{channel_id}:{nonce}");
            let url = crate::routes::slack_app::authorize_url(cfg, &subject);
            let ch = fetch(&state.db, &caller.project_id, &account, &channel_id).await?;
            let mut v = json!(ch);
            v["connect_url"] = json!(url);
            Ok((StatusCode::CREATED, Json(v)))
        }
        _ => Err(unavailable(&kind, "not offered yet.")),
    }
}

#[allow(clippy::too_many_arguments)]
async fn insert_pending(db: &PgPool, id: &str, project: &str, account: &str, agent: &str, kind: &str, return_url: Option<&str>, nonce_hash: Option<&str>) -> Result<(), PlatformError> {
    sqlx::query(
        "INSERT INTO platform_channel_connections (id, project_id, account_id, agent_id, kind, status, return_url, connect_nonce_hash) VALUES ($1, $2, $3, $4, $5, 'pending', $6, $7)",
    )
    .bind(id)
    .bind(project)
    .bind(account)
    .bind(agent)
    .bind(kind)
    .bind(return_url)
    .bind(nonce_hash)
    .execute(db)
    .await?;
    Ok(())
}

async fn discard(db: &PgPool, id: &str) -> Result<(), PlatformError> {
    sqlx::query("UPDATE platform_channel_connections SET deleted_at = NOW(), updated_at = NOW() WHERE id = $1").bind(id).execute(db).await?;
    Ok(())
}

async fn delete_channel(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    layered: HostExt,
    Path((id, channel)): Path<(String, String)>,
) -> Result<Json<Value>, PlatformError> {
    caller.require("channels")?;
    let account = visible_account(&state.db, &caller, &id).await?;
    let ch = fetch(&state.db, &caller.project_id, &account, &channel).await?;
    if ch.status == "connected" {
        // The runtime must let go first: a row marked deleted while the agent still
        // answers on the channel would tell the developer something false.
        let host = host_for(&state, layered);
        let rt = host.runtime(&caller.project_id).await?;
        let (status, v) = host
            .call(&rt, "DELETE", &format!("/api/v1/platform/channels/{}", ch.id), &json!({ "kind": ch.kind, "agentId": ch.agent_id, "accountId": account, "externalId": ch.external_id }))
            .await?;
        if status >= 300 && status != 404 {
            return Err(runtime_refused(status, &v));
        }
        if ch.kind == "slack" {
            if let Some(team) = &ch.external_id {
                sqlx::query("DELETE FROM slack_installs WHERE team_id = $1 AND user_id = $2")
                    .bind(team)
                    .bind(runtime_owner(&caller.project_id))
                    .execute(&state.db)
                    .await?;
            }
        }
    }
    discard(&state.db, &ch.id).await?;
    Ok(Json(json!({ "id": ch.id, "object": "channel", "deleted": true })))
}

// ---------------------------------------------------------------- Slack OAuth finish

/// Where the end customer's browser goes after Slack's consent page.
pub struct SlackFinish {
    pub return_url: Option<String>,
    pub channel_id: String,
    pub connected: bool,
    pub reason: Option<String>,
}

impl SlackFinish {
    /// `return_url?channel_id=…&status=…[&reason=…]`, or None when the developer gave none.
    pub fn redirect(&self) -> Option<String> {
        let mut url = reqwest::Url::parse(self.return_url.as_deref()?).ok()?;
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("channel_id", &self.channel_id);
            q.append_pair("status", if self.connected { "connected" } else { "failed" });
            if let Some(r) = &self.reason {
                q.append_pair("reason", r);
            }
        }
        Some(url.to_string())
    }
}

/// Finish a Platform API Slack connection from Slack's OAuth callback. `subject`
/// is the verified state subject after [`SLACK_STATE_PREFIX`]: `<channel_id>:<nonce>`.
/// Exchanges the code (as the project's runtime owner), binds the team to the
/// agent on the runtime, and marks the row connected (or failed, with why).
pub async fn finish_slack(
    state: &Arc<ApiState>,
    host: &dyn AgentHost,
    http: &dyn crate::routes::slack_app::SlackHttp,
    cfg: &crate::routes::slack_app::SlackAppConfig,
    subject: &str,
    code: &str,
) -> Result<SlackFinish, PlatformError> {
    let (channel_id, nonce) = subject.split_once(':').ok_or_else(|| PlatformError::invalid_request("bad_state", "malformed state"))?;
    let row: Option<(String, String, String, Option<String>, Option<String>, String)> = sqlx::query_as(
        "SELECT project_id, account_id, agent_id, return_url, connect_nonce_hash, status FROM platform_channel_connections WHERE id = $1 AND kind = 'slack' AND deleted_at IS NULL",
    )
    .bind(channel_id)
    .fetch_optional(&state.db)
    .await?;
    let Some((project, account, agent_id, return_url, nonce_hash, status)) = row else {
        return Err(PlatformError::not_found("channel_not_found", "This connection was deleted or never started."));
    };
    if status != "pending" || nonce_hash.as_deref() != Some(hash(nonce).as_str()) {
        return Err(PlatformError::conflict("connect_link_used", "This connect link was already used. Start the connection again."));
    }
    // One use: the nonce is gone before the code is exchanged.
    sqlx::query("UPDATE platform_channel_connections SET connect_nonce_hash = NULL, updated_at = NOW() WHERE id = $1").bind(channel_id).execute(&state.db).await?;
    let mut finish = SlackFinish { return_url, channel_id: channel_id.to_string(), connected: false, reason: None };
    let fail = |reason: &str| reason.to_string();

    let owner = runtime_owner(&project);
    let outcome: Result<(String, String), String> = async {
        let installed = crate::routes::slack_app::complete_install(state, http, cfg, code, &owner).await.map_err(|e| fail(&e.to_string()))?;
        let team = installed["teamId"].as_str().unwrap_or_default().to_string();
        let team_name = installed["teamName"].as_str().unwrap_or(&team).to_string();
        let taken: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM platform_channel_connections WHERE project_id = $1 AND kind = 'slack' AND external_id = $2 AND deleted_at IS NULL AND id <> $3",
        )
        .bind(&project)
        .bind(&team)
        .bind(channel_id)
        .fetch_one(&state.db)
        .await
        .map_err(|e| e.to_string())?;
        if taken > 0 {
            return Err("this Slack workspace is already connected to another account".to_string());
        }
        // The caller is the project's own runtime owner: a stand-in with the project's identity.
        let caller = project_caller(&state.db, &project).await.map_err(|e| e.message)?;
        let rt = ensure_agent(state, host, &caller, &agent_id).await.map_err(|e| e.message)?;
        crate::routes::slack_app::claim_install(state, &owner, &team, &rt.runtime_id).await.map_err(|e| e.to_string())?;
        let (st, v) = host
            .call(&rt, "PUT", &format!("/api/v1/platform/channels/{channel_id}"), &json!({ "kind": "slack", "agentId": agent_id, "accountId": account, "teamId": team, "teamName": team_name }))
            .await
            .map_err(|e| e.message)?;
        if st >= 300 {
            return Err(v["message"].as_str().unwrap_or("the runtime refused the Slack connection").to_string());
        }
        Ok((team, team_name))
    }
    .await;
    match outcome {
        Ok((team, name)) => {
            sqlx::query(
                "UPDATE platform_channel_connections SET status = 'connected', external_id = $2, display_name = $3, error = NULL, connected_at = NOW(), updated_at = NOW() WHERE id = $1",
            )
            .bind(channel_id)
            .bind(&team)
            .bind(&name)
            .execute(&state.db)
            .await?;
            finish.connected = true;
        }
        Err(why) => {
            tracing::warn!(channel = %channel_id, "platform slack connect failed: {why}");
            sqlx::query("UPDATE platform_channel_connections SET status = 'failed', error = $2, updated_at = NOW() WHERE id = $1")
                .bind(channel_id)
                .bind(&why)
                .execute(&state.db)
                .await?;
            finish.reason = Some("connect_failed".into());
        }
    }
    Ok(finish)
}

/// The project acting for itself (an OAuth callback has no API key): unbound,
/// with the `agents` scope only, so `ensure_agent` can sync the agent.
async fn project_caller(db: &PgPool, project: &str) -> Result<PlatformCaller, PlatformError> {
    let (env, plan, owner, org): (String, String, String, Option<String>) =
        sqlx::query_as("SELECT env, plan, owner_user_id, org_id FROM platform_projects WHERE id = $1 AND archived_at IS NULL")
            .bind(project)
            .fetch_optional(db)
            .await?
            .ok_or_else(|| PlatformError::not_found("project_not_found", "The project is archived."))?;
    Ok(PlatformCaller {
        project_id: project.to_string(),
        project_env: super::ProjectEnv::parse(&env).unwrap_or(super::ProjectEnv::Sandbox),
        account_id: None,
        key_id: "oauth_callback".into(),
        scopes: vec!["agents".into()],
        owner_user_id: owner,
        org_id: org,
        plan: super::caller::Plan::parse(&plan),
        rpm_override: None,
        call_cap_override: None,
    })
}

/// The plain page shown when a Slack connection finishes without a `return_url`.
pub fn finish_page(f: &SlackFinish) -> String {
    let (title, line) = if f.connected {
        ("Slack connected", "Slack is connected. You can close this tab.")
    } else {
        ("Slack not connected", "Slack could not be connected. Close this tab and try again from the app you started in.")
    };
    format!("<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\"><title>{title}</title></head><body style=\"font-family:system-ui,sans-serif;margin:3rem auto;max-width:32rem;padding:0 1rem\"><h1 style=\"font-size:1.3rem\">{title}</h1><p>{line}</p></body></html>")
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn kinds_without_an_app_are_unavailable_in_plain_words() {
        let none = ChannelApps::default();
        for k in ["email", "slack", "discord", "teams", "telegram", "whatsapp"] {
            assert_eq!(check_kind(&none, k).unwrap_err().code, "channel_kind_unavailable", "{k}");
        }
        assert_eq!(check_kind(&none, "fax").unwrap_err().code, "invalid_channel_kind");
        let email = ChannelApps { email: true, ..Default::default() };
        assert!(check_kind(&email, "email").is_ok());
        assert_eq!(check_kind(&email, "discord").unwrap_err().code, "channel_kind_unavailable");
    }

    #[test]
    fn return_urls_are_https_and_redirects_carry_the_outcome() {
        assert!(check_return_url("https://app.example.com/done").is_ok());
        assert!(check_return_url("http://localhost:3000/done").is_ok());
        assert!(check_return_url("http://app.example.com/done").is_err());
        assert!(check_return_url("javascript:alert(1)").is_err());
        let f = SlackFinish { return_url: Some("https://app.example.com/done?x=1".into()), channel_id: "ch_1".into(), connected: true, reason: None };
        assert_eq!(f.redirect().unwrap(), "https://app.example.com/done?x=1&channel_id=ch_1&status=connected");
        assert!(check_local_part("front.desk").is_ok() && check_local_part("a b").is_err() && check_local_part(".x").is_err());
    }
}
