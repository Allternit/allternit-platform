//! Agent Gateway vendor turns through the kernel turn router (O2/O14, shadow).
//!
//! Vendor-bound turns never run gizzi's model loop (`gateway_runner::run_turn`
//! sends them to the vendor and appends the reply), so the gizzi turn router
//! never saw them. This module gives them the same shadow decisions:
//!
//! * ROUTE (`bank.route.v0`, the same options as the gizzi turn router, with a
//!   vendor calibration domain so its rows are calibrated separately, Q26):
//!   what kind of turn this is. Labelled from what the vendor turn did (tool
//!   events), with the gizzi router's rules.
//! * CONSEQUENTIAL (`bank.vendor_consequential.v0`, GATE): does this turn need
//!   the Allternit approval? Tighten-only: S1 may only ever ADD the approval
//!   requirement, never remove it ([`tighten_consequential`]). In shadow it
//!   changes nothing; it is labelled with the caller's own flag.
//!
//! The vendor model is fixed by the binding, so there is no ROUTE_MODEL here.
//! Every call is fire-and-forget: a slow or absent S1 never delays a turn.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

pub const ROUTE_BANK: &str = "bank.route.v0";
pub const CONSEQUENTIAL_BANK: &str = "bank.vendor_consequential.v0";
pub const ROUTE_OPTIONS: &[&str] =
    &["answer_from_memory", "retrieval", "single_tool", "agent_run", "coding", "computer_use", "template", "clarify"];

/// The only way S1 may combine with the caller's consequential flag: it can
/// raise friction (true), never lower it.
pub fn tighten_consequential(incumbent: bool, s1_says_consequential: Option<bool>) -> bool {
    incumbent || s1_says_consequential == Some(true)
}

/// A tool that runs a template (`run_template`, `templates.run`, ...); the same
/// rule as the gizzi turn router's `TEMPLATE_TOOL`.
pub fn is_template_tool(name: &str) -> bool {
    const VERBS: [&str; 6] = ["run", "use", "apply", "exec", "execute", "start"];
    let n = name.to_lowercase();
    let seps = |c: char| c == '_' || c == '.' || c == '-';
    let parts: Vec<&str> = n.split(seps).filter(|p| !p.is_empty()).collect();
    parts.windows(2).any(|w| {
        let tpl = |x: &str| x == "template" || x == "templates";
        (VERBS.contains(&w[0]) && tpl(w[1])) || (tpl(w[0]) && VERBS.contains(&w[1]))
    }) || VERBS.iter().any(|v| parts.iter().any(|p| *p == format!("{v}template") || *p == format!("{v}templates")))
}

/// What a vendor turn needed, from the tool names in its events. Same rules as
/// the gizzi turn router's `routeLabel` ("template" when a template tool ran).
pub fn route_label(tools: &[String]) -> &'static str {
    let t: Vec<String> = tools.iter().map(|x| x.to_lowercase()).collect();
    let starts = |x: &str, ps: &[&str]| ps.iter().any(|p| x.starts_with(p));
    let has = |f: &dyn Fn(&str) -> bool| t.iter().any(|x| f(x));
    let read_like = ["read", "grep", "glob", "list", "ls", "webfetch", "websearch", "search", "memory", "codesearch"];
    if t.iter().any(|x| is_template_tool(x)) {
        return "template";
    }
    if t.is_empty() {
        return "answer_from_memory";
    }
    if has(&|x| starts(x, &["question", "ask_user", "askuser"])) {
        return "clarify";
    }
    if has(&|x| ["computer", "browser_", "desktop", "screenshot", "mouse", "keyboard"].iter().any(|k| x.contains(k))) {
        return "computer_use";
    }
    if has(&|x| ["edit", "write", "multiedit", "patch", "apply_patch"].contains(&x)) {
        return "coding";
    }
    if has(&|x| ["task", "agent", "subagent"].contains(&x)) || t.len() > 3 {
        return "agent_run";
    }
    if t.len() == 1 && !starts(&t[0], &read_like[..9]) {
        return "single_tool";
    }
    if t.iter().all(|x| starts(x, &read_like)) {
        return "retrieval";
    }
    if t.len() == 1 { "single_tool" } else { "agent_run" }
}

/// Tool names in a vendor event payload (`{data, envelope}` as bridged).
pub fn tool_name(event_type: &str, payload: &Value) -> Option<String> {
    if !event_type.contains("tool") {
        return None;
    }
    let d = &payload["data"];
    ["name", "tool", "tool_name", "toolName"].iter().find_map(|k| d[*k].as_str().map(str::to_string))
}

fn s1_base() -> (String, Option<String>) {
    let r = allternit_commrails::kernel::s1_outcome::OutcomeReporter::from_env();
    (r.base_url, r.token)
}

fn request(bank: &str, domain: &str, op: &str, instructions: &str, candidates: Vec<Value>, state: &str, corr: &str) -> Value {
    let tail: String = state.chars().rev().take(2000).collect::<Vec<_>>().into_iter().rev().collect();
    json!({ "state": tail, "reversible": true, "backend": "auto", "request": {
        "envelope": { "abi_version": "1.0.0", "schema_id": "allternit.kernel.DecisionRequestV1", "schema_version": "1.0.0",
                      "run_id": corr, "node_id": bank },
        "operation": op, "state_projection_ref": format!("vendor-turn:{corr}"), "instructions": instructions,
        "decision_bank_id": bank, "candidates": candidates, "calibration_domain": domain } })
}

async fn decide(body: Value) -> Option<String> {
    let (url, token) = s1_base();
    let c = reqwest::Client::builder().timeout(std::time::Duration::from_secs(3)).build().ok()?;
    let mut rq = c.post(format!("{url}/v1/decision")).json(&body);
    if let Some(t) = token {
        rq = rq.bearer_auth(t);
    }
    let r = rq.send().await.ok()?;
    if !r.status().is_success() {
        return None;
    }
    let v: Value = r.json().await.ok()?;
    v["extensions"]["x-decision_id"].as_str().map(str::to_string)
}

#[derive(Default, Clone)]
struct Pending {
    route: Option<String>,
    consequential: Option<String>,
}

fn pending() -> &'static Mutex<HashMap<String, Pending>> {
    static P: OnceLock<Mutex<HashMap<String, Pending>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(HashMap::new()))
}

fn enabled() -> bool {
    std::env::var("ALLTERNIT_TURN_ROUTE_SHADOW").map(|v| v != "0").unwrap_or(true) && !cfg!(test)
}

/// Before the turn is sent: ROUTE + CONSEQUENTIAL shadow decisions, keyed by
/// the turn's correlation id. Never awaited by the caller.
pub fn before_send(corr: &str, vendor: &str, text: &str, consequential: bool) {
    if !enabled() {
        return;
    }
    let (corr, vendor, text) = (corr.to_string(), vendor.to_string(), text.to_string());
    tokio::spawn(async move {
        let route_cands: Vec<Value> = ROUTE_OPTIONS.iter().map(|o| json!({ "candidate_id": o, "label": o }))
            .chain(std::iter::once(json!({ "candidate_id": "unknown", "label": "unknown", "is_unknown": true }))).collect();
        let route = decide(request(ROUTE_BANK, "route.vendor", "CHOICE",
            &format!("what kind of turn is this request to the {vendor} agent"), route_cands, &text, &corr)).await;
        let cons = decide(request(CONSEQUENTIAL_BANK, "vendor.consequential", "GATE",
            "does sending this request need the user's approval first (spends money, contacts people, changes or deletes something outside the chat)",
            vec![], &text, &corr)).await;
        if let Some(cons_id) = &cons {
            // Shadow label: the caller's own flag. Tighten-only means a later live
            // mode may only ever add the approval, never remove it.
            report(cons_id, if consequential { "true" } else { "false" }, "gateway.consequential_flag");
        }
        if let Ok(mut p) = pending().lock() {
            p.insert(corr, Pending { route, consequential: cons });
        }
    });
}

/// Count one sent vendor turn in the usage ledger (cost 0; see
/// `usage_ledger::vendor_turn_row`), attributed to the owner's org.
pub fn record_turn(db: &crate::db::DbHandle, owner: &str, vendor: &str, corr: &str, latency_ms: u64, ok: bool) {
    let tenant: Option<String> = db.connect().ok().and_then(|c| {
        c.query_row("SELECT organization_id FROM users WHERE id = ?1", [owner], |r| r.get(0)).ok().flatten()
    });
    crate::usage_ledger::record(crate::usage_ledger::vendor_turn_row(vendor, corr, tenant.as_deref(), owner, latency_ms, ok));
}

/// After the vendor's events were pulled: label ROUTE from the turn's tools.
pub fn after_events(corr: &str, tools: Vec<String>) {
    let rec = pending().lock().ok().and_then(|mut p| p.remove(corr));
    if let Some(Pending { route: Some(id), .. }) = rec {
        report(&id, route_label(&tools), "gateway.turn_tools");
    }
}

fn report(decision_id: &str, truth: &str, source: &str) {
    allternit_commrails::kernel::s1_outcome::OutcomeReporter::from_env()
        .spawn_report(decision_id.to_string(), truth.to_string(), source.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tighten_only_never_removes_the_approval() {
        for incumbent in [false, true] {
            for s1 in [None, Some(false), Some(true)] {
                let out = tighten_consequential(incumbent, s1);
                assert!(out >= incumbent, "S1 lowered friction: incumbent={incumbent} s1={s1:?}");
            }
        }
        assert!(tighten_consequential(false, Some(true)));
        assert!(tighten_consequential(true, Some(false)));
    }

    #[test]
    fn route_label_matches_the_gizzi_rules() {
        let v = |xs: &[&str]| xs.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(route_label(&[]), "answer_from_memory");
        assert_eq!(route_label(&v(&["ask_user"])), "clarify");
        assert_eq!(route_label(&v(&["browser_click"])), "computer_use");
        assert_eq!(route_label(&v(&["edit"])), "coding");
        assert_eq!(route_label(&v(&["read", "grep"])), "retrieval");
        assert_eq!(route_label(&v(&["send_email"])), "single_tool");
        assert_eq!(route_label(&v(&["a", "b", "c", "d"])), "agent_run");
        assert_eq!(route_label(&v(&["read", "run_template"])), "template");
        assert_eq!(route_label(&v(&["templates.run"])), "template");
        assert_eq!(route_label(&v(&["runTemplate"])), "template");
        assert_eq!(route_label(&v(&["list_templates"])), "retrieval");
    }

    #[test]
    fn tool_names_come_only_from_tool_events() {
        assert_eq!(tool_name("agent.tool.started", &json!({ "data": { "name": "WebSearch" } })), Some("WebSearch".into()));
        assert_eq!(tool_name("agent.message.completed", &json!({ "data": { "name": "x" } })), None);
    }
}
