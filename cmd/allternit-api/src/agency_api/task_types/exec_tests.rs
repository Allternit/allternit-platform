//! WP-X2: X1's eval sets through the real executor (scripted cognition),
//! end to end via the Agency API: create with `task_type`, drive, answer
//! WAIT gates through attention, read the outcome from the run + events.

use crate::agency_api::tests::{call, get_req, post, run_limits, setup, wait_settled, E2E_ENV};
use crate::agency_api::{compiler, executor, guard, store::AgencyStore};
use axum::http::StatusCode;
use serde_json::{json, Value};

const EVALS: &[&str] = &[
    include_str!("evals/general_task.v1.json"),
    include_str!("evals/thread_work.v1.json"),
    include_str!("evals/computer_use.v1.json"),
    include_str!("evals/research_doc.v1.json"),
    include_str!("evals/template.v1.json"),
    include_str!("evals/campaign.v1.json"),
];

#[test]
fn enable_list_defaults_to_bug_fix_only() {
    assert_eq!(super::enabled_from(None), vec!["BUG_FIX".to_string()]);
    assert_eq!(super::enabled_from(Some(" ")), vec!["BUG_FIX".to_string()]);
    assert_eq!(super::enabled_from(Some("bug_fix, GENERAL_TASK")), vec!["BUG_FIX".to_string(), "GENERAL_TASK".to_string()]);
}

#[test]
fn compiler_selects_validates_and_gates_task_types() {
    let reg = compiler::TemplateRegistry::default().with_enabled(&["BUG_FIX"]);
    let c = compiler::compile(&json!({ "goal": "x" }), "r1", &reg).unwrap();
    assert_eq!(c.task_ir["task_type"], "BUG_FIX", "default stays BUG_FIX");
    let e = compiler::compile(&json!({ "goal": "x", "task_type": "GENERAL_TASK" }), "r1", &reg).err().unwrap();
    assert_eq!((e.status, e.code), (422, "ERR_POLICY_DENIED"), "not enabled on this server");
    let e = compiler::compile(&json!({ "goal": "x", "task_type": "NOPE" }), "r1", &reg.clone().with_enabled(&["NOPE"])).err().unwrap();
    assert_eq!(e.param.as_deref(), Some("task_type"), "unknown type");
    let reg = reg.with_enabled(&["THREAD_WORK"]);
    let c = compiler::compile(&json!({ "goal": "x", "task_type": "THREAD_WORK", "workspace": { "resources": ["thread:th_1"] } }), "r2", &reg).unwrap();
    assert_eq!(c.task_ir["task_type"], "THREAD_WORK");
    assert_eq!(c.task_ir["completion"]["contract_id"], "completion.thread_work");
    assert_eq!(c.task_ir["wih_policy"]["write_set"], json!(["thread:th_1"]));
    assert_eq!(c.task_ir["judge_policy"]["completion_policy"], "completion.thread_work");
}

async fn progress_steps(t: &crate::agency_api::tests::T, id: &str) -> Vec<String> {
    let (_, _, b) = call(&t.app, get_req(&format!("/v1/runs/{id}/events"), "u1")).await;
    let ev: Value = serde_json::from_str(&b).unwrap();
    ev["data"].as_array().cloned().unwrap_or_default().iter()
        .filter(|e| e["type"] == "run.progress").filter_map(|e| e["data"]["step"].as_str().map(String::from)).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn x1_eval_sets_pass_through_the_real_executor() {
    let _env = E2E_ENV.lock().await;
    let runs = tempfile::tempdir().unwrap();
    std::env::set_var("ALLTERNIT_AGENCY_COGNITION", "scripted");
    std::env::set_var("ALLTERNIT_AGENCY_RUNS_DIR", runs.path());
    // Superset of the default (BUG_FIX stays on): parallel tests share the env.
    std::env::set_var(super::ENABLE_ENV, format!("BUG_FIX,{}", super::ids().join(",")));
    let t = setup().await;
    let s = AgencyStore::new(t.st.rails.ledger.clone());
    // WP-C3a: THREAD_WORK and TEMPLATE run their real connectors against the
    // in-process stores, so the resources the eval sets name must exist.
    use crate::agency_api::{effect_template, effect_thread};
    effect_thread::seed(&t.st.db, "th_1", "u1", false);
    effect_template::seed(&t.st.db, "weekly-report", "u1", effect_template::model_steps());
    let mut n = 0;
    for raw in EVALS {
        let set: Value = serde_json::from_str(raw).unwrap();
        let tt = set["task_type"].as_str().unwrap();
        for c in set["cases"].as_array().unwrap() {
            n += 1;
            let tag = format!("{tt}/{}", c["id"].as_str().unwrap());
            let exp = &c["expect"];
            let mut body = json!({ "goal": c["goal"], "task_type": tt });
            if let Some(r) = c["params"]["writable_resources"].as_array().filter(|r| !r.is_empty()) {
                body["workspace"] = json!({ "resources": r });
            }
            let (st, _, b) = call(&t.app, post("/v1/agency", "u1", Some(&format!("x2-eval-{n:04}-{tt}")), body)).await;
            if exp["status"] == "refused" {
                assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{tag}: {b}");
                continue;
            }
            assert_eq!(st, StatusCode::ACCEPTED, "{tag}: {b}");
            let id = serde_json::from_str::<Value>(&b).unwrap()["id"].as_str().unwrap().to_string();
            let script = &c["script"];
            std::fs::create_dir_all(runs.path().join(&id)).unwrap();
            std::fs::write(runs.path().join(&id).join("scripted.json"),
                json!({ "fail": script["fail"].as_array().cloned().unwrap_or_default(), "withhold": script["withhold"].as_array().cloned().unwrap_or_default() }).to_string()).unwrap();
            // Child template runs (C3a connector) count toward the org run rate too.
            let limits = run_limits(&[(guard::ORGS_ENV, "user:u1"), (guard::ORG_RUNS_PER_HOUR_ENV, "1000")]);
            assert!(executor::admit_and_start(t.st.clone(), id.clone(), limits.clone()).await, "{tag}: admitted");
            let mut rec = wait_settled(&s, &id).await;
            let mut waited_at = None;
            for _ in 0..4 {
                if rec.run["status"] != "needs_attention" { break; }
                let att = rec.attention.iter().find(|a| a["status"] == "open").cloned().unwrap();
                assert_eq!(att["reason"], executor::generic::TASK_WAIT_REASON, "{tag}: {}", rec.run);
                let node = att["node_id"].as_str().unwrap().to_string();
                let Some(answer) = script["wait"][&node].as_str() else { waited_at = Some(node); break };
                let kind = if answer == "open" { "approval" } else { "rejection" };
                let (st, _, b) = call(&t.app, post(&format!("/v1/attention/{}/responses", att["id"].as_str().unwrap()), "u1", None, json!({ "type": kind }))).await;
                assert_eq!(st, StatusCode::OK, "{tag}: {b}");
                assert!(executor::admit_and_start(t.st.clone(), id.clone(), limits.clone()).await, "{tag}: re-admitted");
                rec = wait_settled(&s, &id).await;
            }
            let steps = progress_steps(&t, &id).await;
            let reason = rec.run["status_reason"].as_str().unwrap_or_default().to_string();
            let status = match rec.run["status"].as_str().unwrap() {
                "needs_attention" if waited_at.is_some() => "waiting",
                "failed" if rec.run["completion"]["status"] == "unverified" => "needs_evidence",
                other => other,
            };
            assert_eq!(status, exp["status"].as_str().unwrap(), "{tag}: {reason} {steps:?}");
            let stopped_at = waited_at.clone().or_else(|| steps.last().cloned());
            if let Some(sa) = exp["stopped_at"].as_str() {
                assert_eq!(stopped_at.as_deref(), Some(sa), "{tag}: {steps:?}");
            }
            for v in exp["visited"].as_array().into_iter().flatten() {
                assert!(steps.iter().any(|x| x == v.as_str().unwrap()), "{tag}: expected visit {v} in {steps:?}");
            }
            for v in exp["not_visited"].as_array().into_iter().flatten() {
                assert!(!steps.iter().any(|x| x == v.as_str().unwrap()), "{tag}: {v} must not run: {steps:?}");
            }
            for m in exp["missing"].as_array().into_iter().flatten() {
                assert!(reason.contains(m.as_str().unwrap()), "{tag}: {reason}");
            }
            if status == "completed" {
                assert_eq!(rec.run["completion"]["status"], "verified", "{tag}");
                assert!(rec.run["completion"]["criteria"].as_array().unwrap().iter().all(|c| c["result"] == "pass"), "{tag}: {}", rec.run["completion"]);
            }
        }
    }
    assert!(n >= 24, "ran {n} cases");
    // posts_reply + not_delivered reached T06 (policy_denies stopped at T05): one post each.
    assert_eq!(effect_thread::count(&t.st.db, "th_1"), 2, "one thread message per effect");
    let text: String = t.st.db.connect().unwrap().query_row(
        "SELECT json_extract(payload, '$.text') FROM bot_events WHERE thread_id = 'th_1' LIMIT 1", [], |r| r.get(0)).unwrap();
    assert!(text.starts_with("scripted output of T03"), "the reply candidate is what gets posted: {text}");
    // runs_steps + step_left_open + checks_fail reached P03: one completed child template run each.
    let kids = s.runs_with_status(&["completed"]).await.unwrap().into_iter()
        .filter(|r| r.run["agent"] == "template" && r.run["metadata"]["template_id"] == "weekly-report").count();
    assert_eq!(kids, 3, "one child template run per effect");
}
