//! Per-user tool permissions (`/tool-policies`).
//!
//! The account copy of Customize's Tools tab: for each connection (owner) and
//! tool, Allow or Block. Ask is the default and is never stored, so
//! `PUT` with `ask` deletes the row. Rows live in `tool_policies` (V296).
//!
//! Enforcement reads `load_user_tool_policies` (the bridge's
//! `permission.asked` handler and the `blockedTools` runtime marker, step C2).
//! Precedence there: user Block > bot settings > user Allow > Autopilot >
//! prompt.

use axum::{
    extract::{rejection::JsonRejection, Extension, Json, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tracing::warn;

use crate::auth::AuthUser;
use crate::AppState;

/// Most items one `PUT` may carry.
pub const MAX_POLICIES_PER_PUT: usize = 500;
/// Longest connection id after the `connectors:` / `mcps:` prefix.
const MAX_OWNER_KEY_LEN: usize = 128;
/// Longest tool id, in characters.
const MAX_TOOL_ID_LEN: usize = 200;
/// Owner prefixes the clients use today (`tool-policy.store.ts`).
const OWNER_PREFIXES: [&str; 2] = ["connectors:", "mcps:"];

pub fn tool_policy_router() -> Router<Arc<AppState>> {
    Router::new().route(
        "/tool-policies",
        get(get_tool_policies).put(put_tool_policies),
    )
}

/// One user choice for one tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolPolicyValue {
    Allow,
    Ask,
    Block,
}

impl ToolPolicyValue {
    pub fn as_str(self) -> &'static str {
        match self {
            ToolPolicyValue::Allow => "allow",
            ToolPolicyValue::Ask => "ask",
            ToolPolicyValue::Block => "block",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "allow" => Some(ToolPolicyValue::Allow),
            "ask" => Some(ToolPolicyValue::Ask),
            "block" => Some(ToolPolicyValue::Block),
            _ => None,
        }
    }
}

/// A stored policy row (never `Ask`: Ask is no row).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolPolicy {
    pub owner_id: String,
    pub tool_id: String,
    pub policy: ToolPolicyValue,
    pub updated_at: String,
}

/// The stored form of a tool id. For an owner `connectors:<id>`, a leading
/// `<id>.` is removed (the web may send `linear.search_issues`; iOS and
/// Android send `search_issues`). Only that exact prefix is removed, so other
/// dots stay (`github.x` under `connectors:linear` is kept as is). `mcps:`
/// owners and any other owner are unchanged.
pub fn normalize_tool_id<'a>(owner_id: &str, tool_id: &'a str) -> &'a str {
    owner_id
        .strip_prefix("connectors:")
        .filter(|id| !id.is_empty())
        .and_then(|id| tool_id.strip_prefix(id))
        .and_then(|rest| rest.strip_prefix('.'))
        .unwrap_or(tool_id)
}

/// Every policy the user has set, ordered by owner then tool. Only this
/// user's rows are read. Tool ids come back normalized (`normalize_tool_id`);
/// if two rows meet on one id, Block wins.
pub fn load_user_tool_policies(
    conn: &Connection,
    user_id: &str,
) -> rusqlite::Result<Vec<ToolPolicy>> {
    let mut stmt = conn.prepare(
        "SELECT owner_id, tool_id, policy, updated_at FROM tool_policies
         WHERE user_id = ?1 ORDER BY owner_id, tool_id",
    )?;
    let rows = stmt.query_map(params![user_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    let mut by_key: std::collections::BTreeMap<(String, String), ToolPolicy> =
        std::collections::BTreeMap::new();
    for row in rows {
        let (owner_id, tool_id, policy, updated_at) = row?;
        // The CHECK constraint keeps other values out; an `ask` row (none are
        // written) would mean the default, so it is not reported.
        let policy = match ToolPolicyValue::parse(&policy) {
            Some(ToolPolicyValue::Ask) | None => continue,
            Some(policy) => policy,
        };
        let tool_id = normalize_tool_id(&owner_id, &tool_id).to_string();
        let key = (owner_id.clone(), tool_id.clone());
        let keep_existing = by_key.get(&key).is_some_and(|existing| {
            existing.policy == ToolPolicyValue::Block && policy != ToolPolicyValue::Block
        });
        if !keep_existing {
            by_key.insert(
                key,
                ToolPolicy {
                    owner_id,
                    tool_id,
                    policy,
                    updated_at,
                },
            );
        }
    }
    Ok(by_key.into_values().collect())
}

/// One item of a `PUT` body.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolPolicyInput {
    pub owner_id: String,
    pub tool_id: String,
    /// Kept as text so an unknown value is a 400 naming the item.
    pub policy: String,
}

#[derive(Debug, Deserialize)]
struct PutToolPoliciesBody {
    policies: Vec<ToolPolicyInput>,
}

/// Why a `PUT` was refused. Nothing is written when any item is bad.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidToolPolicies {
    pub error: &'static str,
    pub message: String,
    pub index: Option<usize>,
}

fn is_valid_owner_id(owner_id: &str) -> bool {
    OWNER_PREFIXES.iter().any(|prefix| {
        owner_id.strip_prefix(prefix).is_some_and(|key| {
            (1..=MAX_OWNER_KEY_LEN).contains(&key.len())
                && key
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        })
    })
}

/// Check every item before anything is written.
pub fn validate_tool_policies(
    items: &[ToolPolicyInput],
) -> Result<Vec<(String, String, ToolPolicyValue)>, InvalidToolPolicies> {
    if items.len() > MAX_POLICIES_PER_PUT {
        return Err(InvalidToolPolicies {
            error: "too_many_policies",
            message: format!("Send at most {MAX_POLICIES_PER_PUT} policies at once."),
            index: None,
        });
    }
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            if !is_valid_owner_id(&item.owner_id) {
                return Err(InvalidToolPolicies {
                    error: "invalid_owner_id",
                    message: format!(
                        "\"{}\" is not a connection id (use connectors:<id> or mcps:<id>).",
                        item.owner_id
                    ),
                    index: Some(index),
                });
            }
            // Length is checked on the stored (normalized) form.
            let tool_id = normalize_tool_id(&item.owner_id, &item.tool_id);
            let tool_len = tool_id.chars().count();
            if !(1..=MAX_TOOL_ID_LEN).contains(&tool_len) {
                return Err(InvalidToolPolicies {
                    error: "invalid_tool_id",
                    message: format!("A tool id must be 1 to {MAX_TOOL_ID_LEN} characters."),
                    index: Some(index),
                });
            }
            let Some(policy) = ToolPolicyValue::parse(&item.policy) else {
                return Err(InvalidToolPolicies {
                    error: "invalid_policy",
                    message: format!(
                        "\"{}\" is not a policy (use allow, ask or block).",
                        item.policy
                    ),
                    index: Some(index),
                });
            };
            Ok((item.owner_id.clone(), tool_id.to_string(), policy))
        })
        .collect()
}

/// Apply validated items in one transaction: Allow and Block upsert with
/// `now` as `updated_at`, Ask deletes. Returns the user's full list.
pub fn apply_tool_policies(
    conn: &mut Connection,
    user_id: &str,
    items: &[(String, String, ToolPolicyValue)],
    now: &str,
) -> rusqlite::Result<Vec<ToolPolicy>> {
    let tx = conn.transaction()?;
    {
        let mut upsert = tx.prepare(
            "INSERT INTO tool_policies (user_id, owner_id, tool_id, policy, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (user_id, owner_id, tool_id)
             DO UPDATE SET policy = excluded.policy, updated_at = excluded.updated_at",
        )?;
        let mut delete = tx.prepare(
            "DELETE FROM tool_policies WHERE user_id = ?1 AND owner_id = ?2 AND tool_id = ?3",
        )?;
        for (owner_id, tool_id, policy) in items {
            match policy {
                ToolPolicyValue::Ask => {
                    delete.execute(params![user_id, owner_id, tool_id])?;
                }
                _ => {
                    upsert.execute(params![user_id, owner_id, tool_id, policy.as_str(), now])?;
                }
            }
        }
    }
    tx.commit()?;
    load_user_tool_policies(conn, user_id)
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn bad_request(error: &str, message: String, index: Option<usize>) -> Response {
    let mut body = json!({ "error": error, "message": message });
    if let Some(index) = index {
        body["index"] = json!(index);
    }
    (StatusCode::BAD_REQUEST, Json(body)).into_response()
}

fn internal_error(context: &str, e: impl std::fmt::Display) -> Response {
    warn!("{context}: {e}");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": "internal error" })),
    )
        .into_response()
}

// ─── GET /tool-policies ────────────────────────────────────────────────────

async fn get_tool_policies(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Response {
    let db = state.db.clone();
    let user_id = user.user_id;
    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        load_user_tool_policies(&conn, &user_id)
    })
    .await;
    match result {
        Ok(Ok(policies)) => Json(json!({ "policies": policies })).into_response(),
        Ok(Err(e)) => internal_error("DB error reading tool policies", e),
        Err(e) => internal_error("tool policies task panicked", e),
    }
}

// ─── PUT /tool-policies ────────────────────────────────────────────────────

async fn put_tool_policies(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    body: Result<Json<PutToolPoliciesBody>, JsonRejection>,
) -> Response {
    // A malformed body is a bad request too (not axum's 422), so every bad
    // item reads the same to the client.
    let Json(body) = match body {
        Ok(body) => body,
        Err(rejection) => return bad_request("invalid_body", rejection.body_text(), None),
    };
    let items = match validate_tool_policies(&body.policies) {
        Ok(items) => items,
        Err(e) => return bad_request(e.error, e.message, e.index),
    };
    let db = state.db.clone();
    let user_id = user.user_id;
    let result = tokio::task::spawn_blocking(move || {
        let mut conn = db.connect()?;
        apply_tool_policies(&mut conn, &user_id, &items, &now_rfc3339())
    })
    .await;
    match result {
        Ok(Ok(policies)) => Json(json!({ "policies": policies })).into_response(),
        Ok(Err(e)) => internal_error("DB error saving tool policies", e),
        Err(e) => internal_error("tool policies task panicked", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{header, Method, Request};
    use serde_json::Value;
    use tower::ServiceExt;

    fn user(id: &str) -> AuthUser {
        AuthUser {
            user_id: id.to_string(),
            email: None,
            name: None,
            avatar_url: None,
            tenant_id: None,
            organization_id: None,
            organization_role: None,
            organization_slug: None,
        }
    }

    async fn state() -> Arc<AppState> {
        let dir = std::env::temp_dir().join(format!(
            "allternit-tool-policies-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        crate::test_helpers::app_state(&dir).await
    }

    async fn call(
        state: &Arc<AppState>,
        as_user: &str,
        method: Method,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder()
            .method(method)
            .uri("/tool-policies")
            .header(header::CONTENT_TYPE, "application/json")
            .body(
                body.map(|b| Body::from(b.to_string()))
                    .unwrap_or_else(Body::empty),
            )
            .unwrap();
        req.extensions_mut().insert(user(as_user));
        let resp = tool_policy_router()
            .with_state(state.clone())
            .oneshot(req)
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn put(state: &Arc<AppState>, as_user: &str, policies: Value) -> (StatusCode, Value) {
        call(
            state,
            as_user,
            Method::PUT,
            Some(json!({ "policies": policies })),
        )
        .await
    }

    fn row_count(state: &Arc<AppState>) -> i64 {
        state
            .db
            .connect()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM tool_policies", [], |r| r.get(0))
            .unwrap()
    }

    fn k(owner: &str, tool: &str, policy: &str) -> (String, String, String) {
        (owner.to_string(), tool.to_string(), policy.to_string())
    }

    fn keys(list: &Value) -> Vec<(String, String, String)> {
        list["policies"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                (
                    p["owner_id"].as_str().unwrap().to_string(),
                    p["tool_id"].as_str().unwrap().to_string(),
                    p["policy"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    }

    #[tokio::test]
    async fn another_users_rows_are_never_returned() {
        let state = state().await;
        let (status, _) = put(
            &state,
            "u1",
            json!([{ "owner_id": "connectors:gmail", "tool_id": "send", "policy": "block" }]),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, body) = put(
            &state,
            "u2",
            json!([{ "owner_id": "mcps:github", "tool_id": "merge", "policy": "allow" }]),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        // The PUT reply is the caller's list only.
        assert_eq!(keys(&body), vec![k("mcps:github", "merge", "allow")]);

        let (status, body) = call(&state, "u2", Method::GET, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(keys(&body), vec![k("mcps:github", "merge", "allow")]);

        let (_, body) = call(&state, "u3", Method::GET, None).await;
        assert_eq!(body, json!({ "policies": [] }));

        let conn = state.db.connect().unwrap();
        let u1 = load_user_tool_policies(&conn, "u1").unwrap();
        assert_eq!(u1.len(), 1);
        assert_eq!(
            (u1[0].owner_id.as_str(), u1[0].policy),
            ("connectors:gmail", ToolPolicyValue::Block)
        );
    }

    #[tokio::test]
    async fn ask_deletes_the_row() {
        let state = state().await;
        put(
            &state,
            "u1",
            json!([
                { "owner_id": "connectors:gmail", "tool_id": "send", "policy": "block" },
                { "owner_id": "connectors:gmail", "tool_id": "search", "policy": "allow" },
            ]),
        )
        .await;
        assert_eq!(row_count(&state), 2);
        let (status, body) = put(
            &state,
            "u1",
            json!([{ "owner_id": "connectors:gmail", "tool_id": "send", "policy": "ask" }]),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(keys(&body), vec![k("connectors:gmail", "search", "allow")]);
        assert_eq!(row_count(&state), 1);
        // Ask on a tool with no row is fine and changes nothing.
        let (status, _) = put(
            &state,
            "u1",
            json!([{ "owner_id": "mcps:none", "tool_id": "x", "policy": "ask" }]),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(row_count(&state), 1);
    }

    #[tokio::test]
    async fn more_than_500_items_is_400_and_writes_nothing() {
        let state = state().await;
        let items = |n: usize| -> Value {
            (0..n)
                .map(|i| json!({ "owner_id": "connectors:gmail", "tool_id": format!("tool{i}"), "policy": "block" }))
                .collect()
        };
        let (status, body) = put(&state, "u1", items(501)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "too_many_policies");
        assert_eq!(row_count(&state), 0);

        let (status, body) = put(&state, "u1", items(500)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["policies"].as_array().unwrap().len(), 500);
    }

    #[tokio::test]
    async fn bad_owner_id_is_400_and_writes_nothing() {
        let state = state().await;
        let long = format!("connectors:{}", "a".repeat(129));
        for bad in [
            "gmail",
            "connectors:",
            "agents:gmail",
            "connectors:a/b",
            "mcps:a b",
            "CONNECTORS:x",
            long.as_str(),
        ] {
            let (status, body) = put(
                &state,
                "u1",
                json!([
                    { "owner_id": "connectors:gmail", "tool_id": "send", "policy": "block" },
                    { "owner_id": bad, "tool_id": "send", "policy": "block" },
                ]),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "owner_id {bad:?}");
            assert_eq!(body["error"], "invalid_owner_id");
            assert_eq!(body["index"], 1);
        }
        // The good item in each batch was not written either.
        assert_eq!(row_count(&state), 0);
        let max = format!("mcps:{}", "a".repeat(128));
        let (status, _) = put(
            &state,
            "u1",
            json!([{ "owner_id": max, "tool_id": "t", "policy": "allow" }]),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn bad_tool_id_policy_or_body_is_400() {
        let state = state().await;
        let cases = [
            json!([{ "owner_id": "connectors:gmail", "tool_id": "", "policy": "block" }]),
            json!([{ "owner_id": "connectors:gmail", "tool_id": "x".repeat(201), "policy": "block" }]),
            json!([{ "owner_id": "connectors:gmail", "tool_id": "send", "policy": "deny" }]),
            json!([{ "owner_id": "connectors:gmail", "tool_id": "send" }]),
        ];
        for case in cases {
            let (status, _) = put(&state, "u1", case.clone()).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{case}");
        }
        assert_eq!(row_count(&state), 0);
        // 200 characters (not bytes) is allowed.
        let (status, _) = put(&state, "u1", json!([{ "owner_id": "connectors:gmail", "tool_id": "é".repeat(200), "policy": "block" }])).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[test]
    fn upsert_updates_policy_and_updated_at() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../migrations/V296__tool_policies.sql"))
            .unwrap();
        let item = |policy| vec![("connectors:gmail".to_string(), "send".to_string(), policy)];

        let first = apply_tool_policies(
            &mut conn,
            "u1",
            &item(ToolPolicyValue::Allow),
            "2026-11-26T10:00:00.000Z",
        )
        .unwrap();
        assert_eq!(first[0].updated_at, "2026-11-26T10:00:00.000Z");

        let second = apply_tool_policies(
            &mut conn,
            "u1",
            &item(ToolPolicyValue::Block),
            "2026-11-26T11:00:00.000Z",
        )
        .unwrap();
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].policy, ToolPolicyValue::Block);
        assert_eq!(second[0].updated_at, "2026-11-26T11:00:00.000Z");

        // Same value again still moves updated_at (the device sync reads it).
        let third = apply_tool_policies(
            &mut conn,
            "u1",
            &item(ToolPolicyValue::Block),
            "2026-11-26T12:00:00.000Z",
        )
        .unwrap();
        assert_eq!(third[0].updated_at, "2026-11-26T12:00:00.000Z");
    }

    #[test]
    fn connector_tool_ids_drop_only_their_own_prefix() {
        assert_eq!(
            normalize_tool_id("connectors:linear", "linear.search_issues"),
            "search_issues"
        );
        assert_eq!(
            normalize_tool_id("connectors:linear", "search_issues"),
            "search_issues"
        );
        assert_eq!(
            normalize_tool_id("connectors:linear", "github.x"),
            "github.x"
        );
        assert_eq!(
            normalize_tool_id("connectors:linear", "linearx.search"),
            "linearx.search"
        );
        assert_eq!(normalize_tool_id("connectors:linear", "linear.a.b"), "a.b");
        assert_eq!(
            normalize_tool_id("mcps:linear", "linear.search_issues"),
            "linear.search_issues"
        );
        assert_eq!(
            normalize_tool_id("mcps:github", "github.merge"),
            "github.merge"
        );
    }

    #[tokio::test]
    async fn put_and_get_store_the_normalized_tool_id() {
        let state = state().await;
        let (status, body) = put(
            &state,
            "u1",
            json!([
                { "owner_id": "connectors:linear", "tool_id": "linear.search_issues", "policy": "block" },
                { "owner_id": "connectors:linear", "tool_id": "github.x", "policy": "allow" },
                { "owner_id": "mcps:linear", "tool_id": "linear.search_issues", "policy": "allow" },
            ]),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let expected = vec![
            k("connectors:linear", "github.x", "allow"),
            k("connectors:linear", "search_issues", "block"),
            k("mcps:linear", "linear.search_issues", "allow"),
        ];
        assert_eq!(keys(&body), expected);
        // The bare name from a phone is the same row: Ask clears it.
        let (_, body) = put(&state, "u1", json!([{ "owner_id": "connectors:linear", "tool_id": "search_issues", "policy": "ask" }])).await;
        assert_eq!(body["policies"].as_array().unwrap().len(), 2);
        // A prefix with nothing after it is not a tool id.
        let (status, body) = put(
            &state,
            "u1",
            json!([{ "owner_id": "connectors:linear", "tool_id": "linear.", "policy": "block" }]),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "invalid_tool_id");
    }

    #[test]
    fn load_normalizes_old_rows_and_block_wins_a_collision() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../migrations/V296__tool_policies.sql"))
            .unwrap();
        conn.execute_batch(
            "INSERT INTO tool_policies VALUES ('u1', 'connectors:linear', 'linear.search_issues', 'block', 't1');
             INSERT INTO tool_policies VALUES ('u1', 'connectors:linear', 'search_issues', 'allow', 't2');",
        )
        .unwrap();
        let got = load_user_tool_policies(&conn, "u1").unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(
            (got[0].tool_id.as_str(), got[0].policy),
            ("search_issues", ToolPolicyValue::Block)
        );
    }

    #[test]
    fn the_table_refuses_unknown_policies() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../migrations/V296__tool_policies.sql"))
            .unwrap();
        let err = conn.execute(
            "INSERT INTO tool_policies (user_id, owner_id, tool_id, policy, updated_at) VALUES ('u', 'mcps:a', 't', 'deny', 'now')",
            [],
        );
        assert!(err.is_err());
    }
}
