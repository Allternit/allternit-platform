//! Section 5: activity feed from hook decision ledger events (`HarnessToolGated`)
//! plus the caller's agency runs. Hook events are machine-level, so they are
//! shown only to org owners/admins (or callers with no org).

use super::*;
use crate::agency_api::store::AgencyStore;
use axum::extract::{Query, State};
use axum::http::HeaderValue;
use axum::Extension;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct Q {
    limit: Option<usize>,
    cursor: Option<String>,
    filter: Option<String>,
}

fn hook_entry(p: &Value, at: &str) -> Value {
    let decision = p["decision"].as_str().unwrap_or("allow");
    let result = if decision == "deny" { "blocked" } else { "recorded" };
    let reason = p["reason"].as_str().filter(|r| !r.is_empty()).map(|r| format!(": {r}")).unwrap_or_default();
    json!({ "at": at, "surface": format!("hook:{}", p["harness"].as_str().unwrap_or("harness")),
        "thread_ref": p["wih_id"].as_str().or(p["harness_session_id"].as_str()),
        "summary": format!("{} {}{}", p["tool"].as_str().unwrap_or("tool"), decision, reason),
        "result": result, "duration_ms": null, "tokens": null, "tok_per_s": null, "cost_usd": null,
        "over_budget": false, "unresolved": decision == "unresolved" })
}

fn run_entry(run: &Value) -> Value {
    let status = run["status"].as_str().unwrap_or_default();
    let halted = run["budget_usage"]["spend_halted"] == json!(true) || run["status_reason"].as_str().is_some_and(|r| r.contains("budget"));
    let verified = run["completion"]["status"].as_str().is_some_and(|s| matches!(s, "verified" | "passed" | "complete" | "completed")) || status == "completed";
    let result = match status {
        "completed" if verified => "verified",
        "failed" | "cancelled" => "failed",
        "needs_attention" | "paused" => "approval",
        _ if halted => "blocked",
        _ => "recorded",
    };
    let sp = &run["speed"];
    let tokens = run["budget_usage"]["tokens"].as_u64();
    json!({ "at": run["updated_at"], "surface": "agency", "thread_ref": run["thread_id"],
        "summary": run["goal"], "result": result, "duration_ms": sp["duration_ms"],
        "tokens": tokens, "tok_per_s": sp["tok_per_s"], "cost_usd": run["budget_usage"]["cost_usd"], "over_budget": halted, "unresolved": false })
}

fn keep(e: &Value, filter: &str) -> bool {
    let r = e["result"].as_str().unwrap_or_default();
    match filter {
        "approval" => r == "approval",
        "denied" => r == "blocked",
        "unverified" => e["unresolved"] == json!(true) || matches!(r, "recorded" | "failed"),
        "over_budget" => e["over_budget"] == json!(true),
        _ => true,
    }
}

pub async fn list(State(st): State<Arc<AppState>>, Extension(u): Extension<AuthUser>, Query(q): Query<Q>) -> Result<axum::response::Response, KErr> {
    use axum::response::IntoResponse;
    let filter = q.filter.clone().unwrap_or_else(|| "all".into());
    if !["all", "approval", "denied", "unverified", "over_budget"].contains(&filter.as_str()) {
        return Err(KErr::bad("filter must be all, approval, denied, unverified or over_budget"));
    }
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let offset = match q.cursor.as_deref() {
        None | Some("") => 0,
        Some(c) => c.strip_prefix("o:").and_then(|n| n.parse::<usize>().ok()).ok_or_else(|| KErr::bad("invalid cursor"))?,
    };
    let s = AgencyStore::new(st.rails.ledger.clone());
    let mut all: Vec<Value> = vec![];
    let privileged = u.organization_id.as_deref().map_or(true, |o| o.is_empty()) || matches!(u.organization_role.as_deref(), Some("owner" | "admin" | "org:admin"));
    if privileged {
        for ev in s.events_of_type(allternit_commrails::hook::HOOK_EVENT).await.map_err(KErr::internal)? {
            all.push(hook_entry(&ev.payload, &ev.ts));
        }
    }
    for rec in s.list_runs(&u.user_id).await.map_err(KErr::internal)? {
        all.push(run_entry(&rec.run));
    }
    all.retain(|e| keep(e, &filter));
    all.sort_by(|a, b| b["at"].as_str().cmp(&a["at"].as_str()));
    let total = all.len();
    let page: Vec<Value> = all.into_iter().skip(offset).take(limit).collect();
    let mut resp = Json(Value::Array(page)).into_response();
    if offset + limit < total {
        if let Ok(v) = HeaderValue::from_str(&format!("o:{}", offset + limit)) {
            resp.headers_mut().insert("x-next-cursor", v);
        }
    }
    Ok(resp)
}
