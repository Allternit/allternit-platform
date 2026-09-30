//! WP7 router tests. Pure: no network, no model calls.

use super::*;
use serde_json::json;

const VENDOR: &[&str] = &[
    "jev", "anyjev", "gliner", "clm", "raven", "openai", "anthropic", "claude", "codex", "qwen", "gpt", "gemini",
    "llama", "mistral", "deepseek", "sonnet", "together", "fireworks",
];

fn entry(id: &str, roles: &[Role], modes: &[Mode], cap: &str, conf: f64, cost: f64, res: Residency) -> PoolEntry {
    let mut ext = Map::new();
    // Registry data: concrete identity lives here and must never leak into a plan.
    ext.insert("x-model_ref".into(), json!("openai/gpt-4o via anthropic claude llama"));
    PoolEntry {
        schema_id: POOL_ENTRY_SCHEMA_ID.into(),
        schema_version: SCHEMA_VERSION.into(),
        backend_id: id.into(),
        cognitive_roles: roles.to_vec(),
        modes: modes.to_vec(),
        capabilities: vec![cap.into()],
        trust_tags: vec!["PUBLIC".into(), "INTERNAL".into()],
        confidence_estimate: conf,
        latency_ms: 500.0,
        cost,
        residency: res,
        backbone_id: Some("backbone.openai.gpt".into()),
        model_revision: Some("claude-2026".into()),
        runtime: None,
        quantization: None,
        layer_stop: None,
        readout_head_id: None,
        calibration_manifest_id: None,
        memory_mb: None,
        load_latency_ms: None,
        extensions: Some(ext),
    }
}

/// Publish S1 calibration status on a pool entry the way gizzi-code's ModelPool does.
fn calibrate(mut e: PoolEntry, status: &str, s1_mode: &str, gate_passed: bool) -> PoolEntry {
    let x = e.extensions.get_or_insert_with(Map::new);
    x.insert("x-calibration_status".into(), json!(status));
    x.insert("x-s1_mode".into(), json!(s1_mode));
    x.insert(
        "x-calibrations".into(),
        json!([{"primitive_id": "decide.choice", "manifest_id": "calib.decide.choice.v1", "level": "L1", "gate_passed": gate_passed}]),
    );
    e
}

fn pool() -> StaticModelPool {
    StaticModelPool {
        entries: vec![
            calibrate(entry("be.s1.head", &[Role::S1], &[Mode::M2CalibratedReadout, Mode::M3HiddenHead], "cap.decide.choice", 0.9, 0.01, Residency::Hot), "calibrated", "live", true),
            calibrate(entry("be.s1.decider", &[Role::S1], &[Mode::M4DedicatedDecider], "cap.decide.choice", 0.95, 0.02, Residency::Warm), "calibrated", "live", true),
            entry("be.gen.remote", &[Role::S2], &[Mode::M5Generative], "cap.code.edit", 0.8, 0.5, Residency::Remote),
            entry("be.gen.local", &[Role::S2], &[Mode::M5Generative], "cap.code.edit", 0.7, 0.1, Residency::Warm),
            entry("be.deep", &[Role::S2, Role::S3], &[Mode::M5Generative, Mode::M6DeepSolver], "cap.code.edit", 0.95, 2.0, Residency::Remote),
        ],
    }
}

fn node(id: &str, role: Option<&str>, cap: Option<&str>, trust: &str) -> GraphNode {
    let mut v = json!({
        "node_id": id, "primitive_id": "decide.choice", "node_kind": "COMPUTE",
        "inputs": [], "outputs": [], "read_set": [], "write_set": [], "lock_scope": [],
        "on_failure": {"strategy": "FAIL"}
    });
    if let Some(r) = role {
        v["cognitive_role"] = json!(r);
    }
    if let Some(c) = cap {
        v["capability_request"] = json!({
            "schema_id": "allternit.kernel.CapabilityRequestV1", "schema_version": "1.0.0",
            "capability": c, "modality": "CODE", "latency_class": "NORMAL",
            "trust_requirement": trust, "budget": {}
        });
    }
    serde_json::from_value(v).expect("node")
}

fn ledger(c: f64) -> BudgetLedger {
    BudgetLedger { remaining_cost_units: c, remaining_wall_ms: None }
}

/// Calibration now comes from the pool; the config carries none.
fn calibrated() -> RouterConfig {
    RouterConfig::default()
}

// ---- plan generation per mode

#[test]
fn m0_for_s0_goes_to_primitive_registry() {
    let (p, cfg) = (pool(), RouterConfig::default());
    let plan = Router::new(&p, &cfg).route(&node("n0", Some("S0"), None, "PUBLIC"), &ledger(1.0)).unwrap();
    assert_eq!(plan.execution_mode, Mode::M0Deterministic);
    assert_eq!(plan.backend_id, S0_BACKEND_ID);
    assert_eq!(plan.cognitive_role, Role::S0);
}

#[test]
fn m2_for_calibrated_s1_cheapest_mode_first() {
    let (p, cfg) = (pool(), calibrated());
    let plan = Router::new(&p, &cfg).route(&node("n1", Some("S1"), Some("cap.decide.choice"), "PUBLIC"), &ledger(1.0)).unwrap();
    assert_eq!(plan.execution_mode, Mode::M2CalibratedReadout);
    assert_eq!(plan.backend_id, "be.s1.head");
    assert_eq!(plan.fallback_chain, vec!["be.s1.decider".to_string()]);
    assert_eq!(plan.calibration_manifest_id.as_deref(), Some("calib.decide.choice.v1"));
    assert_eq!(plan.calibration_level, Some(CalibrationLevel::L1));
}

#[test]
fn m3_and_m4_via_allowed_modes() {
    let (p, cfg) = (pool(), calibrated());
    let r = Router::new(&p, &cfg);
    let mut n = node("n3", Some("S1"), Some("cap.decide.choice"), "PUBLIC");
    n.allowed_modes = vec!["M3.HIDDEN_HEAD".into()];
    assert_eq!(r.route(&n, &ledger(1.0)).unwrap().execution_mode, Mode::M3HiddenHead);
    n.allowed_modes = vec!["M4.DEDICATED_DECIDER".into()];
    let plan = r.route(&n, &ledger(1.0)).unwrap();
    assert_eq!((plan.execution_mode, plan.backend_id.as_str()), (Mode::M4DedicatedDecider, "be.s1.decider"));
}

#[test]
fn m1_is_never_legal_for_s1() {
    let (p, cfg) = (pool(), calibrated());
    let mut n = node("n1b", Some("S1"), Some("cap.decide.choice"), "PUBLIC");
    n.allowed_modes = vec!["M1.LOGIT_READOUT".into()];
    assert!(matches!(Router::new(&p, &cfg).route(&n, &ledger(1.0)), Err(RouteError::NoLegalMode(_))));
}

#[test]
fn m5_for_s2_scores_utility_and_orders_fallback() {
    let (p, cfg) = (pool(), RouterConfig::default());
    let plan = Router::new(&p, &cfg).route(&node("n5", Some("S2"), Some("cap.code.edit"), "PUBLIC"), &ledger(10.0)).unwrap();
    assert_eq!(plan.execution_mode, Mode::M5Generative);
    // local 0.7-0.1-0.02 = 0.58 > remote 0.8-0.5-0.05 = 0.25 > deep 0.95-2-0.05 < 0
    assert_eq!(plan.backend_id, "be.gen.local");
    assert_eq!(plan.fallback_chain, vec!["be.gen.remote".to_string(), "be.deep".to_string()]);
    assert_eq!(plan.residency_requirement, Some(Residency::Warm));
}

#[test]
fn m6_for_s3() {
    let (p, cfg) = (pool(), RouterConfig::default());
    let plan = Router::new(&p, &cfg).route(&node("n6", Some("S3"), Some("cap.code.edit"), "PUBLIC"), &ledger(10.0)).unwrap();
    assert_eq!((plan.execution_mode, plan.backend_id.as_str()), (Mode::M6DeepSolver, "be.deep"));
}

#[test]
fn capability_node_without_role_defaults_to_s2() {
    let (p, cfg) = (pool(), RouterConfig::default());
    let plan = Router::new(&p, &cfg).route(&node("nd", None, Some("cap.code.edit"), "PUBLIC"), &ledger(10.0)).unwrap();
    assert_eq!(plan.cognitive_role, Role::S2);
}

#[test]
fn missing_capability_backend_is_an_error() {
    let (p, cfg) = (pool(), RouterConfig::default());
    let r = Router::new(&p, &cfg).route(&node("nx", Some("S2"), Some("cap.nothing.here"), "PUBLIC"), &ledger(10.0));
    assert!(matches!(r, Err(RouteError::NoEligibleBackend { .. })));
}

// ---- policy filters

#[test]
fn policy_rejects_remote_for_secret_and_uncleared_trust() {
    let (p, cfg) = (pool(), RouterConfig::default());
    // SECRET: no entry carries the SECRET trust tag → all rejected.
    let r = Router::new(&p, &cfg).route(&node("np", Some("S3"), Some("cap.code.edit"), "SECRET"), &ledger(10.0));
    match r {
        Err(RouteError::PolicyRejected { reasons, .. }) => {
            assert!(reasons.iter().any(|x| x.contains("requires local residency")), "{reasons:?}")
        }
        other => panic!("expected PolicyRejected, got {other:?}"),
    }
}

#[test]
fn policy_denied_backend_and_no_remote() {
    let p = pool();
    let mut cfg = RouterConfig::default();
    cfg.policy.allow_remote = false;
    cfg.policy.denied_backends.insert("be.gen.local".into());
    let r = Router::new(&p, &cfg).route(&node("np2", Some("S2"), Some("cap.code.edit"), "PUBLIC"), &ledger(10.0));
    assert!(matches!(r, Err(RouteError::PolicyRejected { .. })));
    cfg.policy.denied_backends.clear();
    let plan = Router::new(&p, &cfg).route(&node("np2", Some("S2"), Some("cap.code.edit"), "PUBLIC"), &ledger(10.0)).unwrap();
    assert_eq!(plan.backend_id, "be.gen.local");
    assert!(plan.fallback_chain.is_empty(), "remote backends must not appear as fallbacks");
}

#[test]
fn policy_runs_before_scoring_quality_floor() {
    let (p, cfg) = (pool(), RouterConfig::default());
    let mut n = node("nq", Some("S2"), Some("cap.code.edit"), "PUBLIC");
    n.capability_request.as_mut().unwrap()["quality_floor"] = json!(0.9);
    let plan = Router::new(&p, &cfg).route(&n, &ledger(10.0)).unwrap();
    assert_eq!(plan.backend_id, "be.deep"); // only one ≥ 0.9, despite worst utility
    assert_eq!(plan.confidence_floor, 0.9);
}

// ---- budget

#[test]
fn exhausted_ledger_fails_closed_even_for_m0() {
    let (p, cfg) = (pool(), RouterConfig::default());
    let r = Router::new(&p, &cfg);
    assert!(matches!(r.route(&node("b0", Some("S0"), None, "PUBLIC"), &ledger(0.0)), Err(RouteError::BudgetExhausted(_))));
    assert!(matches!(r.route(&node("b1", Some("S2"), Some("cap.code.edit"), "PUBLIC"), &ledger(-1.0)), Err(RouteError::BudgetExhausted(_))));
    assert!(matches!(r.route(&node("b2", Some("S2"), Some("cap.code.edit"), "PUBLIC"), &ledger(f64::NAN)), Err(RouteError::BudgetExhausted(_))));
}

#[test]
fn nothing_affordable_fails_closed_and_node_budget_caps() {
    let (p, cfg) = (pool(), RouterConfig::default());
    let r = Router::new(&p, &cfg);
    // S3 only has be.deep (cost 2.0); ledger allows 1.0.
    assert!(matches!(r.route(&node("b3", Some("S3"), Some("cap.code.edit"), "PUBLIC"), &ledger(1.0)), Err(RouteError::BudgetExhausted(_))));
    // node budget 0.2 caps a 10.0 ledger → only be.gen.local fits.
    let mut n = node("b4", Some("S2"), Some("cap.code.edit"), "PUBLIC");
    n.budget = Some(json!({"max_cost_units": 0.2, "max_wall_ms": 1000}));
    let plan = r.route(&n, &ledger(10.0)).unwrap();
    assert_eq!(plan.backend_id, "be.gen.local");
    assert!(plan.fallback_chain.is_empty());
    assert_eq!(plan.cost_budget, Some(0.2));
    assert_eq!(plan.latency_budget_ms, Some(1000.0));
}

// ---- S1 calibration gate + shadow default

/// The WP8 decision runtime exactly as gizzi-code's ModelPool lists it today:
/// no gate-passing manifests → uncalibrated, M1 readout only, shadow.
fn decision_runtime_today() -> PoolEntry {
    serde_json::from_value(json!({
        "schema_id": "allternit.kernel.ModelPoolEntryV1", "schema_version": "1.0.0",
        "backend_id": "be.s1.decision_runtime", "cognitive_roles": ["S1"], "modes": ["M1.LOGIT_READOUT"],
        "capabilities": ["cap.decide.choice", "cap.decide.gate"],
        "trust_tags": ["PUBLIC", "INTERNAL", "RESTRICTED", "SECRET"],
        "confidence_estimate": 0, "latency_ms": 50, "cost": 0, "residency": "WARM",
        "extensions": {"x-source": "system-one-local", "x-calibration_status": "uncalibrated",
                       "x-s1_mode": "shadow", "x-calibrations": []}
    }))
    .unwrap()
}

#[test]
fn uncalibrated_s1_is_refused() {
    let cfg = RouterConfig::default();
    let n = node("s1", Some("S1"), Some("cap.decide.choice"), "PUBLIC");
    // Pool with only the decision runtime as it is today.
    let p = StaticModelPool { entries: vec![decision_runtime_today()] };
    assert!(matches!(Router::new(&p, &cfg).route(&n, &ledger(1.0)), Err(RouteError::UncalibratedS1 { .. })));
    // A manifest that failed the Q22 gate is refused like no manifest.
    let mut p = pool();
    for e in p.entries.iter_mut().filter(|e| e.cognitive_roles.contains(&Role::S1)) {
        *e = calibrate(e.clone(), "calibrated", "live", false);
    }
    assert!(matches!(Router::new(&p, &cfg).route(&n, &ledger(1.0)), Err(RouteError::UncalibratedS1 { .. })));
    // Even live config cannot promote an uncalibrated runtime.
    let live = RouterConfig { s1_mode: S1Mode::Live, ..RouterConfig::default() };
    let p = StaticModelPool { entries: vec![decision_runtime_today()] };
    assert!(matches!(Router::new(&p, &live).route(&n, &ledger(1.0)), Err(RouteError::UncalibratedS1 { .. })));
}

#[test]
fn shadow_only_pool_entry_never_yields_s1_auto() {
    // Calibrated but the pool publishes shadow: live config still ends in Shadow.
    let e = calibrate(decision_runtime_today(), "calibrated", "shadow", true);
    let mut e = e;
    e.modes = vec![Mode::M2CalibratedReadout];
    let p = StaticModelPool { entries: vec![e] };
    let cfg = RouterConfig { s1_mode: S1Mode::Live, ..RouterConfig::default() };
    let plan = Router::new(&p, &cfg).route(&node("s1", Some("S1"), Some("cap.decide.choice"), "PUBLIC"), &ledger(1.0)).unwrap();
    assert_eq!(plan.backend_id, "be.s1.decision_runtime");
    assert_eq!(plan.extensions.as_ref().unwrap()["x-s1_mode"], json!("shadow"));
    let auto = result("CALIBRATED", "L1", Some("calib.decide.choice.v1"), "AUTO", 0.99);
    assert_eq!(apply_s1_result(&plan, &auto), S1Verdict::Shadow { confidence: 0.99 });
}

fn result(semantics: &str, level: &str, calib: Option<&str>, action: &str, conf: f64) -> DecisionResultView {
    serde_json::from_value(json!({
        "envelope": {}, "operation": "CHOICE", "answer": "a", "evidence_refs": [], "latency_ms": 3,
        "confidence": conf, "confidence_semantics": semantics, "calibration_level_served": level,
        "calibration_id": calib, "threshold_action": action, "extensions": {"x-refused_uncalibrated": calib.is_none()}
    }))
    .unwrap()
}

#[test]
fn s1_is_shadow_by_default() {
    assert_eq!(S1Mode::default(), S1Mode::Shadow);
    assert_eq!(RouterConfig::default().s1_mode, S1Mode::Shadow);
    let (p, cfg) = (pool(), calibrated());
    let plan = Router::new(&p, &cfg).route(&node("s1", Some("S1"), Some("cap.decide.choice"), "PUBLIC"), &ledger(1.0)).unwrap();
    assert_eq!(plan.extensions.as_ref().unwrap()["x-s1_mode"], json!("shadow"));
    let good = result("CALIBRATED", "L1", Some("calib.decide.choice.v1"), "AUTO", 0.99);
    assert_eq!(apply_s1_result(&plan, &good), S1Verdict::Shadow { confidence: 0.99 });
}

#[test]
fn s1_live_accepts_only_calibrated_auto_matching_manifest() {
    let (p, mut cfg) = (pool(), calibrated());
    cfg.s1_mode = S1Mode::Live;
    let plan = Router::new(&p, &cfg).route(&node("s1", Some("S1"), Some("cap.decide.choice"), "PUBLIC"), &ledger(1.0)).unwrap();
    let id = Some("calib.decide.choice.v1");
    assert_eq!(apply_s1_result(&plan, &result("CALIBRATED", "L1", id, "AUTO", 0.99)), S1Verdict::Accept);
    assert!(matches!(apply_s1_result(&plan, &result("CALIBRATED", "L1", id, "REVIEW", 0.99)), S1Verdict::Escalate { .. }));
    assert!(matches!(apply_s1_result(&plan, &result("UNCALIBRATED", "NONE", None, "REVIEW", 0.99)), S1Verdict::Refused { .. }));
    assert!(matches!(apply_s1_result(&plan, &result("CALIBRATED", "L1", Some("calib.other"), "AUTO", 0.99)), S1Verdict::Refused { .. }));
}

// ---- vendor neutrality + ABI shape

fn all_plans() -> Vec<ExecutionPlan> {
    let (p, cfg) = (pool(), calibrated());
    let r = Router::new(&p, &cfg);
    vec![
        r.route(&node("v0", Some("S0"), None, "PUBLIC"), &ledger(10.0)).unwrap(),
        r.route(&node("v1", Some("S1"), Some("cap.decide.choice"), "PUBLIC"), &ledger(10.0)).unwrap(),
        r.route(&node("v2", Some("S2"), Some("cap.code.edit"), "PUBLIC"), &ledger(10.0)).unwrap(),
        r.route(&node("v3", Some("S3"), Some("cap.code.edit"), "PUBLIC"), &ledger(10.0)).unwrap(),
    ]
}

#[test]
fn serialized_plans_contain_no_vendor_or_model_names() {
    for plan in all_plans() {
        let text = serde_json::to_string(&plan).unwrap().to_lowercase();
        let hits: Vec<_> = VENDOR.iter().filter(|v| text.contains(*v)).collect();
        assert!(hits.is_empty(), "vendor names leaked into plan: {hits:?} in {text}");
    }
}

#[test]
fn serialized_plans_fit_the_frozen_execution_plan_schema() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../spec/Contracts/kernel/v1/schemas/capability.schema.json");
    let schema: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let def = &schema["$defs"]["ExecutionPlanV1"];
    let props = def["properties"].as_object().unwrap();
    for plan in all_plans() {
        let v = serde_json::to_value(&plan).unwrap();
        let obj = v.as_object().unwrap();
        for k in obj.keys() {
            assert!(props.contains_key(k), "field {k} not in ExecutionPlanV1");
        }
        for req in def["required"].as_array().unwrap() {
            assert!(obj.contains_key(req.as_str().unwrap()), "missing required {req}");
        }
        assert_eq!(obj["schema_id"], json!(PLAN_SCHEMA_ID));
        let modes = props["execution_mode"]["enum"].as_array().unwrap();
        assert!(modes.contains(&obj["execution_mode"]));
        // round-trips through the closed contract
        let back: ExecutionPlan = serde_json::from_value(v).unwrap();
        assert_eq!(back, plan);
    }
}

#[test]
fn pool_http_body_parses_and_rejects_unknown_fields() {
    let e = serde_json::to_value(&pool().entries[0]).unwrap();
    let body = json!({"schema_version": "1.0.0", "entries": [e.clone()]}).to_string();
    let snap = StaticModelPool::from_http_body(&body).unwrap();
    assert_eq!(snap.entries.len(), 1);
    assert_eq!(snap.entries[0].capabilities, vec!["cap.decide.choice".to_string()]);
    let mut bad = e;
    bad["provider"] = json!("x");
    let body = json!({"entries": [bad]}).to_string();
    assert!(matches!(StaticModelPool::from_http_body(&body), Err(RouteError::PoolUnavailable(_))));
}

#[test]
fn invalid_nonempty_allowed_modes_fail_closed() {
    let (p, cfg) = (pool(), RouterConfig::default());
    let r = Router::new(&p, &cfg);
    let mut n = node("typo", Some("S2"), Some("cap.code.edit"), "PUBLIC");
    n.allowed_modes = vec!["M5.GENERATIV".into()];
    assert!(r.route(&n, &ledger(10.0)).is_err());
    n.allowed_modes = vec!["M5.GENERATIVE".into(), "M5.GENERATIV".into()];
    assert!(r.route(&n, &ledger(10.0)).is_err());
    n.allowed_modes.clear();
    assert_eq!(r.route(&n, &ledger(10.0)).unwrap().execution_mode, Mode::M5Generative);
}
