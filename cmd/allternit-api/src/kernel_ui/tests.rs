use super::*;
use crate::agency_api::store::{AgencyStore, RunRecord};
use crate::test_helpers::app_state;
use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt;

fn user(id: &str, org: Option<&str>) -> AuthUser {
    AuthUser { user_id: id.into(), email: None, name: None, avatar_url: None, tenant_id: None, organization_id: org.map(str::to_string), organization_role: None, organization_slug: None }
}

struct T { _d: tempfile::TempDir, st: Arc<AppState>, app: Router }

async fn setup() -> T {
    let d = tempfile::tempdir().unwrap();
    let st = app_state(d.path()).await;
    let app = router().with_state(st.clone());
    T { _d: d, st, app }
}

async fn call(t: &T, m: &str, uri: &str, u: &AuthUser, body: Option<Value>) -> (StatusCode, Value, axum::http::HeaderMap) {
    let r = Request::builder().method(m).uri(uri).header("content-type", "application/json").extension(u.clone());
    let b = body.map(|b| Body::from(b.to_string())).unwrap_or_else(Body::empty);
    let resp = t.app.clone().oneshot(r.body(b).unwrap()).await.unwrap();
    let (p, body) = resp.into_parts();
    let bytes = axum::body::to_bytes(body, 1 << 22).await.unwrap();
    (p.status, serde_json::from_slice(&bytes).unwrap_or(Value::Null), p.headers)
}

#[tokio::test]
async fn rules_defaults_put_inherit_reset_and_locked_field() {
    let t = setup().await;
    let u = user("u1", Some("acme"));
    let (s, v, _) = call(&t, "GET", "/v1/agent-rules?scope=org:acme", &u, None).await;
    assert_eq!(s, 200);
    assert_eq!(v["effective"]["guardrails"]["level"], "guardrails");
    assert_eq!(v["sources"]["guardrails.level"], "default");
    assert_eq!(v["effective"]["completion"]["agent_may_self_close"], false);
    // org override, workspace override of another field
    let (s, _, _) = call(&t, "PUT", "/v1/agent-rules?scope=org:acme", &u, Some(json!({ "guardrails": { "level": "strict" }, "budgets": { "daily_usd": 5 } }))).await;
    assert_eq!(s, 200);
    let (_, v, _) = call(&t, "PUT", "/v1/agent-rules?scope=workspace:w1", &u, Some(json!({ "budgets": { "daily_usd": null }, "approvals": { "spend_over_usd": 2.5 } }))).await;
    assert_eq!(v["effective"]["guardrails"]["level"], "strict", "inherited from org");
    assert_eq!(v["sources"]["guardrails.level"], "org");
    assert_eq!(v["effective"]["budgets"]["daily_usd"], Value::Null, "explicit null override wins");
    assert_eq!(v["sources"]["budgets.daily_usd"], "workspace");
    assert_eq!(v["sources"]["approvals.spend_over_usd"], "workspace");
    // project scope with explicit parents chain
    let (_, v, _) = call(&t, "GET", "/v1/agent-rules?scope=project:p1&parents=workspace:w1,org:acme", &u, None).await;
    assert_eq!(v["sources"]["approvals.spend_over_usd"], "workspace");
    // reset falls back to parent
    let (s, v, _) = call(&t, "DELETE", "/v1/agent-rules/field?scope=workspace:w1&path=budgets.daily_usd", &u, None).await;
    assert_eq!(s, 200);
    assert_eq!(v["effective"]["budgets"]["daily_usd"], 5.0);
    assert_eq!(v["sources"]["budgets.daily_usd"], "org");
    // validation, locked field, unknown path, foreign org
    assert_eq!(call(&t, "PUT", "/v1/agent-rules?scope=org:acme", &u, Some(json!({ "guardrails": { "level": "nope" } }))).await.0, 422);
    assert_eq!(call(&t, "PUT", "/v1/agent-rules?scope=org:acme", &u, Some(json!({ "completion": { "agent_may_self_close": true } }))).await.0, 422);
    assert_eq!(call(&t, "PUT", "/v1/agent-rules?scope=org:acme", &u, Some(json!({ "custom": [{ "id": "a", "text": "t", "when": "w", "action": "allow" }] }))).await.0, 422);
    assert_eq!(call(&t, "DELETE", "/v1/agent-rules/field?scope=org:acme&path=nope", &u, None).await.0, 422);
    assert_eq!(call(&t, "GET", "/v1/agent-rules?scope=org:other", &u, None).await.0, 403);
    assert_eq!(call(&t, "GET", "/v1/agent-rules?scope=bogus", &u, None).await.0, 422);
}

#[tokio::test]
async fn credential_read_rule_reaches_judge_policy() {
    let t = setup().await;
    let u = user("u1", Some("acme"));
    call(&t, "PUT", "/v1/agent-rules?scope=org:acme", &u, Some(json!({ "guardrails": { "allow_credential_read": ["~/.config/tool"] } }))).await;
    let mut p = allternit_commrails::judge::policy::JudgePolicy::default();
    agent_rules::apply_to_policy(&t.st, "acme", &mut p).await;
    assert_eq!(p.allow_credential_read, Some(vec!["~/.config/tool".to_string()]));
    let mut other = allternit_commrails::judge::policy::JudgePolicy::default();
    agent_rules::apply_to_policy(&t.st, "nobody", &mut other).await;
    assert_eq!(other.allow_credential_read, None);
}

#[tokio::test]
async fn routing_policy_scopes_backends_and_s1_change_resets_to_shadow() {
    let t = setup().await;
    let u = user("u1", Some("acme"));
    let (_, v, _) = call(&t, "GET", "/v1/routing-policy?scope=org:acme", &u, None).await;
    assert_eq!(v["s1_backend"], "off");
    assert_eq!(v["local_only"], false);
    let (s, v, _) = call(&t, "PUT", "/v1/routing-policy?scope=workspace:w1", &u, Some(json!({ "s2": { "default": "fast", "overrides": { "code": "big" } }, "local_only": true }))).await;
    assert_eq!(s, 200);
    assert_eq!(v["s2"]["overrides"]["code"], "big");
    assert_eq!(v["sources"]["s2.default"], "workspace");
    assert_eq!(v["decision_types_reset"], false);
    assert_eq!(call(&t, "PUT", "/v1/routing-policy?scope=org:acme", &u, Some(json!({ "s1_backend": "bogus" }))).await.0, 422);
    // jev_api is unavailable without TYPESAFE_API_KEY, and the key is never echoed
    std::env::remove_var("TYPESAFE_API_KEY");
    assert_eq!(call(&t, "PUT", "/v1/routing-policy?scope=org:acme", &u, Some(json!({ "s1_backend": "jev_api" }))).await.0, 409);
    let (s, b, _) = call(&t, "GET", "/v1/routing-policy/backends", &u, None).await;
    assert_eq!(s, 200);
    let jev = b["s1"].as_array().unwrap().iter().find(|x| x["id"] == "jev_api").unwrap();
    assert_eq!(jev["available"], false);
    assert!(b["models"].is_array() && (b["pool_error"].is_string() || b["pool_error"].is_null()), "pool read or reported, never a failure");
    // promote a type to live (manifest bound to laya_bundled), then change backend -> shadow
    let dir = tempfile::tempdir().unwrap();
    let mp = dir.path().join("m.json");
    std::fs::write(&mp, json!([{ "primitive_id": "GATE", "gate": { "passed": true }, "scope": { "backend_id": "laya-bundled" } }]).to_string()).unwrap();
    std::env::set_var("ALLTERNIT_S1_MANIFESTS", &mp);
    assert_eq!(call(&t, "PUT", "/v1/routing-policy?scope=org:acme", &u, Some(json!({ "s1_backend": "laya_bundled" }))).await.0, 200);
    let (s, v, _) = call(&t, "PUT", "/v1/decision-types/GATE/status?scope=org:acme", &u, Some(json!({ "status": "live" }))).await;
    assert_eq!((s, v["status"].as_str()), (StatusCode::OK, Some("live")));
    let (s, v, _) = call(&t, "PUT", "/v1/routing-policy?scope=org:acme", &u, Some(json!({ "s1_backend": "off" }))).await;
    assert_eq!((s, &v["decision_types_reset"]), (StatusCode::OK, &json!(true)));
    let (_, list, _) = call(&t, "GET", "/v1/decision-types?scope=org:acme", &u, None).await;
    assert!(list.as_array().unwrap().iter().all(|d| d["status"] == "shadow"), "{list}");
    // back to the bound backend: still shadow until re-promoted
    call(&t, "PUT", "/v1/routing-policy?scope=org:acme", &u, Some(json!({ "s1_backend": "laya_bundled" }))).await;
    let (_, list, _) = call(&t, "GET", "/v1/decision-types?scope=org:acme", &u, None).await;
    let gate = list.as_array().unwrap().iter().find(|d| d["id"] == "GATE").unwrap();
    assert_eq!((gate["status"].as_str(), gate["calibration"]["qualified"].clone()), (Some("shadow"), json!(true)));
    // reset of s1_backend via DELETE also reports a reset
    let (_, v, _) = call(&t, "DELETE", "/v1/routing-policy/field?scope=org:acme&path=s1_backend", &u, None).await;
    assert_eq!(v["s1_backend"], "off");
    std::env::remove_var("ALLTERNIT_S1_MANIFESTS");
}

#[test]
fn pool_summary_keeps_only_safe_fields() {
    let v = summarize(json!({ "entries": [{ "backend_id": "b1", "cognitive_roles": ["executor"], "capabilities": ["code"], "residency": "local", "cost": 1.0, "extensions": { "x-model_ref": "p/m" } }] }));
    assert_eq!(v[0]["backend_id"], "b1");
    assert!(v[0].get("extensions").is_none() && v[0].get("cost").is_none());
}
fn summarize(v: Value) -> Vec<Value> { routing_policy::summarize_pool(&v) }

#[tokio::test]
async fn decision_types_builtin_calibration_and_custom() {
    let t = setup().await;
    let u = user("u1", None);
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("decisions")).unwrap();
    std::fs::create_dir_all(dir.path().join("outcomes")).unwrap();
    std::fs::write(dir.path().join("decisions/d.jsonl"), "{\"decision_id\":\"a\",\"primitive_id\":\"ROUTE\"}\n{\"decision_id\":\"b\",\"primitive_id\":\"ROUTE\"}\n").unwrap();
    std::fs::write(dir.path().join("outcomes/d.jsonl"), "{\"decision_id\":\"a\",\"truth\":\"x\",\"source\":\"verifier\"}\n").unwrap();
    std::env::set_var("ALLTERNIT_S1_SHADOW_DIR", dir.path());
    let (s, list, _) = call(&t, "GET", "/v1/decision-types", &u, None).await;
    assert_eq!(s, 200);
    let route = list.as_array().unwrap().iter().find(|d| d["id"] == "ROUTE").unwrap();
    assert_eq!((route["calibration"]["rows"].as_u64(), route["calibration"]["needed"].as_u64(), route["status"].as_str()), (Some(1), Some(300), Some("shadow")));
    std::env::remove_var("ALLTERNIT_S1_SHADOW_DIR");
    // live refused without a qualifying manifest
    assert_eq!(call(&t, "PUT", "/v1/decision-types/ROUTE/status", &u, Some(json!({ "status": "live" }))).await.0, 409);
    // custom: always shadow, no grant/close, no collisions
    let (s, v, _) = call(&t, "POST", "/v1/decision-types", &u, Some(json!({ "id": "refund_gate", "operation": "GATE", "category": "gate", "inputs": "ticket", "status": "live" }))).await;
    assert_eq!((s, v["status"].as_str(), v["builtin"].clone()), (StatusCode::OK, Some("shadow"), json!(false)));
    assert_eq!(call(&t, "POST", "/v1/decision-types", &u, Some(json!({ "id": "refund_gate", "operation": "GATE" }))).await.0, 409);
    assert_eq!(call(&t, "POST", "/v1/decision-types", &u, Some(json!({ "id": "route", "operation": "GATE" }))).await.0, 409);
    assert_eq!(call(&t, "POST", "/v1/decision-types", &u, Some(json!({ "id": "x1", "operation": "GATE", "authority": "may grant access" }))).await.0, 422);
    assert_eq!(call(&t, "POST", "/v1/decision-types", &u, Some(json!({ "id": "x2", "operation": "NOPE" }))).await.0, 422);
    let (_, list, _) = call(&t, "GET", "/v1/decision-types", &u, None).await;
    assert!(list.as_array().unwrap().iter().any(|d| d["id"] == "refund_gate"));
    assert!(call(&t, "GET", "/v1/decision-types", &user("u2", None), None).await.1.as_array().unwrap().iter().all(|d| d["id"] != "refund_gate"), "customs are per owner");
}

fn tpl(slash: &str, steps: Value) -> Value {
    json!({ "name": "Check", "category": "qa", "slash": slash, "steps": steps, "completion": "all_pass", "auto_run": { "before_done_of": "bug_fix" } })
}

async fn wait_terminal(t: &T, run_id: &str) -> Value {
    let s = AgencyStore::new(t.st.rails.ledger.clone());
    for _ in 0..200 {
        let r = s.load_run(run_id).await.unwrap().unwrap().run;
        if r["terminal"] == true { return r; }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("run did not finish");
}

#[tokio::test]
async fn templates_crud_and_derived_flags() {
    let t = setup().await;
    let u = user("u1", None);
    let (s, a, _) = call(&t, "POST", "/v1/templates", &u, Some(tpl("qa", json!([{ "kind": "s0_command", "label": "tests", "command": ["true"] }, { "kind": "verifier", "label": "verify" }])))).await;
    assert_eq!(s, 200, "{a}");
    assert_eq!((a["uses_model"].clone(), a["reproducible"].clone()), (json!(false), json!(false)));
    let id = a["id"].as_str().unwrap().to_string();
    assert_eq!(call(&t, "POST", "/v1/templates", &u, Some(tpl("qa", json!([{ "kind": "verifier", "label": "v" }])))).await.0, 409, "slash unique per scope");
    assert_eq!(call(&t, "POST", "/v1/templates", &u, Some(tpl("bad", json!([{ "kind": "s0_command", "label": "x" }])))).await.0, 422, "command required");
    // update: model step flips uses_model, pinned inputs flip reproducible (both server-derived)
    let mut upd = tpl("qa", json!([{ "kind": "s2_generate", "label": "draft", "capability": "text" }]));
    upd["pinned_inputs"] = json!({ "seed": 1 });
    upd["reproducible"] = json!(false);
    let (s, b, _) = call(&t, "PUT", &format!("/v1/templates/{id}"), &u, Some(upd)).await;
    assert_eq!((s, b["uses_model"].clone(), b["reproducible"].clone()), (StatusCode::OK, json!(true), json!(true)));
    let (_, list, _) = call(&t, "GET", "/v1/templates", &u, None).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    let (_, none, _) = call(&t, "GET", "/v1/templates?scope=project:zzz", &u, None).await;
    assert_eq!(none.as_array().unwrap().len(), 0);
    assert_eq!(call(&t, "GET", "/v1/templates", &user("u2", None), None).await.1.as_array().unwrap().len(), 0, "owner-scoped");
    assert_eq!(call(&t, "DELETE", &format!("/v1/templates/{id}"), &u, None).await.0, 200);
    assert_eq!(call(&t, "DELETE", &format!("/v1/templates/{id}"), &u, None).await.0, 404);
}

#[tokio::test]
async fn template_run_s0_only_needs_no_model_and_records_speed() {
    let t = setup().await;
    let u = user("u1", None);
    let (_, ok, _) = call(&t, "POST", "/v1/templates", &u, Some(tpl("pass", json!([{ "kind": "s0_command", "label": "t", "command": ["true"] }, { "kind": "verifier", "label": "v" }])))).await;
    let (_, bad, _) = call(&t, "POST", "/v1/templates", &u, Some(tpl("fail", json!([{ "kind": "s0_command", "label": "t", "command": ["false"] }, { "kind": "verifier", "label": "v" }])))).await;
    let (_, model, _) = call(&t, "POST", "/v1/templates", &u, Some(tpl("gen", json!([{ "kind": "s2_generate", "label": "g" }])))).await;
    let (_, wait, _) = call(&t, "POST", "/v1/templates", &u, Some(tpl("hold", json!([{ "kind": "wait", "label": "w" }])))).await;
    std::env::remove_var(templates::S0_EXEC_ENV);
    let path = |v: &Value| format!("/v1/templates/{}/run", v["id"].as_str().unwrap());
    // local exec off: parked, nothing ran
    let (s, r, _) = call(&t, "POST", &path(&ok), &u, Some(json!({}))).await;
    assert_eq!((s, r["status"].as_str()), (StatusCode::OK, Some("waiting")));
    std::env::set_var(templates::S0_EXEC_ENV, "1");
    std::env::remove_var("ALLTERNIT_AGENCY_EXECUTE");
    let (s, r, _) = call(&t, "POST", &path(&ok), &u, Some(json!({ "thread_id": "thr_x" }))).await;
    assert_eq!(s, 200);
    let run = wait_terminal(&t, r["run_id"].as_str().unwrap()).await;
    assert_eq!((run["status"].as_str(), run["thread_id"].as_str()), (Some("completed"), Some("thr_x")));
    assert!(run["speed"]["duration_ms"].is_number() && run["speed"]["tokens"] == 0, "{run}");
    let evs = AgencyStore::new(t.st.rails.ledger.clone()).events(run["id"].as_str().unwrap()).await.unwrap();
    let prog: Vec<&Value> = evs.iter().filter(|e| e["type"] == "run.progress").collect();
    assert_eq!(prog.len(), 2);
    for k in ["started_at", "duration_ms", "tokens_in", "tokens_out", "tok_per_s", "wait_ms"] {
        assert!(prog[0]["data"].get(k).is_some(), "missing {k}");
    }
    assert_eq!(prog[0]["data"]["tok_per_s"], Value::Null, "S0 step has no model speed");
    // failing command fails the run and the verifier does not pass
    let (_, r, _) = call(&t, "POST", &path(&bad), &u, None).await;
    assert_eq!(wait_terminal(&t, r["run_id"].as_str().unwrap()).await["status"], "failed");
    // model steps never start without the executor; unsupported S0 kinds refused
    let (_, r, _) = call(&t, "POST", &path(&model), &u, None).await;
    assert_eq!(r["status"], "waiting");
    assert_eq!(call(&t, "POST", &path(&wait), &u, None).await.0, 422);
    assert_eq!(call(&t, "POST", "/v1/templates/tpl_missing/run", &u, None).await.0, 404);
    std::env::remove_var(templates::S0_EXEC_ENV);
}

#[tokio::test]
async fn activity_merges_hooks_and_runs_with_filters_and_paging() {
    let t = setup().await;
    let u = user("u1", None);
    let s = AgencyStore::new(t.st.rails.ledger.clone());
    let hook = allternit_commrails::hook::HOOK_EVENT;
    s.append_raw(hook, "wih_1", json!({ "wih_id": "wih_1", "harness": "claude", "tool": "Bash", "decision": "deny", "reason": "outside fence" })).await.unwrap();
    s.append_raw(hook, "wih_1", json!({ "wih_id": "wih_1", "harness": "claude", "tool": "Read", "decision": "allow", "reason": "" })).await.unwrap();
    // a verified run and a budget-halted run
    let mk = |id: &str, status: &str, halted: bool| RunRecord { owner: "u1".into(), idempotency_key: None, task_ir: json!({}), attention: vec![],
        run: json!({ "id": id, "status": status, "status_reason": "x", "terminal": true, "thread_id": "thr_1", "goal": format!("goal {id}"), "updated_at": crate::agency_api::store::now(),
            "completion": { "status": "pending" }, "budget_usage": { "tokens": 120, "cost_usd": 0.5, "spend_halted": halted }, "speed": { "duration_ms": 900, "tok_per_s": 40.0 }, "version": 1 }) };
    s.save(mk("run_a", "completed", false)).await.unwrap();
    s.save(mk("run_b", "waiting", true)).await.unwrap();
    let get = |q: &str| { let q = q.to_string(); let t = &t; let u = u.clone(); async move { call(t, "GET", &format!("/v1/activity{q}"), &u, None).await } };
    let (st_, all, _) = get("").await;
    assert_eq!(st_, 200);
    assert_eq!(all.as_array().unwrap().len(), 4, "{all}");
    let (_, denied, _) = get("?filter=denied").await;
    assert!(denied.as_array().unwrap().iter().any(|e| e["surface"] == "hook:claude" && e["result"] == "blocked"));
    assert!(denied.as_array().unwrap().iter().any(|e| e["summary"] == "goal run_b"), "halted run is blocked");
    let (_, ob, _) = get("?filter=over_budget").await;
    assert_eq!((ob.as_array().unwrap().len(), ob[0]["summary"].as_str()), (1, Some("goal run_b")));
    assert_eq!(ob[0]["tokens"], 120);
    let (_, unv, _) = get("?filter=unverified").await;
    assert!(unv.as_array().unwrap().iter().all(|e| e["result"] != "verified" && e["result"] != "blocked"));
    let (_, ap, _) = get("?filter=approval").await;
    assert_eq!(ap.as_array().unwrap().len(), 0);
    let (_, ver, _) = get("?limit=1&filter=all").await;
    assert_eq!(ver.as_array().unwrap().len(), 1);
    let (_, _, h) = get("?limit=1").await;
    let cur = h.get("x-next-cursor").unwrap().to_str().unwrap().to_string();
    let (_, p2, _) = get(&format!("?limit=10&cursor={cur}")).await;
    assert_eq!(p2.as_array().unwrap().len(), 3);
    assert_eq!(get("?filter=nope").await.0, 422);
    assert_eq!(get("?cursor=zzz").await.0, 422);
    // speed fields surface on the verified run
    let a = all.as_array().unwrap().iter().find(|e| e["summary"] == "goal run_a").unwrap();
    assert_eq!((a["result"].as_str(), a["duration_ms"].as_u64(), a["tok_per_s"].as_f64()), (Some("verified"), Some(900), Some(40.0)));
    // another user sees neither run (hooks are machine-level: no-org callers see them)
    let (_, other, _) = call(&t, "GET", "/v1/activity", &user("u2", Some("o2")), None).await;
    assert_eq!(other.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn api_v1_paths_and_kernel_run_aliases_resolve() {
    let t = setup().await;
    let app = api_router().with_state(t.st.clone());
    let u = user("u1", Some("acme"));
    let hit = |m: &'static str, uri: String, b: Option<Value>| {
        let (app, u) = (app.clone(), u.clone());
        async move {
            let req = Request::builder().method(m).uri(uri).header("content-type", "application/json").extension(u)
                .body(b.map(|b| Body::from(b.to_string())).unwrap_or_else(Body::empty)).unwrap();
            let r = app.oneshot(req).await.unwrap();
            (r.status(), r.headers().clone())
        }
    };
    for uri in ["/api/v1/agent-rules?scope=org:acme", "/api/v1/routing-policy?scope=org:acme", "/api/v1/routing-policy/backends",
                "/api/v1/decision-types", "/api/v1/templates", "/api/v1/activity"] {
        assert_eq!(hit("GET", uri.to_string(), None).await.0, 200, "{uri}");
    }
    assert_eq!(hit("PUT", "/api/v1/agent-rules?scope=org:acme".into(), Some(json!({ "budgets": { "daily_usd": 1 } }))).await.0, 200);
    assert_eq!(hit("POST", "/api/v1/templates/tpl_none/run".into(), None).await.0, 404, "route exists, template does not");
    // run aliases read the same run as /v1/runs/{id}
    let s = AgencyStore::new(t.st.rails.ledger.clone());
    s.save(RunRecord { owner: "u1".into(), idempotency_key: None, task_ir: json!({}), attention: vec![],
        run: json!({ "id": "run_alias", "object": "run", "status": "completed", "terminal": true, "version": 1 }) }).await.unwrap();
    let (st_, h) = hit("GET", "/api/v1/kernel/runs/run_alias".into(), None).await;
    assert_eq!(st_, 200);
    assert!(h.get("allternit-request-id").is_some(), "agency version layer applies");
    assert_eq!(hit("GET", "/api/v1/kernel/runs/run_alias/events".into(), None).await.0, 200);
    assert_eq!(hit("GET", "/api/v1/kernel/runs/run_nope".into(), None).await.0, 404);
    // /api/v1/runs stays Cowork's: not served by this router
    assert_eq!(hit("GET", "/api/v1/runs".into(), None).await.0, 404);
}

#[tokio::test(flavor = "multi_thread")]
async fn model_template_compiles_and_runs_on_the_executor_with_scripted_cognition() {
    use crate::agency_api::template_exec;
    let t = setup().await;
    let u = user("u2", None);
    std::env::set_var("ALLTERNIT_AGENCY_COGNITION", "scripted");
    let steps = json!([{ "kind": "s1_decision", "label": "pick" }, { "kind": "s2_generate", "label": "draft" }, { "kind": "parallel", "label": "grp" }, { "kind": "verifier", "label": "v" }]);
    let (_, m, _) = call(&t, "POST", "/v1/templates", &u, Some(tpl("mgen", steps))).await;
    // compiled graph: one node per step, an S2 fallback for the s1 step, verifier completes
    let g = template_exec::compile(&m).unwrap();
    assert_eq!((g.nodes.len(), g.edges.len(), g.completion_nodes.clone()), (5, 3, vec!["T04".to_string()]));
    assert!(template_exec::compile(&json!({ "id": "x", "steps": [{ "kind": "s2_generate", "label": "g" }] })).is_err(), "must end in a verifier");

    let store = AgencyStore::new(t.st.rails.ledger.clone());
    let drive = |rid: String| {
        let (h, st, s) = (tokio::runtime::Handle::current(), t.st.clone(), AgencyStore::new(t.st.rails.ledger.clone()));
        async move { tokio::task::spawn_blocking(move || template_exec::drive(&h, &st, &s, &rid, "default")).await.unwrap().unwrap() }
    };
    let (_, r, _) = call(&t, "POST", &format!("/v1/templates/{}/run", m["id"].as_str().unwrap()), &u, Some(json!({ "inputs": { "topic": "x" } }))).await;
    let rid = r["run_id"].as_str().unwrap().to_string();
    assert_eq!(store.load_run(&rid).await.unwrap().unwrap().run["status"], "waiting");
    drive(rid.clone()).await;
    let run = store.load_run(&rid).await.unwrap().unwrap().run;
    assert_eq!((run["status"].as_str(), run["completion"]["status"].as_str()), (Some("completed"), Some("verified")), "{run}");
    let evs = store.events(&rid).await.unwrap();
    let prog: Vec<&Value> = evs.iter().filter(|e| e["type"] == "run.progress").collect();
    assert_eq!(prog.len(), 4);
    assert_eq!(prog[0]["data"]["cognitive_role"], "S2", "uncalibrated S1 falls back to the S2 node");
    assert_eq!(prog[3]["data"]["kind"], "verifier");
    for k in ["started_at", "duration_ms", "tokens_in", "tokens_out", "tok_per_s", "wait_ms"] {
        assert!(prog[1]["data"].get(k).is_some(), "missing {k}");
    }
    assert_eq!(store.events_of_type(crate::agency_api::executor::EV_ROUTING).await.unwrap().len(), 1, "routing trace recorded");

    // an attention step opens a real attention request and parks the run
    let (_, a, _) = call(&t, "POST", "/v1/templates", &u, Some(tpl("mask", json!([{ "kind": "s2_generate", "label": "g" }, { "kind": "attention", "label": "ok?" }, { "kind": "verifier", "label": "v" }])))).await;
    let (_, r, _) = call(&t, "POST", &format!("/v1/templates/{}/run", a["id"].as_str().unwrap()), &u, None).await;
    let rid = r["run_id"].as_str().unwrap().to_string();
    drive(rid.clone()).await;
    let run = store.load_run(&rid).await.unwrap().unwrap();
    assert_eq!((run.run["status"].as_str(), run.attention.len(), run.attention[0]["resume_from"].as_u64()), (Some("needs_attention"), 1, Some(1)));
    std::env::remove_var("ALLTERNIT_AGENCY_COGNITION");
}
