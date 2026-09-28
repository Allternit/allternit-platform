//! Spend limits for bots and threads (spec P8.1). The limit lives on the bot
//! (`agents.config.spendLimit.monthlyUsd`) and is enforced by gizzi, where
//! every turn runs: over budget, a session pauses until the 1st (bot) or
//! until the limit is raised (thread) instead of running up a bill.

use std::sync::Arc;

use axum::{
    extract::{Extension, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::warn;

use crate::{auth::AuthUser, db::DbHandle, AppState};

pub fn spend_limit_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/agents/:id/spend", get(get_bot_spend).put(set_bot_limit))
        .route("/threads/:id/budget", get(get_thread_budget).put(set_thread_budget))
}

async fn gizzi_budget(method: reqwest::Method, scope: &str, id: &str, body: Option<Value>) -> Result<Value, String> {
    let url = format!(
        "{}/v1/session/budget/{scope}/{}",
        crate::agent_session_routes::gizzi_base(),
        urlencoding::encode(id)
    );
    let client = crate::agent_session_routes::gizzi_client(&HeaderMap::new());
    let mut req = client.request(method, url);
    if let Some(b) = body {
        req = req.json(&b);
    }
    let res = req.send().await.map_err(|e| format!("gizzi unreachable: {e}"))?;
    if !res.status().is_success() {
        return Err(format!("gizzi refused ({})", res.status()));
    }
    res.json::<Value>().await.map_err(|e| e.to_string())
}

/// Push a bot's monthly limit to gizzi (null clears it).
pub async fn push_bot_limit(bot_id: &str, monthly_usd: Option<f64>) -> Result<Value, String> {
    gizzi_budget(reqwest::Method::PUT, "agent", bot_id, Some(json!({ "limitUsd": monthly_usd }))).await
}

fn owns(db: &DbHandle, user_id: &str, bot_id: &str) -> bool {
    db.connect()
        .ok()
        .and_then(|c| {
            c.query_row("SELECT 1 FROM agents WHERE id = ?1 AND user_id = ?2", params![bot_id, user_id], |_| Ok(()))
                .optional()
                .ok()
                .flatten()
        })
        .is_some()
}

async fn get_bot_spend(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(id): Path<String>) -> Response {
    if !owns(&state.db, &user.user_id, &id) {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "bot not found" }))).into_response();
    }
    match gizzi_budget(reqwest::Method::GET, "agent", &id, None).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({ "error": e }))).into_response(),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BotLimitBody {
    monthly_usd: Option<f64>,
}

async fn set_bot_limit(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<BotLimitBody>,
) -> Response {
    if body.monthly_usd.map_or(false, |v| !(v >= 0.0)) {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "monthlyUsd must be 0 or more" }))).into_response();
    }
    let updated = state.db.connect().ok().and_then(|c| {
        c.execute(
            "UPDATE agents SET config = CASE WHEN ?3 IS NULL
                 THEN json_remove(COALESCE(config, '{}'), '$.spendLimit')
                 ELSE json_set(COALESCE(config, '{}'), '$.spendLimit', json_object('monthlyUsd', ?3)) END
             WHERE id = ?1 AND user_id = ?2",
            params![id, user.user_id, body.monthly_usd],
        )
        .ok()
    });
    if updated != Some(1) {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "bot not found" }))).into_response();
    }
    match push_bot_limit(&id, body.monthly_usd).await {
        Ok(v) => Json(v).into_response(),
        // Saved on the bot; gizzi picks it up at the next sync.
        Err(e) => Json(json!({ "limitUsd": body.monthly_usd, "pending": e })).into_response(),
    }
}

/// The thread's first window: its budget covers every window after it.
fn thread_root_session(db: &DbHandle, user_id: &str, thread_id: &str) -> Option<String> {
    db.connect().ok().and_then(|c| {
        c.query_row(
            "SELECT s.session_id FROM bot_thread_sessions s JOIN bot_threads t ON t.id = s.thread_id
             WHERE s.thread_id = ?1 AND t.user_id = ?2 ORDER BY s.generation ASC LIMIT 1",
            params![thread_id, user_id],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()
    })
}

async fn get_thread_budget(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(id): Path<String>) -> Response {
    let Some(root) = thread_root_session(&state.db, &user.user_id, &id) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "thread not found" }))).into_response();
    };
    match gizzi_budget(reqwest::Method::GET, "session", &root, None).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({ "error": e }))).into_response(),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadBudgetBody {
    usd: Option<f64>,
}

async fn set_thread_budget(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<ThreadBudgetBody>,
) -> Response {
    let Some(root) = thread_root_session(&state.db, &user.user_id, &id) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "thread not found" }))).into_response();
    };
    match set_thread_budget_for(&root, body.usd).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({ "error": e }))).into_response(),
    }
}

pub async fn set_thread_budget_for(root_session: &str, usd: Option<f64>) -> Result<Value, String> {
    gizzi_budget(reqwest::Method::PUT, "session", root_session, Some(json!({ "limitUsd": usd }))).await
}

/// gizzi keeps limits in its own database; bring it up to date after a
/// restart of either side (retries until gizzi answers).
pub fn spawn_sync(state: Arc<AppState>) {
    tokio::spawn(async move {
        for attempt in 0..30u64 {
            let limits: Vec<(String, f64)> = state
                .db
                .connect()
                .ok()
                .and_then(|c| {
                    c.prepare("SELECT id, json_extract(config, '$.spendLimit.monthlyUsd') FROM agents WHERE json_extract(config, '$.spendLimit.monthlyUsd') IS NOT NULL")
                        .and_then(|mut st| st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map(|rows| rows.filter_map(Result::ok).collect()))
                        .ok()
                })
                .unwrap_or_default();
            let mut ok = true;
            for (bot, usd) in &limits {
                if let Err(e) = push_bot_limit(bot, Some(*usd)).await {
                    ok = false;
                    if attempt == 29 {
                        warn!(bot = %bot, error = %e, "spend limit sync gave up");
                    }
                    break;
                }
            }
            if ok {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_secs(10 + attempt * 5)).await;
        }
    });
}
