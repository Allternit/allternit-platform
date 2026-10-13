//! C5: graders, suites, bootstrap CI, the offline gate report, suite
//! execution through recorded-only replay, promotion of runs into cases.

use super::graders::*;
use super::*;
use crate::agency_api::tests::{call, get_req, post, run_limits, setup, wait_settled, T, E2E_ENV};
use crate::agency_api::{executor, guard, store::AgencyStore};

fn traj() -> Trajectory {
    let st = |n: &str, p: &str, ok: bool| Step { node: n.into(), primitive: p.into(), ok };
    Trajectory {
        status: "completed".into(),
        verifier: Some(true),
        steps: vec![st("R02", "obs.search_text", true), st("R04", "ctl.compose_final_report", true), st("R06", "policy.authorize_tool_call", true),
                    st("R07", "mut.create_file", true), st("R08", "ver.check_artifact_existence", true)],
        effects: vec![Effect { node: "R07".into(), tool: "tool.write_file".into(), committed: true }],
        texts: vec!["Sandboxes isolate code [src:doc-1]; see also [src:doc-2] and [src:doc-1].".into()],
        stopped_at: Some("R08".into()),
        missing: vec![],
        cost_usd: 0.02,
        tokens: 1200,
        latency_ms: 900.0,
    }
}

fn g(v: Value) -> Box<dyn Grader> {
    parse(&v).unwrap_or_else(|e| panic!("{v}: {e}"))
}

fn passes(v: Value, t: &Trajectory) -> bool {
    g(v).grade(t).pass
}

#[test]
fn eval_graders_grade_trajectories() {
    let t = traj();
    // Order: policy before the write (by primitive or node id).
    assert!(passes(json!({ "type": "step_order", "steps": ["policy.authorize_tool_call", "R07"] }), &t));
    assert!(!passes(json!({ "type": "step_order", "steps": ["R07", "R06"] }), &t), "wrong order");
    let mut failed = t.clone();
    failed.steps[2].ok = false;
    assert!(!passes(json!({ "type": "step_order", "steps": ["R06", "R07"] }), &failed), "a failed step does not count");
    assert!(passes(json!({ "type": "required_steps", "steps": ["R08", "R02"] }), &t));
    assert!(!passes(json!({ "type": "required_steps", "steps": ["R04_deep"] }), &t));
    assert!(passes(json!({ "type": "forbidden_step", "steps": ["R04_deep"] }), &t));
    assert!(!passes(json!({ "type": "forbidden_step", "steps": ["mut.create_file"] }), &t));
    assert!(passes(json!({ "type": "effect_count", "max": 1 }), &t));
    assert!(!passes(json!({ "type": "effect_count", "max": 0 }), &t));
    assert!(passes(json!({ "type": "effect_count", "min": 0, "max": 0, "tool": "tool.execute" }), &t));
    assert!(!passes(json!({ "type": "effect_count", "min": 2 }), &t));
    assert!(passes(json!({ "type": "must_cite", "min": 2 }), &t), "two distinct sources");
    assert!(!passes(json!({ "type": "must_cite", "min": 3 }), &t));
    assert!(passes(json!({ "type": "verifier_pass" }), &t));
    let mut unverified = t.clone();
    unverified.verifier = Some(false);
    assert!(!passes(json!({ "type": "verifier_pass" }), &unverified));
    assert!(passes(json!({ "type": "cost_ceiling", "max_usd": 0.05, "max_tokens": 2000 }), &t));
    assert!(!passes(json!({ "type": "cost_ceiling", "max_tokens": 1000 }), &t));
    assert!(passes(json!({ "type": "latency_ceiling", "max_ms": 1000 }), &t));
    assert!(!passes(json!({ "type": "latency_ceiling", "max_ms": 500 }), &t));
    assert!(passes(json!({ "type": "status", "equals": "completed" }), &t));
    assert!(passes(json!({ "type": "stopped_at", "step": "R08" }), &t));
    assert!(passes(json!({ "type": "missing_evidence", "equals": [] }), &t));
    let gs: Vec<Box<dyn Grader>> = vec![g(json!({ "type": "verifier_pass" })), g(json!({ "type": "effect_count", "max": 0 }))];
    let (all, grades) = grade_all(&gs, &t);
    assert!(!all);
    assert_eq!(grades.iter().map(|x| (x.grader, x.pass)).collect::<Vec<_>>(), vec![("verifier_pass", true), ("effect_count", false)]);

    for bad in [json!({}), json!({ "type": "nope" }), json!({ "type": "step_order", "steps": [] }), json!({ "type": "effect_count" }),
                json!({ "type": "effect_count", "min": 3, "max": 1 }), json!({ "type": "cost_ceiling" }), json!({ "type": "latency_ceiling" }),
                json!({ "type": "verifier_pass", "x": 1 }), json!({ "type": "must_cite", "min": -1 }), json!({ "type": "status" })] {
        assert!(parse(&bad).is_err(), "accepted {bad}");
    }
}

#[test]
fn eval_bootstrap_ci_is_deterministic_and_sane() {
    let outcomes: Vec<bool> = (0..40).map(|i| i % 4 != 0).collect(); // 30/40
    let a = stats::bootstrap_ci95(&outcomes, 42, 2000).unwrap();
    let b = stats::bootstrap_ci95(&outcomes, 42, 2000).unwrap();
    assert_eq!(a, b, "same seed, same interval");
    assert!(a.0 <= 0.75 && 0.75 <= a.1 && a.0 >= 0.5 && a.1 <= 0.95, "{a:?}");
    assert_ne!(stats::bootstrap_ci95(&outcomes, 43, 2000).unwrap(), (0.0, 0.0));
    assert_eq!(stats::bootstrap_ci95(&[true; 10], 1, 500), Some((1.0, 1.0)));
    assert_eq!(stats::bootstrap_ci95(&[false; 3], 1, 500), Some((0.0, 0.0)));
    assert_eq!(stats::bootstrap_ci95(&[], 1, 500), None);
    // The generator itself is pinned (a change here changes every committed interval).
    let mut r = stats::SplitMix64::new(0);
    assert_eq!(r.next_u64(), 0xE220_A839_7B1D_CDAF);
    let m = suite_metrics(&[
        json!({ "case_id": "a", "skipped": false, "pass": true, "passes": 3, "k": 3, "stakes": "high" }),
        json!({ "case_id": "b", "skipped": false, "pass": false, "passes": 1, "k": 3, "stakes": "high" }),
        json!({ "case_id": "c", "skipped": false, "pass": true, "passes": 2, "k": 3, "stakes": "normal" }),
        json!({ "case_id": "d", "skipped": true, "pass": false, "k": 1, "stakes": "normal" }),
    ], 7, 1000);
    assert_eq!((m["cases"].as_u64(), m["graded"].as_u64(), m["skipped"].as_u64(), m["passed"].as_u64()), (Some(4), Some(3), Some(1), Some(2)));
    assert_eq!(m["pass_rate"].as_f64(), Some(0.6667));
    assert_eq!(m["attempt_pass_rate"].as_f64(), Some(0.6667));
    assert_eq!(m["high_stakes_failures"], json!(["b"]));
    assert_eq!((m["ci95"]["seed"].as_u64(), m["ci95"]["resamples"].as_u64()), (Some(7), Some(1000)));
    assert_eq!(suite_metrics(&[], 7, 1000)["ci95"], Value::Null);
}

#[test]
fn eval_suite_format_validates() {
    let ok = json!({ "schema": SUITE_SCHEMA, "id": "s1", "name": "S", "cases": [
        { "id": "a", "task_type": "general_task", "graders": [{ "type": "verifier_pass" }], "k": 3, "pass_rule": { "n_of_k": 2 } },
        { "id": "b", "source_run_id": "run_abc", "graders": [{ "type": "verifier_pass" }], "stakes": "high",
          "overrides": { "model_class": "mc.local" }, "fixture": { "task_type": "RESEARCH_DOC", "params": { "writable_resources": ["fs:docs/x.md"] } } }
    ] });
    let s = parse_suite(&ok).unwrap();
    assert_eq!((s.id.as_str(), s.cases.len(), s.cases[0].k, s.cases[0].rule.clone()), ("s1", 2, 3, PassRule::NOfK(2)));
    assert!(matches!(&s.cases[0].source, Source::Scripted(f) if f.task_type == "GENERAL_TASK"));
    assert!(matches!(&s.cases[1].source, Source::Run { fixture: Some(_), .. }));
    assert!(s.cases[1].high_stakes && s.cases[1].rule == PassRule::AllOfK);
    assert!(PassRule::NOfK(2).passes(2, 3) && !PassRule::AllOfK.passes(2, 3));

    let bad = |patch: &dyn Fn(&mut Value), param: &str| {
        let mut v = ok.clone();
        patch(&mut v);
        let f = parse_suite(&v).err().unwrap_or_else(|| panic!("accepted {v}"));
        assert_eq!(f.param.as_deref(), Some(param), "{}", f.message);
    };
    bad(&|v| v["schema"] = json!("x"), "schema");
    bad(&|v| v["cases"][0]["k"] = json!(11), "cases[0].k");
    bad(&|v| v["cases"][0]["pass_rule"] = json!({ "n_of_k": 4 }), "cases[0].pass_rule");
    bad(&|v| v["cases"][0]["graders"] = json!([]), "cases[0].graders");
    bad(&|v| v["cases"][0]["graders"][0] = json!({ "type": "nope" }), "cases[0].graders[0]");
    bad(&|v| v["cases"][0]["task_type"] = json!("NOPE"), "cases[0].task_type");
    bad(&|v| v["cases"][0]["overrides"] = json!({ "model_class": "mc.local" }), "cases[0].overrides");
    bad(&|v| v["cases"][1]["pass_rule"] = json!({ "n_of_k": 1 }), "cases[1].pass_rule");
    bad(&|v| v["cases"][1]["overrides"] = json!({ "nope": 1 }), "cases[1].overrides");
    bad(&|v| v["cases"][1]["id"] = json!("a"), "cases");
    bad(&|v| v["cases"][1]["source_run_id"] = json!("x"), "cases[1].source_run_id");
    bad(&|v| v["cases"][0]["script"] = json!({ "fail": 3 }), "cases[0].script");
    bad(&|v| v["extra"] = json!(1), "extra");
}

#[tokio::test]
async fn eval_gate_offline_report() {
    // The CI gate's input: every shipped eval set and committed suite, scripted.
    let report = offline_report(stats::DEFAULT_SEED).await;
    assert_eq!(report["schema"], REPORT_SCHEMA);
    assert_eq!(report, offline_report(stats::DEFAULT_SEED).await, "deterministic");
    if let Ok(path) = std::env::var("AGENCY_EVAL_REPORT") {
        std::fs::write(&path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    let baseline: Value = serde_json::from_str(include_str!("baseline.v1.json")).unwrap();
    let suites = report["suites"].as_object().unwrap();
    for (name, b) in baseline["suites"].as_object().unwrap() {
        let s = &suites[name];
        assert_eq!(s["pass_rate"], b["pass_rate"], "{name}: {s}");
        assert_eq!(s["graded"], b["graded"], "{name}: {s}");
        assert_eq!(s["high_stakes_failures"], json!([]), "{name}: {s}");
    }
    assert_eq!(suites.len(), baseline["suites"].as_object().unwrap().len(), "every suite is in the baseline");
    let core = &suites["core"];
    assert_eq!(core["skipped"], 0, "the promoted case runs offline from its fixture: {core}");
}

#[tokio::test]
async fn eval_scripted_cases_follow_their_pass_rules() {
    let suite = parse_suite(&json!({ "schema": SUITE_SCHEMA, "id": "r", "name": "rules", "cases": [
        { "id": "ok", "task_type": "GENERAL_TASK", "script": { "wait": { "G06": "open" } }, "graders": [{ "type": "verifier_pass" }], "k": 3, "stakes": "high" },
        { "id": "bad_high", "task_type": "GENERAL_TASK", "graders": [{ "type": "verifier_pass" }], "k": 2, "stakes": "high" },
        { "id": "replay_offline", "source_run_id": "run_missing", "graders": [{ "type": "verifier_pass" }] }
    ] })).unwrap();
    let r = run_suite(&Ctx::Offline, &suite, 9, 500).await;
    let c = r["cases"].as_array().unwrap();
    assert_eq!((c[0]["pass"].as_bool(), c[0]["passes"].as_u64(), c[0]["attempts"].as_array().unwrap().len()), (Some(true), Some(3), 3));
    assert_eq!((c[1]["pass"].as_bool(), c[1]["via"].as_str()), (Some(false), Some("scripted")));
    assert_eq!(c[1]["attempts"][0]["grades"][0]["detail"], "verifier not reached, status waiting");
    assert_eq!((c[2]["skipped"].as_bool(), c[2]["skip_reason"]["code"].as_str()), (Some(true), Some("ERR_EVAL_OFFLINE")));
    assert_eq!(r["metrics"]["high_stakes_failures"], json!(["bad_high"]));
    assert_eq!((r["metrics"]["graded"].as_u64(), r["metrics"]["pass_rate"].as_f64()), (Some(2), Some(0.5)));
}

// ── end to end: replay-backed suites and promotion ──────────────────────────

const USER: &str = "u_eval";

async fn finished_run(t: &T, key: &str, tt: &str, goal: &str, resources: Option<Value>) -> String {
    let mut body = json!({ "goal": goal, "task_type": tt });
    if let Some(r) = resources {
        body["workspace"] = json!({ "resources": r });
    }
    let (st, _, b) = call(&t.app, post("/v1/agency", USER, Some(key), body)).await;
    assert_eq!(st, StatusCode::ACCEPTED, "{b}");
    let id = serde_json::from_str::<Value>(&b).unwrap()["id"].as_str().unwrap().to_string();
    let s = AgencyStore::new(t.st.rails.ledger.clone());
    let limits = run_limits(&[(guard::ORGS_ENV, &format!("user:{USER}")), (guard::ORG_RUNS_PER_HOUR_ENV, "1000")]);
    assert!(executor::admit_and_start(t.st.clone(), id.clone(), limits.clone()).await);
    let mut rec = wait_settled(&s, &id).await;
    for _ in 0..3 {
        if rec.run["status"] != "needs_attention" {
            break;
        }
        let att = rec.attention.iter().find(|a| a["status"] == "open").cloned().unwrap();
        let (st, _, b) = call(&t.app, post(&format!("/v1/attention/{}/responses", att["id"].as_str().unwrap()), USER, None, json!({ "type": "approval" }))).await;
        assert_eq!(st, StatusCode::OK, "{b}");
        assert!(executor::admit_and_start(t.st.clone(), id.clone(), limits.clone()).await);
        rec = wait_settled(&s, &id).await;
    }
    assert_eq!(rec.run["status"], "completed", "{}", rec.run);
    id
}

fn journal_rows(t: &T, run: &str) -> i64 {
    t.st.db.connect().unwrap().query_row("SELECT COUNT(*) FROM agency_effect_journal WHERE run_id = ?1", [run], |r| r.get(0)).unwrap()
}

fn receipts(t: &T, run: &str) -> usize {
    t.st.rails.receipts.chain_store().unwrap().read_run(run).unwrap().len()
}

async fn post_json(t: &T, uri: &str, user: &str, body: Value) -> (StatusCode, Value) {
    let (st, _, b) = call(&t.app, post(uri, user, None, body)).await;
    (st, serde_json::from_str(&b).unwrap_or(Value::Null))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn eval_suites_run_through_recorded_only_replay_and_promote_runs() {
    let _env = E2E_ENV.lock().await;
    let runs = tempfile::tempdir().unwrap();
    std::env::set_var("ALLTERNIT_AGENCY_COGNITION", "scripted");
    std::env::set_var("ALLTERNIT_AGENCY_RUNS_DIR", runs.path());
    std::env::set_var(task_types::ENABLE_ENV, format!("BUG_FIX,{}", task_types::ids().join(",")));
    let t = setup().await;
    let research = finished_run(&t, "c5-e2e-research-0001", "RESEARCH_DOC", "Write a brief", Some(json!(["fs:docs/brief.md"]))).await;
    let (j0, c0) = (journal_rows(&t, &research), receipts(&t, &research));

    // The trace monitor checked the finished run (off path; poll).
    let s = AgencyStore::new(t.st.rails.ledger.clone());
    let mut findings = None;
    for _ in 0..100 {
        findings = monitor::stored(&s, &research, USER).await.unwrap();
        if findings.is_some() { break; }
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }
    let findings = findings.expect("monitor ran on the terminal transition");
    assert!(!findings["data"].as_array().unwrap().iter().any(|f| f["check"] == "effect_without_policy_allow" || f["check"] == "effect_content_divergence"),
        "a clean scripted run: {findings}");

    // Promote it: a regression case in the default suite, snapshot without the goal text.
    let (st, c) = post_json(&t, &format!("/v1/evals/cases/from-run/{research}"), USER, json!({ "k": 2 })).await;
    assert_eq!(st, StatusCode::CREATED, "{c}");
    assert_eq!(c["suite_id"], DEFAULT_REGRESSION_SUITE);
    let case = &c["case"];
    assert_eq!((case["source_run_id"].as_str(), case["k"].as_u64()), (Some(research.as_str()), Some(2)));
    assert!(case["snapshot"]["goal_sha256"].as_str().unwrap().starts_with("sha256:"));
    assert!(!case.to_string().contains("Write a brief"), "no goal text in the case: {case}");
    assert_eq!(case["snapshot"]["verifier"], "pass");
    assert_eq!(case["fixture"]["task_type"], "RESEARCH_DOC");
    let kinds: Vec<&str> = case["graders"].as_array().unwrap().iter().map(|g| g["type"].as_str().unwrap()).collect();
    assert!(kinds.contains(&"verifier_pass") && kinds.contains(&"effect_count") && kinds.contains(&"step_order"), "{kinds:?}");
    // Not someone else's run; not an unfinished field.
    let (st, _) = post_json(&t, &format!("/v1/evals/cases/from-run/{research}"), "u_someone", json!({})).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, e) = post_json(&t, &format!("/v1/evals/cases/from-run/{research}"), USER, json!({ "nope": 1 })).await;
    assert_eq!((st, e["error"]["param"].as_str()), (StatusCode::BAD_REQUEST, Some("nope")));

    // Add a scripted case with a must-fail high-stakes grader to a second suite, inline.
    let (st, sv) = post_json(&t, "/v1/evals/suites", USER, json!({ "schema": SUITE_SCHEMA, "id": "mine", "name": "Mine", "cases": [
        { "id": "replayed", "source_run_id": research, "graders": [{ "type": "verifier_pass" }, { "type": "effect_count", "max": 1 },
          { "type": "step_order", "steps": ["R06", "R07"] }], "k": 3, "pass_rule": "all_of_k", "stakes": "high" },
        { "id": "candidate_model", "source_run_id": research, "overrides": { "model_class": "mc.local" }, "graders": [{ "type": "verifier_pass" }], "k": 2, "pass_rule": { "n_of_k": 1 } },
        { "id": "scripted", "task_type": "GENERAL_TASK", "script": { "wait": { "G06": "open" } }, "graders": [{ "type": "verifier_pass" }] }
    ] })).await;
    assert_eq!(st, StatusCode::CREATED, "{sv}");
    let (st, _, b) = call(&t.app, get_req("/v1/evals/suites/mine", "u_someone")).await;
    assert_eq!(st, StatusCode::NOT_FOUND, "suites are owner scoped: {b}");

    let (st, e) = post_json(&t, "/v1/evals/runs", USER, json!({ "suite_id": "mine", "effects": "live" })).await;
    assert_eq!((st, e["error"]["param"].as_str()), (StatusCode::BAD_REQUEST, Some("effects")));
    let (st, run) = post_json(&t, "/v1/evals/runs", USER, json!({ "suite_id": "mine", "seed": 11 })).await;
    assert_eq!(st, StatusCode::ACCEPTED, "{run}");
    assert_eq!((run["object"].as_str(), run["status"].as_str(), run["effects"].as_str()), (Some("eval_run"), Some("completed"), Some("recorded_only")));
    let cases = run["suites"][0]["cases"].as_array().unwrap();
    let by = |id: &str| cases.iter().find(|c| c["case_id"] == id).unwrap().clone();
    let rep = by("replayed");
    assert_eq!((rep["via"].as_str(), rep["pass"].as_bool(), rep["passes"].as_u64()), (Some("replay"), Some(true), Some(3)), "{rep}");
    let rid = rep["attempts"][0]["replay_id"].as_str().unwrap();
    let (st, _, b) = call(&t.app, get_req(&format!("/v1/replays/{rid}"), USER)).await;
    assert_eq!(st, StatusCode::OK, "each attempt is a stored replay: {b}");
    assert_eq!(serde_json::from_str::<Value>(&b).unwrap()["effects"], "recorded_only");
    let cand = by("candidate_model");
    assert_eq!((cand["pass"].as_bool(), cand["attempts"][0]["verdict"].as_str()), (Some(true), Some("equivalent")), "{cand}");
    assert_eq!(by("scripted")["via"], "scripted");
    let m = &run["metrics"];
    assert_eq!((m["graded"].as_u64(), m["passed"].as_u64(), m["pass_rate"].as_f64()), (Some(3), Some(3), Some(1.0)));
    assert_eq!((m["ci95"]["low"].as_f64(), m["ci95"]["high"].as_f64(), m["ci95"]["seed"].as_u64()), (Some(1.0), Some(1.0), Some(11)));
    // Readable later, by its owner only.
    let id = run["id"].as_str().unwrap();
    let (st, _, b) = call(&t.app, get_req(&format!("/v1/evals/runs/{id}"), USER)).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(serde_json::from_str::<Value>(&b).unwrap()["metrics"], run["metrics"]);
    let (st, _, _) = call(&t.app, get_req(&format!("/v1/evals/runs/{id}"), "u_someone")).await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // The promoted regression suite plus the shipped sets; another user's
    // eval sees no run (skipped), never the recording.
    let (st, run) = post_json(&t, "/v1/evals/runs", USER, json!({ "suite_id": DEFAULT_REGRESSION_SUITE, "include_shipped": true })).await;
    assert_eq!(st, StatusCode::ACCEPTED, "{run}");
    assert_eq!(run["suites"].as_array().unwrap().len(), 1 + SHIPPED_EVALS.len());
    assert_eq!(run["suites"][0]["cases"][0]["pass"], true, "{}", run["suites"][0]);
    assert_eq!(run["metrics"]["high_stakes_failures"], json!([]));
    let (st, other) = post_json(&t, "/v1/evals/runs", "u_someone", json!({ "suite": { "schema": SUITE_SCHEMA, "name": "peek", "cases": [
        { "id": "x", "source_run_id": research, "graders": [{ "type": "verifier_pass" }] }] } })).await;
    assert_eq!(st, StatusCode::ACCEPTED, "{other}");
    assert_eq!(other["suites"][0]["cases"][0]["skip_reason"]["code"], "ERR_NOT_FOUND");

    // Nothing happened for real.
    assert_eq!((journal_rows(&t, &research), receipts(&t, &research)), (j0, c0), "evals are recorded_only");
}
