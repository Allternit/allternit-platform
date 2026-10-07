//! Runtime side of the Platform API twin layer and channels (spec §4 Twin,
//! Approvals, Channels; P4).
//!
//! A project's hosted runtime (signed owner `platform:<project_id>`) holds the
//! agents of every end-customer account of that project. cloud-api relays these
//! calls here, signed ([`crate::relay_auth`]), always naming the account in the
//! path. **Isolation:** an account's data is what its own agents produced: an
//! agent belongs to one account (`agents.config.platformAgent.accountId`, set by
//! [`crate::platform_agents::upsert_bot`]), and every read and write here is
//! limited to rows of that account's agents:
//!
//! * people: someone with a thread or a remembered fact from one of the
//!   account's agents. Only those agents' facts are shown. A person two accounts
//!   both talk to (same phone number) can't be renamed through one of them
//!   (409 `person_shared`): the name would change for the other account too.
//! * inbox items (`inbox_items.agent_id`) and approvals
//!   (`gateway_approvals.bot_id`, Allternit authority).
//!
//! Routes (all relayed, owner must be a `platform:` owner):
//! * `GET   /api/v1/platform/accounts/:account/people` `{limit, after}`
//! * `GET|PATCH /api/v1/platform/accounts/:account/people/:person`
//! * `GET   /api/v1/platform/accounts/:account/inbox` `{limit, after, status}`
//! * `POST  /api/v1/platform/accounts/:account/inbox/:item/resolve`
//! * `GET   /api/v1/platform/accounts/:account/approvals` `{limit, after, status}`
//! * `POST  /api/v1/platform/accounts/:account/approvals/:id/decide` `{decision, actor}`
//! * `PUT|DELETE /api/v1/platform/channels/:channel_id` `{kind, agentId, accountId, …}`:
//!   bind (or release) a channel for one agent: `email` provisions the agent's
//!   own address; `slack` records the installed team and switches the agent on
//!   for it.
//!
//! List bodies travel in the signed request body (the relay signs method, path
//! and body, not the query string). Errors are `{"error": code, "message"}`.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Extension, Json, Router,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::relay_auth::{RelaySecret, RelayedAuth};
use crate::AppState;

pub fn platform_twin_router() -> Router<Arc<AppState>> {
    platform_twin_router_with(crate::relay_auth::process_secret())
}

pub fn platform_twin_router_with(secret: Arc<dyn RelaySecret>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/platform/accounts/:account/people", get(people_h))
        .route("/api/v1/platform/accounts/:account/people/:person", get(person_h).patch(person_patch_h))
        .route("/api/v1/platform/accounts/:account/inbox", get(inbox_h))
        .route("/api/v1/platform/accounts/:account/inbox/:item/resolve", post(resolve_h))
        .route("/api/v1/platform/accounts/:account/approvals", get(approvals_h))
        .route("/api/v1/platform/accounts/:account/approvals/:approval/decide", post(decide_h))
        .route("/api/v1/platform/channels/:channel_id", put(channel_put_h).delete(channel_delete_h))
        .layer(Extension(secret))
}

fn fail(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(json!({ "error": code, "message": message }))).into_response()
}

fn internal(e: impl std::fmt::Display) -> Response {
    tracing::warn!("platform twin: {e}");
    fail(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", "The runtime hit an internal error.")
}

/// Only a project's hosted runtime serves these routes.
fn platform_owner(auth: &RelayedAuth) -> Result<(), Response> {
    if auth.owner.starts_with("platform:") {
        Ok(())
    } else {
        Err(fail(StatusCode::FORBIDDEN, "not_a_platform_runtime", "This runtime doesn't host Platform API agents."))
    }
}

/// The account's agents on this runtime (SQL over `?1` = owner, `?2` = account).
const ACCOUNT_BOTS: &str = "SELECT id FROM agents WHERE user_id = ?1 AND json_extract(config, '$.platformAgent.accountId') = ?2";

/// The account an agent belongs to, when it is one of the owner's platform agents.
pub fn agent_account(conn: &Connection, owner: &str, agent: &str) -> Option<String> {
    conn.query_row(
        "SELECT json_extract(config, '$.platformAgent.accountId') FROM agents WHERE id = ?1 AND user_id = ?2",
        params![agent, owner],
        |r| r.get::<_, Option<String>>(0),
    )
    .optional()
    .ok()
    .flatten()
    .flatten()
    .filter(|a| !a.is_empty())
}

#[derive(Debug, Deserialize, Default)]
struct ListBody {
    limit: Option<i64>,
    after: Option<String>,
    status: Option<String>,
}

fn list_body(auth: &RelayedAuth) -> ListBody {
    if auth.body.is_empty() {
        ListBody::default()
    } else {
        auth.json().unwrap_or_default()
    }
}

fn limit_of(b: &ListBody) -> i64 {
    b.limit.unwrap_or(20).clamp(1, 100)
}

fn encode_cursor(at: &str, id: &str) -> String {
    hex::encode(format!("{at}|{id}"))
}

fn decode_cursor(raw: Option<&str>) -> Option<(String, String)> {
    let s = String::from_utf8(hex::decode(raw?).ok()?).ok()?;
    let (at, id) = s.split_once('|')?;
    Some((at.to_string(), id.to_string()))
}

/// `{data, has_more, next_cursor}` from up to `limit + 1` rows of `(created_at, id, item)`.
fn page(mut rows: Vec<(String, String, Value)>, limit: i64) -> Value {
    let has_more = rows.len() as i64 > limit;
    rows.truncate(limit as usize);
    let next = if has_more { rows.last().map(|(at, id, _)| encode_cursor(at, id)) } else { None };
    json!({ "data": rows.into_iter().map(|(_, _, v)| v).collect::<Vec<_>>(), "has_more": has_more, "next_cursor": next })
}

// ---------------------------------------------------------------- people

/// Is this (canonical) person one the account's agents know?
fn person_in_account(conn: &Connection, owner: &str, account: &str, person: &str) -> bool {
    conn.query_row(
        &format!(
            "SELECT 1 FROM people WHERE id = ?3 AND owner = ?1 AND merged_into IS NULL AND (
                 EXISTS (SELECT 1 FROM person_threads WHERE owner = ?1 AND person_id = ?3 AND bot_id IN ({ACCOUNT_BOTS}))
              OR EXISTS (SELECT 1 FROM person_facts WHERE owner = ?1 AND person_id = ?3 AND bot_id IN ({ACCOUNT_BOTS})))"
        ),
        params![owner, account, person],
        |_| Ok(()),
    )
    .optional()
    .ok()
    .flatten()
    .is_some()
}

fn person_json(conn: &Connection, db: &crate::db::DbHandle, owner: &str, account: &str, person: &crate::people::Person, created_at: &str) -> Value {
    let identities: Vec<Value> = crate::people::identities_of(db, owner, &person.id)
        .into_iter()
        .map(|i| json!({ "channel": i.provider, "kind": i.kind, "handle": i.label.unwrap_or(i.external_id), "last_seen_at": i.last_seen_at }))
        .collect();
    let facts: Vec<Value> = conn
        .prepare(&format!(
            "SELECT id, fact, bot_id, created_at FROM person_facts WHERE owner = ?1 AND person_id = ?3 AND bot_id IN ({ACCOUNT_BOTS})
             ORDER BY created_at DESC, rowid DESC LIMIT 50"
        ))
        .and_then(|mut q| {
            q.query_map(params![owner, account, person.id], |r| {
                Ok(json!({ "id": r.get::<_, String>(0)?, "fact": r.get::<_, String>(1)?, "agent_id": r.get::<_, Option<String>>(2)?, "created_at": r.get::<_, String>(3)? }))
            })?
            .collect()
        })
        .unwrap_or_default();
    json!({
        "id": person.id, "object": "person", "name": person.display_name, "org": person.org, "notes": person.notes,
        "identities": identities, "facts": facts, "created_at": created_at,
    })
}

async fn people_h(State(state): State<Arc<AppState>>, Path(account): Path<String>, auth: RelayedAuth) -> Response {
    if let Err(r) = platform_owner(&auth) {
        return r;
    }
    let body = list_body(&auth);
    let limit = limit_of(&body);
    let (at, id) = decode_cursor(body.after.as_deref()).unwrap_or_default();
    let conn = match state.db.connect() {
        Ok(c) => c,
        Err(e) => return internal(e),
    };
    let rows: Vec<(String, String)> = match conn
        .prepare(&format!(
            "SELECT id, created_at FROM people WHERE owner = ?1 AND merged_into IS NULL
               AND (id IN (SELECT person_id FROM person_threads WHERE owner = ?1 AND bot_id IN ({ACCOUNT_BOTS}))
                 OR id IN (SELECT person_id FROM person_facts WHERE owner = ?1 AND bot_id IN ({ACCOUNT_BOTS})))
               AND (?3 = '' OR created_at > ?3 OR (created_at = ?3 AND id > ?4))
             ORDER BY created_at, id LIMIT ?5"
        ))
        .and_then(|mut q| q.query_map(params![auth.owner, account, at, id, limit + 1], |r| Ok((r.get(0)?, r.get(1)?)))?.collect())
    {
        Ok(r) => r,
        Err(e) => return internal(e),
    };
    let items: Vec<(String, String, Value)> = rows
        .into_iter()
        .filter_map(|(pid, at)| crate::people::get_person(&state.db, &auth.owner, &pid).map(|p| (at.clone(), pid, person_json(&conn, &state.db, &auth.owner, &account, &p, &at))))
        .collect();
    Json(page(items, limit)).into_response()
}

fn load_person(state: &AppState, owner: &str, account: &str, person: &str) -> Result<(Connection, crate::people::Person, String), Response> {
    let conn = state.db.connect().map_err(internal)?;
    let p = crate::people::get_person(&state.db, owner, person).filter(|p| person_in_account(&conn, owner, account, &p.id));
    let Some(p) = p else { return Err(fail(StatusCode::NOT_FOUND, "person_not_found", "No such person.")) };
    let created: String = conn.query_row("SELECT created_at FROM people WHERE id = ?1", params![p.id], |r| r.get(0)).map_err(internal)?;
    Ok((conn, p, created))
}

async fn person_h(State(state): State<Arc<AppState>>, Path((account, person)): Path<(String, String)>, auth: RelayedAuth) -> Response {
    if let Err(r) = platform_owner(&auth) {
        return r;
    }
    match load_person(&state, &auth.owner, &account, &person) {
        Ok((conn, p, created)) => Json(person_json(&conn, &state.db, &auth.owner, &account, &p, &created)).into_response(),
        Err(r) => r,
    }
}

async fn person_patch_h(State(state): State<Arc<AppState>>, Path((account, person)): Path<(String, String)>, auth: RelayedAuth) -> Response {
    if let Err(r) = platform_owner(&auth) {
        return r;
    }
    let (conn, p, _) = match load_person(&state, &auth.owner, &account, &person) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let accounts: i64 = conn
        .query_row(
            "SELECT COUNT(DISTINCT json_extract(config, '$.platformAgent.accountId')) FROM agents WHERE user_id = ?1 AND id IN (
                 SELECT bot_id FROM person_threads WHERE owner = ?1 AND person_id = ?2
                 UNION SELECT bot_id FROM person_facts WHERE owner = ?1 AND person_id = ?2 AND bot_id IS NOT NULL)",
            params![auth.owner, p.id],
            |r| r.get(0),
        )
        .unwrap_or(2);
    if accounts > 1 {
        return fail(StatusCode::CONFLICT, "person_shared", "This person also talks to another account's agents, so their name and notes can't be changed from one account.");
    }
    let patch: Value = match auth.json() {
        Ok(v) => v,
        Err(r) => return r,
    };
    let mut allowed = serde_json::Map::new();
    for k in ["displayName", "notes", "org"] {
        if let Some(v) = patch.get(k) {
            allowed.insert(k.into(), v.clone());
        }
    }
    if let Err(e) = crate::people::update_person(&state.db, &auth.owner, &p.id, &Value::Object(allowed)) {
        return fail(StatusCode::BAD_REQUEST, "invalid_person", &e);
    }
    match load_person(&state, &auth.owner, &account, &p.id) {
        Ok((conn, p, created)) => Json(person_json(&conn, &state.db, &auth.owner, &account, &p, &created)).into_response(),
        Err(r) => r,
    }
}

// ---------------------------------------------------------------- inbox

fn inbox_item(r: &rusqlite::Row<'_>) -> rusqlite::Result<(String, String, Value)> {
    let id: String = r.get(0)?;
    let created: String = r.get::<_, Option<String>>(8)?.unwrap_or_default();
    let meta: Value = r.get::<_, Option<String>>(7)?.and_then(|m| serde_json::from_str(&m).ok()).unwrap_or(Value::Null);
    let status: String = r.get(6)?;
    let item = json!({
        "id": id, "object": "inbox_item", "agent_id": r.get::<_, Option<String>>(1)?, "kind": r.get::<_, String>(2)?,
        "title": r.get::<_, String>(3)?, "body": r.get::<_, Option<String>>(4)?, "severity": r.get::<_, String>(5)?,
        "status": if status == "resolved" { "resolved" } else { "open" },
        "thread_id": meta.get("threadId").cloned().unwrap_or(Value::Null), "channel": meta.get("channel").cloned().unwrap_or(Value::Null),
        "approval_id": meta.get("approvalId").cloned().unwrap_or(Value::Null), "created_at": created,
    });
    Ok((created, id, item))
}

const INBOX_COLS: &str = "id, agent_id, type, title, body, severity, status, metadata, created_at";

async fn inbox_h(State(state): State<Arc<AppState>>, Path(account): Path<String>, auth: RelayedAuth) -> Response {
    if let Err(r) = platform_owner(&auth) {
        return r;
    }
    let body = list_body(&auth);
    let limit = limit_of(&body);
    let status = body.status.clone().unwrap_or_else(|| "open".into());
    let (at, id) = decode_cursor(body.after.as_deref()).unwrap_or_default();
    let conn = match state.db.connect() {
        Ok(c) => c,
        Err(e) => return internal(e),
    };
    let rows = conn
        .prepare(&format!(
            "SELECT {INBOX_COLS} FROM inbox_items WHERE user_id = ?1 AND agent_id IN ({ACCOUNT_BOTS})
               AND (?3 = 'all' OR (?3 = 'resolved' AND status = 'resolved') OR (?3 = 'open' AND status <> 'resolved'))
               AND (?4 = '' OR created_at < ?4 OR (created_at = ?4 AND id < ?5))
             ORDER BY created_at DESC, id DESC LIMIT ?6"
        ))
        .and_then(|mut q| q.query_map(params![auth.owner, account, status, at, id, limit + 1], inbox_item)?.collect::<rusqlite::Result<Vec<_>>>());
    match rows {
        Ok(rows) => Json(page(rows, limit)).into_response(),
        Err(e) => internal(e),
    }
}

async fn resolve_h(State(state): State<Arc<AppState>>, Path((account, item)): Path<(String, String)>, auth: RelayedAuth) -> Response {
    if let Err(r) = platform_owner(&auth) {
        return r;
    }
    let conn = match state.db.connect() {
        Ok(c) => c,
        Err(e) => return internal(e),
    };
    let n = conn.execute(
        &format!("UPDATE inbox_items SET status = 'resolved', updated_at = CURRENT_TIMESTAMP WHERE id = ?3 AND user_id = ?1 AND agent_id IN ({ACCOUNT_BOTS})"),
        params![auth.owner, account, item],
    );
    match n {
        Ok(0) => fail(StatusCode::NOT_FOUND, "inbox_item_not_found", "No such inbox item."),
        Ok(_) => match conn.query_row(&format!("SELECT {INBOX_COLS} FROM inbox_items WHERE id = ?1"), params![item], inbox_item) {
            Ok((_, _, v)) => Json(v).into_response(),
            Err(e) => internal(e),
        },
        Err(e) => internal(e),
    }
}

// ---------------------------------------------------------------- approvals

fn approval_item(r: &rusqlite::Row<'_>) -> rusqlite::Result<(String, String, Value)> {
    let id: String = r.get(0)?;
    let created: String = r.get(6)?;
    let detail: Value = serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or(Value::Null);
    let item = json!({
        "id": id, "object": "approval", "agent_id": r.get::<_, String>(1)?, "thread_id": r.get::<_, String>(2)?,
        "action": r.get::<_, String>(3)?, "detail": detail, "status": r.get::<_, String>(5)?,
        "created_at": created, "resolved_at": r.get::<_, Option<String>>(7)?,
    });
    Ok((created, id, item))
}

const APPROVAL_COLS: &str = "id, bot_id, thread_id, action, detail_json, state, created_at, resolved_at";

async fn approvals_h(State(state): State<Arc<AppState>>, Path(account): Path<String>, auth: RelayedAuth) -> Response {
    if let Err(r) = platform_owner(&auth) {
        return r;
    }
    let body = list_body(&auth);
    let limit = limit_of(&body);
    let status = body.status.clone().unwrap_or_else(|| "pending".into());
    let (at, id) = decode_cursor(body.after.as_deref()).unwrap_or_default();
    let conn = match state.db.connect() {
        Ok(c) => c,
        Err(e) => return internal(e),
    };
    let rows = conn
        .prepare(&format!(
            "SELECT {APPROVAL_COLS} FROM gateway_approvals WHERE owner = ?1 AND authority = 'allternit' AND bot_id IN ({ACCOUNT_BOTS})
               AND (?3 = 'all' OR state = ?3)
               AND (?4 = '' OR created_at < ?4 OR (created_at = ?4 AND id < ?5))
             ORDER BY created_at DESC, id DESC LIMIT ?6"
        ))
        .and_then(|mut q| q.query_map(params![auth.owner, account, status, at, id, limit + 1], approval_item)?.collect::<rusqlite::Result<Vec<_>>>());
    match rows {
        Ok(rows) => Json(page(rows, limit)).into_response(),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
struct DecideBody {
    decision: String,
    #[serde(default)]
    actor: String,
}

async fn decide_h(State(state): State<Arc<AppState>>, Path((account, approval)): Path<(String, String)>, auth: RelayedAuth) -> Response {
    if let Err(r) = platform_owner(&auth) {
        return r;
    }
    let body: DecideBody = match auth.json() {
        Ok(b) => b,
        Err(r) => return r,
    };
    let mine = state.db.connect().ok().and_then(|c| {
        c.query_row(
            &format!("SELECT 1 FROM gateway_approvals WHERE id = ?3 AND owner = ?1 AND authority = 'allternit' AND bot_id IN ({ACCOUNT_BOTS})"),
            params![auth.owner, account, approval],
            |_| Ok(()),
        )
        .optional()
        .ok()
        .flatten()
    });
    if mine.is_none() {
        return fail(StatusCode::NOT_FOUND, "approval_not_found", "No such approval.");
    }
    let actor = if body.actor.is_empty() { "platform_api".to_string() } else { body.actor.chars().take(128).collect() };
    let tx = crate::gateway_runner::transport(&state);
    // A person pressed approve in the developer's app: the actor is a user, recorded by key.
    match crate::gateway_runner::respond_approval(&state.db, tx.as_ref(), &auth.owner, &approval, &body.decision, ("user", &actor)).await {
        Ok(_) => {}
        Err(e) if e.status == 409 => return fail(StatusCode::CONFLICT, "approval_already_resolved", "This approval is already resolved."),
        Err(e) if e.status == 400 => return fail(StatusCode::BAD_REQUEST, "invalid_decision", "decision must be approve or deny."),
        Err(e) => return fail(StatusCode::BAD_GATEWAY, "approval_failed", &e.message),
    }
    let conn = match state.db.connect() {
        Ok(c) => c,
        Err(e) => return internal(e),
    };
    match conn.query_row(&format!("SELECT {APPROVAL_COLS} FROM gateway_approvals WHERE id = ?1"), params![approval], approval_item) {
        Ok((_, _, v)) => Json(v).into_response(),
        Err(e) => internal(e),
    }
}

// ---------------------------------------------------------------- channels

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChannelBody {
    kind: String,
    agent_id: String,
    account_id: String,
    #[serde(default)]
    local_part: Option<String>,
    #[serde(default)]
    team_id: Option<String>,
    #[serde(default)]
    team_name: Option<String>,
    #[serde(default)]
    external_id: Option<String>,
}

/// The agent must be this owner's and belong to the named account.
fn check_agent(state: &AppState, owner: &str, agent: &str, account: &str) -> Result<(), Response> {
    let conn = state.db.connect().map_err(internal)?;
    match agent_account(&conn, owner, agent) {
        Some(a) if a == account => Ok(()),
        _ => Err(fail(StatusCode::NOT_FOUND, "agent_not_found", "The agent isn't on this runtime for that account.")),
    }
}

async fn channel_put_h(State(state): State<Arc<AppState>>, Path(_channel): Path<String>, auth: RelayedAuth) -> Response {
    if let Err(r) = platform_owner(&auth) {
        return r;
    }
    let b: ChannelBody = match auth.json() {
        Ok(b) => b,
        Err(r) => return r,
    };
    if let Err(r) = check_agent(&state, &auth.owner, &b.agent_id, &b.account_id) {
        return r;
    }
    match b.kind.as_str() {
        "email" => match crate::allternit_bus_routes::provision_email_brokered(&state, &auth.owner, &b.agent_id, b.local_part.as_deref(), None).await {
            Ok(address) => Json(json!({ "address": address })).into_response(),
            // Already a response (status + `{error}` body) from the email broker.
            Err(r) => r.into_response(),
        },
        "slack" => {
            let (Some(team), name) = (b.team_id.as_deref().filter(|t| !t.is_empty()), b.team_name.as_deref()) else {
                return fail(StatusCode::BAD_REQUEST, "invalid_channel", "teamId is required");
            };
            let conn_json = match crate::channel_slack_app::upsert_shared_connection(&state.db, &auth.owner, team, name.unwrap_or(team)) {
                Ok(v) => v,
                Err((status, msg)) => return fail(status, "slack_not_connected", &msg),
            };
            let Some(connection) = conn_json["account"]["id"].as_str() else { return internal("slack connection has no id") };
            let bound = state.db.connect().and_then(|c| {
                c.execute(
                    "INSERT OR IGNORE INTO channel_account_bots (account_id, bot_id, owner, is_default, created_at) VALUES (?1, ?2, ?3, 1, CURRENT_TIMESTAMP)",
                    params![connection, b.agent_id, auth.owner],
                )
            });
            match bound {
                Ok(_) => Json(json!({ "connected": true, "connectionId": connection })).into_response(),
                Err(e) => internal(e),
            }
        }
        _ => fail(StatusCode::BAD_REQUEST, "channel_kind_unavailable", "This channel kind isn't offered for Platform API accounts."),
    }
}

async fn channel_delete_h(State(state): State<Arc<AppState>>, Path(_channel): Path<String>, auth: RelayedAuth) -> Response {
    if let Err(r) = platform_owner(&auth) {
        return r;
    }
    let b: ChannelBody = match auth.json() {
        Ok(b) => b,
        Err(r) => return r,
    };
    if let Err(r) = check_agent(&state, &auth.owner, &b.agent_id, &b.account_id) {
        return r;
    }
    match b.kind.as_str() {
        "email" => {
            if !crate::agent_email_routes::revoke_agent_mailbox(&b.agent_id, &state.db).await {
                return fail(StatusCode::BAD_GATEWAY, "email_not_released", "The address couldn't be removed right now; retry in a minute.");
            }
            match state.db.connect().and_then(|c| crate::allternit_bus_routes::clear_email_channel(&c, &b.agent_id)) {
                Ok(_) => Json(json!({ "deleted": true })).into_response(),
                Err(e) => internal(e),
            }
        }
        "slack" => {
            let Some(team) = b.external_id.as_deref() else { return fail(StatusCode::BAD_REQUEST, "invalid_channel", "externalId is required") };
            let res = state.db.connect().and_then(|c| {
                c.execute(
                    "DELETE FROM channel_account_bots WHERE bot_id = ?1 AND owner = ?2 AND account_id IN (
                         SELECT id FROM provider_account_bindings WHERE owner = ?2 AND vendor = 'slack' AND external_account_id = ?3)",
                    params![b.agent_id, auth.owner, team],
                )?;
                c.execute(
                    "UPDATE provider_account_bindings SET state = 'DISCONNECTED', updated_at = ?3 WHERE owner = ?1 AND vendor = 'slack' AND external_account_id = ?2",
                    params![auth.owner, team, crate::agent_gateway_routes::now()],
                )
            });
            match res {
                Ok(_) => Json(json!({ "deleted": true })).into_response(),
                Err(e) => internal(e),
            }
        }
        _ => Json(json!({ "deleted": true })).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay_auth::{signed_headers, StaticRelaySecret};
    use axum::body::Body;
    use tower::ServiceExt;

    const TOKEN: &str = "device-token-for-twin-tests";
    const OWNER: &str = "platform:proj_t";

    async fn app() -> (Router, Arc<AppState>) {
        let dir = std::env::temp_dir().join(format!("allternit-platform-twin-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = crate::test_helpers::app_state(&dir).await;
        let secret: Arc<dyn RelaySecret> = Arc::new(StaticRelaySecret { token: TOKEN.into(), owner: OWNER.into() });
        (platform_twin_router_with(secret).with_state(state.clone()), state)
    }

    fn signed(method: &str, path: &str, body: Value) -> axum::http::Request<Body> {
        let bytes = serde_json::to_vec(&body).unwrap();
        let mut req = axum::http::Request::builder().method(method).uri(path).header("content-type", "application/json");
        for (k, v) in signed_headers(TOKEN, OWNER, method, path, &bytes) {
            req = req.header(k, v);
        }
        req.body(Body::from(bytes)).unwrap()
    }

    async fn send(app: &Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
        let r = app.clone().oneshot(signed(method, path, body)).await.unwrap();
        let s = r.status();
        let b = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        (s, serde_json::from_slice(&b).unwrap_or(Value::Null))
    }

    /// Two accounts, one agent each, both talking to the same phone number (one
    /// person on this runtime), each with its own fact, inbox card and approval.
    fn seed(state: &AppState) {
        for (agent, acct) in [("agent_a", "acct_a"), ("agent_b", "acct_b")] {
            crate::platform_agents::upsert_bot(&state.db, OWNER, agent, &json!({ "name": agent, "accountId": acct })).unwrap();
        }
        let c = state.db.connect().unwrap();
        let t = "2026-10-07T10:00:00Z";
        c.execute("INSERT INTO people (id, owner, display_name, created_at, updated_at) VALUES ('per_shared', ?1, 'Dana', ?2, ?2)", params![OWNER, t]).unwrap();
        c.execute("INSERT INTO people (id, owner, display_name, created_at, updated_at) VALUES ('per_only_a', ?1, 'Ari', ?2, ?2)", params![OWNER, t]).unwrap();
        c.execute("INSERT INTO people (id, owner, display_name, created_at, updated_at) VALUES ('per_only_b', ?1, 'Bo', ?2, ?2)", params![OWNER, t]).unwrap();
        for (th, per, bot) in [("th1", "per_shared", "agent_a"), ("th2", "per_shared", "agent_b"), ("th3", "per_only_a", "agent_a"), ("th4", "per_only_b", "agent_b")] {
            c.execute("INSERT INTO person_threads (thread_id, owner, person_id, bot_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5)", params![th, OWNER, per, bot, t]).unwrap();
        }
        c.execute("INSERT INTO person_facts (id, owner, person_id, bot_id, fact, created_at) VALUES ('f_a', ?1, 'per_shared', 'agent_a', 'Allergic to latex (A)', ?2)", params![OWNER, t]).unwrap();
        c.execute("INSERT INTO person_facts (id, owner, person_id, bot_id, fact, created_at) VALUES ('f_b', ?1, 'per_shared', 'agent_b', 'Has a dog named Rex (B)', ?2)", params![OWNER, t]).unwrap();
        for (id, bot) in [("in_a", "agent_a"), ("in_b", "agent_b")] {
            c.execute("INSERT INTO inbox_items (id, user_id, agent_id, type, title, status, metadata) VALUES (?1, ?2, ?3, 'autonomy.ask', 'Needs your OK', 'unread', '{\"threadId\":\"th\"}')", params![id, OWNER, bot]).unwrap();
        }
        for (id, bot) in [("gap_a", "agent_a"), ("gap_b", "agent_b")] {
            c.execute(
                "INSERT INTO gateway_approvals (id, owner, thread_id, bot_id, authority, action, detail_json, state, created_at) VALUES (?1, ?2, 'th', ?3, 'allternit', 'post to slack', '{\"text\":\"hi\"}', 'pending', ?4)",
                params![id, OWNER, bot, t],
            )
            .unwrap();
        }
    }

    #[tokio::test]
    async fn people_inbox_and_approvals_never_cross_accounts() {
        let (app, state) = app().await;
        seed(&state);

        let (s, a) = send(&app, "GET", "/api/v1/platform/accounts/acct_a/people", json!({})).await;
        assert_eq!(s, StatusCode::OK, "{a}");
        let ids: Vec<&str> = a["data"].as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap()).collect();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"per_shared") && ids.contains(&"per_only_a") && !ids.contains(&"per_only_b"));
        let shared = a["data"].as_array().unwrap().iter().find(|p| p["id"] == "per_shared").unwrap();
        let facts: Vec<&str> = shared["facts"].as_array().unwrap().iter().map(|f| f["fact"].as_str().unwrap()).collect();
        assert_eq!(facts, ["Allergic to latex (A)"], "B's facts never show under A");

        assert_eq!(send(&app, "GET", "/api/v1/platform/accounts/acct_a/people/per_only_b", json!({})).await.0, StatusCode::NOT_FOUND);
        let (s, e) = send(&app, "PATCH", "/api/v1/platform/accounts/acct_a/people/per_shared", json!({ "displayName": "Dana R" })).await;
        assert_eq!((s, e["error"].as_str()), (StatusCode::CONFLICT, Some("person_shared")));
        let (s, p) = send(&app, "PATCH", "/api/v1/platform/accounts/acct_a/people/per_only_a", json!({ "displayName": "Ari K", "replyPin": "sms" })).await;
        assert_eq!((s, p["name"].as_str()), (StatusCode::OK, Some("Ari K")), "{p}");

        let (_, inbox) = send(&app, "GET", "/api/v1/platform/accounts/acct_a/inbox", json!({ "status": "open" })).await;
        assert_eq!(inbox["data"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap()).collect::<Vec<_>>(), ["in_a"]);
        assert_eq!(send(&app, "POST", "/api/v1/platform/accounts/acct_a/inbox/in_b/resolve", json!({})).await.0, StatusCode::NOT_FOUND);
        let (s, r) = send(&app, "POST", "/api/v1/platform/accounts/acct_a/inbox/in_a/resolve", json!({})).await;
        assert_eq!((s, r["status"].as_str()), (StatusCode::OK, Some("resolved")));
        let (_, open) = send(&app, "GET", "/api/v1/platform/accounts/acct_a/inbox", json!({})).await;
        assert!(open["data"].as_array().unwrap().is_empty());

        let (_, aps) = send(&app, "GET", "/api/v1/platform/accounts/acct_b/approvals", json!({})).await;
        assert_eq!(aps["data"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap()).collect::<Vec<_>>(), ["gap_b"]);
        assert_eq!(send(&app, "POST", "/api/v1/platform/accounts/acct_b/approvals/gap_a/decide", json!({ "decision": "approve" })).await.0, StatusCode::NOT_FOUND);
        let (s, d) = send(&app, "POST", "/api/v1/platform/accounts/acct_a/approvals/gap_a/decide", json!({ "decision": "deny", "actor": "api_key:ak_1" })).await;
        assert_eq!((s, d["status"].as_str()), (StatusCode::OK, Some("denied")), "{d}");
        let (s, _) = send(&app, "POST", "/api/v1/platform/accounts/acct_a/approvals/gap_a/decide", json!({ "decision": "approve" })).await;
        assert_eq!(s, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn paging_walks_every_person_once() {
        let (app, state) = app().await;
        seed(&state);
        let (_, first) = send(&app, "GET", "/api/v1/platform/accounts/acct_b/people", json!({ "limit": 1 })).await;
        assert_eq!(first["has_more"], true);
        let (_, second) = send(&app, "GET", "/api/v1/platform/accounts/acct_b/people", json!({ "limit": 1, "after": first["next_cursor"] })).await;
        assert_eq!(second["has_more"], false);
        assert_ne!(first["data"][0]["id"], second["data"][0]["id"]);
    }

    #[tokio::test]
    async fn only_a_platform_runtime_and_the_agents_own_account_can_bind_channels() {
        let (app, state) = app().await;
        seed(&state);
        let (s, e) = send(&app, "PUT", "/api/v1/platform/channels/ch_1", json!({ "kind": "slack", "agentId": "agent_a", "accountId": "acct_b", "teamId": "T1" })).await;
        assert_eq!((s, e["error"].as_str()), (StatusCode::NOT_FOUND, Some("agent_not_found")));
        let (s, e) = send(&app, "PUT", "/api/v1/platform/channels/ch_1", json!({ "kind": "discord", "agentId": "agent_a", "accountId": "acct_a" })).await;
        assert_eq!((s, e["error"].as_str()), (StatusCode::BAD_REQUEST, Some("channel_kind_unavailable")));
        // An unsigned call never gets in.
        let raw = axum::http::Request::builder().method("GET").uri("/api/v1/platform/accounts/acct_a/people").body(Body::empty()).unwrap();
        assert_eq!(app.clone().oneshot(raw).await.unwrap().status(), StatusCode::UNAUTHORIZED);
    }
}
