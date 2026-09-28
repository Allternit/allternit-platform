//! Saved bot and team templates ("Save as template", Bot Mode Templates →
//! Mine / Org). A template is a recipe: instructions, tools, skills,
//! routines (installed off), and for teams the plan. It never carries
//! credentials, memory or threads; the app strips those, and this route strips
//! anything secret-looking again before it's stored.
//!
//! GET    /templates        your templates, newest first
//! POST   /templates        {kind: bot|team, visibility: private|org, forkedFrom?, template}
//! DELETE /templates/:id

use std::sync::Arc;

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get},
    Json, Router,
};
use rusqlite::{params, Connection};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{auth::AuthUser, AppState};

/// Largest template body accepted (a team with its bots' instructions fits well under this).
const MAX_TEMPLATE_BYTES: usize = 256 * 1024;

pub fn templates_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/templates", get(list_templates).post(create_template))
        .route("/templates/:id", delete(delete_template))
}

#[derive(Debug, Deserialize)]
struct CreateBody {
    kind: String,
    #[serde(default)]
    visibility: Option<String>,
    #[serde(default, rename = "forkedFrom")]
    forked_from: Option<String>,
    template: Value,
}

/// Keys that must never be stored in a template, at any depth.
fn is_secret_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase().replace(['_', '-'], "");
    ["secret", "password", "apikey", "token", "credential", "privatekey", "vaultref", "authorization", "cookie"]
        .iter()
        .any(|s| k.contains(s))
}

/// Remove secret-looking fields everywhere in the template.
pub fn strip_secrets(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.retain(|k, _| !is_secret_key(k));
            for v in map.values_mut() {
                strip_secrets(v);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(strip_secrets),
        _ => {}
    }
}

/// Check a body and return (kind, visibility, forkedFrom, cleaned template).
fn validate(body: CreateBody) -> Result<(String, String, Option<String>, Value), String> {
    let kind = body.kind.trim().to_string();
    if kind != "bot" && kind != "team" {
        return Err("kind must be \"bot\" or \"team\"".into());
    }
    let visibility = body.visibility.unwrap_or_else(|| "private".into());
    if visibility != "private" && visibility != "org" {
        return Err("visibility must be \"private\" or \"org\"".into());
    }
    let mut template = body.template;
    if !template.is_object() {
        return Err("template must be an object".into());
    }
    let shape_ok = if kind == "bot" {
        template.get("id").and_then(Value::as_str).is_some() && template.get("name").and_then(Value::as_str).is_some()
    } else {
        template.get("team").and_then(|t| t.get("id")).and_then(Value::as_str).is_some()
            && template.get("bots").map(Value::is_array).unwrap_or(false)
    };
    if !shape_ok {
        return Err(if kind == "bot" { "a bot template needs an id and a name".into() } else { "a team template needs {team, bots}".into() });
    }
    strip_secrets(&mut template);
    if serde_json::to_vec(&template).map(|b| b.len()).unwrap_or(usize::MAX) > MAX_TEMPLATE_BYTES {
        return Err("template is too large".into());
    }
    let forked_from = body.forked_from.map(|s| s.trim().chars().take(200).collect::<String>()).filter(|s| !s.is_empty());
    Ok((kind, visibility, forked_from, template))
}

fn row_json(id: String, kind: String, visibility: String, forked_from: Option<String>, template: String, created_at: String) -> Value {
    json!({
        "id": id,
        "kind": kind,
        "visibility": visibility,
        "forkedFrom": forked_from,
        "createdAt": created_at,
        "template": serde_json::from_str::<Value>(&template).unwrap_or(Value::Null),
    })
}

pub fn list_rows(conn: &Connection, user_id: &str) -> rusqlite::Result<Vec<Value>> {
    let mut stmt = conn.prepare(
        "SELECT id, kind, visibility, forked_from, template, created_at FROM bot_templates
         WHERE user_id = ?1 ORDER BY created_at DESC, rowid DESC",
    )?;
    let rows = stmt.query_map(params![user_id], |r| {
        Ok(row_json(r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
    })?;
    rows.collect()
}

pub fn insert_row(
    conn: &Connection,
    user_id: &str,
    kind: &str,
    visibility: &str,
    forked_from: Option<&str>,
    template: &Value,
) -> rusqlite::Result<Value> {
    let id = format!("tpl_{}", uuid::Uuid::new_v4().simple());
    let created_at = chrono::Utc::now().to_rfc3339();
    let body = template.to_string();
    conn.execute(
        "INSERT INTO bot_templates (id, user_id, kind, visibility, forked_from, template, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![id, user_id, kind, visibility, forked_from, body, created_at],
    )?;
    Ok(row_json(id, kind.into(), visibility.into(), forked_from.map(str::to_string), body, created_at))
}

pub fn delete_row(conn: &Connection, user_id: &str, id: &str) -> rusqlite::Result<bool> {
    Ok(conn.execute("DELETE FROM bot_templates WHERE id = ?1 AND user_id = ?2", params![id, user_id])? > 0)
}

fn db_error(e: impl std::fmt::Display) -> Response {
    tracing::warn!("templates db error: {e}");
    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "internal error" }))).into_response()
}

async fn list_templates(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> Response {
    let db = state.db.clone();
    match tokio::task::spawn_blocking(move || list_rows(&db.connect()?, &user.user_id)).await {
        Ok(Ok(templates)) => Json(json!({ "templates": templates })).into_response(),
        Ok(Err(e)) => db_error(e),
        Err(e) => db_error(e),
    }
}

async fn create_template(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<CreateBody>,
) -> Response {
    let (kind, visibility, forked_from, template) = match validate(body) {
        Ok(v) => v,
        Err(msg) => return (StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))).into_response(),
    };
    let db = state.db.clone();
    let result = tokio::task::spawn_blocking(move || {
        insert_row(&db.connect()?, &user.user_id, &kind, &visibility, forked_from.as_deref(), &template)
    })
    .await;
    match result {
        Ok(Ok(row)) => (StatusCode::CREATED, Json(json!({ "template": row }))).into_response(),
        Ok(Err(e)) => db_error(e),
        Err(e) => db_error(e),
    }
}

async fn delete_template(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Response {
    let db = state.db.clone();
    match tokio::task::spawn_blocking(move || delete_row(&db.connect()?, &user.user_id, &id)).await {
        Ok(Ok(true)) => Json(json!({ "success": true })).into_response(),
        Ok(Ok(false)) => (StatusCode::NOT_FOUND, Json(json!({ "error": "template not found" }))).into_response(),
        Ok(Err(e)) => db_error(e),
        Err(e) => db_error(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("../migrations/V194__bot_templates.sql")).unwrap();
        c
    }

    fn body(v: Value) -> CreateBody {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn secrets_are_stripped_at_any_depth() {
        let mut t = json!({
            "id": "b1", "name": "Bookkeeper",
            "config": { "apiKey": "sk-1", "nested": [{ "access_token": "x", "keep": 1 }] },
            "connections": [{ "provider": "stripe", "vaultRef": "vault://x", "label": "Stripe" }],
            "tokenBudget": 5
        });
        strip_secrets(&mut t);
        assert!(t["config"].get("apiKey").is_none());
        assert!(t["config"]["nested"][0].get("access_token").is_none());
        assert_eq!(t["config"]["nested"][0]["keep"], 1);
        assert!(t["connections"][0].get("vaultRef").is_none());
        assert_eq!(t["connections"][0]["label"], "Stripe");
        // "tokenBudget" is secret-looking by name; dropping it is the safe side.
        assert!(t.get("tokenBudget").is_none());
    }

    #[test]
    fn validates_kind_visibility_and_shape() {
        assert!(validate(body(json!({ "kind": "bot", "template": { "id": "b", "name": "B" } }))).is_ok());
        assert!(validate(body(json!({ "kind": "team", "template": { "team": { "id": "t" }, "bots": [] } }))).is_ok());
        assert!(validate(body(json!({ "kind": "robot", "template": {} }))).is_err());
        assert!(validate(body(json!({ "kind": "bot", "visibility": "public", "template": { "id": "b", "name": "B" } }))).is_err());
        assert!(validate(body(json!({ "kind": "bot", "template": { "name": "no id" } }))).is_err());
        assert!(validate(body(json!({ "kind": "team", "template": { "team": { "id": "t" } } }))).is_err());
    }

    #[test]
    fn stores_lists_and_deletes_per_user() {
        let c = conn();
        let a = insert_row(&c, "u1", "bot", "private", None, &json!({ "id": "b", "name": "Scout" })).unwrap();
        insert_row(&c, "u1", "team", "org", Some("Allternit · Content Engine"), &json!({ "team": { "id": "t" }, "bots": [] })).unwrap();
        insert_row(&c, "u2", "bot", "private", None, &json!({ "id": "x", "name": "Other" })).unwrap();

        let mine = list_rows(&c, "u1").unwrap();
        assert_eq!(mine.len(), 2);
        assert!(mine.iter().any(|r| r["forkedFrom"] == "Allternit · Content Engine" && r["visibility"] == "org"));
        assert_eq!(mine.iter().find(|r| r["kind"] == "bot").unwrap()["template"]["name"], "Scout");

        let id = a["id"].as_str().unwrap();
        assert!(!delete_row(&c, "u2", id).unwrap(), "can't delete someone else's");
        assert!(delete_row(&c, "u1", id).unwrap());
        assert_eq!(list_rows(&c, "u1").unwrap().len(), 1);
    }
}
