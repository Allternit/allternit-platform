//! Kernel UI backend (spec sections 1-6): agent rules, routing policy,
//! decision types, templates, activity. Mounted on the authed router like the
//! agency routes; adds no auth path. Storage is the node's sqlite DB, one JSON
//! document per scope (`migrations/V205__kernel_ui.sql`, also applied
//! idempotently on first use).

pub mod activity;
pub mod agent_rules;
pub mod decision_types;
pub mod routing_policy;
pub mod templates;
pub mod turn_route;

#[cfg(test)]
mod tests;

use crate::auth::AuthUser;
use crate::db::DbHandle;
use crate::AppState;
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
    Json, Router,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Map, Value};
use std::sync::Arc;

pub const SCHEMA: &str = include_str!("../../migrations/V205__kernel_ui.sql");

pub struct KErr(pub StatusCode, pub String);
impl KErr {
    pub fn bad(m: impl Into<String>) -> Self { Self(StatusCode::UNPROCESSABLE_ENTITY, m.into()) }
    pub fn forbidden(m: impl Into<String>) -> Self { Self(StatusCode::FORBIDDEN, m.into()) }
    pub fn not_found(m: impl Into<String>) -> Self { Self(StatusCode::NOT_FOUND, m.into()) }
    pub fn conflict(m: impl Into<String>) -> Self { Self(StatusCode::CONFLICT, m.into()) }
    pub fn internal(e: impl std::fmt::Display) -> Self {
        tracing::error!("kernel_ui: {e}");
        Self(StatusCode::INTERNAL_SERVER_ERROR, "internal error".into())
    }
}
impl IntoResponse for KErr {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": { "message": self.1 } }))).into_response()
    }
}
impl From<rusqlite::Error> for KErr {
    fn from(e: rusqlite::Error) -> Self { Self::internal(e) }
}
pub type KRes = Result<Json<Value>, KErr>;

/// Run `f` on a blocking thread with a connection whose schema exists.
pub async fn blocking<T: Send + 'static>(db: DbHandle, f: impl FnOnce(&Connection) -> Result<T, KErr> + Send + 'static) -> Result<T, KErr> {
    tokio::task::spawn_blocking(move || {
        let conn = db.connect().map_err(KErr::internal)?;
        conn.execute_batch(SCHEMA)?;
        f(&conn)
    })
    .await
    .map_err(KErr::internal)?
}

pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

// ── scopes ──────────────────────────────────────────────────────────────────

fn scope_kind(scope: &str) -> Result<&str, KErr> {
    let (kind, id) = scope.split_once(':').ok_or_else(|| KErr::bad("scope must be org:<id>, workspace:<id> or project:<id>"))?;
    let ok_id = !id.is_empty() && id.len() <= 128 && id.chars().all(|c| c.is_ascii_alphanumeric() || "_-.:@".contains(c));
    if !matches!(kind, "org" | "workspace" | "project") || !ok_id {
        return Err(KErr::bad("scope must be org:<id>, workspace:<id> or project:<id>"));
    }
    Ok(kind)
}

/// An org scope is only readable/writable by that org's members.
pub fn authorize_scope(scope: &str, user: &AuthUser) -> Result<(), KErr> {
    let kind = scope_kind(scope)?;
    if kind == "org" {
        let id = &scope[4..];
        if user.organization_id.as_deref().is_some_and(|o| !o.is_empty() && o != id) {
            return Err(KErr::forbidden("not a member of that organization"));
        }
    }
    Ok(())
}

/// Inheritance chain, most specific first. `parents` (comma list, most
/// specific first) wins; otherwise a non-org scope inherits the caller's org.
pub fn chain(scope: &str, user: &AuthUser, parents: Option<&str>) -> Result<Vec<String>, KErr> {
    authorize_scope(scope, user)?;
    let mut out = vec![scope.to_string()];
    match parents.filter(|p| !p.is_empty()) {
        Some(p) => {
            for s in p.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                authorize_scope(s, user)?;
                if !out.iter().any(|x| x == s) {
                    out.push(s.to_string());
                }
            }
        }
        None => {
            if scope_kind(scope)? != "org" {
                if let Some(o) = user.organization_id.as_deref().filter(|o| !o.is_empty()) {
                    out.push(format!("org:{o}"));
                }
            }
        }
    }
    Ok(out)
}

// ── generic override documents (flat {path: value}) ─────────────────────────

pub fn get_path<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(v, |c, k| c.get(k))
}

pub fn set_path(v: &mut Value, path: &str, val: Value) {
    let mut cur = v;
    let parts: Vec<&str> = path.split('.').collect();
    for (i, k) in parts.iter().enumerate() {
        if !cur.is_object() {
            *cur = json!({});
        }
        let o = cur.as_object_mut().expect("object");
        if i + 1 == parts.len() {
            o.insert((*k).to_string(), val);
            return;
        }
        cur = o.entry((*k).to_string()).or_insert_with(|| json!({}));
    }
}

/// Partial nested body -> flat override map. Unknown keys are refused.
pub fn flatten(body: &Value, leaves: &[&str]) -> Result<Map<String, Value>, KErr> {
    let obj = body.as_object().ok_or_else(|| KErr::bad("body must be a JSON object"))?;
    let mut out = Map::new();
    for (top, val) in obj {
        if leaves.contains(&top.as_str()) {
            out.insert(top.clone(), val.clone());
        } else if let Some(inner) = val.as_object().filter(|_| leaves.iter().any(|l| l.starts_with(&format!("{top}.")))) {
            for (k, v) in inner {
                let p = format!("{top}.{k}");
                if !leaves.contains(&p.as_str()) {
                    return Err(KErr::bad(format!("unknown field {p}")));
                }
                out.insert(p, v.clone());
            }
        } else {
            return Err(KErr::bad(format!("unknown field {top}")));
        }
    }
    Ok(out)
}

pub fn load_doc(conn: &Connection, kind: &str, scope: &str) -> Result<Map<String, Value>, KErr> {
    let raw: Option<String> = conn
        .query_row("SELECT doc_json FROM kernel_ui_docs WHERE kind = ?1 AND scope = ?2", params![kind, scope], |r| r.get(0))
        .optional()?;
    Ok(raw.and_then(|s| serde_json::from_str::<Value>(&s).ok()).and_then(|v| v.as_object().cloned()).unwrap_or_default())
}

pub fn save_doc(conn: &Connection, kind: &str, scope: &str, doc: &Map<String, Value>) -> Result<(), KErr> {
    if doc.is_empty() {
        conn.execute("DELETE FROM kernel_ui_docs WHERE kind = ?1 AND scope = ?2", params![kind, scope])?;
    } else {
        conn.execute(
            "INSERT INTO kernel_ui_docs (kind, scope, doc_json, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(kind, scope) DO UPDATE SET doc_json = excluded.doc_json, updated_at = excluded.updated_at",
            params![kind, scope, Value::Object(doc.clone()).to_string(), now()],
        )?;
    }
    Ok(())
}

/// Effective value = most specific ?? ... ?? default, with a source per field.
pub fn effective(conn: &Connection, kind: &str, defaults: &Value, leaves: &[&str], chain: &[String]) -> Result<(Value, Value), KErr> {
    let mut eff = defaults.clone();
    let mut sources = Map::new();
    for l in leaves {
        sources.insert((*l).to_string(), json!("default"));
    }
    for scope in chain.iter().rev() {
        for (path, val) in load_doc(conn, kind, scope)? {
            set_path(&mut eff, &path, val);
            sources.insert(path, json!(scope.split(':').next().unwrap_or("default")));
        }
    }
    Ok((eff, Value::Object(sources)))
}

/// Kernel UI routes under `prefix`. They live under a `/kernel` segment so
/// they can never overlap existing app routes (a DELETE /api/v1/templates/:id
/// already exists elsewhere; the overlap panicked prod at startup).
fn routes(prefix: &str) -> Router<Arc<AppState>> {
    Router::new()
        .route(&format!("{prefix}/agent-rules"), get(agent_rules::get_rules).put(agent_rules::put_rules))
        .route(&format!("{prefix}/agent-rules/field"), delete(agent_rules::delete_field))
        .route(&format!("{prefix}/routing-policy"), get(routing_policy::get_policy).put(routing_policy::put_policy))
        .route(&format!("{prefix}/routing-policy/field"), delete(routing_policy::delete_field))
        .route(&format!("{prefix}/routing-policy/backends"), get(routing_policy::backends))
        .route(&format!("{prefix}/decision-types"), get(decision_types::list).post(decision_types::create))
        .route(&format!("{prefix}/decision-types/:id/status"), put(decision_types::set_status))
        .route(&format!("{prefix}/templates"), get(templates::list).post(templates::create))
        .route(&format!("{prefix}/templates/:id"), put(templates::update).delete(templates::remove))
        .route(&format!("{prefix}/templates/:id/run"), post(templates::run))
        .route(&format!("{prefix}/activity"), get(activity::list))
        .route(&format!("{prefix}/turn-route"), post(turn_route::turn_route))
}

pub fn router() -> Router<Arc<AppState>> {
    routes("/v1/kernel").merge(crate::agency_api::kernel_alias_router())
}

/// The same routes under `/api/v1/kernel/*` (what the UI's `runtimeApiUrl`
/// reaches through the paired-runtime relay). Mount this next to [`router`].
pub fn api_router() -> Router<Arc<AppState>> {
    routes("/api/v1/kernel").merge(Router::new().nest("/api", crate::agency_api::kernel_alias_router()))
}
