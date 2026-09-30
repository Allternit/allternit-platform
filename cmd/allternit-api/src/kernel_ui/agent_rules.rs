//! Section 1: agent rules. Flat per-scope override documents; effective value
//! = project ?? workspace ?? org ?? default.
//!
//! Enforcement: `guardrails.allow_credential_read` is enforced for Agency API
//! runs (merged into the run's JudgePolicy at creation, see
//! [`apply_to_policy`]). Everything else is stored, not yet enforced; the GET
//! response says so per field under `enforcement`.

use super::*;
use axum::extract::{Query, State};
use axum::Extension;
use serde::Deserialize;

pub const LEAVES: &[&str] = &[
    "guardrails.level", "guardrails.allow_credential_read",
    "approvals.outside_scope", "approvals.private_network", "approvals.spend_over_usd",
    "budgets.daily_usd", "budgets.max_concurrent",
    "completion.require_verifier", "custom",
];

pub fn defaults() -> Value {
    json!({
        "guardrails": { "level": "guardrails", "allow_credential_read": [] },
        "approvals": { "outside_scope": true, "private_network": true, "spend_over_usd": null },
        "budgets": { "daily_usd": null, "max_concurrent": null },
        "completion": { "require_verifier": true, "agent_may_self_close": false },
        "custom": [],
    })
}

fn enforcement() -> Value {
    let mut m = Map::new();
    for l in LEAVES {
        m.insert((*l).to_string(), json!("stored, not yet enforced"));
    }
    m.insert("guardrails.allow_credential_read".into(), json!("enforced (Agency API runs: JudgePolicy.allow_credential_read)"));
    m.insert("guardrails.level".into(), json!("stored, not yet enforced (Agency API runs are always strict)"));
    m.insert("completion.require_verifier".into(), json!("stored, not yet enforced (Agency API runs are always verifier-closed)"));
    m.insert("completion.agent_may_self_close".into(), json!("locked: false"));
    Value::Object(m)
}

fn num_or_null(v: &Value) -> bool {
    v.is_null() || v.as_f64().is_some_and(|n| n >= 0.0 && n.is_finite())
}

pub fn validate(path: &str, v: &Value) -> Result<(), KErr> {
    let ok = match path {
        "guardrails.level" => matches!(v.as_str(), Some("off" | "guardrails" | "strict")),
        "guardrails.allow_credential_read" => v.as_array().is_some_and(|a| a.len() <= 64 && a.iter().all(|s| s.as_str().is_some_and(|s| !s.is_empty() && s.len() <= 512))),
        "approvals.outside_scope" | "approvals.private_network" | "completion.require_verifier" => v.is_boolean(),
        "approvals.spend_over_usd" | "budgets.daily_usd" | "budgets.max_concurrent" => num_or_null(v),
        "custom" => v.as_array().is_some_and(|a| a.len() <= 100 && a.iter().all(|r| {
            let s = |k: &str| r.get(k).and_then(Value::as_str).is_some_and(|s| !s.trim().is_empty() && s.len() <= 1000);
            s("id") && s("text") && s("when") && matches!(r["action"].as_str(), Some("ask" | "deny"))
        })),
        _ => false,
    };
    if ok { Ok(()) } else { Err(KErr::bad(format!("invalid value for {path}"))) }
}

#[derive(Deserialize)]
pub struct Q {
    scope: String,
    parents: Option<String>,
    path: Option<String>,
}

fn view(conn: &Connection, scope: &str, chain: &[String]) -> Result<Value, KErr> {
    let (eff, sources) = effective(conn, "rules", &defaults(), LEAVES, chain)?;
    Ok(json!({ "scope": scope, "effective": eff, "sources": sources, "enforcement": enforcement() }))
}

pub async fn get_rules(State(st): State<Arc<AppState>>, Extension(u): Extension<AuthUser>, Query(q): Query<Q>) -> KRes {
    let ch = chain(&q.scope, &u, q.parents.as_deref())?;
    Ok(Json(blocking(st.db.clone(), move |c| view(c, &q.scope, &ch)).await?))
}

pub async fn put_rules(State(st): State<Arc<AppState>>, Extension(u): Extension<AuthUser>, Query(q): Query<Q>, Json(mut body): Json<Value>) -> KRes {
    let ch = chain(&q.scope, &u, q.parents.as_deref())?;
    // The agent never closes its own work: only `false` is accepted.
    if let Some(c) = body.get_mut("completion").and_then(Value::as_object_mut) {
        if let Some(v) = c.remove("agent_may_self_close") {
            if v != json!(false) {
                return Err(KErr::bad("completion.agent_may_self_close is locked to false"));
            }
        }
    }
    let patch = flatten(&body, LEAVES)?;
    for (p, v) in &patch {
        validate(p, v)?;
    }
    Ok(Json(blocking(st.db.clone(), move |c| {
        let mut doc = load_doc(c, "rules", &q.scope)?;
        doc.extend(patch);
        save_doc(c, "rules", &q.scope, &doc)?;
        view(c, &q.scope, &ch)
    }).await?))
}

pub async fn delete_field(State(st): State<Arc<AppState>>, Extension(u): Extension<AuthUser>, Query(q): Query<Q>) -> KRes {
    let ch = chain(&q.scope, &u, q.parents.as_deref())?;
    let path = q.path.clone().unwrap_or_default();
    if !LEAVES.contains(&path.as_str()) {
        return Err(KErr::bad("path must be a rules field path"));
    }
    Ok(Json(blocking(st.db.clone(), move |c| {
        let mut doc = load_doc(c, "rules", &q.scope)?;
        doc.remove(&path);
        save_doc(c, "rules", &q.scope, &doc)?;
        view(c, &q.scope, &ch)
    }).await?))
}

/// Enforcement path: fold the org's effective credential-read allowlist into a
/// new Agency run's JudgePolicy. Only ever adds declared reads; a failure to
/// read rules never loosens anything (the field stays unset).
pub async fn apply_to_policy(st: &AppState, org: &str, policy: &mut allternit_commrails::judge::policy::JudgePolicy) {
    let ch = vec![format!("org:{org}")];
    if scope_kind(&ch[0]).is_err() {
        return;
    }
    let got = blocking(st.db.clone(), move |c| effective(c, "rules", &defaults(), LEAVES, &ch).map(|(e, _)| e)).await;
    match got {
        Ok(e) => {
            let reads: Vec<String> = e["guardrails"]["allow_credential_read"].as_array().into_iter().flatten()
                .filter_map(|s| s.as_str().map(str::to_string)).collect();
            if !reads.is_empty() {
                policy.allow_credential_read = Some(reads);
            }
        }
        Err(_) => tracing::warn!("kernel_ui: could not load agent rules; credential-read allowlist left empty"),
    }
}
