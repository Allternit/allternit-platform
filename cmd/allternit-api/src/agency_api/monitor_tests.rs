//! C2: trace monitor. Deterministic checks on crafted trails; the findings
//! endpoint (owner scoping, terminal only, on-demand) and the off-path hook.

use super::*;
use crate::agency_api::tests::{call, get_req, post, setup};
use axum::http::StatusCode;

fn ev(seq: i64, ty: &str, data: Value) -> Value {
    json!({ "id": format!("evt_{seq:010}"), "seq": seq, "run_id": "run_t", "type": ty, "data": data })
}

fn receipt(id: &str, body: Value) -> Value {
    let mut b = body;
    b["chain"] = json!({ "receipt_id": id });
    b
}

fn row(key: &str, node: &str, tool: &str, status: &str, args: &str) -> JournalRow {
    JournalRow { key: key.into(), node: node.into(), tool: tool.into(), status: status.into(), args_hash: args.into(), result: None }
}

/// A clean, verified task-type run: policy allow, then the effect, then a passing verifier.
fn clean() -> Trace {
    Trace {
        run: json!({ "id": "run_t", "status": "completed", "budget": { "max_cost_usd": 1.0, "max_steps": 100 },
                     "budget_usage": { "cost_usd": 0.1, "steps": 10 },
                     "completion": { "status": "verified", "criteria": [{ "criterion": "document_exists", "result": "pass" }] } }),
        attention: vec![],
        events: vec![
            ev(1, "run.status_changed", json!({ "to": "accepted" })),
            ev(2, "run.progress", json!({ "step": "R04", "outcome": "committed", "cognitive_role": "S2" })),
            ev(3, "receipt.appended", json!({ "receipt_id": "rcpt_pol", "receipt_type": "policy_decision", "step": "R06" })),
            ev(4, "receipt.appended", json!({ "receipt_id": "rcpt_eff", "receipt_type": "effect", "step": "R07" })),
            ev(5, "receipt.appended", json!({ "receipt_id": "rcpt_ver", "receipt_type": "verification", "step": "gate" })),
            ev(6, "verification.completed", json!({ "result": "pass", "receipt_id": "rcpt_ver", "missing": [] })),
            ev(7, "run.status_changed", json!({ "to": "completed", "terminal": true })),
        ],
        receipts: vec![
            receipt("rcpt_pol", json!({ "type": "policy_decision", "decision": "allow" })),
            receipt("rcpt_eff", json!({ "type": "effect" })),
            receipt("rcpt_ver", json!({ "type": "verification", "result": "PASS", "missing": [] })),
        ],
        journal: vec![row("k_gen", "R04", "model.generate", "committed", ""), row("k_eff", "R07", "tool.write_file", "committed", "sha256:a")],
        decisions: vec![DecisionRow { id: "dec_ok".into(), kind: "element".into(), choice: Some("a".into()), confidence: 0.95, abstained: false, backend: "head".into() }],
        s1_kinds: [("G05".to_string(), "verify".to_string())].into_iter().collect(),
        divergences: vec![],
    }
}

fn checks(t: &Trace) -> Vec<&'static str> {
    run_checks(t).iter().map(|f| f.check).collect()
}

#[test]
fn monitor_clean_trace_has_no_findings() {
    assert_eq!(checks(&clean()), Vec::<&str>::new());
    let r = report("run_t", &clean(), &[], Value::Null);
    assert_eq!((r["summary"]["total"].as_u64(), r["summary"]["max_severity"].is_null()), (Some(0), true));
    assert_eq!(r["summary"]["checks_run"].as_u64(), Some(CHECKS.len() as u64));
}

#[test]
fn monitor_effect_without_policy_allow_is_critical_with_evidence() {
    // No policy receipt at all.
    let mut t = clean();
    t.events.retain(|e| e["data"]["receipt_type"] != "policy_decision");
    let f = check_effect_without_policy(&t).expect("fires");
    assert_eq!(f.severity, Severity::Critical);
    assert_eq!((f.evidence.event_seqs.clone(), f.evidence.journal_keys.clone(), f.evidence.receipt_ids.clone()), (vec![4], vec!["k_eff".to_string()], vec!["rcpt_eff".to_string()]));
    // A policy receipt that says deny does not authorize.
    let mut t = clean();
    t.receipts[0]["decision"] = json!("deny");
    let f = check_effect_without_policy(&t).expect("deny is not allow");
    assert!(f.evidence.receipt_ids.contains(&"rcpt_pol".to_string()), "{:?}", f.evidence);
    // The allow comes after the effect: still a violation.
    let mut t = clean();
    t.events[2]["seq"] = json!(9);
    assert!(check_effect_without_policy(&t).is_some());
    // BUG_FIX writes "ALLOW"; read-only effects (test runs) need no policy.
    let mut t = clean();
    t.receipts[0]["decision"] = json!("ALLOW");
    t.journal.push(row("k_test", "N04", "tool.test_run", "committed", "sha256:t"));
    assert!(check_effect_without_policy(&t).is_none());
    // The finding id is deterministic.
    let mut t = clean();
    t.events.retain(|e| e["data"]["receipt_type"] != "policy_decision");
    let (a, b) = (run_checks(&t)[0].json("run_t"), run_checks(&t)[0].json("run_t"));
    assert_eq!(a["id"], b["id"]);
    assert!(a["id"].as_str().unwrap().starts_with("fnd_"));
    assert_eq!(a["title"], "An effect ran without a preceding policy allow");
}

#[test]
fn monitor_verifier_and_completion_checks() {
    let mut t = clean();
    t.events[5]["data"]["missing"] = json!(["citations_resolve"]);
    let f = check_verifier_pass_missing(&t).expect("pass with missing evidence");
    assert_eq!((f.severity, f.evidence.event_seqs.clone()), (Severity::High, vec![6]));
    let mut t = clean();
    t.receipts[2]["missing"] = json!(["document_exists"]);
    assert!(check_verifier_pass_missing(&t).is_some());
    let mut t = clean();
    t.run["completion"]["criteria"][0]["result"] = json!("fail");
    assert!(check_verifier_pass_missing(&t).unwrap().detail.contains("document_exists"));
    // Completed with no passing verification at all.
    let mut t = clean();
    t.events.retain(|e| e["type"] != "verification.completed");
    t.receipts.retain(|r| r["type"] != "verification");
    let f = check_completed_unverified(&t).expect("unverified completion");
    assert_eq!((f.severity, f.evidence.event_seqs.clone()), (Severity::High, vec![7]));
    t.run["status"] = json!("failed");
    assert!(check_completed_unverified(&t).is_none(), "only completed runs need a passing verifier");
}

#[test]
fn monitor_loops_storms_budget_attention_decisions_divergence() {
    // A step re-run 4 times and an identical tool call 3 times.
    let mut t = clean();
    for i in 0..4 {
        t.events.push(ev(20 + i, "run.progress", json!({ "step": "R03", "outcome": "failed" })));
    }
    for i in 0..3 {
        t.journal.push(row(&format!("k_x{i}"), "R03", "tool.execute", "failed", "sha256:same"));
    }
    let f = check_repeated_steps(&t).expect("loop");
    assert_eq!(f.severity, Severity::Medium);
    assert_eq!(f.evidence.event_seqs, vec![20, 21, 22, 23]);
    assert_eq!(f.evidence.journal_keys.len(), 3);
    for i in 0..4 {
        t.events.push(ev(30 + i, "run.progress", json!({ "step": "R03", "outcome": "failed" })));
    }
    assert_eq!(check_repeated_steps(&t).unwrap().severity, Severity::High, "8 repeats");

    // Attention storm.
    let mut t = clean();
    for i in 0..3 {
        t.events.push(ev(40 + i, "attention.requested", json!({ "attention": { "id": format!("att_{i}") } })));
    }
    let f = check_escalation_storm(&t).expect("storm");
    assert_eq!((f.severity, f.evidence.attention_ids.len()), (Severity::Medium, 3));

    // Budget: 95% of steps, then halted.
    let mut t = clean();
    t.run["budget_usage"]["steps"] = json!(95);
    assert_eq!(check_budget(&t).unwrap().severity, Severity::Medium);
    t.run["budget_usage"]["spend_halted"] = json!(true);
    t.events.push(ev(50, "budget.threshold", json!({ "dimension": "max_steps", "percent": 100 })));
    let f = check_budget(&t).unwrap();
    assert_eq!((f.severity, f.evidence.event_seqs.clone()), (Severity::High, vec![50]));

    // Unanswered attention on a completed run is high.
    let mut t = clean();
    t.attention.push(json!({ "id": "att_open", "status": "open" }));
    t.events.push(ev(60, "attention.requested", json!({ "attention": { "id": "att_open" } })));
    let f = check_unanswered_attention(&t).unwrap();
    assert_eq!((f.severity, f.evidence.event_seqs.clone(), f.evidence.attention_ids.clone()), (Severity::High, vec![60], vec!["att_open".to_string()]));
    t.run["status"] = json!("cancelled");
    assert_eq!(check_unanswered_attention(&t).unwrap().severity, Severity::Medium);

    // A decision served under its kind's threshold (default 0.6); abstentions and trace.error itself don't count.
    let mut t = clean();
    t.decisions.push(DecisionRow { id: "dec_low".into(), kind: "element".into(), choice: Some("b".into()), confidence: 0.3, abstained: false, backend: "local".into() });
    t.decisions.push(DecisionRow { id: "dec_abs".into(), kind: "element".into(), choice: None, confidence: 0.1, abstained: true, backend: "local".into() });
    t.decisions.push(DecisionRow { id: "dec_tr".into(), kind: DECISION_KIND.into(), choice: Some("none".into()), confidence: 0.1, abstained: false, backend: "head".into() });
    let f = check_low_confidence(&t).unwrap();
    assert_eq!(f.evidence.decision_ids, vec!["dec_low".to_string()]);

    // Effect content diverging from the recorded model output.
    let mut t = clean();
    t.divergences.push(json!({ "step_ref": "R07", "type": "EFFECT_REQUEST", "kind": "unexpected" }));
    t.divergences.push(json!({ "step_ref": "R04", "type": "OUTPUT", "kind": "expected" }));
    let f = check_effect_divergence(&t).unwrap();
    assert_eq!((f.severity, f.evidence.nodes.clone(), f.evidence.receipt_ids.clone()), (Severity::High, vec!["R07".to_string()], vec!["rcpt_eff".to_string()]));

    // A prepared-but-never-settled effect.
    let mut t = clean();
    t.journal.push(row("k_prep", "R07", "tool.write_file", "prepared", "sha256:b"));
    assert_eq!(check_effect_unresolved(&t).unwrap().evidence.journal_keys, vec!["k_prep".to_string()]);
}

fn s1(key: &str, node: &str, choice: Option<&str>, confidence: f64) -> JournalRow {
    let v = json!({ "decision_id": format!("dec_{key}"), "choice": choice, "confidence": confidence, "abstained": choice.is_none(), "backend": "local" });
    JournalRow { result: Some(v.to_string()), ..row(key, node, S1_TOOL, "committed", "") }
}

#[test]
fn monitor_s1_decision_nodes_low_confidence_and_escalations() {
    // A served S1 answer above its kind's threshold (verify: 0.8) is fine; under it is a finding.
    let mut t = clean();
    t.journal.push(s1("a", "G05", Some("pass"), 0.85));
    assert!(check_low_confidence(&t).is_none());
    assert!(check_escalation_storm(&t).is_none());
    t.journal.push(s1("b", "G05", Some("pass"), 0.7));
    let f = check_low_confidence(&t).expect("acted under the verify threshold");
    assert_eq!((f.evidence.nodes.clone(), f.evidence.journal_keys.clone(), f.evidence.decision_ids.clone()),
        (vec!["G05".to_string()], vec!["b".to_string()], vec!["dec_b".to_string()]));
    // The same decision found in the decisions table is reported once.
    t.decisions.push(DecisionRow { id: "dec_b".into(), kind: "verify".into(), choice: Some("pass".into()), confidence: 0.7, abstained: false, backend: "local".into() });
    assert_eq!(check_low_confidence(&t).unwrap().evidence.decision_ids, vec!["dec_b".to_string()]);
    // S1 nodes escalating (abstaining) on most of their decisions are an escalation storm.
    let mut t = clean();
    t.journal.push(s1("c", "G05", Some("pass"), 0.95));
    t.journal.push(s1("d", "X01", None, 0.2));
    t.journal.push(s1("e", "X02", None, 0.3));
    let f = check_escalation_storm(&t).expect("2 of 3 S1 decisions escalated");
    assert_eq!(f.evidence.nodes, vec!["X01".to_string(), "X02".to_string()]);
    assert!(f.detail.contains("2 of 3 S1"), "{}", f.detail);
    assert_eq!(signals(&t)["s1_decisions"], 3);
    assert_eq!(signals(&t)["effects"], 1, "S1 answers and model outputs are not effects");
    // S1 answers are not mutating effects either.
    assert!(check_effect_without_policy(&t).is_none());
}

#[test]
fn monitor_orders_by_severity_and_labels_the_decision() {
    let mut t = clean();
    t.events.retain(|e| e["data"]["receipt_type"] != "policy_decision");
    t.run["budget_usage"]["steps"] = json!(95);
    let f = run_checks(&t);
    assert_eq!(f.iter().map(|f| f.check).collect::<Vec<_>>(), vec!["effect_without_policy_allow", "budget_near_exhaustion"]);
    assert_eq!(label_of(&f), "effect_without_policy_allow");
    assert_eq!(label_of(&f[1..]), "none", "medium findings do not label the run as an error");
    let body = decision_request("run_t", &t, &f, Some(vec!["head".into()]));
    assert_eq!(body["kind"], DECISION_KIND);
    assert_eq!(body["options"].as_array().unwrap().len(), CHECKS.len() + 1);
    assert_eq!(body["options"][0]["id"], "none");
    assert_eq!(body["backends"], json!(["head"]));
    assert_eq!(body["task"], "trace:run_t", "not attached to the run's own decisions");
    let r = report("run_t", &t, &f, Value::Null);
    assert_eq!(r["summary"]["by_severity"]["critical"], 1);
    assert_eq!(r["summary"]["max_severity"], "critical");
    assert_eq!(summary_of(&r)["url"], "/v1/runs/run_t/findings");
}

// ── end to end ──────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn monitor_findings_endpoint_is_owner_scoped_terminal_only_and_hooked() {
    let t = setup().await;
    let (st, _, b) = call(&t.app, post("/v1/agency", "u_mon", Some("monitor-e2e-0001"), json!({ "goal": "Fix the failing checkout tests" }))).await;
    assert_eq!(st, StatusCode::ACCEPTED, "{b}");
    let id = serde_json::from_str::<Value>(&b).unwrap()["id"].as_str().unwrap().to_string();
    // Not finished: 409.
    let (st, _, b) = call(&t.app, get_req(&format!("/v1/runs/{id}/findings"), "u_mon")).await;
    assert_eq!(st, StatusCode::CONFLICT, "{b}");
    assert!(b.contains("ERR_RUN_NOT_FINISHED"));
    // Someone else: 404 (no existence leak).
    let (st, _, _) = call(&t.app, get_req(&format!("/v1/runs/{id}/findings"), "u_other")).await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // Craft a terminal run: completed with no verification and an open attention item.
    let s = AgencyStore::new(t.st.rails.ledger.clone());
    let mut rec = s.load_run(&id).await.unwrap().unwrap();
    rec.attention.push(json!({ "id": "att_left", "object": "attention_request", "run_id": id, "status": "open", "reason": "task_wait" }));
    let rec = s.save(rec).await.unwrap();
    s.transition(rec, "completed", Some("crafted")).await.unwrap();
    // The hook (installed by create_run) stores findings off path; poll for them.
    let mut stored_f = None;
    for _ in 0..100 {
        stored_f = stored(&s, &id, "u_mon").await.unwrap();
        if stored_f.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }
    let stored_f = stored_f.expect("the terminal hook stored findings");
    let ids: Vec<&str> = stored_f["data"].as_array().unwrap().iter().map(|f| f["check"].as_str().unwrap()).collect();
    assert!(ids.contains(&"completed_without_verification") && ids.contains(&"unanswered_attention"), "{ids:?}");
    // Severe findings are ledger events, outside the public trail.
    let raw = s.events_of_type(EV_TRACE_FINDING).await.unwrap();
    assert!(raw.iter().any(|e| e.payload["run_id"] == id.as_str() && e.payload["finding"]["check"] == "completed_without_verification"));
    let public = s.events(&id).await.unwrap();
    assert_eq!(public.last().unwrap()["type"], "run.status_changed", "the public trail still ends at the terminal event");

    // Owner reads them; others cannot.
    let (st, _, b) = call(&t.app, get_req(&format!("/v1/runs/{id}/findings"), "u_mon")).await;
    assert_eq!(st, StatusCode::OK, "{b}");
    let got: Value = serde_json::from_str(&b).unwrap();
    assert_eq!((got["object"].as_str(), got["run_id"].as_str(), got["monitor"].as_str()), (Some("run_findings"), Some(id.as_str()), Some(MONITOR_VERSION)));
    assert_eq!(got["summary"]["max_severity"], "high");
    let (st, _, _) = call(&t.app, get_req(&format!("/v1/runs/{id}/findings"), "u_other")).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    // The graph view carries the summary.
    let (st, _, b) = call(&t.app, get_req(&format!("/v1/runs/{id}/graph"), "u_mon")).await;
    assert_eq!(st, StatusCode::OK);
    let g: Value = serde_json::from_str(&b).unwrap();
    assert_eq!(g["findings"]["total"], got["summary"]["total"]);
    assert_eq!(g["findings"]["url"], format!("/v1/runs/{id}/findings"));
}

#[tokio::test]
async fn monitor_computes_on_demand_when_the_hook_never_ran() {
    let t = setup().await;
    // A run saved straight to the store: no create_run, so no hook.
    let s = AgencyStore::new(t.st.rails.ledger.clone());
    let id = "run_ondemand0000000000000000000001";
    let rec = s.save(RunRecord { owner: "u_od".into(), idempotency_key: None, attention: vec![], task_ir: json!({ "task_type": "BUG_FIX" }),
        run: json!({ "id": id, "object": "run", "status": "failed", "version": 0, "budget": {}, "budget_usage": {}, "completion": { "status": "unverified" } }) })
        .await.unwrap();
    assert_eq!(rec.run["status"], "failed");
    assert!(stored(&s, id, "u_od").await.unwrap().is_none());
    let (st, _, b) = call(&t.app, get_req(&format!("/v1/runs/{id}/findings"), "u_od")).await;
    assert_eq!(st, StatusCode::OK, "{b}");
    let got: Value = serde_json::from_str(&b).unwrap();
    assert_eq!(got["summary"]["total"], 0, "{got}");
    assert!(got["decision"].is_null(), "the on-demand path never calls a decision backend");
    assert!(stored(&s, id, "u_od").await.unwrap().is_some(), "and stores the result");
}
