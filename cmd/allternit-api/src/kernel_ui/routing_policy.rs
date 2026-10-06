//! Section 2: models and tiers. Stored per scope; changing `s1_backend`
//! returns every decision type to shadow. `s2`/`s3`/`retrieval`/`local_only`
//! are enforced by the agency executor (`executor::apply_policy`): class
//! preferences, escalation order, local_only (fail closed) and the S1 backend.
//! `retrieval` is recorded in the routing trace only: the context compiler
//! takes no model yet.

use super::*;
use axum::extract::{Query, State};
use axum::Extension;
use serde::Deserialize;

pub const LEAVES: &[&str] = &["s1_backend", "s2.default", "s2.overrides", "s3.solver", "s3.fallback", "retrieval", "local_only"];
pub const S1_BACKENDS: &[&str] = &["laya_bundled", "system_one_local", "jev_api", "off"];

pub fn defaults() -> Value {
    json!({ "s1_backend": "off", "s2": { "default": "", "overrides": {} }, "s3": { "solver": "", "fallback": "" }, "retrieval": "", "local_only": false })
}

pub fn jev_available() -> bool {
    std::env::var("TYPESAFE_API_KEY").is_ok_and(|v| !v.trim().is_empty())
}

fn s1_available(id: &str) -> bool {
    match id {
        "off" | "laya_bundled" => true,
        "system_one_local" => allternit_factory_engine::kernel::s1_outcome::OutcomeReporter::from_env().enabled,
        "jev_api" => jev_available(),
        _ => false,
    }
}

fn validate(path: &str, v: &Value) -> Result<(), KErr> {
    let short = |v: &Value| v.as_str().is_some_and(|s| s.len() <= 200);
    let ok = match path {
        "s1_backend" => match v.as_str() {
            Some(b) if S1_BACKENDS.contains(&b) => {
                if !s1_available(b) {
                    return Err(KErr::conflict(format!("s1 backend {b} is not available on this server")));
                }
                true
            }
            _ => false,
        },
        "s2.default" | "s3.solver" | "s3.fallback" | "retrieval" => short(v),
        "s2.overrides" => v.as_object().is_some_and(|m| m.len() <= 100 && m.iter().all(|(k, x)| !k.is_empty() && k.len() <= 100 && short(x))),
        "local_only" => v.is_boolean(),
        _ => false,
    };
    if ok { Ok(()) } else { Err(KErr::bad(format!("invalid value for {path}"))) }
}

#[derive(Deserialize)]
pub struct Q {
    /// Defaults to the caller's organization, like the decision-types routes.
    scope: Option<String>,
    parents: Option<String>,
    path: Option<String>,
}

impl Q {
    fn scope(&self, u: &AuthUser) -> String {
        self.scope.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| {
            format!("org:{}", u.organization_id.clone().filter(|o| !o.is_empty()).unwrap_or_else(|| "default".into()))
        })
    }
}

fn view(conn: &Connection, scope: &str, chain: &[String]) -> Result<Value, KErr> {
    let (eff, sources) = effective(conn, "routing", &defaults(), LEAVES, chain)?;
    let mut out = eff;
    out["scope"] = json!(scope);
    out["sources"] = sources;
    Ok(out)
}

fn reset_decisions(conn: &Connection) -> Result<(), KErr> {
    conn.execute("DELETE FROM kernel_ui_decision_status", [])?;
    Ok(())
}

pub async fn get_policy(State(st): State<Arc<AppState>>, Extension(u): Extension<AuthUser>, Query(q): Query<Q>) -> KRes {
    let scope = q.scope(&u);
    let ch = chain(&scope, &u, q.parents.as_deref())?;
    Ok(Json(blocking(st.db.clone(), move |c| view(c, &scope, &ch)).await?))
}

pub async fn put_policy(State(st): State<Arc<AppState>>, Extension(u): Extension<AuthUser>, Query(q): Query<Q>, Json(body): Json<Value>) -> KRes {
    let scope = q.scope(&u);
    let ch = chain(&scope, &u, q.parents.as_deref())?;
    let patch = flatten(&body, LEAVES)?;
    for (p, v) in &patch {
        validate(p, v)?;
    }
    Ok(Json(blocking(st.db.clone(), move |c| {
        let before = effective(c, "routing", &defaults(), LEAVES, &ch)?.0["s1_backend"].clone();
        let mut doc = load_doc(c, "routing", &scope)?;
        doc.extend(patch);
        save_doc(c, "routing", &scope, &doc)?;
        let mut out = view(c, &scope, &ch)?;
        let changed = out["s1_backend"] != before;
        if changed {
            reset_decisions(c)?;
        }
        out["decision_types_reset"] = json!(changed);
        Ok(out)
    }).await?))
}

pub async fn delete_field(State(st): State<Arc<AppState>>, Extension(u): Extension<AuthUser>, Query(q): Query<Q>) -> KRes {
    let scope = q.scope(&u);
    let ch = chain(&scope, &u, q.parents.as_deref())?;
    let path = q.path.clone().unwrap_or_default();
    if !LEAVES.contains(&path.as_str()) {
        return Err(KErr::bad("path must be a routing-policy field path"));
    }
    Ok(Json(blocking(st.db.clone(), move |c| {
        let before = effective(c, "routing", &defaults(), LEAVES, &ch)?.0["s1_backend"].clone();
        let mut doc = load_doc(c, "routing", &scope)?;
        doc.remove(&path);
        save_doc(c, "routing", &scope, &doc)?;
        let mut out = view(c, &scope, &ch)?;
        let changed = out["s1_backend"] != before;
        if changed {
            reset_decisions(c)?;
        }
        out["decision_types_reset"] = json!(changed);
        Ok(out)
    }).await?))
}

/// Reduce gizzi's `/model-pool` body to the fields the UI needs.
pub fn summarize_pool(body: &Value) -> Vec<Value> {
    let entries = body.get("entries").and_then(Value::as_array).or_else(|| body.as_array()).cloned().unwrap_or_default();
    entries.iter().map(|e| json!({
        "backend_id": e["backend_id"], "roles": e["cognitive_roles"], "modes": e["modes"],
        "capabilities": e["capabilities"], "residency": e["residency"],
    })).collect()
}

async fn fetch_pool() -> Result<Vec<Value>, String> {
    let url = format!("{}/model-pool", super::super::agency_api::executor::gizzi_url().trim_end_matches('/'));
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(3)).build().map_err(|e| e.to_string())?;
    let resp = client.get(&url).headers(crate::gizzi_provider_auth::gizzi_auth_headers()).send().await.map_err(|e| format!("model pool unreachable: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("model pool returned {}", resp.status()));
    }
    let body: Value = resp.json().await.map_err(|e| e.to_string())?;
    Ok(summarize_pool(&body))
}

async fn reachable(url: String) -> bool {
    let Ok(c) = reqwest::Client::builder().timeout(std::time::Duration::from_millis(800)).build() else { return false };
    c.get(url).send().await.is_ok_and(|r| r.status().is_success())
}

pub async fn backends() -> KRes {
    let (models, pool_error) = match fetch_pool().await {
        Ok(m) => (m, Value::Null),
        Err(e) => (vec![], json!(e)),
    };
    // Decisions go through the local S1 server; Laya also needs its own server.
    let s1_url = allternit_factory_engine::kernel::s1_outcome::OutcomeReporter::from_env().base_url;
    let laya_url = std::env::var("SYSTEM_ONE_LAYA_URL").ok().filter(|u| !u.trim().is_empty())
        .unwrap_or_else(|| "http://127.0.0.1:7718".into());
    let (s1_up, laya_up) = tokio::join!(reachable(format!("{s1_url}/healthz")), reachable(format!("{}/health", laya_url.trim_end_matches('/'))));
    let s1: Vec<Value> = S1_BACKENDS.iter().map(|b| {
        let reason = match *b {
            "jev_api" if !jev_available() => Some("TYPESAFE_API_KEY is not set"),
            "laya_bundled" | "system_one_local" | "jev_api" if !s1_up => Some("the System One server is not running (tools/system-one-local: bun src/cli.ts serve)"),
            "laya_bundled" if !laya_up => Some("Laya is not running (tools/system-one-local/laya/serve-laya.sh)"),
            _ => None,
        };
        json!({ "id": b, "available": reason.is_none(), "reason": reason })
    }).collect();
    Ok(Json(json!({ "s1": s1, "models": models, "pool_error": pool_error })))
}

/// Effective routing policy for a run: the chain is most specific first
/// (project, workspace, org). Returns (effective, per-field source scope kind).
pub fn resolve(db: &crate::db::DbHandle, chain: &[String]) -> Result<(Value, Value), KErr> {
    let conn = db.connect().map_err(KErr::internal)?;
    conn.execute_batch(super::SCHEMA)?;
    effective(&conn, "routing", &defaults(), LEAVES, chain)
}

/// Current S1 backend for a chain (used by decision types).
pub fn current_s1(conn: &Connection, chain: &[String]) -> Result<String, KErr> {
    Ok(effective(conn, "routing", &defaults(), LEAVES, chain)?.0["s1_backend"].as_str().unwrap_or("off").to_string())
}
