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
    let set = |m: &mut Map<String, Value>, k: &str, v: &str| { m.insert(k.to_string(), json!(v)); };
    set(&mut m, "guardrails.level", "enforced: strict sets JudgePolicy.fence=strict; guardrails is the Q25 default; off drops approvals and custom rules (the blocklist stays on). Hosted Agency runs are never weaker than strict");
    set(&mut m, "guardrails.allow_credential_read", "enforced: JudgePolicy.allow_credential_read (org scope only)");
    set(&mut m, "approvals.outside_scope", "enforced: hook ask for writes outside the WIH lease/scope (JudgePolicy.ask_outside_scope)");
    set(&mut m, "approvals.private_network", "enforced: hook + Gate 2 ask for RFC1918/ULA targets; metadata and link-local stay denied (JudgePolicy.ask_private_network)");
    set(&mut m, "approvals.spend_over_usd", "enforced: Agency executor raises attention when a run's spend crosses it");
    set(&mut m, "budgets.daily_usd", "enforced: tightens the org daily USD cap in the Agency spending guard; env cap is the ceiling; stricter of the scope chain wins");
    set(&mut m, "budgets.max_concurrent", "enforced: tightens the per-org concurrency limit; env limit is the ceiling; stricter of the scope chain wins");
    set(&mut m, "completion.require_verifier", "enforced: JudgePolicy.verify=judge");
    set(&mut m, "custom", "enforced: JudgePolicy.custom_rules matched in the hook and Gate 2 (ask or deny, recorded in the hook ledger)");
    set(&mut m, "completion.agent_may_self_close", "locked: false");
    Value::Object(m)
}

/// `enforced: { "<field path>": true|false }` for the UI.
fn enforced() -> Value {
    let mut m = Map::new();
    for l in LEAVES {
        m.insert((*l).to_string(), json!(true));
    }
    m.insert("completion.agent_may_self_close".into(), json!(true));
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
    Ok(json!({ "scope": scope, "effective": eff, "sources": sources, "enforcement": enforcement(), "enforced": enforced() }))
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

/// Fold effective rules into a run's (or session's) JudgePolicy. Pure.
/// Stricter wins: `strict` raises the fence, never lowers it.
pub fn rules_to_policy(e: &Value, policy: &mut allternit_commrails::judge::policy::JudgePolicy) {
    use allternit_commrails::judge::policy::{CustomRule, Fence, VerifyMode};
    let level = e["guardrails"]["level"].as_str().unwrap_or("guardrails");
    if level == "strict" {
        policy.fence = Some(Fence::Strict);
    } else if policy.fence.is_none() {
        policy.fence = Some(Fence::Guardrail);
    }
    if level == "off" {
        policy.ask_outside_scope = Some(false);
        policy.ask_private_network = Some(false);
    } else {
        policy.ask_outside_scope = Some(e["approvals"]["outside_scope"].as_bool().unwrap_or(true));
        policy.ask_private_network = Some(e["approvals"]["private_network"].as_bool().unwrap_or(true));
        let custom: Vec<CustomRule> = e["custom"].as_array().into_iter().flatten()
            .filter_map(|r| serde_json::from_value(r.clone()).ok()).collect();
        if !custom.is_empty() {
            policy.custom_rules = Some(custom);
        }
    }
    if e["completion"]["require_verifier"].as_bool().unwrap_or(true) {
        policy.verify = Some(VerifyMode::Judge);
    }
}

/// Per-run overrides the Agency executor reads from `task_ir.rules`.
pub fn run_overrides(e: &Value) -> Value {
    json!({
        "spend_over_usd": e["approvals"]["spend_over_usd"],
        "daily_usd": e["budgets"]["daily_usd"],
        "max_concurrent": e["budgets"]["max_concurrent"],
    })
}

/// Effective rules for `chain` (most specific first). Budgets take the
/// stricter (smaller) value across the whole chain, so a project cannot
/// exceed its org's budget.
pub fn resolve(conn: &Connection, chain: &[String]) -> Result<Value, KErr> {
    let (mut eff, _) = effective(conn, "rules", &defaults(), LEAVES, chain)?;
    for path in ["budgets.daily_usd", "budgets.max_concurrent"] {
        let mut min: Option<f64> = None;
        for scope in chain {
            if let Some(n) = load_doc(conn, "rules", scope)?.get(path).and_then(Value::as_f64) {
                min = Some(min.map_or(n, |m| m.min(n)));
            }
        }
        if let Some(m) = min {
            set_path(&mut eff, path, json!(m));
        }
    }
    Ok(eff)
}

/// Enforcement path (Agency API runs): the org's credential-read allowlist
/// (org scope only: a project or workspace cannot loosen the blocklist), plus
/// every other rule resolved over `org` and the optional workspace/project
/// scopes, folded into the JudgePolicy and `task_ir.rules`. A failure to read
/// rules never loosens anything.
pub async fn apply_to_run(st: &AppState, org: &str, extra: Vec<String>, policy: &mut allternit_commrails::judge::policy::JudgePolicy, task_ir: &mut Value) {
    let org_scope = format!("org:{org}");
    if scope_kind(&org_scope).is_err() {
        return;
    }
    let mut ch: Vec<String> = extra.into_iter().filter(|s| scope_kind(s).is_ok_and(|k| k != "org")).collect();
    ch.push(org_scope.clone());
    let orgchain = vec![org_scope];
    let got = blocking(st.db.clone(), move |c| Ok((resolve(c, &ch)?, resolve(c, &orgchain)?))).await;
    match got {
        Ok((e, org_e)) => {
            let reads: Vec<String> = org_e["guardrails"]["allow_credential_read"].as_array().into_iter().flatten()
                .filter_map(|s| s.as_str().map(str::to_string)).collect();
            if !reads.is_empty() {
                policy.allow_credential_read = Some(reads);
            }
            rules_to_policy(&e, policy);
            task_ir["rules"] = run_overrides(&e);
        }
        Err(_) => tracing::warn!("kernel_ui: could not load agent rules; rules left unapplied"),
    }
}

/// Org-only convenience wrapper (credential reads + policy, no task_ir).
pub async fn apply_to_policy(st: &AppState, org: &str, policy: &mut allternit_commrails::judge::policy::JudgePolicy) {
    apply_to_run(st, org, Vec::new(), policy, &mut Value::Null).await;
}
