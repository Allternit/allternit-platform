//! Standing per-tool "Allow" grants (the Customize per-tool policy, synced from the client).
//!
//! The per-tool Allow/Ask/Block policy lives only in the client (allternit-ai
//! `tool-policy.store.ts`, localStorage). A person who sets Allow has made a standing human
//! decision, so the client syncs it here with `PUT /api/v1/kernel/standing-grants` (a person's
//! session proof is required, a machine credential is refused). [`is_granted`] is read by
//! `check` for an `ask` rule only: a human-only class (money, client_message, deploy,
//! secret_new_host, irreversible) is `HumanOnly` and never reads this table, so it always needs a
//! fresh token.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use chrono::Utc;
use rusqlite::{params, Connection};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::AuthUser;
use crate::db::DbHandle;
use crate::enterprise_auth::CredentialContext;
use crate::AppState;

const MAX_TOOLS: usize = 500;

/// A client tool id matches a runtime tool name the way `resolveToolPolicy` does: equal, or the
/// name ends with the id after `__`, `.` or `/`.
fn matches(granted: &str, tool: &str) -> bool {
    let (g, t) = (granted.to_lowercase(), tool.to_lowercase());
    !g.is_empty() && (t == g || t.ends_with(&format!("__{g}")) || t.ends_with(&format!(".{g}")) || t.ends_with(&format!("/{g}")))
}

/// Does `user_id` hold a standing Allow for `tool`? Any store error reads as "no".
pub fn is_granted(db: &DbHandle, user_id: &str, tool: &str) -> bool {
    let Ok(conn) = db.connect() else { return false };
    granted_in(&conn, user_id, tool)
}

fn granted_in(conn: &Connection, user_id: &str, tool: &str) -> bool {
    let Ok(mut st) = conn.prepare("SELECT tool FROM kernel_standing_grants WHERE owner_user_id = ?1") else { return false };
    let Ok(rows) = st.query_map(params![user_id], |r| r.get::<_, String>(0)) else { return false };
    rows.flatten().any(|g| matches(&g, tool))
}

pub fn set(conn: &Connection, user_id: &str, tool: &str, allow: bool) -> rusqlite::Result<()> {
    if allow {
        conn.execute(
            "INSERT INTO kernel_standing_grants (owner_user_id, tool, set_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(owner_user_id, tool) DO UPDATE SET set_at = excluded.set_at",
            params![user_id, tool, crate::authority::tokens::ts(Utc::now())],
        )?;
    } else {
        conn.execute("DELETE FROM kernel_standing_grants WHERE owner_user_id = ?1 AND tool = ?2", params![user_id, tool])?;
    }
    Ok(())
}

fn valid_tool(t: &str) -> bool {
    !t.is_empty() && t.len() <= 128 && t.chars().all(|c| c.is_ascii_alphanumeric() || "_.:/-".contains(c))
}

#[derive(Deserialize)]
struct Entry {
    tool: String,
    /// `allow` records the grant; `ask` and `block` remove it.
    policy: String,
}

#[derive(Deserialize)]
struct SyncBody {
    tools: Vec<Entry>,
}

/// `PUT /api/v1/kernel/standing-grants`: `{ "tools": [{ "tool", "policy" }] }`. Person only.
pub async fn sync_handler(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    credential: Option<Extension<CredentialContext>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let deny = |code: StatusCode, why: &str| (code, Json(json!({ "ok": false, "error": why }))).into_response();
    if let Some((_, reason)) = super::routes::machine_marker(&headers, credential.is_some(), &user.user_id) {
        tracing::warn!(reason, "refused a standing grant from a machine credential");
        return deny(StatusCode::FORBIDDEN, "person_required");
    }
    if let Err(why) = crate::subscription_routes::person_acted(&state, &headers, &user.user_id).await {
        tracing::warn!(reason = why, "refused a standing grant without a person's session");
        return deny(StatusCode::FORBIDDEN, "person_required");
    }
    let body: SyncBody = match serde_json::from_value(body) {
        Ok(b) => b,
        Err(_) => return deny(StatusCode::BAD_REQUEST, "invalid_body"),
    };
    if body.tools.len() > MAX_TOOLS || body.tools.iter().any(|e| !valid_tool(&e.tool) || !matches!(e.policy.as_str(), "allow" | "ask" | "block")) {
        return deny(StatusCode::BAD_REQUEST, "invalid_body");
    }
    let (db, uid) = (state.db.clone(), user.user_id.clone());
    let n = body.tools.len();
    let res = tokio::task::spawn_blocking(move || -> rusqlite::Result<()> {
        let conn = db.connect()?;
        for e in &body.tools {
            set(&conn, &uid, &e.tool, e.policy == "allow")?;
        }
        Ok(())
    })
    .await;
    match res {
        Ok(Ok(())) => (StatusCode::OK, Json(json!({ "ok": true, "synced": n }))).into_response(),
        _ => deny(StatusCode::SERVICE_UNAVAILABLE, "store_unavailable"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grant_matches_the_runtime_name_and_can_be_removed() {
        let db = DbHandle::new_memory().unwrap();
        let conn = db.connect().unwrap();
        set(&conn, "u1", "send_summary", true).unwrap();
        assert!(is_granted(&db, "u1", "send_summary"));
        assert!(is_granted(&db, "u1", "mcp__notes__send_summary"));
        assert!(!is_granted(&db, "u1", "send_summary_all"));
        assert!(!is_granted(&db, "u2", "send_summary"));
        set(&conn, "u1", "send_summary", false).unwrap();
        assert!(!is_granted(&db, "u1", "send_summary"));
    }
}
