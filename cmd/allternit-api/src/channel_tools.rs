//! Tool scoping per channel (spec P8.3): a bot can be more careful with
//! work that arrived from outside. `agents.config.channelTools` maps a
//! thread's origin (email, slack, mention, routine, coordinator, user) to tools it
//! may not use (`deny`) or must ask before using (`ask`). The rules become
//! the gizzi permission ruleset of each thread session from that channel.
//! Changes are recorded in the admin audit log.

use std::sync::Arc;

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use rusqlite::params;
use serde_json::{json, Map, Value};

use crate::{auth::AuthUser, db::DbHandle, AppState};

pub const CHANNELS: [&str; 6] = ["email", "slack", "mention", "routine", "coordinator", "user"];

pub fn channel_tools_router() -> Router<Arc<AppState>> {
    Router::new().route("/agents/:id/channel-tools", get(get_rules).put(set_rules))
}

/// The gizzi permission ruleset for a bot's threads from `channel`, if any.
pub fn channel_rules(db: &DbHandle, bot_id: &str, channel: &str) -> Option<Value> {
    let conn = db.connect().ok()?;
    let raw: Option<String> = conn
        .query_row(
            "SELECT json_extract(config, '$.channelTools') FROM agents WHERE id = ?1",
            params![bot_id],
            |r| r.get(0),
        )
        .ok()
        .flatten();
    let policy: Value = serde_json::from_str(&raw?).ok()?;
    ruleset(policy.get(channel)?)
}

/// `{deny: [..], ask: [..]}` → `[{permission, action, pattern}]`.
pub fn ruleset(policy: &Value) -> Option<Value> {
    let mut rules = Vec::new();
    for (key, action) in [("deny", "deny"), ("ask", "ask")] {
        for tool in policy.get(key).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
            rules.push(json!({ "permission": tool, "action": action, "pattern": "*" }));
        }
    }
    (!rules.is_empty()).then(|| Value::Array(rules))
}

fn clean(policy: &Value) -> Result<Value, String> {
    let obj = policy.as_object().ok_or("expected an object of channels")?;
    let mut out = Map::new();
    for (channel, p) in obj {
        if !CHANNELS.contains(&channel.as_str()) {
            return Err(format!("unknown channel \"{channel}\""));
        }
        let list = |k: &str| -> Vec<String> {
            p.get(k)
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
                .unwrap_or_default()
        };
        let (deny, ask) = (list("deny"), list("ask"));
        if !deny.is_empty() || !ask.is_empty() {
            out.insert(channel.clone(), json!({ "deny": deny, "ask": ask }));
        }
    }
    Ok(Value::Object(out))
}

async fn get_rules(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(id): Path<String>) -> Response {
    let raw: Option<Option<String>> = state.db.connect().ok().and_then(|c| {
        c.query_row(
            "SELECT json_extract(config, '$.channelTools') FROM agents WHERE id = ?1 AND user_id = ?2",
            params![id, user.user_id],
            |r| r.get(0),
        )
        .ok()
    });
    match raw {
        None => (StatusCode::NOT_FOUND, Json(json!({ "error": "bot not found" }))).into_response(),
        Some(v) => Json(json!({
            "channels": v.and_then(|s| serde_json::from_str::<Value>(&s).ok()).unwrap_or_else(|| json!({})),
            "known": CHANNELS,
        }))
        .into_response(),
    }
}

async fn set_rules(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let policy = match clean(body.get("channels").unwrap_or(&body)) {
        Ok(p) => p,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
    };
    let Ok(conn) = state.db.connect() else {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "database error" }))).into_response();
    };
    let n = conn
        .execute(
            "UPDATE agents SET config = json_set(COALESCE(config, '{}'), '$.channelTools', json(?3)) WHERE id = ?1 AND user_id = ?2",
            params![id, user.user_id, policy.to_string()],
        )
        .unwrap_or(0);
    if n == 0 {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "bot not found" }))).into_response();
    }
    // Admin audit (best effort: personal installs may have no organization).
    let _ = conn.execute(
        "INSERT INTO audit_events (id, organization_id, actor_id, action, resource_type, resource_id, metadata)
         SELECT ?1, organization_id, ?2, 'bot.channel_tools.updated', 'agent', ?3, ?4
         FROM organization_members WHERE user_id = ?2 LIMIT 1",
        params![uuid::Uuid::new_v4().to_string(), user.user_id, id, policy.to_string()],
    );
    Json(json!({ "channels": policy })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_policies_become_permission_rules() {
        let p = clean(&json!({ "email": { "deny": ["bash", " write "], "ask": ["webfetch"] }, "mention": {} })).unwrap();
        assert_eq!(p, json!({ "email": { "deny": ["bash", "write"], "ask": ["webfetch"] } }));
        assert_eq!(
            ruleset(&p["email"]).unwrap(),
            json!([
                { "permission": "bash", "action": "deny", "pattern": "*" },
                { "permission": "write", "action": "deny", "pattern": "*" },
                { "permission": "webfetch", "action": "ask", "pattern": "*" }
            ])
        );
        assert!(clean(&json!({ "fax": { "deny": ["bash"] } })).is_err());
        assert!(ruleset(&json!({})).is_none());
    }
}
