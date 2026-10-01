//! WP11 Agency API alpha tests. Run: `cargo test -p allternit-api agency_api`.

use super::*;
use crate::test_helpers::app_state;
use allternit_commrails::judge::policy::{effective_policy, CloseBy, PolicyOrigin, VerifyMode};
use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt;

fn user(id: &str) -> AuthUser {
    AuthUser { user_id: id.into(), email: None, name: None, avatar_url: None, tenant_id: None, organization_id: None, organization_role: None, organization_slug: None }
}

struct T {
    _dir: tempfile::TempDir,
    st: Arc<AppState>,
    app: Router,
}

async fn setup() -> T {
    let dir = tempfile::tempdir().unwrap();
    let st = app_state(dir.path()).await;
    let app = agency_router().merge(jwks_public_router()).with_state(st.clone());
    T { _dir: dir, st, app }
}

async fn call(app: &Router, req: Request<Body>) -> (StatusCode, HeaderMap, String) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let (parts, body) = resp.into_parts();
    let b = axum::body::to_bytes(body, 1 << 22).await.unwrap();
    (parts.status, parts.headers, String::from_utf8_lossy(&b).to_string())
}

fn post(uri: &str, u: &str, key: Option<&str>, body: Value) -> Request<Body> {
    let mut r = Request::builder().method("POST").uri(uri).header("content-type", "application/json").extension(user(u));
    if let Some(k) = key {
        r = r.header("idempotency-key", k);
    }
    r.body(Body::from(body.to_string())).unwrap()
}

fn get_req(uri: &str, u: &str) -> Request<Body> {
    Request::builder().uri(uri).extension(user(u)).body(Body::empty()).unwrap()
}

async fn create(t: &T, key: &str) -> Value {
    let (s, h, b) = call(&t.app, post("/v1/agency", "u1", Some(key), json!({ "goal": "Fix the failing checkout tests" }))).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{b}");
    assert!(h.get("location").is_some());
    serde_json::from_str(&b).unwrap()
}

const VENDOR_WORDS: &[&str] = &[
    "openai", "anthropic", "claude", "gpt", "gemini", "google", "kimi", "moonshot", "grok", "xai", "qwen",
    "codex", "mistral", "llama", "deepseek", "providerid", "modelid", "sonnet", "opus", "haiku",
];

fn assert_no_vendor(body: &str) {
    let l = body.to_lowercase();
    for w in VENDOR_WORDS {
        assert!(!l.contains(w), "response leaks vendor/model name `{w}`: {body}");
    }
}

#[tokio::test]
async fn agency_goal_only_creates_durable_run_with_resolved_defaults() {
    let t = setup().await;
    let run = create(&t, "idem-key-0001").await;
    let id = run["id"].as_str().unwrap();
    assert!(id.starts_with("run_"));
    assert_eq!(run["object"], "run");
    assert_eq!(run["status"], "waiting", "left `accepted` only after resolution was recorded");
    let r = &run["resolved"];
    assert_eq!(r["workspace"]["ref"], "sandbox://per-run-scratch");
    assert_eq!(r["authority"]["profile"]["id"], "code-safe");
    assert_eq!(r["authority"]["profile"]["version"], 1);
    assert_eq!(r["budget"]["max_seconds"], 900);
    assert_eq!(r["completion"]["contract_id"], "completion.bug_fix");
    assert_eq!(r["completion"]["allow_partial"], false);
    assert_eq!(r["defaults_source"], "agent:allternit-code@v1/profile:code-safe@1");
    assert!(r["resolved_at"].is_string());
    assert_eq!(run["agent"], "allternit-code@v1");

    // Durable: a fresh store over the same on-disk ledger (≈ restart) sees it,
    // with the TaskIR that was compiled for it.
    let fresh = AgencyStore::new(Arc::new(allternit_commrails::ledger::Ledger::new(allternit_commrails::ledger::LedgerOptions {
        root_dir: Some(t.st.rails.root_dir.clone()),
        ledger_dir: Some(std::path::PathBuf::from(".allternit/ledger")),
    })));
    let rec = fresh.load_run(id).await.unwrap().expect("run persisted");
    assert_eq!(rec.run["resolved"], run["resolved"]);
    assert_eq!(rec.task_ir["task_type"], "BUG_FIX");
    assert_eq!(rec.task_ir["template"]["source"], "kernel", "WP10 BUG_FIX graph");
    assert!(!rec.task_ir["nodes"].as_array().unwrap().is_empty());

    // Idempotent replay returns the same run.
    let (s, h, b) = call(&t.app, post("/v1/agency", "u1", Some("idem-key-0001"), json!({ "goal": "Fix the failing checkout tests" }))).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    assert_eq!(h.get("idempotency-replayed").unwrap(), "true");
    assert_eq!(serde_json::from_str::<Value>(&b).unwrap()["id"], id);
    // Same key, different body → 409.
    let (s, _, b) = call(&t.app, post("/v1/agency", "u1", Some("idem-key-0001"), json!({ "goal": "Fix the failing checkout tests", "budget": { "max_seconds": 60 } }))).await;
    assert_eq!(s, StatusCode::CONFLICT, "{b}");
    assert!(b.contains("ERR_IDEMPOTENCY_KEY_REUSED"));
}

#[tokio::test]
async fn agency_rejects_missing_key_unknown_fields_and_disabling_enforcement() {
    let t = setup().await;
    let (s, _, _) = call(&t.app, post("/v1/agency", "u1", None, json!({ "goal": "x" }))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    for body in [
        json!({ "goal": "x", "enforcement": { "judge_fail_closed": false } }),
        json!({ "goal": "x", "verifier_owned_completion": false }),
        json!({ "goal": "x", "requires_lease_for_write": false }),
    ] {
        let (s, _, b) = call(&t.app, post("/v1/agency", "u1", Some("idem-key-0002"), body)).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{b}");
        assert!(b.contains("ERR_INPUT_UNSUPPORTED"));
    }
    let (s, _, b) = call(&t.app, post("/v1/agency", "u1", Some("idem-key-0003"), json!({ "goal": "x", "authority": { "profile": "root" } }))).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{b}");
    assert!(b.contains("ERR_AUTHORITY_PROFILE_FORBIDDEN"));
}

#[tokio::test]
async fn agency_get_run_status_and_owner_scoping() {
    let t = setup().await;
    let run = create(&t, "idem-key-0010").await;
    let id = run["id"].as_str().unwrap();
    let (s, h, b) = call(&t.app, get_req(&format!("/v1/runs/{id}"), "u1")).await;
    assert_eq!(s, StatusCode::OK);
    assert!(h.get("etag").is_some());
    let got: Value = serde_json::from_str(&b).unwrap();
    assert_eq!(got["status"], "waiting");
    assert_eq!(got["links"]["events"], format!("/v1/runs/{id}/events"));
    let (s, _, _) = call(&t.app, get_req(&format!("/v1/runs/{id}"), "someone-else")).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _, b) = call(&t.app, get_req("/v1/runs", "u1")).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(serde_json::from_str::<Value>(&b).unwrap()["data"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn agency_sse_event_stream_replays_from_ledger_and_closes_on_terminal() {
    let t = setup().await;
    let run = create(&t, "idem-key-0020").await;
    let id = run["id"].as_str().unwrap();
    let (s, _, _) = call(&t.app, post(&format!("/v1/runs/{id}/cancel"), "u1", None, json!({}))).await;
    assert_eq!(s, StatusCode::OK);
    let req = Request::builder().uri(format!("/v1/runs/{id}/events")).header("accept", "text/event-stream").extension(user("u1")).body(Body::empty()).unwrap();
    let (s, h, body) = tokio::time::timeout(Duration::from_secs(10), call(&t.app, req)).await.expect("stream closes after terminal event");
    assert_eq!(s, StatusCode::OK);
    assert!(h.get("content-type").unwrap().to_str().unwrap().starts_with("text/event-stream"));
    assert!(body.contains("retry:"));
    assert!(body.contains("id: evt_0000000001"));
    assert!(body.contains("event: run.status_changed"));
    assert!(body.contains("\"to\":\"cancelled\""));

    // Resume strictly after Last-Event-ID.
    let req = Request::builder().uri(format!("/v1/runs/{id}/events")).header("accept", "text/event-stream")
        .header("last-event-id", "evt_0000000002").extension(user("u1")).body(Body::empty()).unwrap();
    let (_, _, body) = tokio::time::timeout(Duration::from_secs(10), call(&t.app, req)).await.unwrap();
    assert!(!body.contains("id: evt_0000000001") && !body.contains("id: evt_0000000002"));
    assert!(body.contains("id: evt_0000000003"));

    // JSON page mode.
    let (s, _, b) = call(&t.app, get_req(&format!("/v1/runs/{id}/events"), "u1")).await;
    assert_eq!(s, StatusCode::OK);
    let p: Value = serde_json::from_str(&b).unwrap();
    let seqs: Vec<i64> = p["data"].as_array().unwrap().iter().map(|e| e["seq"].as_i64().unwrap()).collect();
    assert_eq!(seqs, (1..=seqs.len() as i64).collect::<Vec<_>>(), "gap-free seq");
}

#[tokio::test]
async fn agency_budget_exhaustion_hard_stops_effects_before_asking() {
    let t = setup().await;
    let run = create(&t, "idem-key-0030").await;
    let id = run["id"].as_str().unwrap();
    let s = AgencyStore::new(t.st.rails.ledger.clone());
    assert!(s.admit_effect(id).await.is_ok());
    s.charge(id, 10.0, 1.0, 1).await.unwrap();
    assert!(s.admit_effect(id).await.is_ok(), "under budget");
    let rec = s.charge(id, 10.0, 1.5, 1).await.unwrap(); // 2.5 >= 2.0
    assert_eq!(rec.run["status"], "needs_attention");
    assert_eq!(rec.run["budget_usage"]["spend_halted"], true);
    assert_eq!(rec.run["attention"]["reason"], "budget_exhausted");
    assert!(matches!(s.admit_effect(id).await, Err(store::EffectDenied::SpendHalted)));

    // Ordering: the halt snapshot lands before the attention request exists.
    let snaps: Vec<_> = s.raw_events().await.unwrap().into_iter().filter(|e| e.r#type == store::EV_RUN_STATE && e.payload["run_id"] == id).collect();
    let halted_at = snaps.iter().position(|e| e.payload["run"]["budget_usage"]["spend_halted"] == true).unwrap();
    assert!(snaps[halted_at].payload["attention"].as_array().unwrap().is_empty(), "spend halted before asking");
    let evs = s.events(id).await.unwrap();
    let thr = evs.iter().position(|e| e["type"] == "budget.threshold").unwrap();
    let att = evs.iter().position(|e| e["type"] == "attention.requested").unwrap();
    assert!(thr < att);

    // Resume can't bypass it; only a human answer that raises the budget can.
    let (st, _, _) = call(&t.app, post(&format!("/v1/runs/{id}/pause"), "u1", None, json!({}))).await;
    assert_eq!(st, StatusCode::OK);
    let (_, _, b) = call(&t.app, post(&format!("/v1/runs/{id}/resume"), "u1", None, json!({}))).await;
    assert_eq!(serde_json::from_str::<Value>(&b).unwrap()["status"], "needs_attention");
    let att_id = rec.run["attention"]["id"].as_str().unwrap();
    let (st, _, b) = call(&t.app, post(&format!("/v1/attention/{att_id}/responses"), "u1", None,
        json!({ "type": "approval", "value": { "budget": { "max_cost_usd": 5.0 } } }))).await;
    assert_eq!(st, StatusCode::OK, "{b}");
    assert!(s.admit_effect(id).await.is_ok());
}

#[tokio::test]
async fn agency_forces_verifier_owned_completion_and_leased_writes() {
    let t = setup().await;
    let run = create(&t, "idem-key-0040").await;
    let id = run["id"].as_str().unwrap();
    assert_eq!(run["resolved"]["enforcement"], json!({ "judge_fail_closed": true, "verifier_owned_completion": true, "fence": "strict" }));
    let s = AgencyStore::new(t.st.rails.ledger.clone());
    let rec = s.load_run(id).await.unwrap().unwrap();
    let dag = rec.task_ir["dag_id"].as_str().unwrap();
    // The real commrails judge sees an agency-origin plan: judge + verifier close.
    let node = rec.task_ir["nodes"][0]["id"].as_str().unwrap().to_string();
    let eff = effective_policy(&s.raw_events().await.unwrap(), dag, Some(&node));
    assert_eq!(eff.origin, Some(PolicyOrigin::Agency));
    assert_eq!(eff.verify, VerifyMode::Judge);
    assert_eq!(eff.close_by, CloseBy::Verifier);
    assert_eq!(rec.task_ir["wih_policy"]["requires_lease_for_write"], true);
}

struct PermissiveTemplate;
impl compiler::RunTemplate for PermissiveTemplate {
    fn id(&self) -> &'static str { "BUG_FIX" }
    fn version(&self) -> u32 { 9 }
    fn source(&self) -> &'static str { "test" }
    fn completion_policy(&self) -> &'static str { "completion.bug_fix" }
    fn instantiate(&self, _: &str, _: &Value) -> Result<compiler::TemplateGraph, String> {
        Ok(compiler::TemplateGraph { nodes: vec![], edges: vec![], wih_policy: json!({ "requires_lease_for_write": false, "x": 1 }) })
    }
}

#[test]
fn agency_compiler_never_emits_requires_lease_for_write_false() {
    let reg = compiler::TemplateRegistry::with_bug_fix(Arc::new(PermissiveTemplate));
    let c = compiler::compile(&json!({ "goal": "g" }), "run_x", &reg).unwrap();
    assert_eq!(c.task_ir["wih_policy"]["requires_lease_for_write"], true);
    assert_eq!(c.task_ir["template"]["source"], "test", "templates drop in through the interface");
    assert!(!c.task_ir.to_string().contains("\"requires_lease_for_write\":false"));
}

#[tokio::test]
async fn agency_default_responses_carry_no_vendor_or_model_names() {
    let t = setup().await;
    let run = create(&t, "idem-key-0050").await;
    let id = run["id"].as_str().unwrap();
    assert_no_vendor(&run.to_string());
    for uri in [
        format!("/v1/runs/{id}"), format!("/v1/runs/{id}/events"), "/v1/runs".into(), "/v1/agents".into(),
        "/v1/capabilities".into(), "/v1/authority-profiles".into(), "/v1/completion-criteria".into(),
        format!("/v1/runs/{id}/attention"), format!("/v1/runs/{id}/artifacts"),
    ] {
        let (s, _, b) = call(&t.app, get_req(&uri, "u1")).await;
        assert_eq!(s, StatusCode::OK, "{uri}: {b}");
        assert_no_vendor(&b);
    }
    let (_, _, b) = call(&t.app, get_req("/v1/agents", "u1")).await;
    let a: Value = serde_json::from_str(&b).unwrap();
    assert_eq!(a["data"][0]["id"], "allternit-code");
    assert_eq!(a["data"][0]["templates"], json!(["BUG_FIX"]));
}

#[tokio::test]
async fn agency_jwks_is_public_and_has_no_private_key_fields() {
    let t = setup().await;
    // No AuthUser extension: the route needs no auth.
    let (s, _, b) = call(&t.app, Request::builder().uri("/.well-known/jwks.json").body(Body::empty()).unwrap()).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let j: Value = serde_json::from_str(&b).unwrap();
    let keys = j["keys"].as_array().unwrap();
    assert!(!keys.is_empty());
    for k in keys {
        let fields: Vec<&str> = k.as_object().unwrap().keys().map(String::as_str).collect();
        for f in &fields {
            assert!(["kty", "crv", "x", "kid", "use", "alg"].contains(f), "unexpected JWK field {f}");
        }
        for private in ["d", "p", "q", "dp", "dq", "qi", "k", "seed", "private", "secret"] {
            assert!(k.get(private).is_none(), "JWKS exposes private field {private}");
        }
        assert_eq!(k["kty"], "OKP");
        assert_eq!(k["crv"], "Ed25519");
    }
    // The signing key's secret bytes never appear in the body.
    let key_file = t.st.rails.receipts.receipts_dir().join("_keys/receipt-signing.key");
    if let Ok(raw) = std::fs::read(&key_file) {
        use base64::Engine;
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&raw);
        assert!(!b.contains(&b64));
    }
}

#[tokio::test]
async fn agency_version_and_preview_headers() {
    let t = setup().await;
    let (s, h, _) = call(&t.app, get_req("/v1/agents", "u1")).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(h.get("allternit-version").unwrap(), SUPPORTED_VERSIONS[0]);
    assert_eq!(h.get(PREVIEW_HEADER).unwrap(), PREVIEW_VALUE);
    assert!(h.get("allternit-request-id").unwrap().to_str().unwrap().starts_with("req_"));
    let req = Request::builder().uri("/v1/agents").header("allternit-version", "2026-09-29").extension(user("u1")).body(Body::empty()).unwrap();
    let (s, h, _) = call(&t.app, req).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(h.get("allternit-version").unwrap(), "2026-09-29");
    let req = Request::builder().uri("/v1/agents").header("allternit-version", "1999-01-01").extension(user("u1")).body(Body::empty()).unwrap();
    let (s, h, b) = call(&t.app, req).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(b.contains("ERR_VERSION_UNSUPPORTED"));
    assert_eq!(h.get(PREVIEW_HEADER).unwrap(), PREVIEW_VALUE);
}

#[tokio::test]
async fn agency_campaign_wake_scheduling_stays_off() {
    let t = setup().await;
    let (s, _, b) = call(&t.app, post("/v1/campaigns", "u1", None, json!({ "objective": "Keep checkout green" }))).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let c: Value = serde_json::from_str(&b).unwrap();
    assert_eq!(c["scheduling"], "disabled_pending_golive");
    assert!(c["next_wake"].is_null());
    let id = c["id"].as_str().unwrap();
    let (_, _, b) = call(&t.app, post(&format!("/v1/campaigns/{id}/resume"), "u1", None, json!({}))).await;
    assert_eq!(serde_json::from_str::<Value>(&b).unwrap()["scheduling"], "disabled_pending_golive");
}

#[tokio::test]
async fn agency_template_instantiation_failure_is_422_not_500() {
    let t = setup().await;
    // A workspace with no locator declares no write set: BUG_FIX fails closed.
    let (s, _, b) = call(&t.app, post("/v1/agency", "u1", Some("idem-key-0060"), json!({ "goal": "x", "workspace": { "resources": [] } }))).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{b}");
    assert!(b.contains("ERR_INPUT_INVALID") && b.contains("\"param\":\"workspace\""), "{b}");
    let (_, _, b) = call(&t.app, get_req("/v1/runs", "u1")).await;
    assert_eq!(serde_json::from_str::<Value>(&b).unwrap()["data"], json!([]), "no run was created");
}

#[tokio::test]
async fn agency_graph_view_reads_primitive_ids() {
    let t = setup().await;
    let run = create(&t, "idem-key-0070").await;
    let (s, _, b) = call(&t.app, get_req(&format!("/v1/runs/{}/graph", run["id"].as_str().unwrap()), "u1")).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let g: Value = serde_json::from_str(&b).unwrap();
    for n in g["nodes"].as_array().unwrap() {
        assert!(n["primitive_id"].is_string(), "{n}");
    }
    assert_no_vendor(&b);
}

// ── WP11b: executor bridge, models, strict fence ─────────────────────────────

#[test]
fn agency_models_field_accepts_classes_and_rejects_vendor_names() {
    let reg = compiler::TemplateRegistry::default();
    let base = json!({ "goal": "fix it", "workspace": { "repo": "https://example.com/r.git" } });
    let mut ok = base.clone();
    ok["models"] = json!({ "allow_classes": ["mc.local"], "locality": "any" });
    let c = compiler::compile(&ok, "run_m", &reg).expect("models accepted");
    assert_eq!(c.task_ir["models"]["allow_classes"], json!(["mc.local"]));
    for bad in [json!({ "allow_classes": ["gpt-4o"] }), json!({ "allow_classes": ["mc.claude"] }), json!({ "vendor": "x" })] {
        let mut b = base.clone();
        b["models"] = bad.clone();
        let e = compiler::compile(&b, "run_m", &reg).unwrap_err();
        assert_eq!(e.status, 400, "{bad}");
    }
    let mut unknown = base.clone();
    unknown["models"] = json!({ "allow_classes": ["mc.quantum"] });
    assert_eq!(compiler::compile(&unknown, "run_m", &reg).unwrap_err().status, 422);
}

#[test]
fn agency_runs_always_use_the_strict_fence() {
    let reg = compiler::TemplateRegistry::default();
    let c = compiler::compile(&json!({ "goal": "g", "workspace": { "repo": "https://example.com/r.git" } }), "run_f", &reg).unwrap();
    assert_eq!(c.task_ir["wih_policy"]["fence"], "strict");
    assert_eq!(c.task_ir["judge_policy"]["fence"], "strict");
    assert_eq!(c.judge_policy.fence, Some(allternit_commrails::judge::policy::Fence::Strict));
    assert_eq!(c.resolved["enforcement"]["fence"], "strict");
    // A template cannot switch the fence off either.
    assert_eq!(compiler::enforce_wih_policy(&json!({ "fence": "guardrail" }))["fence"], "strict");
}

#[test]
fn agency_models_constraints_filter_the_pool_by_class_and_locality() {
    use allternit_commrails::kernel::router::Residency;
    let g = allternit_commrails::kernel::bug_fix::instantiate("t", &["fs:repo".into()]).unwrap();
    let full = executor::tests_support::scripted_pool(&g);
    let n = full.entries.len();
    let (local, _) = executor::constrain(full.clone(), &json!({ "allow_classes": ["mc.local"] }));
    assert!(!local.entries.is_empty() && local.entries.iter().all(|e| e.residency != Residency::Remote));
    let (remote, _) = executor::constrain(full.clone(), &json!({ "allow_classes": ["mc.remote"] }));
    assert!(!remote.entries.is_empty() && remote.entries.iter().all(|e| e.residency == Residency::Remote));
    let (lo, cfg) = executor::constrain(full.clone(), &json!({ "locality": "local_only" }));
    assert!(!cfg.policy.allow_remote && lo.entries.len() < n);
    assert_eq!(executor::constrain(full, &Value::Null).0.entries.len(), n);
}

#[tokio::test]
async fn agency_executor_off_by_default_parks_runs_with_a_reason() {
    let t = setup().await;
    let run = create(&t, "wp11b-parked-0001").await;
    assert_eq!(run["status"], "waiting");
    if !executor::enabled() {
        assert_eq!(run["status_reason"], executor::PARKED_REASON);
    }
}

/// The scripted e2e runs share process env (runs dir, local-repo allowlist).
static E2E_ENV: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Offline e2e of the bridge: BUG_FIX on a disposable node repo, scripted
/// cognition (attempt 1 imperfect, attempt 2 correct), strict fence.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agency_executor_drives_bug_fix_to_verified_completion() {
    let _env = E2E_ENV.lock().await;
    if std::process::Command::new("npm").arg("--version").output().is_err() {
        eprintln!("npm not available; skipping");
        return;
    }
    let repo = tempfile::tempdir().unwrap();
    let runs = tempfile::tempdir().unwrap();
    let w = |p: &str, c: &str| { let f = repo.path().join(p); std::fs::create_dir_all(f.parent().unwrap()).unwrap(); std::fs::write(f, c).unwrap(); };
    w("package.json", r#"{"name":"fx","private":true,"scripts":{"test":"node test.js"}}"#);
    w("math.js", "exports.add = (a, b) => a - b;\n");
    w("test.js", "const { add } = require('./math');\nif (add(2, 3) !== 5 || add(-2, 3) !== 1) { console.error('FAIL'); process.exit(1); }\n");
    w(".allternit/scripted-patches.json", &json!([
        { "path": "math.js", "content": "exports.add = (a, b) => a + b + 1;\n" },
        { "path": "math.js", "content": "exports.add = (a, b) => a + b;\n" }
    ]).to_string());
    let git = |args: &[&str]| assert!(std::process::Command::new("git").args(args).current_dir(repo.path())
        .env("GIT_CONFIG_GLOBAL", "/dev/null").status().unwrap().success());
    git(&["init", "-q", "-b", "main"]);
    git(&["add", "."]);
    git(&["-c", "user.name=f", "-c", "user.email=f@example.invalid", "-c", "commit.gpgsign=false", "commit", "-qm", "seeded bug"]);
    std::env::set_var("ALLTERNIT_AGENCY_COGNITION", "scripted");
    std::env::set_var("ALLTERNIT_AGENCY_RUNS_DIR", runs.path());
    std::env::set_var("ALLTERNIT_AGENCY_LOCAL_REPOS", repo.path());

    let t = setup().await;
    let body = json!({ "goal": "Fix add", "workspace": { "repo": repo.path().display().to_string(), "ref": "main" },
                       "budget": { "max_seconds": 120, "max_cost_usd": 1 } });
    let (s, _, b) = call(&t.app, post("/v1/agency", "u1", Some("wp11b-e2e-000001"), body)).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{b}");
    let id = serde_json::from_str::<Value>(&b).unwrap()["id"].as_str().unwrap().to_string();
    executor::start(t.st.clone(), id.clone());
    let mut run = Value::Null;
    for _ in 0..600 {
        let (_, _, b) = call(&t.app, get_req(&format!("/v1/runs/{id}"), "u1")).await;
        run = serde_json::from_str(&b).unwrap();
        if run["terminal"] == true { break; }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(run["status"], "completed", "{run}");
    assert_eq!(run["completion"]["status"], "verified");
    let (_, _, b) = call(&t.app, get_req(&format!("/v1/runs/{id}/receipts"), "u1")).await;
    let receipts: Value = serde_json::from_str(&b).unwrap();
    let types: Vec<String> = receipts["data"].as_array().unwrap().iter().filter_map(|r| r["type"].as_str().map(String::from)).collect();
    assert!(types.contains(&"verification".into()) && types.contains(&"run_completion".into()), "{types:?}");
    let (_, _, b) = call(&t.app, get_req(&format!("/v1/runs/{id}/receipts/verification"), "u1")).await;
    assert_eq!(serde_json::from_str::<Value>(&b).unwrap()["hash_chain_valid"], true, "{b}");
    let (_, _, b) = call(&t.app, get_req(&format!("/v1/runs/{id}/artifacts"), "u1")).await;
    let arts: Value = serde_json::from_str(&b).unwrap();
    assert_eq!(arts["data"][0]["kind"], "patch", "{b}");
    assert!(arts["data"][0]["content"].as_str().unwrap().contains("a + b;"));
    // Two repair attempts: the imperfect one failed N15, the second passed.
    let (_, _, b) = call(&t.app, get_req(&format!("/v1/runs/{id}/events"), "u1")).await;
    assert_eq!(b.matches("\"step\":\"N15\"").count(), 4, "N15 progress + receipt events"); 
    assert_no_vendor(&b);
    // Cleanup runs right after the terminal snapshot; the store is fast now, so wait for it.
    for _ in 0..100 {
        if !runs.path().join(&id).exists() { break; }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(!runs.path().join(&id).exists(), "disposable workspace removed after the run");
}

#[tokio::test]
async fn agency_store_index_does_not_rescan_ledger() {
    use allternit_commrails::ledger::{Ledger, LedgerOptions};
    let dir = tempfile::tempdir().unwrap();
    let open = || Arc::new(Ledger::new(LedgerOptions { root_dir: Some(dir.path().to_path_buf()), ledger_dir: Some(std::path::PathBuf::from("ledger")) }));
    let s = AgencyStore::new(open());
    for i in 0..150 {
        let id = format!("run_{i}");
        let rec = store::RunRecord {
            owner: "u1".into(),
            idempotency_key: Some(format!("k{i}")),
            run: json!({ "id": id, "status": "running", "version": 0 }),
            task_ir: json!({}),
            attention: vec![],
        };
        s.save(rec).await.unwrap();
        s.emit(&id, 1, "run.started", json!({})).await.unwrap();
    }
    assert_eq!(s.ledger_scans(), 1, "writes index incrementally, one initial build");
    // Restart: new ledger handle -> new index, rebuilt from disk with one scan.
    let s = AgencyStore::new(open());
    for i in 0..300 {
        let id = format!("run_{}", i % 150);
        assert!(s.load_run(&id).await.unwrap().is_some());
        assert_eq!(s.events(&id).await.unwrap().len(), 1);
        assert_eq!(s.list_runs("u1").await.unwrap().len(), 150);
        assert!(s.find_by_idempotency("u1", &format!("k{}", i % 150)).await.unwrap().is_some());
    }
    s.emit("run_3", 1, "run.x", json!({})).await.unwrap();
    assert_eq!(s.events("run_3").await.unwrap().len(), 2);
    assert_eq!(s.list_runs("u1").await.unwrap().len(), 150);
    assert_eq!(s.ledger_scans(), 1, "get/list/emit never rescan the ledger");
}

/// Timing on a synthetic ~17 MB ledger: `cargo test -p allternit-api --lib agency_store_bench -- --ignored --nocapture`.
#[tokio::test]
#[ignore]
async fn agency_store_bench_large_ledger() {
    use allternit_commrails::ledger::{Ledger, LedgerOptions};
    let dir = tempfile::tempdir().unwrap();
    let open = || Arc::new(Ledger::new(LedgerOptions { root_dir: Some(dir.path().to_path_buf()), ledger_dir: Some(std::path::PathBuf::from("ledger")) }));
    let s = AgencyStore::new(open());
    let pad = "x".repeat(5000);
    for i in 0..3400 {
        let rec = store::RunRecord { owner: "u1".into(), idempotency_key: None, run: json!({ "id": format!("run_{i}"), "status": "running", "version": 0, "pad": pad }), task_ir: json!({}), attention: vec![] };
        s.save(rec).await.unwrap();
    }
    let s = AgencyStore::new(open());
    let t0 = std::time::Instant::now();
    s.load_run("run_1").await.unwrap();
    let first = t0.elapsed();
    let t1 = std::time::Instant::now();
    for _ in 0..20 {
        s.load_run("run_7").await.unwrap();
        s.list_runs("u1").await.unwrap();
    }
    println!("BENCH first call {first:?}; 20x(get+list) {:?}", t1.elapsed());
}

// ── spending guard (agency-exec-readiness) ─────────────────────────────────

fn limits(pairs: &[(&str, &str)]) -> guard::Limits {
    let m: std::collections::HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    guard::Limits::from_lookup(|k| m.get(k).cloned())
}

/// Limits for tests that start a run. The executor's active-run slots are
/// process-wide and many tests run the executor for the same test org in
/// parallel, so default concurrency caps would refuse admission depending on
/// test timing. Tests about those caps use [`limits`] with explicit values.
fn run_limits(pairs: &[(&str, &str)]) -> guard::Limits {
    let mut v: Vec<(&str, &str)> = pairs.to_vec();
    for k in [guard::MAX_CONC_ENV, guard::ORG_MAX_CONC_ENV] {
        if !v.iter().any(|(key, _)| *key == k) {
            v.push((k, "1000"));
        }
    }
    limits(&v)
}

#[test]
fn agency_guard_org_allowlist_empty_means_no_org_executes() {
    let none = limits(&[]);
    assert!(!none.org_allowed("org_a") && !none.org_allowed(""), "unset allowlist: nobody executes");
    assert!(!limits(&[(guard::ORGS_ENV, " , ")]).org_allowed("org_a"));
    let some = limits(&[(guard::ORGS_ENV, "org_a, user:u1")]);
    assert!(some.org_allowed("org_a") && some.org_allowed("user:u1") && !some.org_allowed("org_b"));
    assert!(limits(&[(guard::ORGS_ENV, "*")]).org_allowed("anyone"));
}

#[test]
fn agency_guard_daily_caps_parse_default_and_reach() {
    use guard::{Cap, Spend};
    let d = limits(&[]);
    assert_eq!((d.daily, d.org_daily), (guard::DEFAULT_DAILY, guard::DEFAULT_ORG_DAILY));
    assert_eq!(Cap::parse("tokens=1000,usd=2.5"), Cap { tokens: Some(1000), usd: Some(2.5) });
    assert_eq!(Cap::parse("3"), Cap { tokens: None, usd: Some(3.0) });
    assert_eq!(Cap::parse("off"), Cap::OFF);
    assert_eq!(Cap::parse("tokens=lots"), Cap { tokens: Some(0), usd: Some(0.0) }, "garbage fails closed");
    let l = limits(&[(guard::DAILY_ENV, "tokens=1000,usd=10"), (guard::ORG_DAILY_ENV, "tokens=100")]);
    let s = |tokens, usd| Spend { tokens, usd };
    assert_eq!(l.daily_reached(&s(500, 1.0), &s(50, 0.5)), None);
    assert_eq!(l.daily_reached(&s(500, 1.0), &s(100, 0.5)), Some(("org", "tokens")));
    assert_eq!(l.daily_reached(&s(999, 10.0), &s(0, 0.0)), Some(("global", "usd")));
    assert_eq!(l.daily_reached(&s(1000, 0.0), &s(0, 0.0)), Some(("global", "tokens")));
}

#[test]
fn agency_guard_concurrency_caps_global_and_per_org() {
    let l = limits(&[(guard::MAX_CONC_ENV, "2"), (guard::ORG_MAX_CONC_ENV, "1")]);
    let d = limits(&[]);
    assert_eq!((d.max_concurrent, d.org_max_concurrent), (guard::DEFAULT_MAX_CONCURRENT, guard::DEFAULT_ORG_MAX_CONCURRENT));
    let mut active = std::collections::HashMap::new();
    assert!(l.admits(&active, "a"));
    active.insert("run_1".to_string(), "a".to_string());
    assert!(!l.admits(&active, "a"), "per-org cap");
    assert!(l.admits(&active, "b"));
    active.insert("run_2".to_string(), "b".to_string());
    assert!(!l.admits(&active, "c"), "global cap");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agency_guard_admission_refuses_unlisted_org_and_full_slots() {
    let t = setup().await;
    let run = create(&t, "guard-admit-0001").await;
    let id = run["id"].as_str().unwrap().to_string();
    let s = AgencyStore::new(t.st.rails.ledger.clone());
    assert_eq!(s.load_run(&id).await.unwrap().unwrap().task_ir["org_id"], "user:u1");
    // Flag on but no allowlist: nothing starts.
    assert!(!executor::admit_and_start(t.st.clone(), id.clone(), limits(&[])).await);
    // Allowlisted but no free slot.
    let full = limits(&[(guard::ORGS_ENV, "user:u1"), (guard::MAX_CONC_ENV, "0")]);
    assert!(!executor::admit_and_start(t.st.clone(), id.clone(), full).await);
    let full_org = limits(&[(guard::ORGS_ENV, "user:u1"), (guard::ORG_MAX_CONC_ENV, "0")]);
    assert!(!executor::admit_and_start(t.st.clone(), id.clone(), full_org).await);
    assert_eq!(s.load_run(&id).await.unwrap().unwrap().run["status"], "waiting", "left queued");
    let _ = executor::active_count();
}

async fn wait_settled(s: &AgencyStore, id: &str) -> store::RunRecord {
    for _ in 0..200 {
        let r = s.load_run(id).await.unwrap().unwrap();
        if r.run["status"] != "waiting" && r.run["status"] != "running" { return r; }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("run {id} never settled");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agency_guard_org_daily_cap_parks_run_before_any_effect() {
    let t = setup().await;
    let run = create(&t, "guard-cap-org-0001").await;
    let id = run["id"].as_str().unwrap().to_string();
    let s = AgencyStore::new(t.st.rails.ledger.clone());
    let l = run_limits(&[(guard::ORGS_ENV, "user:u1"), (guard::DAILY_ENV, "off"), (guard::ORG_DAILY_ENV, "usd=0")]);
    assert!(executor::admit_and_start(t.st.clone(), id.clone(), l).await);
    let rec = wait_settled(&s, &id).await;
    assert_eq!(rec.run["status"], "needs_attention", "{}", rec.run);
    assert_eq!(rec.run["budget_usage"]["spend_halted"], true);
    assert_eq!(rec.run["attention"]["title"], "budget cap reached");
    assert_eq!(rec.run["attention"]["reason"], guard::CAP_REASON);
    assert_eq!(rec.run["budget_usage"]["steps"], 0, "no effect ran");
    assert!(matches!(s.admit_effect(&id).await, Err(store::EffectDenied::SpendHalted)));
    let evs = s.events(&id).await.unwrap();
    assert!(!evs.iter().any(|e| e["type"] == "receipt.appended"), "no receipts: spend stopped before the first effect");
    // Stopping it is always allowed.
    let att_id = rec.run["attention"]["id"].as_str().unwrap();
    let (st, _, b) = call(&t.app, post(&format!("/v1/attention/{att_id}/responses"), "u1", None, json!({ "type": "rejection" }))).await;
    assert_eq!(st, StatusCode::OK, "{b}");
    assert_eq!(serde_json::from_str::<Value>(&b).unwrap()["run"]["status"], "failed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agency_guard_global_daily_cap_counts_other_orgs_spend() {
    let t = setup().await;
    let s = AgencyStore::new(t.st.rails.ledger.clone());
    // Org A (user:u1) already spent 150 tokens today on another run.
    let a = create(&t, "guard-cap-glob-0001").await;
    s.charge_usage(a["id"].as_str().unwrap(), 1.0, 0.0, 1, 150).await.unwrap();
    // Org B's first run is refused by the global cap, not its own.
    let (st, _, b) = call(&t.app, post("/v1/agency", "u2", Some("guard-cap-glob-0002"), json!({ "goal": "Fix the failing checkout tests" }))).await;
    assert_eq!(st, StatusCode::ACCEPTED, "{b}");
    let id = serde_json::from_str::<Value>(&b).unwrap()["id"].as_str().unwrap().to_string();
    let (g, o) = s.daily_spend("user:u2").await.unwrap();
    assert!(g.tokens >= 150 && o.tokens == 0);
    let l = run_limits(&[(guard::ORGS_ENV, "user:u2"), (guard::DAILY_ENV, "tokens=100"), (guard::ORG_DAILY_ENV, "off")]);
    assert!(executor::admit_and_start(t.st.clone(), id.clone(), l).await);
    let rec = wait_settled(&s, &id).await;
    assert_eq!(rec.run["status"], "needs_attention", "{}", rec.run);
    assert_eq!(rec.run["attention"]["title"], "budget cap reached");
    let evs = s.events(&id).await.unwrap();
    assert!(evs.iter().any(|e| e["type"] == "budget.threshold" && e["data"]["dimension"] == "daily_global_tokens"), "{evs:?}");
    // The server's own caps (env defaults here) are not reached, so approving
    // lifts the halt and re-queues the run; it stays `waiting` because
    // execution is off in tests.
    let att_id = rec.run["attention"]["id"].as_str().unwrap();
    let (st, _, b) = call(&t.app, post(&format!("/v1/attention/{att_id}/responses"), "u2", None, json!({ "type": "approval" }))).await;
    assert_eq!(st, StatusCode::OK, "{b}");
    let v: Value = serde_json::from_str(&b).unwrap();
    assert_eq!(v["run"]["status"], "waiting", "{v}");
    assert_eq!(v["run"]["budget_usage"]["spend_halted"], false, "{v}");
}

/// Real-model local end-to-end: the same seeded-bug fixture as the scripted
/// e2e, but cognition goes to the live gizzi-code at TERMINAL_SERVER_URL /
/// 127.0.0.1:4096 with `locality: local_only` (no metered model can be
/// routed). Opt-in: `AGENCY_REAL_MODEL_E2E=1 cargo test -p allternit-api --lib
/// agency_real_model_local_e2e -- --ignored --exact --nocapture`.
/// `AGENCY_REAL_MODEL_LOCALITY=any` drops `local_only` so a subscription CLI
/// backend can be routed (point TERMINAL_SERVER_URL at a pool narrowed to it).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs a live gizzi-code with a local model"]
async fn agency_real_model_local_e2e() {
    if std::env::var("AGENCY_REAL_MODEL_E2E").as_deref() != Ok("1") {
        eprintln!("AGENCY_REAL_MODEL_E2E!=1; skipping");
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("allternit_api::agency_api=debug,allternit_api::gizzi_completion=debug").with_test_writer().try_init();
    let repo = tempfile::tempdir().unwrap();
    let runs = tempfile::tempdir().unwrap();
    let w = |p: &str, c: &str| { let f = repo.path().join(p); std::fs::create_dir_all(f.parent().unwrap()).unwrap(); std::fs::write(f, c).unwrap(); };
    w("package.json", r#"{"name":"fx","private":true,"scripts":{"test":"node test.js"}}"#);
    w("math.js", "exports.add = (a, b) => a - b;\n");
    w("test.js", "const { add } = require('./math');\nif (add(2, 3) !== 5 || add(-2, 3) !== 1) { console.error('FAIL'); process.exit(1); }\n");
    let git = |args: &[&str]| assert!(std::process::Command::new("git").args(args).current_dir(repo.path())
        .env("GIT_CONFIG_GLOBAL", "/dev/null").status().unwrap().success());
    git(&["init", "-q", "-b", "main"]);
    git(&["add", "."]);
    git(&["-c", "user.name=f", "-c", "user.email=f@example.invalid", "-c", "commit.gpgsign=false", "commit", "-qm", "seeded bug"]);
    std::env::remove_var("ALLTERNIT_AGENCY_COGNITION");
    std::env::set_var("ALLTERNIT_AGENCY_RUNS_DIR", runs.path());
    std::env::set_var("ALLTERNIT_AGENCY_LOCAL_REPOS", repo.path());

    let t = setup().await;
    let body = json!({ "goal": "Fix add so the tests pass", "workspace": { "repo": repo.path().display().to_string(), "ref": "main" },
                       "models": { "locality": std::env::var("AGENCY_REAL_MODEL_LOCALITY").unwrap_or_else(|_| "local_only".into()) },
                       "budget": { "max_seconds": 900, "max_cost_usd": 0.5 } });
    let (s, _, b) = call(&t.app, post("/v1/agency", "u1", Some("real-model-e2e-0001"), body)).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{b}");
    let id = serde_json::from_str::<Value>(&b).unwrap()["id"].as_str().unwrap().to_string();
    let l = run_limits(&[(guard::ORGS_ENV, "user:u1"), (guard::DAILY_ENV, "tokens=200000,usd=0.5"), (guard::ORG_DAILY_ENV, "tokens=200000,usd=0.5")]);
    assert!(executor::admit_and_start(t.st.clone(), id.clone(), l).await);
    let mut run = Value::Null;
    for _ in 0..1800 {
        let (_, _, b) = call(&t.app, get_req(&format!("/v1/runs/{id}"), "u1")).await;
        run = serde_json::from_str(&b).unwrap();
        if run["terminal"] == true || run["status"] == "needs_attention" { break; }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    let s = AgencyStore::new(t.st.rails.ledger.clone());
    for e in s.events(&id).await.unwrap() {
        eprintln!("EVENT {} {}", e["type"], e["data"].to_string().chars().take(300).collect::<String>());
    }
    eprintln!("RUN status={} completion={} usage={} reason={}", run["status"], run["completion"], run["budget_usage"], run["status_reason"]);
    eprintln!("math.js now: {:?}", std::fs::read_to_string(repo.path().join("math.js")).ok());
    assert_eq!(run["status"], "completed", "{run}");
    assert_eq!(run["completion"]["status"], "verified");
}

#[test]
fn agency_live_pool_bridges_task_caps_to_generic_model_caps() {
    use allternit_commrails::kernel::router::Role;
    let g = allternit_commrails::kernel::bug_fix::instantiate("task.bug_fix", &["fs:math.js".to_string()]).unwrap();
    let mut pool = executor::tests_support::scripted_pool(&g);
    for e in pool.entries.iter_mut() {
        e.capabilities = vec!["cap.text.generate".into()];
    }
    pool.entries[0].capabilities.push("cap.code.edit".into());
    let mut s1 = pool.entries[0].clone();
    s1.cognitive_roles = vec![Role::S1];
    pool.entries.push(s1);
    let b = executor::bridge_task_caps(pool, &g);
    let has = |i: usize, c: &str| b.entries[i].capabilities.iter().any(|x| x == c);
    assert!(has(0, "cap.bug_fix.patch_candidate") && has(0, "cap.bug_fix.acceptance"));
    assert!(!has(1, "cap.bug_fix.patch_candidate"), "no code.edit, no patch step");
    assert!(b.entries.iter().filter(|e| e.cognitive_roles.iter().all(|r| *r == Role::S1)).all(|e| !e.capabilities.iter().any(|c| c.starts_with("cap.bug_fix."))), "S1 entries untouched");
}

// ── WP-P1 production safety ─────────────────────────────────────────────────

fn org_user(id: &str, org: &str) -> AuthUser {
    AuthUser { organization_id: Some(org.into()), ..user(id) }
}

fn req_as(method: &str, uri: &str, u: AuthUser, key: Option<&str>, body: Value) -> Request<Body> {
    let mut r = Request::builder().method(method).uri(uri).header("content-type", "application/json").extension(u);
    if let Some(k) = key {
        r = r.header("idempotency-key", k);
    }
    r.body(Body::from(body.to_string())).unwrap()
}

fn seed_org(st: &AppState, org: &str, members: &[&str]) {
    let c = st.db.connect().unwrap();
    c.execute("INSERT OR IGNORE INTO organizations (id, name) VALUES (?1, 'Org')", rusqlite::params![org]).unwrap();
    for u in members {
        c.execute("INSERT OR IGNORE INTO users (id, email) VALUES (?1, ?2)", rusqlite::params![u, format!("{u}@t.local")]).unwrap();
        c.execute("INSERT OR IGNORE INTO organization_members (id, organization_id, user_id, role) VALUES (?1, ?2, ?3, 'member')",
            rusqlite::params![format!("{org}:{u}"), org, u]).unwrap();
    }
}

async fn create_as(t: &T, u: AuthUser, key: &str) -> String {
    let (s, _, b) = call(&t.app, req_as("POST", "/v1/agency", u, Some(key), json!({ "goal": "Fix the failing tests" }))).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{b}");
    serde_json::from_str::<Value>(&b).unwrap()["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn agency_safety_consequential_approval_needs_another_member() {
    let t = setup().await;
    seed_org(&t.st, "org_p1", &["ra", "rb"]);
    let id = create_as(&t, org_user("ra", "org_p1"), "p1-nonreq-00001").await;
    let s = AgencyStore::new(t.st.rails.ledger.clone());
    let rec = s.park_halted(&id, safety::STUCK_REASON, "run is stuck", "test", json!({ "consequential": true })).await.unwrap();
    assert_eq!(rec.run["status"], "needs_attention");
    assert_eq!(rec.run["budget_usage"]["spend_halted"], true, "spend halted before asking");
    let att = rec.run["attention"]["id"].as_str().unwrap().to_string();
    // The requester cannot approve their own consequential request.
    let (st, _, b) = call(&t.app, req_as("POST", &format!("/v1/attention/{att}/responses"), org_user("ra", "org_p1"), None, json!({ "type": "approval" }))).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{b}");
    assert!(b.contains("ERR_APPROVAL_REQUIRES_NON_REQUESTER"));
    // Another member sees it in the org approvals queue and approves it.
    let (st, _, b) = call(&t.app, req_as("GET", "/v1/agency-safety/approvals", org_user("rb", "org_p1"), None, json!({}))).await;
    assert_eq!(st, StatusCode::OK, "{b}");
    let q: Value = serde_json::from_str(&b).unwrap();
    assert_eq!(q["data"][0]["id"], att.as_str());
    assert_eq!(q["data"][0]["you_requested"], false);
    // A user of another org cannot reach it.
    let (st, _, _) = call(&t.app, req_as("POST", &format!("/v1/agency-safety/approvals/{att}/responses"), org_user("zz", "org_other"), None, json!({ "type": "approval" }))).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _, b) = call(&t.app, req_as("POST", &format!("/v1/agency-safety/approvals/{att}/responses"), org_user("rb", "org_p1"), None, json!({ "type": "approval" }))).await;
    assert_eq!(st, StatusCode::OK, "{b}");
    let r: Value = serde_json::from_str(&b).unwrap();
    assert_eq!(r["run"]["status"], "waiting");
    assert_eq!(r["resolution"]["self_approval"], false);
    assert_eq!(r["resolution"]["requested_by"], "ra");
    assert_eq!(s.load_run(&id).await.unwrap().unwrap().run["budget_usage"]["spend_halted"], false);
}

#[tokio::test]
async fn agency_safety_single_member_org_may_self_approve_and_rejection_always_allowed() {
    let t = setup().await;
    let id = create(&t, "p1-selfok-00001").await["id"].as_str().unwrap().to_string();
    let s = AgencyStore::new(t.st.rails.ledger.clone());
    let rec = s.park_halted(&id, safety::STUCK_REASON, "run is stuck", "test", json!({})).await.unwrap();
    let att = rec.run["attention"]["id"].as_str().unwrap().to_string();
    let (st, _, b) = call(&t.app, post(&format!("/v1/attention/{att}/responses"), "u1", None, json!({ "type": "approval" }))).await;
    assert_eq!(st, StatusCode::OK, "personal org: default off: {b}");
    // Explicit policy on: self-approval refused, self-rejection allowed.
    let (st, _, b) = call(&t.app, req_as("PUT", "/v1/agency-safety/policy", user("u1"), None, json!({ "require_non_requester_approval": true, "max_steps": 50 }))).await;
    assert_eq!(st, StatusCode::OK, "{b}");
    let p: Value = serde_json::from_str(&b).unwrap();
    assert_eq!(p["effective_require_non_requester_approval"], true);
    assert_eq!(p["effective_caps"]["max_steps"], 50);
    let id2 = create(&t, "p1-selfok-00002").await["id"].as_str().unwrap().to_string();
    let rec = s.park_halted(&id2, safety::RUN_CAP_REASON, "run cap reached", "test", json!({})).await.unwrap();
    let att = rec.run["attention"]["id"].as_str().unwrap().to_string();
    let (st, _, _) = call(&t.app, post(&format!("/v1/attention/{att}/responses"), "u1", None, json!({ "type": "approval" }))).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _, b) = call(&t.app, post(&format!("/v1/attention/{att}/responses"), "u1", None, json!({ "type": "rejection" }))).await;
    assert_eq!(st, StatusCode::OK, "{b}");
    assert_eq!(serde_json::from_str::<Value>(&b).unwrap()["run"]["status"], "failed");
    // Non-admin member of a real org cannot change the policy.
    seed_org(&t.st, "org_q", &["m1"]);
    let (st, _, _) = call(&t.app, req_as("PUT", "/v1/agency-safety/policy", org_user("m1", "org_q"), None, json!({ "runs_per_hour": 1 }))).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agency_safety_run_cap_parks_before_any_effect_and_approval_must_raise_it() {
    let t = setup().await;
    let id = create_as(&t, user("cap1"), "p1-cap-0000001").await;
    safety::save_org(&t.st.db, "user:cap1", &safety::OrgSafety { max_steps: Some(0), ..Default::default() }, "cap1").unwrap();
    let s = AgencyStore::new(t.st.rails.ledger.clone());
    assert!(executor::admit_and_start(t.st.clone(), id.clone(), run_limits(&[(guard::ORGS_ENV, "user:cap1")])).await);
    let rec = wait_settled(&s, &id).await;
    assert_eq!(rec.run["status"], "needs_attention", "{}", rec.run);
    assert_eq!(rec.run["attention"]["reason"], safety::RUN_CAP_REASON);
    assert_eq!(rec.run["attention"]["dimension"], "max_steps");
    assert_eq!(rec.run["budget_usage"]["spend_halted"], true);
    assert_eq!(rec.run["budget_usage"]["steps"], 0, "no effect ran");
    assert_eq!(rec.run["safety"]["fence_epoch"], 1, "the drive took a fencing token");
    assert!(rec.run["safety"]["started_at"].is_string());
    let att = rec.run["attention"]["id"].as_str().unwrap().to_string();
    // Approving without raising the cap is refused and records nothing.
    let (st, _, b) = call(&t.app, req_as("POST", &format!("/v1/attention/{att}/responses"), user("cap1"), None, json!({ "type": "approval" }))).await;
    assert_eq!(st, StatusCode::CONFLICT, "{b}");
    // The org policy ceiling still applies: a per-run override is granted explicitly.
    let (st, _, b) = call(&t.app, req_as("POST", &format!("/v1/attention/{att}/responses"), user("cap1"), None,
        json!({ "type": "rejection" }))).await;
    assert_eq!(st, StatusCode::OK, "{b}");
    // Journal is visible to the owner only.
    let (st, _, _) = call(&t.app, get_req(&format!("/v1/agency-safety/runs/{id}/journal"), "someone-else")).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _, b) = call(&t.app, req_as("GET", &format!("/v1/agency-safety/runs/{id}/journal"), user("cap1"), None, json!({}))).await;
    assert_eq!(st, StatusCode::OK, "{b}");
}

#[tokio::test]
async fn agency_safety_org_run_rate_keeps_run_queued() {
    let t = setup().await;
    let id = create_as(&t, user("rate1"), "p1-rate-000001").await;
    safety::save_org(&t.st.db, "user:rate1", &safety::OrgSafety { runs_per_hour: Some(0), ..Default::default() }, "rate1").unwrap();
    assert!(!executor::admit_and_start(t.st.clone(), id.clone(), limits(&[(guard::ORGS_ENV, "user:rate1")])).await);
    let s = AgencyStore::new(t.st.rails.ledger.clone());
    assert_eq!(s.load_run(&id).await.unwrap().unwrap().run["status"], "waiting");
    let l = limits(&[(guard::ORG_RUNS_PER_HOUR_ENV, "3")]).with_org_policy(&safety::OrgSafety { runs_per_hour: Some(9), ..Default::default() });
    assert_eq!(l.org_runs_per_hour, 3, "org policy never raises the server ceiling");
}

#[tokio::test]
async fn agency_safety_unknown_effect_approval_resolves_journal() {
    let t = setup().await;
    let id = create(&t, "p1-unknown-0001").await["id"].as_str().unwrap().to_string();
    let e1 = safety::acquire_fence(&t.st.db, &id, "w1").unwrap();
    let key = format!("{id}:N30:tool.push:1");
    safety::prepare(&t.st.db, &key, &id, "N30", "tool.push", "h", e1, false).unwrap();
    let e2 = safety::acquire_fence(&t.st.db, &id, "w2").unwrap();
    assert_eq!(safety::prepare(&t.st.db, &key, &id, "N30", "tool.push", "h", e2, false).unwrap(), safety::Prepared::Unknown);
    let s = AgencyStore::new(t.st.rails.ledger.clone());
    let rec = s.park_halted(&id, safety::UNKNOWN_EFFECT_REASON, "effect outcome unknown", "t", json!({ "idempotency_key": key })).await.unwrap();
    let att = rec.run["attention"]["id"].as_str().unwrap().to_string();
    let (st, _, _) = call(&t.app, post(&format!("/v1/attention/{att}/responses"), "u1", None, json!({ "type": "approval" }))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "the approver must say whether it happened");
    let (st, _, b) = call(&t.app, post(&format!("/v1/attention/{att}/responses"), "u1", None, json!({ "type": "approval", "value": { "effect_outcome": "applied" } }))).await;
    assert_eq!(st, StatusCode::OK, "{b}");
    assert!(matches!(safety::prepare(&t.st.db, &key, &id, "N30", "tool.push", "h", e2, false).unwrap(), safety::Prepared::Committed(_)),
        "an effect the approver says happened is never re-applied");
}

/// WP-B1: one attempt with three scripted candidates (a wrong fix, a correct
/// anchored SEARCH/REPLACE fix, an invalid anchor). Each distinct valid one is
/// tested in its own isolated checkout; the passing one wins and the run
/// verifies on the first attempt.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agency_executor_selects_the_passing_candidate_from_isolated_checkouts() {
    let _env = E2E_ENV.lock().await;
    if std::process::Command::new("npm").arg("--version").output().is_err() {
        eprintln!("npm not available; skipping");
        return;
    }
    let repo = tempfile::tempdir().unwrap();
    let runs = tempfile::tempdir().unwrap();
    let w = |p: &str, c: &str| { let f = repo.path().join(p); std::fs::create_dir_all(f.parent().unwrap()).unwrap(); std::fs::write(f, c).unwrap(); };
    w("package.json", r#"{"name":"fx","private":true,"scripts":{"test":"node test.js"}}"#);
    w("math.js", "// math helpers\nexports.add = (a, b) => a - b;\nexports.mul = (a, b) => a * b;\n");
    w("test.js", "const { add } = require('./math');\nif (add(2, 3) !== 5 || add(-2, 3) !== 1) { console.error('FAIL'); process.exit(1); }\n");
    w(".allternit/scripted-patches.json", &json!([[
        { "edits": [{ "path": "math.js", "search": "exports.add = (a, b) => a - b;\n", "replace": "exports.add = (a, b) => a + b + 1;\n" }] },
        { "text": "math.js\n<<<<<<< SEARCH\nexports.add = (a, b) => a - b;\n=======\nexports.add = (a, b) => a + b;\n>>>>>>> REPLACE\n" },
        { "edits": [{ "path": "math.js", "search": "exports.nope = 1;\n", "replace": "x\n" }] }
    ]]).to_string());
    let git = |args: &[&str]| assert!(std::process::Command::new("git").args(args).current_dir(repo.path())
        .env("GIT_CONFIG_GLOBAL", "/dev/null").status().unwrap().success());
    git(&["init", "-q", "-b", "main"]);
    git(&["add", "."]);
    git(&["-c", "user.name=f", "-c", "user.email=f@example.invalid", "-c", "commit.gpgsign=false", "commit", "-qm", "seeded bug"]);
    std::env::set_var("ALLTERNIT_AGENCY_COGNITION", "scripted");
    std::env::set_var("ALLTERNIT_AGENCY_RUNS_DIR", runs.path());
    std::env::set_var("ALLTERNIT_AGENCY_LOCAL_REPOS", repo.path());

    let t = setup().await;
    let body = json!({ "goal": "Fix add", "workspace": { "repo": repo.path().display().to_string(), "ref": "main" },
                       "budget": { "max_seconds": 120, "max_cost_usd": 1 } });
    let (s, _, b) = call(&t.app, post("/v1/agency", "u1", Some("wpb1-cand-000001"), body)).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{b}");
    let id = serde_json::from_str::<Value>(&b).unwrap()["id"].as_str().unwrap().to_string();
    executor::start(t.st.clone(), id.clone());
    let mut run = Value::Null;
    for _ in 0..600 {
        let (_, _, b) = call(&t.app, get_req(&format!("/v1/runs/{id}"), "u1")).await;
        run = serde_json::from_str(&b).unwrap();
        if run["terminal"] == true { break; }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(run["status"], "completed", "{run}");
    assert_eq!(run["completion"]["status"], "verified");
    let (_, _, b) = call(&t.app, get_req(&format!("/v1/runs/{id}/artifacts"), "u1")).await;
    let arts: Value = serde_json::from_str(&b).unwrap();
    let diff = arts["data"][0]["content"].as_str().unwrap();
    assert!(diff.contains("+exports.add = (a, b) => a + b;") && !diff.contains("a + b + 1"), "{diff}");
    let (_, _, b) = call(&t.app, get_req(&format!("/v1/runs/{id}/events"), "u1")).await;
    assert_eq!(b.matches("\"step\":\"N15\"").count(), 2, "one attempt: N15 progress + receipt");
    assert!(!b.contains("\"step\":\"N17\""), "no repair round needed");
    assert_no_vendor(&b);
}
