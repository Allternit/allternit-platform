//! Agency executor bridge (WP11b): a compiled Agency run → kernel execution.
//!
//! Off by default. `ALLTERNIT_AGENCY_EXECUTE=1` turns it on; without it every
//! run parks in `waiting` with [`PARKED_REASON`], exactly as before. Never set
//! the flag in prod without a founder decision.
//!
//! What one run does (the WP10 BUG_FIX graph, driven like WP10's e2e rig):
//! * the graph is re-instantiated from the TaskIR (`bug_fix::instantiate`) and
//!   each visited node walks the kernel lifecycle (declared → … → closed);
//! * every node is routed by the commrails [`Router`] into an `ExecutionPlanV1`
//!   over the ModelPool gizzi-code serves at `GET /model-pool`; S0 nodes
//!   (tests, parsers, diff review, policy) run deterministically here;
//! * cognition (the patch-proposing S2 nodes) goes to gizzi-code over HTTP
//!   (`gizzi_completion`), addressed by the plan's backend. allternit-api never
//!   calls a model provider. `ALLTERNIT_AGENCY_COGNITION=scripted` swaps in a
//!   deterministic scripted executor (dev/conformance only, no model);
//! * every effect is reserved through the Gate (`Gate::reserve_tool_effect`)
//!   and completed on the signed chain (`record_tool_effect`); the
//!   mutation is preceded by a policy receipt (N13);
//! * Q11: `admit_effect` is checked before every effect and `charge` after it;
//!   a zero budget halts spend before the first effect;
//! * completion only through the verifier (N21): `close_by` must resolve to
//!   `verifier` from the run's `JudgePolicySet`, and every blocking criterion
//!   of `completion.bug_fix` needs a receipt-backed evidence ref.
//!
//! Q25 strict fence (required for Agency API runs): the run gets a disposable
//! per-run directory (never the server's own repo), commands run with a
//! cleared environment plus an allowlist, cwd = the run's checkout, HOME and
//! TMPDIR inside the run dir, and the only network egress (the clone) passes
//! the shared egress guard first. Local repo paths are refused unless listed
//! in `ALLTERNIT_AGENCY_LOCAL_REPOS` (dev only).

use super::bugfix::{self, edits::{Edit, Planned}}; // WP-B1
#[path = "bugfix/exec_hooks.rs"]
mod bugfix_hooks; // WP-B2: repro tests, baseline/flakes, judge, exploration (memo C 4–8)
pub(crate) mod generic; // WP-X2
use super::guard::Limits;
use super::store::{now, new_id, AgencyStore, EffectDenied, TERMINAL};
use crate::AppState;
use allternit_commrails::judge::completion::{load_policy, missing_evidence};
use allternit_commrails::judge::policy::{effective_policy, CloseBy};
use allternit_commrails::kernel::bug_fix;
use allternit_commrails::kernel::graph::ComputeGraph;
use allternit_commrails::kernel::lifecycle::{try_close, try_transition};
use allternit_commrails::kernel::router::{
    fetch_model_pool, BudgetLedger, ExecutionPlan, Mode, PoolEntry, Residency, Role, RouteError, Router, RouterConfig,
    RouterPolicy, StaticModelPool, POOL_ENTRY_SCHEMA_ID, SCHEMA_VERSION,
};
use allternit_commrails::kernel::router::{apply_s1_result_recording, DecisionResultView};
use allternit_commrails::kernel::s1_outcome::OutcomeReporter;
use allternit_commrails::kernel::{CloseOutcome, NodeState};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::runtime::Handle;

pub const EXECUTE_FLAG: &str = "ALLTERNIT_AGENCY_EXECUTE";
pub const PARKED_REASON: &str = "queued for execution; the executor is off on this server (ALLTERNIT_AGENCY_EXECUTE is not set)";
/// Q25 fence profile for every Agency API run.
pub const FENCE: &str = "strict";
const ENV_ALLOW: &[&str] = &["PATH", "LANG", "LC_ALL", "TERM"];
const CMD_TIMEOUT: Duration = Duration::from_secs(300);
const RUN_RECEIPT: &str = "allternit.kernel.RunReceiptV1";
const VERIFY_RECEIPT: &str = "allternit.kernel.VerificationReceiptV1";
const POLICY_RECEIPT: &str = "allternit.kernel.PolicyReceiptV1";
pub const EV_PLAN: &str = "agency.exec.plan";

pub fn enabled() -> bool {
    std::env::var(EXECUTE_FLAG).is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

pub const ORG_REASON: &str = "queued for execution; this organization is not allowed to execute on this server (ALLTERNIT_AGENCY_EXECUTE_ORGS)";

/// Why a new `waiting` run is waiting, for a run billed to `org`.
pub fn queued_reason_for(org: &str) -> &'static str {
    if !enabled() {
        PARKED_REASON
    } else if !Limits::from_env().org_allowed(org) {
        ORG_REASON
    } else {
        "queued for execution"
    }
}

/// Runs being driven now: run id → org (the concurrency caps count these).
static ACTIVE: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);
static RESUMED: std::sync::Once = std::sync::Once::new();

/// Hand a `waiting` run to the executor (no-op when the flag is off, the org
/// is not allowlisted, a concurrency cap is full, or the run is already being
/// driven; a run left `waiting` by a full cap starts when a slot frees).
pub fn spawn(st: Arc<AppState>, run_id: String) {
    if enabled() {
        tokio::spawn(admit_and_start(st, run_id, Limits::from_env()));
    }
}

/// Drive a kernel-UI template run (model steps). The caller has checked the
/// flags and org allowlist; admission here is the run being `waiting`.
pub(crate) fn spawn_template(st: Arc<AppState>, run_id: String) {
    let h = Handle::current();
    tokio::task::spawn_blocking(move || {
        let s = super::store(&st);
        let org = h.block_on(s.load_run(&run_id)).ok().flatten().map(|r| super::guard::run_org(&r.task_ir)).unwrap_or_default();
        if let Err(e) = super::template_exec::drive(&h, &st, &s, &run_id, &org) {
            tracing::warn!(run_id = %run_id, error = %e, "template executor stopped with an error");
            let _ = h.block_on(finish(&s, &run_id, "failed", &format!("executor error: {e}"), None));
        }
    });
}

/// Start driving regardless of the flag, with no spending guard (tests;
/// `spawn` is the gated entry).
pub(crate) fn start(st: Arc<AppState>, run_id: String) {
    tokio::spawn(admit_and_start(st, run_id, Limits::unlimited()));
}

/// Admission: the run must be `waiting`, its org allowlisted, and a global
/// and per-org concurrency slot free. Returns whether it started.
pub(crate) async fn admit_and_start(st: Arc<AppState>, run_id: String, limits: Limits) -> bool {
    let s = super::store(&st);
    let Ok(Some(rec)) = s.load_run(&run_id).await else { return false };
    if rec.run["status"] != "waiting" || rec.task_ir.get("template_id").is_some() {
        return false; // kernel-UI template runs are driven by kernel_ui::templates
    }
    // WP-X2: only task types enabled on this server run; others stay queued.
    if !super::task_types::is_enabled(rec.task_ir["task_type"].as_str().unwrap_or("BUG_FIX")) {
        return false;
    }
    let org = super::guard::run_org(&rec.task_ir);
    let limits = limits.tightened(&rec.task_ir["rules"]).with_org_policy(&super::safety::load_org(&st.db, &org).unwrap_or_default());
    if !limits.org_allowed(&org) {
        return false;
    }
    // Prod lanes (WP-P1): per-org run rate. Over it, the run stays `waiting`
    // and admission is retried in a minute (unless it starts some other way).
    if limits.org_runs_per_hour != usize::MAX
        && s.org_runs_started_last_hour(&org, &run_id).await.unwrap_or(0) >= limits.org_runs_per_hour
    {
        tracing::info!(run_id = %run_id, org = %org, "agency org run rate reached; run stays queued");
        let (st, id) = (st.clone(), run_id.clone());
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(60)).await;
            if enabled() {
                admit_boxed(st, id, Limits::from_env()).await;
            }
        });
        return false;
    }
    {
        let mut g = ACTIVE.lock().unwrap_or_else(|p| p.into_inner());
        let active = g.get_or_insert_with(HashMap::new);
        if active.contains_key(&run_id) || !limits.admits(active, &org) {
            return false;
        }
        active.insert(run_id.clone(), org.clone());
    }
    let h = Handle::current();
    tokio::task::spawn_blocking(move || {
        let s = super::store(&st);
        if let Err(e) = drive(&h, &st, &s, &run_id, &limits, &org) {
            tracing::warn!(run_id = %run_id, error = %e, "agency executor stopped with an error");
            let _ = h.block_on(finish(&s, &run_id, "failed", &format!("executor error: {e}"), None));
        }
        ACTIVE.lock().unwrap_or_else(|p| p.into_inner()).get_or_insert_with(HashMap::new).remove(&run_id);
        // A slot freed: start the oldest runs that were waiting on a cap.
        if enabled() {
            let st = st.clone();
            h.spawn(async move {
                let s = super::store(&st);
                let mut waiting = s.runs_with_status(&["waiting"]).await.unwrap_or_default();
                waiting.reverse(); // oldest first
                for r in waiting {
                    if let Some(id) = r.run["id"].as_str() {
                        admit_boxed(st.clone(), id.to_string(), Limits::from_env()).await;
                    }
                }
            });
        }
    });
    true
}

/// [`admit_and_start`] behind a `Send` box: the slot-freed re-admission
/// calls it from inside `admit_and_start` itself, and the box breaks the
/// recursive opaque-future type.
fn admit_boxed(st: Arc<AppState>, run_id: String, limits: Limits) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send>> {
    Box::pin(admit_and_start(st, run_id, limits))
}

#[cfg(test)]
pub(crate) fn active_count() -> usize {
    ACTIVE.lock().unwrap_or_else(|p| p.into_inner()).as_ref().map_or(0, HashMap::len)
}

/// Move a `waiting` run to `running` and take its fencing token (WP-P1):
/// every earlier worker of this run is stale from here on. Also stamps the
/// wall-time origin (`run.safety.started_at`) on the first drive.
pub(crate) fn begin_drive(h: &Handle, st: &AppState, s: &AgencyStore, run_id: &str, why: &str) -> Result<Option<(super::store::RunRecord, i64)>> {
    h.block_on(async {
        let _g = s.lock().await;
        match s.load_run(run_id).await? {
            Some(mut r) if r.run["status"] == "waiting" => {
                let holder = format!("pid{}:{}", std::process::id(), uuid::Uuid::new_v4().simple());
                let epoch = super::safety::acquire_fence(&st.db, run_id, &holder)?;
                super::safety::note_drive(&mut r.run, epoch);
                Ok(Some((s.transition(r, "running", Some(why)).await?, epoch)))
            }
            _ => Ok(None),
        }
    })
}

/// After a restart: runs left `running`/`waiting` are re-queued once.
pub fn resume_inflight_once(st: &Arc<AppState>) {
    if !enabled() {
        return;
    }
    RESUMED.call_once(|| {
        let st = st.clone();
        tokio::spawn(async move {
            let s = super::store(&st);
            for mut rec in s.runs_with_status(&["waiting", "running"]).await.unwrap_or_default() {
                let id = rec.run["id"].as_str().unwrap_or_default().to_string();
                if rec.run["status"] == "running" {
                    let _g = s.lock().await;
                    match s.load_run(&id).await {
                        Ok(Some(r)) => rec = r,
                        _ => continue,
                    }
                    if s.transition(rec, "waiting", Some("re-queued after restart")).await.is_err() {
                        continue;
                    }
                }
                spawn(st.clone(), id);
            }
        });
    });
}

pub(crate) async fn finish(s: &AgencyStore, run_id: &str, to: &str, reason: &str, patch: Option<Value>) -> Result<()> {
    let _g = s.lock().await;
    let Some(mut rec) = s.load_run(run_id).await? else { return Ok(()) };
    let st = rec.run["status"].as_str().unwrap_or_default();
    if TERMINAL.contains(&st) || matches!(st, "paused" | "needs_attention") {
        return Ok(()); // a caller or the budget already settled it
    }
    if let Some(Value::Object(p)) = patch {
        for (k, v) in p {
            rec.run[k.as_str()] = v;
        }
    }
    s.transition(rec, to, Some(reason)).await?;
    Ok(())
}

// ── model pool / router ──────────────────────────────────────────────────────

fn class_of(e: &PoolEntry) -> String {
    allternit_commrails::kernel::router::model_class(e)
}

use allternit_commrails::kernel::router::class_matches;

pub const EV_ROUTING: &str = "agency.exec.routing";

/// Enforce the stored routing policy (kernel UI section 2) on a pool snapshot.
/// `eff` is the effective policy, `sources` its per-field scope kinds.
/// * `s2.default` / `s2.overrides[cap]` become the preferred model class for
///   S2 nodes (soft: an unmatched class falls through to any eligible entry);
/// * `s3.solver` then `s3.fallback` is the escalation order for S3 nodes;
/// * `local_only` drops remote entries and fails closed (Err) when no local
///   generative candidate is left;
/// * `retrieval` is recorded in the trace only (the context compiler takes no
///   model yet). Plans keep opaque backend ids.
/// Returns the pool, router config and the trace (Routing tab data).
pub fn apply_policy(mut pool: StaticModelPool, mut cfg: RouterConfig, eff: &Value, sources: &Value) -> Result<(StaticModelPool, RouterConfig, Value), String> {
    let text = |v: &Value| v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let local_only = eff["local_only"] == true;
    if local_only {
        pool.entries.retain(|e| e.residency != Residency::Remote);
        cfg.policy.allow_remote = false;
        if !pool.entries.iter().any(|e| e.cognitive_roles.iter().any(|r| matches!(r, Role::S2 | Role::S3))) {
            return Err("the routing policy is local_only and no local model candidate is available".into());
        }
    }
    let default = text(&eff["s2"]["default"]);
    if let Some(d) = &default {
        cfg.class_preference.insert("role:S2".into(), vec![d.clone()]);
    }
    let mut overrides = Map::new();
    for (cap, v) in eff["s2"]["overrides"].as_object().into_iter().flatten() {
        if let Some(c) = text(v) {
            cfg.class_preference.insert(format!("{cap}@S2"), std::iter::once(c.clone()).chain(default.clone()).collect());
            overrides.insert(cap.clone(), json!(c));
        }
    }
    let escalation: Vec<String> = [&eff["s3"]["solver"], &eff["s3"]["fallback"]].into_iter().filter_map(text).collect();
    if !escalation.is_empty() {
        cfg.class_preference.insert("role:S3".into(), escalation.clone());
    }
    let from_policy = sources.as_object().is_some_and(|m| m.values().any(|v| v != "default"));
    let trace = json!({
        "policy_source": if from_policy { "routing_policy" } else { "default" }, "field_sources": sources,
        "s2": { "default": default, "overrides": overrides }, "s3": { "escalation": escalation },
        "retrieval": { "model": text(&eff["retrieval"]), "applied": false, "note": "the context compiler takes no model yet; recorded only" },
        "local_only": local_only, "s1_backend": eff["s1_backend"], "candidates": pool.entries.len(),
    });
    Ok((pool, cfg, trace))
}

/// S1 shadow backend for a run: the stored routing policy's `s1_backend`, or,
/// with none stored, `ALLTERNIT_S1_BACKEND` (a server default; e.g.
/// `laya_bundled`), else `auto`: the S1 server uses Laya while it is healthy
/// and its local engine otherwise, so a Laya install or load that finishes
/// after this process started is picked up without a restart.
pub fn s1_backend_for(policy: Option<&(Value, Value)>) -> String {
    match policy {
        Some((e, src)) if src["s1_backend"] != "default" => e["s1_backend"].as_str().unwrap_or("off").to_string(),
        _ => std::env::var("ALLTERNIT_S1_BACKEND").ok()
            .filter(|b| b == "auto" || crate::kernel_ui::routing_policy::S1_BACKENDS.contains(&b.as_str()))
            .unwrap_or_else(|| "auto".into()),
    }
}

/// Effective routing policy for a run (most specific scope first, org last).
pub fn policy_for_run(st: &AppState, ir: &Value, org: &str) -> Option<(Value, Value)> {
    let mut chain: Vec<String> = ir["routing_scopes"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
    let o = format!("org:{org}");
    if !chain.contains(&o) {
        chain.push(o);
    }
    crate::kernel_ui::routing_policy::resolve(&st.db, &chain).ok()
}

/// Section-6 speed fields for one step. `tokens_in/out` are the gizzi split
/// (null when gizzi reported none); `tok_per_s` uses output tokens when known,
/// else the total, over model time.
pub fn speed_fields(started_at: String, duration_ms: u64, tin: u64, tout: u64, total: u64, model_ms: u64, wait_ms: u64) -> Value {
    let rate_tokens = if tout > 0 { tout } else { total };
    let tok_per_s = (rate_tokens > 0 && model_ms > 0).then(|| (rate_tokens as f64 * 1000.0 / model_ms as f64 * 10.0).round() / 10.0);
    let split = tin > 0 || tout > 0;
    json!({ "started_at": started_at, "duration_ms": duration_ms, "tokens_in": split.then_some(tin), "tokens_out": split.then_some(tout),
            "tokens": total, "tok_per_s": tok_per_s, "wait_ms": wait_ms })
}

/// Apply the request's `models` constraints (model classes / locality) to a
/// pool snapshot. Classes are logical; no vendor or model name is involved.
pub fn constrain(pool: StaticModelPool, models: &Value) -> (StaticModelPool, RouterConfig) {
    let list = |k: &str| -> Vec<String> {
        models[k].as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default()
    };
    let (allow, deny) = (list("allow_classes"), list("deny_classes"));
    let local_only = models["locality"] == "local_only";
    let entries = pool.entries.into_iter().filter(|e| {
        let c = class_of(e);
        (allow.is_empty() || allow.iter().any(|a| class_matches(&c, a)))
            && !deny.iter().any(|d| class_matches(&c, d))
            && !(local_only && e.residency == Residency::Remote)
    }).collect();
    let cfg = RouterConfig { policy: RouterPolicy { allow_remote: !local_only, ..Default::default() }, ..Default::default() };
    (StaticModelPool { entries }, cfg)
}

fn scripted_entry(id: &str, role: Role, mode: Mode, residency: Residency, caps: &[String]) -> PoolEntry {
    PoolEntry {
        schema_id: POOL_ENTRY_SCHEMA_ID.into(), schema_version: SCHEMA_VERSION.into(), backend_id: id.into(),
        cognitive_roles: vec![role], modes: vec![mode], capabilities: caps.to_vec(),
        trust_tags: vec!["PUBLIC".into(), "INTERNAL".into()], confidence_estimate: 0.9, latency_ms: 1.0, cost: 0.0,
        residency, backbone_id: None, model_revision: None, runtime: None, quantization: None, layer_stop: None,
        readout_head_id: None, calibration_manifest_id: None, memory_mb: None, load_latency_ms: None,
        extensions: Some(Map::new()),
    }
}

/// Deterministic stand-in pool for the scripted executor: one local and one
/// remote class, so `models.allow_classes` routing is exercised for real.
pub(crate) fn scripted_pool(g: &ComputeGraph) -> StaticModelPool {
    let caps: Vec<String> = g.nodes.iter()
        .filter_map(|n| n.capability_request.as_ref()?.get("capability")?.as_str().map(str::to_string)).collect();
    let mut entries = vec![];
    for (id, res) in [("be.scripted.local", Residency::Warm), ("be.scripted.remote", Residency::Remote)] {
        entries.push(scripted_entry(&format!("{id}.s2"), Role::S2, Mode::M5Generative, res, &caps));
        entries.push(scripted_entry(&format!("{id}.s3"), Role::S3, Mode::M6DeepSolver, res, &caps));
    }
    StaticModelPool { entries }
}

#[cfg(test)]
pub mod tests_support {
    pub fn scripted_pool(g: &super::ComputeGraph) -> super::StaticModelPool { super::scripted_pool(g) }
    /// A remote S2 entry with no capabilities (lane tests).
    pub fn entry(id: &str) -> super::PoolEntry {
        super::scripted_entry(id, super::Role::S2, super::Mode::M5Generative, super::Residency::Remote, &[])
    }
}

/// gizzi-code's live pool advertises generic model capabilities
/// (`cap.code.edit`, `cap.text.generate`, ...); task templates request
/// task-scoped ones (`cap.bug_fix.patch_candidate`, ...). Without this bridge
/// no live backend is ever eligible and every real run fails at its first
/// cognitive node. A task capability no entry offers is granted to the
/// generative (S2/S3) entries that offer its generic counterpart: code-writing
/// steps need `cap.code.edit`, every other step `cap.text.generate`. S1
/// entries are left alone (the decision runtime keeps its own `cap.decide.*`).
pub(crate) fn bridge_task_caps(mut pool: StaticModelPool, g: &ComputeGraph) -> StaticModelPool {
    let requested: Vec<String> = g.nodes.iter()
        .filter_map(|n| n.capability_request.as_ref()?.get("capability")?.as_str().map(str::to_string)).collect();
    for cap in requested {
        if pool.entries.iter().any(|e| e.capabilities.contains(&cap)) {
            continue;
        }
        let generic = if cap.ends_with(".patch_candidate") || cap.ends_with(".mutation") { "cap.code.edit" } else { "cap.text.generate" };
        for e in pool.entries.iter_mut() {
            let generative = e.cognitive_roles.iter().any(|r| matches!(r, Role::S2 | Role::S3));
            if generative && e.capabilities.iter().any(|c| c == generic) && !e.capabilities.contains(&cap) {
                e.capabilities.push(cap.clone());
            }
        }
    }
    pool
}

/// Backends one cognitive step may try before it fails.
pub(crate) const MAX_BACKEND_FALLBACKS: usize = 4;

pub(crate) fn scripted() -> bool {
    std::env::var("ALLTERNIT_AGENCY_COGNITION").is_ok_and(|v| v == "scripted")
}

// WP-B1: the patch prompt's context comes from the retrieval funnel
// (`bugfix::funnel`, same 16 KB / 48 KB caps), not every tracked file.

pub(crate) fn gizzi_url() -> String {
    crate::v1_routes::gizzi_base()
}

// ── strict-fence workspace ──────────────────────────────────────────────────

fn runs_root() -> PathBuf {
    std::env::var_os("ALLTERNIT_AGENCY_RUNS_DIR").map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("allternit-agency-runs"))
}

/// Repo roots of the server itself (cwd and binary): never a run workspace.
fn server_repo_roots() -> Vec<PathBuf> {
    let mut out = vec![];
    for start in [std::env::current_dir().ok(), std::env::current_exe().ok()].into_iter().flatten() {
        if let Some(root) = start.ancestors().find(|a| a.join(".git").exists()) {
            out.push(root.canonicalize().unwrap_or_else(|_| root.to_path_buf()));
        }
    }
    out
}

enum Source {
    Remote(String),
    Local(PathBuf),
}

fn resolve_source(h: &Handle, repo: &str) -> Result<Source, String> {
    if repo.starts_with("https://") {
        let url = url::Url::parse(repo).map_err(|e| format!("workspace.repo is not a valid URL: {e}"))?;
        let host = url.host_str().ok_or("workspace.repo has no host")?.to_string();
        if allternit_commrails::egress::host_is_forbidden_literal(&host) {
            return Err("egress guard refused the repository host".into());
        }
        h.block_on(allternit_commrails::egress::resolve_public(&host))
            .map_err(|e| format!("egress guard refused the repository host: {e}"))?;
        return Ok(Source::Remote(repo.to_string()));
    }
    let p = PathBuf::from(repo.strip_prefix("file://").unwrap_or(repo));
    let p = p.canonicalize().map_err(|_| "workspace.repo is not reachable".to_string())?;
    let allowed: Vec<PathBuf> = std::env::var("ALLTERNIT_AGENCY_LOCAL_REPOS").unwrap_or_default().split(':')
        .filter(|s| !s.is_empty()).filter_map(|s| PathBuf::from(s).canonicalize().ok()).collect();
    if !allowed.iter().any(|a| p.starts_with(a)) {
        return Err("local repository paths are refused under the strict fence".into());
    }
    if server_repo_roots().iter().any(|r| p.starts_with(r) || r.starts_with(&p)) {
        return Err("the server's own repository is never a run workspace".into());
    }
    Ok(Source::Local(p))
}

pub(crate) struct Ws {
    pub(crate) root: PathBuf,
    pub(crate) repo: PathBuf,
}

impl Ws {
    pub(crate) fn new(run_id: &str) -> Result<Self> {
        let base = runs_root();
        std::fs::create_dir_all(&base)?;
        let base = base.canonicalize()?;
        if server_repo_roots().iter().any(|r| base.starts_with(r)) {
            bail!("ALLTERNIT_AGENCY_RUNS_DIR is inside the server's repository");
        }
        let root = base.join(run_id);
        for d in ["home", "tmp"] {
            std::fs::create_dir_all(root.join(d))?;
        }
        Ok(Self { repo: root.join("repo"), root })
    }

    /// Run a command under the strict fence: cleared env + allowlist, cwd in
    /// the run's checkout, HOME/TMPDIR inside the run dir, bounded time.
    pub(crate) fn cmd(&self, cwd: &Path, args: &[&str]) -> Result<(bool, String)> {
        let mut c = Command::new(args[0]);
        c.args(&args[1..]).current_dir(cwd).env_clear();
        for k in ENV_ALLOW {
            if let Some(v) = std::env::var_os(k) {
                c.env(k, v);
            }
        }
        c.env("HOME", self.root.join("home")).env("TMPDIR", self.root.join("tmp")).env("CI", "1").env("NO_COLOR", "1").env("ALLTERNIT_FENCE", "strict") // commrails hook::FENCE_ENV
            .env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_TERMINAL_PROMPT", "0");
        let out_path = self.root.join("tmp").join(format!("cmd-{}.out", uuid::Uuid::new_v4().simple()));
        let out = std::fs::OpenOptions::new().create_new(true).read(true).write(true).open(&out_path)?;
        let mut child = c.stdin(Stdio::null()).stdout(out.try_clone()?).stderr(out.try_clone()?).spawn()
            .with_context(|| format!("spawn {}", args[0]))?;
        let t0 = Instant::now();
        let status = loop {
            if let Some(s) = child.try_wait()? {
                break s;
            }
            if t0.elapsed() > CMD_TIMEOUT {
                let _ = child.kill();
                let _ = child.wait();
                return Ok((false, format!("timed out after {}s", CMD_TIMEOUT.as_secs())));
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        use std::io::{Read, Seek};
        let mut out = out;
        out.rewind()?;
        let mut buf = String::new();
        let _ = out.take(64 * 1024).read_to_string(&mut buf);
        let _ = std::fs::remove_file(&out_path);
        Ok((status.success(), buf))
    }

    fn test_command(&self) -> Option<Vec<&'static str>> {
        let has = |f: &str| self.repo.join(f).exists();
        if has("package.json") {
            let pj: Value = serde_json::from_str(&std::fs::read_to_string(self.repo.join("package.json")).ok()?).ok()?;
            return pj["scripts"]["test"].is_string().then(|| vec!["npm", "test", "--silent"]);
        }
        if has("Cargo.toml") {
            return Some(vec!["cargo", "test", "-q", "--offline"]);
        }
        if has("pyproject.toml") || has("pytest.ini") || has("setup.py") {
            return Some(vec!["python3", "-m", "pytest", "-q"]);
        }
        None
    }
}

// ── the drive ───────────────────────────────────────────────────────────────

struct Exec<'a> {
    h: &'a Handle,
    st: &'a AppState,
    s: &'a AgencyStore,
    run_id: String,
    dag_id: String,
    seq: u32,
    ws: Ws,
    graph: ComputeGraph,
    pool: Option<StaticModelPool>,
    cfg: RouterConfig,
    attempts: std::collections::BTreeMap<String, u32>,
    limits: Limits,
    org: String,
    /// Output of the most recent S0 test run (input to the S1 shadow classification).
    last_test_output: String,
    /// Section-6 run speed: the current step's clock, tokens and model time
    /// (reset after each `run.progress`). Usage reports a token total only.
    mark: std::cell::Cell<Instant>,
    mark_at: std::cell::RefCell<String>,
    step_tokens: std::cell::Cell<u64>,
    step_tokens_in: std::cell::Cell<u64>,
    step_tokens_out: std::cell::Cell<u64>,
    step_model_ms: std::cell::Cell<u64>,
    /// Attention wait time not yet reported (added to the next step's wait_ms).
    pending_wait_ms: std::cell::Cell<u64>,
    /// Routing policy trace (why this route), added to every plan record.
    routing: Value,
    /// S1 shadow backend from the routing policy ("env" = no policy stored).
    s1_backend: String,
    /// WP-P1 fencing token of this drive (see `safety`).
    epoch: i64,
    /// Occurrences of (node, tool) in this drive: the stable idempotency key suffix.
    occ: HashMap<String, u32>,
    stuck: super::safety::StuckDetector,
    /// Per-run caps (server defaults tightened by org policy; a granted
    /// `run.safety.caps` override applies on top at check time).
    caps: super::safety::RunCaps,
    /// Subscription-lane entries held back under `api_first`.
    fallback: Vec<PoolEntry>,
}

/// Deterministic (S0) error code for a failing test run's output. Only a known
/// class is returned; anything uncertain is "UNKNOWN" (never guessed).
fn s0_error_code(out: &str) -> &'static str {
    let o = out.to_lowercase();
    let has = |ks: &[&str]| ks.iter().any(|k| o.contains(k));
    if has(&["syntaxerror", "syntax error"]) { "SYNTAX_ERROR" }
    else if has(&["modulenotfounderror", "importerror", "cannot find module"]) { "IMPORT_ERROR" }
    else if has(&["typeerror"]) { "TYPE_ERROR" }
    else if has(&["assertionerror", "assertion failed", "expected"]) { "TEST_ASSERTION" }
    else if has(&["timed out", "timeout"]) { "TEST_TIMEOUT" }
    else { "UNKNOWN" }
}

/// JSON Schema of a proposed whole-file patch (O10).
pub fn patch_schema() -> Value {
    json!({ "type": "object", "additionalProperties": false, "required": ["path", "content"],
        "properties": { "path": { "type": "string", "minLength": 1 }, "content": { "type": "string" } } })
}

/// Ask the S1 decision runtime (SHADOW, POST /v1/decision, CLASSIFY_ERROR) to
/// classify the failure S0 just reproduced, record the result, then report the
/// S0 truth as ground truth. Advisory only: the verdict is ignored, every error
/// (incl. an unreachable runtime) is swallowed, and control flow never changes.
fn s1_shadow_classify(h: &Handle, reporter: &OutcomeReporter, backend: &str, run_id: &str, failure: &str, evidence: &mut Vec<String>) {
    // The routing policy's s1_backend: `off` skips the shadow call.
    if backend == "off" || !reporter.enabled {
        return;
    }
    let bank = bug_fix::error_ontology();
    // The bank's classes already include its unknown class: mark it, never add a duplicate option.
    let mut candidates: Vec<Value> = bank.classes.iter()
        .map(|c| if *c == bank.unknown { json!({ "candidate_id": c, "label": c, "is_unknown": true }) } else { json!({ "candidate_id": c, "label": c }) })
        .collect();
    if !bank.classes.contains(&bank.unknown) {
        candidates.push(json!({ "candidate_id": bank.unknown, "label": bank.unknown, "is_unknown": true }));
    }
    let tail: String = failure.chars().rev().take(4000).collect::<Vec<_>>().into_iter().rev().collect();
    let body = json!({ "state": tail, "reversible": true, "backend": backend, "request": {
        "envelope": { "abi_version": "1.0.0", "schema_id": "allternit.kernel.DecisionRequestV1", "schema_version": "1.0.0",
            "run_id": run_id, "node_id": "N10" },
        "operation": "CHOICE", "state_projection_ref": format!("run:{run_id}:N10"),
        "instructions": "classify the error class of this failing test output", "decision_bank_id": bank.bank_id,
        "candidates": candidates, "calibration_domain": bank.primitive_id } });
    let url = format!("{}/v1/decision", reporter.base_url);
    let (timeout, token) = (reporter.timeout, reporter.token.clone());
    let asked = Instant::now();
    let result: Option<DecisionResultView> = h.block_on(async move {
        let c = reqwest::Client::builder().timeout(timeout).build().ok()?;
        let mut rq = c.post(url).json(&body);
        if let Some(t) = token { rq = rq.bearer_auth(t); }
        let r = rq.send().await.ok()?;
        if !r.status().is_success() { return None; }
        r.json::<DecisionResultView>().await.ok()
    });
    let Some(result) = result else { return };
    // O15: the decision is counted (cost 0). Shadow: S1 did not serve it.
    let ctx = crate::usage_ledger::current().unwrap_or_else(|| crate::usage_ledger::LedgerCtx::surface("agency")).run(run_id, Some("N10"));
    crate::usage_ledger::record(crate::usage_ledger::s1_decision_row(ctx, backend, false, asked.elapsed().as_millis() as u64, None));
    // The plan is a record of the shadow call; the verdict is deliberately unused.
    let plan: Option<ExecutionPlan> = serde_json::from_value(json!({
        "schema_id": "allternit.kernel.ExecutionPlanV1", "schema_version": "1.0.0", "plan_id": format!("s1shadow:{run_id}:N10"),
        "node_id": "N10", "cognitive_role": "S1", "capability_id": bank.primitive_id, "execution_mode": "M2.CALIBRATED_READOUT",
        "backend_id": "system-one-local", "confidence_floor": 1.0, "fallback_chain": [] })).ok();
    if let Some(plan) = plan {
        let _ = apply_s1_result_recording(&plan, &result, evidence);
    }
    // spawn_report needs a runtime context; the executor thread has none.
    let _g = h.enter();
    bug_fix::reconcile_s0_classification(reporter, Some(&result), s0_error_code(failure));
}

/// Time earlier drives of this run spent parked on attention/approvals
/// (created_at to resolved_at of every resolved request), in ms.
pub fn attention_wait_ms(attention: &[Value]) -> u64 {
    let parse = |v: &Value| v.as_str().and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());
    attention.iter().filter_map(|a| {
        let (c, r) = (parse(&a["created_at"])?, parse(&a["resolution"]["resolved_at"])?);
        Some((r - c).num_milliseconds().max(0) as u64)
    }).sum()
}

/// Why the drive stopped early without an error (the run is already settled
/// by someone else: paused, budget-halted, cancelled).
struct Stop;

type Step<T> = std::result::Result<T, StepErr>;
enum StepErr {
    Stop,
    Fail(anyhow::Error),
}
impl From<anyhow::Error> for StepErr {
    fn from(e: anyhow::Error) -> Self { StepErr::Fail(e) }
}
impl From<Stop> for StepErr {
    fn from(_: Stop) -> Self { StepErr::Stop }
}

impl Exec<'_> {
    fn version(&self) -> i64 {
        self.h.block_on(self.s.load_run(&self.run_id)).ok().flatten().and_then(|r| r.run["version"].as_i64()).unwrap_or(0)
    }

    fn emit(&self, ty: &str, data: Value) -> Result<()> {
        self.h.block_on(self.s.emit(&self.run_id, self.version(), ty, json!({ "data": data })))?;
        Ok(())
    }

    /// Section 6: speed fields for the step that just ended, then restart the
    /// step clock. `tok_per_s` only for model (S2) steps; `tokens_in/out` are
    /// gizzi's split; `wait_ms` is attention/approval wait time from earlier
    /// drives of this run, reported on the first step after the resume.
    fn take_speed(&self) -> Value {
        let started_at = self.mark_at.replace(super::store::now());
        let duration_ms = self.mark.replace(Instant::now()).elapsed().as_millis() as u64;
        speed_fields(started_at, duration_ms, self.step_tokens_in.replace(0), self.step_tokens_out.replace(0),
            self.step_tokens.replace(0), self.step_model_ms.replace(0), self.pending_wait_ms.replace(0))
    }

    /// Record the gizzi token split for the model call being charged.
    fn note_split(&self, tin: u64, tout: u64) {
        self.step_tokens_in.set(self.step_tokens_in.get() + tin);
        self.step_tokens_out.set(self.step_tokens_out.get() + tout);
    }

    fn admit(&self) -> Step<()> {
        // WP-P1 fence: a worker whose token is no longer current is stale
        // (another drive took the run over) and stops without touching it.
        if !super::safety::fence_current(&self.st.db, &self.run_id, self.epoch)? {
            tracing::warn!(run_id = %self.run_id, epoch = self.epoch, "stale agency worker; stopping");
            return Err(StepErr::Stop);
        }
        match self.h.block_on(self.s.admit_effect(&self.run_id)) {
            Ok(()) => Ok(()),
            Err(EffectDenied::NotFound) => Err(StepErr::Fail(anyhow!("run vanished"))),
            Err(_) => return Err(StepErr::Stop),
        }?;
        // WP-P1 per-run caps (steps, wall time, spend): halt spend, then park
        // with a `run_cap_reached` attention item.
        if let Some(rec) = self.h.block_on(self.s.load_run(&self.run_id))? {
            if let Some((dim, detail)) = self.caps.with_override(&rec.run).reached(&rec.run, &rec.attention) {
                tracing::info!(run_id = %self.run_id, dim, "agency per-run cap reached; parking run");
                self.h.block_on(self.s.park_halted(&self.run_id, super::safety::RUN_CAP_REASON, "run cap reached",
                    &format!("{detail} Spend stopped. Approve with value.caps raised to continue, or reject to stop."),
                    json!({ "dimension": dim, "consequential": true })))?;
                return Err(StepErr::Stop);
            }
        }
        // Daily spending caps (global and per org), before every effect and
        // model call: reached → park with "budget cap reached", spend stops.
        let (global, org) = self.h.block_on(self.s.daily_spend(&self.org))?;
        if let Some((scope, dim)) = self.limits.daily_reached(&global, &org) {
            tracing::info!(run_id = %self.run_id, scope, dim, "agency daily budget cap reached; parking run");
            self.h.block_on(self.s.park_for_cap(&self.run_id, scope, dim))?;
            return Err(StepErr::Stop);
        }
        // Agent rules: per-run spend threshold raises attention before continuing.
        if let Some(rec) = self.h.block_on(self.s.load_run(&self.run_id))? {
            if let Some(t) = rec.task_ir["rules"]["spend_over_usd"].as_f64() {
                if self.h.block_on(self.s.park_for_spend(&self.run_id, t))? {
                    return Err(StepErr::Stop);
                }
            }
        }
        Ok(())
    }

    fn charge(&self, secs: f64, usd: f64, steps: i64) -> Step<()> {
        self.charge_tokens(secs, usd, steps, 0)
    }

    fn charge_tokens(&self, secs: f64, usd: f64, steps: i64, tokens: u64) -> Step<()> {
        if tokens > 0 {
            self.step_tokens.set(self.step_tokens.get() + tokens);
            self.step_model_ms.set(self.step_model_ms.get() + (secs * 1000.0) as u64);
        }
        let rec = self.h.block_on(self.s.charge_usage(&self.run_id, secs, usd, steps, tokens))?;
        if rec.run["budget_usage"]["spend_halted"] == true {
            return Err(StepErr::Stop);
        }
        Ok(())
    }

    fn last_receipt_id(&self) -> Option<String> {
        let cs = self.st.rails.receipts.chain_store().ok()?;
        cs.read_run(&self.run_id).ok()?.last().and_then(|r| r["chain"]["receipt_id"].as_str().map(str::to_string))
    }

    /// Stable idempotency key of the next `tool` effect on `node` in this
    /// drive: `<run>:<node>:<tool>:<n>`. Journaled model outputs make the
    /// drive deterministic, so a re-drive derives the same keys.
    fn next_key(&mut self, node: &str, tool: &str) -> String {
        let n = { let c = self.occ.entry(format!("{node}:{tool}")).or_insert(0); *c += 1; *c };
        format!("{}:{node}:{tool}:{n}", self.run_id)
    }

    /// Park the run (spend halted first) because it is stuck.
    fn stuck_stop(&self, why: String) -> StepErr {
        tracing::info!(run_id = %self.run_id, %why, "agency stuck detector tripped; parking run");
        match self.h.block_on(self.s.park_halted(&self.run_id, super::safety::STUCK_REASON, "run is stuck",
            &format!("Stopped: {why}. Approve to retry from the journal, or reject to stop."), json!({ "consequential": true }))) {
            Ok(_) => StepErr::Stop,
            Err(e) => StepErr::Fail(e),
        }
    }

    /// One gated effect, two-phase (WP-P1): admitted (Q11 + fence + caps),
    /// PREPARED in the effect journal under this drive's fencing token,
    /// reserved and recorded on the signed chain through the gate, then
    /// COMMITTED with a compare-and-set on the token. A committed key is
    /// served from the journal and never re-applied; a stale worker's
    /// prepare or commit is refused and it stops. `retry_safe`: the effect
    /// may be re-applied when a dead worker left it unresolved. `f` returns
    /// the evidence ref.
    fn effect(&mut self, node: &str, tool: &str, class: &str, args: Value, f: impl FnOnce(&Ws) -> Result<String>) -> Step<String> {
        self.effect_with(node, tool, class, args, true, f)
    }

    fn effect_with(&mut self, node: &str, tool: &str, class: &str, args: Value, retry_safe: bool, f: impl FnOnce(&Ws) -> Result<String>) -> Step<String> {
        use super::safety::{self, Prepared};
        use allternit_commrails::receipts::store::ToolEffectAdmission;
        self.admit()?;
        self.seq += 1;
        let key = self.next_key(node, tool);
        let mut payload = args;
        payload["effect_class"] = json!(class);
        payload["node_id"] = json!(node);
        payload["fence"] = json!(FENCE);
        let args_hash = allternit_commrails::receipts::jcs::hash_value(&payload).map_err(|e| anyhow!("hash {tool} args: {e}"))?;
        let action = format!("{node}:{tool}:{args_hash}");
        let db = &self.st.db;
        let wih = self.run_id.strip_prefix("run_").unwrap_or(&self.run_id).to_string();
        let reserve = |p: &Value| self.st.rails.gate.reserve_tool_effect(&wih, tool, p);
        let chain_key = match safety::prepare(db, &key, &self.run_id, node, tool, &args_hash, self.epoch, retry_safe)? {
            Prepared::Stale => return Err(StepErr::Stop),
            Prepared::Committed(prev) => {
                // Replay: the effect already happened; serve its result.
                if let Some(why) = self.stuck.observe(&action, &prev) { return Err(self.stuck_stop(why)); }
                return Ok(prev);
            }
            Prepared::Diverged => return Err(StepErr::Fail(anyhow!("replay diverged: {key} was committed with different arguments (fail closed)"))),
            Prepared::FailedPermanent(e) => return Err(StepErr::Fail(anyhow!("{tool} failed earlier with a non-retryable error: {e}"))),
            Prepared::Unknown => {
                self.h.block_on(self.s.park_halted(&self.run_id, safety::UNKNOWN_EFFECT_REASON, "effect outcome unknown",
                    &format!("A previous worker started {tool} on {node} and stopped before recording the result. It is not safe to repeat automatically. Approve with value.effect_outcome = \"applied\" or \"not_applied\"."),
                    json!({ "idempotency_key": key, "consequential": true })))?;
                return Err(StepErr::Stop);
            }
            Prepared::Fresh { chain_key } => chain_key,
            Prepared::Takeover { old_chain_key, chain_key } => {
                // Did the dead worker get as far as committing on the chain?
                let mut old = payload.clone();
                old["idempotency_key"] = json!(old_chain_key);
                match reserve(&old) {
                    Ok(ToolEffectAdmission::AlreadyCommitted(prev)) => {
                        if !safety::commit(db, &key, &self.run_id, self.epoch, &prev)? { return Err(StepErr::Stop); }
                        return Ok(prev);
                    }
                    // We now hold the old chain claim: execute under it.
                    Ok(ToolEffectAdmission::Reserved(_)) => old_chain_key,
                    // Unresolved on the chain: retry-safe, so retry under a new claim.
                    _ => chain_key,
                }
            }
        };
        payload["idempotency_key"] = json!(chain_key);
        // Reserve-then-complete (review #10) on the signed chain; an earlier
        // reservation of this exact claim (Takeover → Reserved) is ours.
        match reserve(&payload) {
            Ok(ToolEffectAdmission::AlreadyCommitted(prev)) => {
                if !safety::commit(db, &key, &self.run_id, self.epoch, &prev)? { return Err(StepErr::Stop); }
                return Ok(prev);
            }
            Ok(_) => {}
            Err(e) if e.to_string().contains("already reserved") => {}
            Err(e) => {
                let msg = format!("gate refused {tool}: {e}");
                let _ = safety::fail(db, &key, &self.run_id, self.epoch, &msg);
                return Err(StepErr::Fail(anyhow!(msg)));
            }
        }
        let t0 = Instant::now();
        let ws = &self.ws;
        let res = self.st.rails.receipts.record_tool_effect(&self.run_id, tool, &payload, || f(ws));
        if let Some(rid) = self.last_receipt_id() {
            self.emit("receipt.appended", json!({ "receipt_id": rid, "receipt_type": "effect", "step": node }))?;
        }
        match &res {
            Ok(r) => {
                if !safety::commit(db, &key, &self.run_id, self.epoch, r)? {
                    tracing::warn!(run_id = %self.run_id, %key, "stale agency worker: commit refused by the fence");
                    return Err(StepErr::Stop);
                }
            }
            Err(e) => { let _ = safety::fail(db, &key, &self.run_id, self.epoch, &e.to_string()); }
        }
        self.charge(t0.elapsed().as_secs_f64(), 0.0, 1)?;
        let stuck = match &res { Ok(r) => self.stuck.observe(&action, r), Err(_) => self.stuck.observe_error(&action) };
        if let Some(why) = stuck { return Err(self.stuck_stop(why)); }
        Ok(res?)
    }

    /// Route one node (S1 without a gate-passing manifest falls back to its
    /// explicit F-node), walk its lifecycle, record the plan internally and
    /// report progress publicly (no backend identity).
    fn step(&mut self, id: &str, verified: bool) -> Step<Option<(String, ExecutionPlan)>> {
        let (ran, plan) = self.route_node(id)?;
        self.close_node(&ran, &plan, verified)?;
        Ok(Some((ran, plan)))
    }

    /// The routing half of [`Self::step`]: pick the node (or its F-node
    /// fallback) and its plan, without closing it. A node whose work runs
    /// after routing (patch generation) closes with [`Self::close_node`] once
    /// the work is done, so its duration and tokens land on it, not the next step.
    fn route_node(&mut self, id: &str) -> Step<(String, ExecutionPlan)> {
        self.admit()?;
        let ledger = BudgetLedger { remaining_cost_units: 1.0e9, remaining_wall_ms: None };
        let empty = StaticModelPool::default();
        let pool = self.pool.as_ref().unwrap_or(&empty);
        let router = Router::new(pool, &self.cfg);
        let node = self.graph.node(id).ok_or_else(|| anyhow!("graph has no node {id}"))?;
        let (ran, plan) = match router.route(node, &ledger) {
            Ok(p) => (id.to_string(), p),
            Err(RouteError::UncalibratedS1 { .. }) => {
                let fb = format!("F{}", &id[1..]);
                let n = self.graph.node(&fb).ok_or_else(|| anyhow!("no fallback for {id}"))?;
                (fb, router.route(n, &ledger).map_err(|e| anyhow!("route {id}: {e}"))?)
            }
            Err(e) => return Err(StepErr::Fail(anyhow!("route {id}: {e}"))),
        };
        Ok((ran, plan))
    }

    /// The lifecycle + progress half of [`Self::step`].
    fn close_node(&mut self, ran: &str, plan: &ExecutionPlan, verified: bool) -> Step<()> {
        let ran = ran.to_string();
        let mut s = NodeState::Declared;
        for to in [NodeState::Admitted, NodeState::Ready, NodeState::Leased, NodeState::Spawned, NodeState::Running,
                   NodeState::OutputReady, NodeState::Verifying] {
            s = try_transition(s, to).map_err(|e| anyhow!("{ran}: {e:?}"))?;
        }
        let closed = if verified {
            try_transition(s, NodeState::Committed).and_then(|s| try_close(s, CloseOutcome::Committed))
        } else {
            try_transition(s, NodeState::Replan).and_then(|s| try_close(s, CloseOutcome::Failed))
        };
        closed.map_err(|e| anyhow!("{ran}: {e:?}"))?;
        let attempt = { let a = self.attempts.entry(ran.clone()).or_insert(0); *a += 1; *a };
        let primitive = self.graph.node(&ran).map(|n| n.primitive_id.clone()).unwrap_or_default();
        let lane = super::safety::lane_for(self.pool.as_ref(), &plan.backend_id);
        self.h.block_on(self.s.append_raw(EV_PLAN, &self.run_id,
            json!({ "run_id": self.run_id, "node_id": ran, "attempt": attempt, "plan": plan, "routing": self.routing, "lane": lane,
                    "fence_epoch": self.epoch, "outcome": if verified { "committed" } else { "failed" } })))?;
        if !verified {
            if let Some(why) = self.stuck.observe_idle() { return Err(self.stuck_stop(why)); }
        }
        let speed = self.take_speed();
        self.emit("run.progress", json!({ "step": ran, "attempt": attempt, "primitive_id": primitive,
            "cognitive_role": plan.cognitive_role, "outcome": if verified { "committed" } else { "failed" },
            "started_at": speed["started_at"], "duration_ms": speed["duration_ms"], "tokens_in": speed["tokens_in"],
            "tokens_out": speed["tokens_out"], "tokens": speed["tokens"], "tok_per_s": speed["tok_per_s"],
            "wait_ms": speed["wait_ms"] }))?;
        Ok(())
    }

    fn tests(&mut self, node: &str, phase: &str) -> Step<(bool, String)> {
        let cmd = self.ws.test_command().ok_or_else(|| anyhow!("no test command detected in the workspace"))?;
        let mut captured = String::new();
        let id = self.effect(node, "tool.test_run", "EXECUTE", json!({ "phase": phase, "command": cmd }), |ws| {
            let (ok, out) = ws.cmd(&ws.repo, &cmd)?;
            captured = out.clone();
            let digest = allternit_commrails::receipts::jcs::sha256_tagged(out.as_bytes());
            Ok(format!("tests:{phase}:{}:{digest}", if ok { "PASS" } else { "FAIL" }))
        })?;
        self.last_test_output = captured;
        Ok((id.contains(":PASS:"), id))
    }

    /// Cognition for a patch-proposing node: the plan's backend via gizzi-code
    /// over HTTP, or the dev scripted executor. WP-B1: returns N candidate
    /// patches (anchored SEARCH/REPLACE edits, parsed but not yet validated
    /// against the checkout); a candidate that does not parse carries its error.
    fn propose(&mut self, plan: &ExecutionPlan, attempt: u32, goal: &str, failure: &str) -> Step<Vec<Result<Vec<Edit>, String>>> {
        self.admit()?;
        // WP-P1 replay: a proposal journaled by an earlier drive is served
        // as-is (no model call, no spend), so the drive stays deterministic
        // and its effect keys stay stable.
        let jkey = format!("{}:{}:model.propose:{attempt}", self.run_id, plan.node_id.as_deref().unwrap_or("N11"));
        if let Some(v) = super::safety::journaled_value(&self.st.db, &jkey)? {
            return Ok(bugfix::edits::candidates_from_journal(&v)); // WP-B1: candidate set (legacy {path, content} still read)
        }
        let t0 = Instant::now();
        // O15: every model call in this proposal lands on the run's ledger rows.
        let _ledger = crate::usage_ledger::enter(crate::usage_ledger::LedgerCtx::surface("agency")
            .run(&self.run_id, plan.node_id.as_deref()).tier("S2").tenant(Some(&self.org), None));
        let mut used = crate::gizzi_completion::Usage::default();
        let proposals = if scripted() {
            // Scripted entry per attempt: one proposal, or an array of candidates.
            let f = self.ws.repo.join(".allternit/scripted-patches.json");
            let v: Value = serde_json::from_str(&std::fs::read_to_string(&f).context("scripted executor: no .allternit/scripted-patches.json")?)
                .context("scripted-patches.json")?;
            let entry = v.get((attempt - 1) as usize).cloned().ok_or_else(|| anyhow!("scripted executor has no patch for attempt {attempt}"))?;
            match entry {
                Value::Array(c) => c.iter().map(bugfix::edits::from_value).collect(),
                one => vec![bugfix::edits::from_value(&one)],
            }
        } else {
            // Retrieval funnel: repo map + ranked files + focused snippets.
            let ls = self.ws.cmd(&self.ws.repo, &["git", "ls-files"]).map(|x| x.1).unwrap_or_default();
            let files: Vec<String> = ls.lines().map(str::trim).filter(|f| !f.is_empty()).map(String::from).collect();
            let repo = self.ws.repo.clone();
            let read = |f: &str| std::fs::read(repo.join(f)).ok().filter(|b| !b.contains(&0)).and_then(|b| String::from_utf8(b).ok());
            let funnel = bugfix::funnel::build(&files, goal, failure, read);
            let n = bugfix::candidate_count();
            // The pool lists every configured provider, including local ones
            // that are not running. A backend that does not answer, or answers
            // without a parseable edit block, is dropped for the rest of the
            // run and the node re-routed to the next eligible backend (bounded).
            let mut backend = plan.backend_id.clone();
            let mut found: Option<Vec<Result<Vec<Edit>, String>>> = None;
            let mut last_error: Option<String> = None;
            for _ in 0..MAX_BACKEND_FALLBACKS {
                let entry = self.pool.as_ref().and_then(|p| p.entries.iter().find(|e| e.backend_id == backend)).cloned();
                let model = entry.as_ref().and_then(|e| e.extensions.as_ref()?.get("x-model_ref")?.as_str()?.split_once('/'))
                    .map(|(p, m)| (p.to_string(), m.to_string()));
                // N candidates in parallel, plain-text replies (code is never
                // put inside JSON), tools off; a provider error is surfaced (#1106).
                let prompts: Vec<String> = (0..n).map(|k| bugfix::prompt(goal, &funnel.context, failure, k, n)).collect();
                let replies = self.h.block_on(futures::future::join_all(prompts.iter().map(|p|
                    crate::gizzi_completion::complete_ephemeral_usage(p, Some(bugfix::SYSTEM), model.as_ref()))));
                let mut parsed = vec![];
                for r in replies {
                    match r {
                        Ok((text, u)) => {
                            used.tokens += u.tokens;
                            used.tokens_in += u.tokens_in;
                            used.tokens_out += u.tokens_out;
                            used.cost_usd += u.cost_usd;
                            parsed.push(bugfix::edits::parse_blocks(&text));
                        }
                        Err(e) => last_error = Some(e),
                    }
                }
                if parsed.iter().any(Result::is_ok) {
                    found = Some(parsed);
                    break;
                }
                if let Some(Err(e)) = parsed.first() {
                    last_error.get_or_insert_with(|| e.clone());
                }
                tracing::warn!(run_id = %self.run_id, backend = %backend, "cognition backend gave no parseable edit blocks; trying the next backend");
                // Prod lanes: cool the failing backend down for every run.
                super::safety::cool_down(&backend);
                let Some(pool) = self.pool.as_mut() else { break };
                pool.entries.retain(|e| e.backend_id != backend);
                let ledger = BudgetLedger { remaining_cost_units: 1.0e9, remaining_wall_ms: None };
                let Some(node) = plan.node_id.as_deref().and_then(|n| self.graph.node(n)) else { break };
                match Router::new(pool, &self.cfg).route(node, &ledger) {
                    Ok(next) => backend = next.backend_id,
                    Err(_) if !self.fallback.is_empty() => {
                        // No API lane left: fall back to the held-back subscription lanes.
                        pool.entries.append(&mut self.fallback);
                        match Router::new(pool, &self.cfg).route(node, &ledger) {
                            Ok(next) => backend = next.backend_id,
                            Err(_) => break,
                        }
                    }
                    Err(_) => break,
                }
                self.admit()?;
            }
            self.note_split(used.tokens_in, used.tokens_out);
            let Some(found) = found else {
                self.charge_tokens(t0.elapsed().as_secs_f64(), used.cost_usd, 1, used.tokens)?;
                return Err(StepErr::Fail(match last_error {
                    Some(e) => anyhow!("cognition returned no patch (model error: {e})"),
                    None => anyhow!("cognition returned no patch"),
                }));
            };
            found
        };
        let cost = self.pool.as_ref().and_then(|p| p.entries.iter().find(|e| e.backend_id == plan.backend_id)).map(|e| e.cost).unwrap_or(0.0);
        // The reported cost when there is one, else the pool's estimate (per candidate).
        let usd = if used.cost_usd > 0.0 { used.cost_usd } else { cost * proposals.len().max(1) as f64 };
        self.charge_tokens(t0.elapsed().as_secs_f64(), usd, 1, used.tokens)?;
        let node = plan.node_id.clone().unwrap_or_default();
        if !super::safety::journal_value(&self.st.db, &jkey, &self.run_id, &node, "model.propose", self.epoch,
            &bugfix::edits::candidates_to_journal(&proposals))? {
            return Err(StepErr::Stop); // stale worker
        }
        Ok(proposals)
    }

    /// WP-B1 (N12): validate every candidate's anchored edits in memory against
    /// the checkout, drop duplicates, and when more than one distinct valid
    /// candidate remains, test each in its own isolated checkout (one gated
    /// effect) and pick the winner. Returns the winner (None = all invalid)
    /// and feedback text for the repair round.
    fn select_candidate(&mut self, plan: &ExecutionPlan, goal: &str, failure: &str, x: &bugfix::Extras, attempt: u32, proposals: &[Result<Vec<Edit>, String>]) -> Step<(Option<Planned>, String)> {
        let repo = self.ws.repo.clone();
        let read = |p: &str| std::fs::read_to_string(repo.join(p)).ok();
        let (mut valid, mut errors) = (vec![], vec![]);
        for (i, p) in proposals.iter().enumerate() {
            match p.as_ref().map_err(String::clone).and_then(|e| bugfix::edits::plan(e, &read)) {
                Ok(planned) => valid.push(planned),
                Err(e) => errors.push(format!("candidate {}: {e}", i + 1)),
            }
        }
        if valid.is_empty() {
            return Ok((None, format!("every proposed patch was rejected by validation:\n{}", errors.join("\n"))));
        }
        let kept = bugfix::select::dedupe(&valid);
        if kept.len() == 1 {
            return Ok((Some(valid.swap_remove(kept[0].0)), String::new()));
        }
        let cmd = self.ws.test_command().ok_or_else(|| anyhow!("no test command detected in the workspace"))?;
        let mut captured: Vec<bugfix::select::Outcome> = vec![];
        let idx: Vec<usize> = kept.iter().map(|k| k.0).collect();
        let ev = self.effect("N12", "tool.test_run", "EXECUTE", json!({ "phase": format!("candidates.{attempt}"), "candidates": idx.len(), "command": cmd }), |ws| {
            let items: Vec<(usize, &Planned)> = idx.iter().map(|&i| (i, &valid[i])).collect();
            captured = bugfix::evaluate(ws, attempt, &items, &cmd, x);
            Ok(bugfix::select::encode(attempt, &idx, &captured))
        })?;
        if captured.len() != idx.len() {
            // Re-driven run: the effect was already committed; recover its outcomes.
            let d = bugfix::select::decode(&ev);
            captured = idx.iter().map(|i| d.iter().find(|(j, _)| j == i).map(|x| x.1.clone())
                .unwrap_or(bugfix::select::Outcome::failed("", false))).collect();
        }
        let planned: Vec<Planned> = idx.iter().map(|&i| valid[i].clone()).collect();
        // WP-B2: votes over the full candidate set; the judge only orders ties after tests.
        let votes: Vec<usize> = kept.iter().map(|k| k.1).collect();
        let judged = self.b2_judge(plan, attempt, goal, failure, &planned, &captured, &votes)?;
        let win = bugfix::select::pick_with(&planned, &captured, &votes, judged).unwrap_or(0);
        tracing::info!(run_id = %self.run_id, attempt, candidates = idx.len(), winner = idx[win], evidence = %ev, "agency bug_fix candidate selected");
        let fb = if captured[win].tests_pass { String::new() } else { captured[win].output.clone() };
        Ok((Some(planned[win].clone()), fb))
    }

    fn policy_receipt(&self, node: &str, decision: &str, paths: &[String]) -> Result<String> {
        let cs = self.st.rails.receipts.chain_store()?;
        let write_set: Vec<String> = paths.iter().map(|p| format!("fs:{p}")).collect();
        let r = cs.append(json!({ "envelope": { "schema_id": POLICY_RECEIPT, "schema_version": "1.0.0", "run_id": self.run_id, "node_id": node },
            "type": "policy_decision", "decision": decision, "write_set": write_set, "fence": FENCE }))?;
        let id = r["chain"]["receipt_id"].as_str().unwrap_or_default().to_string();
        self.emit("receipt.appended", json!({ "receipt_id": id, "receipt_type": "policy_decision", "step": node }))?;
        Ok(id)
    }

    fn run(&mut self, goal: &str, repo: &str, git_ref: Option<&str>) -> Step<()> {
        // N00–N02: intake, criteria, environment (the disposable checkout).
        self.step("N00", true)?;
        self.step("N01", true)?;
        let src = resolve_source(self.h, repo);
        let dest = self.ws.repo.clone();
        let cloned = self.effect("N02", "tool.workspace_clone", "EXECUTE", json!({ "repo": repo, "ref": git_ref }), |ws| {
            let src = src.map_err(|e| anyhow!(e))?;
            if dest.join(".git").exists() {
                return Ok("workspace:ready".into());
            }
            let s = match &src { Source::Remote(u) => u.clone(), Source::Local(p) => p.display().to_string() };
            let d = dest.display().to_string();
            let mut args = vec!["git", "clone", "-q"];
            if let Some(r) = git_ref { args.extend(["--branch", r]); }
            args.extend(["--", s.as_str(), d.as_str()]);
            let (ok, out) = ws.cmd(&ws.root, &args)?;
            if !ok { bail!("clone failed: {}", out.lines().last().unwrap_or_default()); }
            Ok("workspace:ready".into())
        });
        if let Err(StepErr::Fail(e)) = &cloned {
            let msg = e.to_string();
            self.step("N02", false)?;
            return Err(StepErr::Fail(anyhow!("workspace unavailable: {msg}")));
        }
        cloned?;
        self.step("N02", true)?;
        for id in ["N03", "N04", "N05", "N06", "N07", "N08", "N09", "N10"] {
            self.step(id, true)?;
        }
        // Reproduce: the suite must fail before any change (evidence "before").
        let (ok, before) = self.tests("N10", "before")?;
        if ok {
            return Err(StepErr::Fail(anyhow!("could not reproduce: the test suite already passes, so a fix cannot be verified")));
        }
        let baseline_out = self.last_test_output.clone(); // WP-B2 regression baseline
        // S1 shadow CLASSIFY_ERROR on the reproduced failure: advisory only, never changes flow.
        {
            let out = self.last_test_output.clone();
            let mut evidence = Vec::new(); // s1-verify refs; kept for the node's completion decision
            let _ledger = crate::usage_ledger::enter(crate::usage_ledger::LedgerCtx::surface("agency").tenant(Some(&self.org), None));
            s1_shadow_classify(self.h, &OutcomeReporter::from_env(), &self.s1_backend, &self.run_id, &out, &mut evidence);
        }
        // ── WP-B1: N candidates of anchored multi-file edits per attempt ──
        let mut passed: Option<(String, Vec<String>)> = None; // (target receipt ref, changed paths)
        let mut failure = format!("{before}\n{}", self.last_test_output);
        let (mut prep, mut notes) = (bugfix::Extras::default(), String::new()); // WP-B2
        for (attempt, gen) in [(1u32, "N11"), (2, "N17")] {
            let (ran, plan) = self.route_node(gen)?;
            if attempt == 1 {
                // WP-B2: reproduction tests ∥ exploration ∥ planner, before the first patch.
                match self.b2_prepare(&plan, goal, &failure, &baseline_out) {
                    Ok(p) => (prep, notes) = p,
                    Err(e) => { self.close_node(&ran, &plan, false)?; return Err(e); }
                }
            }
            let proposed = self.propose(&plan, attempt, goal, &format!("{failure}{notes}"));
            self.close_node(&ran, &plan, proposed.is_ok())?;
            let proposals = proposed?;
            // N12 parse/shape + anchor validation (S0) and candidate selection.
            let (winner, feedback) = self.select_candidate(&plan, goal, &failure, &prep, attempt, &proposals)?;
            self.step("N12", winner.is_some())?;
            let Some(planned) = winner else {
                failure = feedback;
                continue;
            };
            let paths = planned.paths();
            // N13 policy (gate) then N14 the mutation, both on the chain.
            self.policy_receipt("N13", "ALLOW", &paths)?;
            self.step("N13", true)?;
            let key = planned.normalized_key();
            let p2 = planned.clone();
            self.effect("N14", "tool.fs_write", "WORKSPACE_WRITE", json!({ "paths": paths, "attempt": attempt,
                "content_sha256": key, "diff_lines": planned.diff_lines }), move |ws| {
                bugfix::select::write_planned(&ws.repo, &p2)?;
                Ok(format!("patch:{attempt}:{key}"))
            })?;
            self.step("N14", true)?;
            let (ok, id) = self.tests("N15", &format!("target.{attempt}"))?;
            self.step("N15", ok)?;
            if ok {
                passed = Some((id, paths));
                break;
            }
            // Repair round input: the test output plus what was tried; then
            // N16 restores the touched files so the next attempt anchors on
            // the original checkout.
            failure = format!("{id}\n{}\n{}\nThe previous attempt changed {} and did not pass.", self.last_test_output, feedback, paths.join(", "));
            let (tracked, created): (Vec<String>, Vec<String>) = paths.iter().cloned().partition(|p| !planned.created.contains(p));
            self.effect("N16", "tool.fs_write", "WORKSPACE_WRITE", json!({ "restore": tracked, "remove": created, "attempt": attempt }), move |ws| {
                for p in &created {
                    let _ = std::fs::remove_file(ws.repo.join(p));
                }
                if !tracked.is_empty() {
                    let mut args = vec!["git", "checkout", "--"];
                    args.extend(tracked.iter().map(String::as_str));
                    let (ok, out) = ws.cmd(&ws.repo, &args)?;
                    if !ok { bail!("restore failed: {}", out.lines().last().unwrap_or_default()); }
                }
                Ok(format!("restore:{attempt}"))
            })?;
            self.step("N16", true)?;
        }
        let evidence_run = passed.is_some();
        let (target, paths) = match &passed { Some((t, p)) => (t.clone(), p.clone()), None => (String::new(), vec![]) };
        let path = if paths.len() == 1 { paths[0].clone() } else { "bug-fix".to_string() };
        let mut evidence = vec![];
        let mut diff_text = String::new();
        if evidence_run {
            evidence.push(format!("target_tests_pass:receipt:{target}"));
            let (ok, affected) = self.b2_gate("N18", "affected", &baseline_out, &mut evidence)?; // WP-B2
            self.step("N18", ok)?;
            if ok { evidence.push(format!("affected_tests_pass:receipt:{affected}")); }
            self.b2_label(&before, &prep, &mut evidence)?; // WP-B2
            self.step("N19", true)?;
            // Requirements: the failure reproduced before and the target passes after.
            evidence.push(format!("requirements_satisfied:receipt:{before}->{target}"));
            let expect = paths.clone();
            let mut diff_out = String::new();
            let diff = self.effect("N20", "tool.git_diff", "EXECUTE", json!({ "expect": paths }), |ws| {
                // New files enter the diff as intent-to-add (WP-B1: patches may create files).
                let mut add = vec!["git", "add", "-N", "--"];
                add.extend(expect.iter().map(String::as_str).filter(|p| ws.repo.join(p).exists()));
                if add.len() > 4 { ws.cmd(&ws.repo, &add)?; }
                let (_, names) = ws.cmd(&ws.repo, &["git", "diff", "--name-only"])?;
                let changed: Vec<&str> = names.lines().filter(|l| !l.is_empty()).collect();
                let ok = !changed.is_empty() && changed.iter().all(|c| expect.iter().any(|e| e == c));
                diff_out = ws.cmd(&ws.repo, &["git", "diff"])?.1;
                Ok(format!("diff:{}:{}", if ok { "ACCEPT" } else { "REJECT" }, changed.join(",")))
            })?;
            diff_text = diff_out;
            let accept = diff.starts_with("diff:ACCEPT");
            self.step("N20", accept)?;
            if accept { evidence.push(format!("diff_review_accept:receipt:{diff}")); }
            let (ok, full) = self.b2_gate("N20", "regression", &baseline_out, &mut evidence)?; // WP-B2
            if ok { evidence.push(format!("no_new_regressions:receipt:{full}")); }
        }
        self.verify_and_close(&evidence, &path, &diff_text)
    }

    /// N21: verifier-owned completion. `close_by` must resolve to `verifier`
    /// for this DAG, and every blocking criterion needs receipt evidence.
    fn verify_and_close(&mut self, evidence: &[String], path: &str, diff: &str) -> Step<()> {
        let events = self.h.block_on(self.s.events_of_type(allternit_commrails::judge::events::POLICY_SET))?;
        let eff = effective_policy(&events, &self.dag_id, Some("N21"));
        if eff.close_by != CloseBy::Verifier {
            return Err(StepErr::Fail(anyhow!("completion is not verifier-owned for this run (fail closed)")));
        }
        let policy = load_policy("completion.bug_fix").ok_or_else(|| anyhow!("completion.bug_fix policy missing"))?;
        let missing = missing_evidence(&policy, evidence);
        let pass = missing.is_empty();
        let cs = self.st.rails.receipts.chain_store()?;
        let vr = cs.append(json!({ "envelope": { "schema_id": VERIFY_RECEIPT, "schema_version": "1.0.0", "run_id": self.run_id, "node_id": "N21" },
            "type": "verification", "verifier": "completion.bug_fix", "result": if pass { "PASS" } else { "FAIL" },
            "evidence_refs": evidence, "missing": missing }))?;
        let vid = vr["chain"]["receipt_id"].as_str().unwrap_or_default().to_string();
        self.emit("receipt.appended", json!({ "receipt_id": vid, "receipt_type": "verification", "step": "N21" }))?;
        self.step("N21", pass)?;
        // Per-criterion results on the Run (the request's criteria + the contract's).
        let rec = self.h.block_on(self.s.load_run(&self.run_id))?.ok_or_else(|| anyhow!("run vanished"))?;
        let criteria: Vec<Value> = rec.run["completion"]["criteria"].as_array().cloned().unwrap_or_default().into_iter().map(|mut c| {
            let id = c["criterion"].as_str().unwrap_or_default().to_string();
            let ok = evidence.iter().any(|e| e.starts_with(&format!("{id}:")));
            c["result"] = json!(if ok { "pass" } else { "fail" });
            c["receipt_ids"] = json!([vid]);
            c
        }).collect();
        self.emit("verification.completed", json!({ "result": if pass { "pass" } else { "fail" }, "receipt_id": vid, "missing": missing }))?;
        let mut patch = json!({ "completion": { "status": if pass { "verified" } else { "unverified" },
            "required": rec.run["completion"]["required"], "criteria": criteria, "verification_receipt_id": vid } });
        if !pass {
            self.run_receipt("failed", &vid)?;
            self.h.block_on(finish(self.s, &self.run_id, "failed", &format!("verifier: missing evidence for {}", missing.join(", ")), Some(patch)))?;
            return Ok(());
        }
        let hash = allternit_commrails::receipts::jcs::sha256_tagged(diff.as_bytes());
        let art = json!({ "id": new_id("art"), "object": "artifact", "run_id": self.run_id, "name": format!("{path}.patch"),
            "kind": "patch", "mime_type": "text/x-diff", "hash": hash, "size_bytes": diff.len(),
            "verification_status": "verified", "receipt_id": vid, "created_at": now(),
            "content": if diff.len() <= 256 * 1024 { json!(diff) } else { Value::Null } });
        self.emit("artifact.created", art.clone())?;
        self.step("N22", true)?;
        self.step("N23", true)?;
        let rid = self.run_receipt("completed", &vid)?;
        patch["output"] = json!({ "summary": format!("Fixed; verified by {} receipt-backed criteria.", evidence.len()), "artifact_ids": [art["id"]] });
        patch["completion"]["run_receipt_id"] = json!(rid);
        self.h.block_on(finish(self.s, &self.run_id, "completed", "verified by completion.bug_fix", Some(patch)))?;
        Ok(())
    }

    fn run_receipt(&self, outcome: &str, verification: &str) -> Result<String> {
        let cs = self.st.rails.receipts.chain_store()?;
        let r = cs.append(json!({ "envelope": { "schema_id": RUN_RECEIPT, "schema_version": "1.0.0", "run_id": self.run_id },
            "type": "run_completion", "outcome": outcome, "verification_receipt_id": verification, "fence": FENCE }))?;
        let id = r["chain"]["receipt_id"].as_str().unwrap_or_default().to_string();
        self.emit("receipt.appended", json!({ "receipt_id": id, "receipt_type": "run_completion" }))?;
        Ok(id)
    }
}

fn drive(h: &Handle, st: &AppState, s: &AgencyStore, run_id: &str, limits: &Limits, org: &str) -> Result<()> {
    let Some((_rec, epoch)) = begin_drive(h, st, s, run_id, "executing")? else { return Ok(()) };
    // Q11: a budget that is already exhausted halts spend before any effect.
    let rec = h.block_on(s.charge(run_id, 0.0, 0.0, 0))?;
    if rec.run["budget_usage"]["spend_halted"] == true {
        return Ok(());
    }
    let ir = rec.task_ir.clone();
    let limits = &limits.tightened(&ir["rules"]);
    let task_id = ir["wih_policy"]["task_id"].as_str().unwrap_or("task.bug_fix").to_string();
    let write_set: Vec<String> = ir["wih_policy"]["write_set"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
    let task_type = ir["task_type"].as_str().unwrap_or("BUG_FIX").to_string();
    let kernel_type = if task_type == "BUG_FIX" { None } else { Some(super::task_types::require(&task_type)?) };
    let graph = match kernel_type {
        None => bug_fix::instantiate(&task_id, &write_set)?,
        Some(t) => super::task_types::instantiate(t, &task_id, &write_set)?,
    };
    let models = ir["models"].clone();
    let raw_pool = if scripted() {
        Ok(scripted_pool(&graph))
    } else {
        h.block_on(fetch_model_pool(&gizzi_url(), None)).map_err(|e| anyhow!("{e}")).map(|p| bridge_task_caps(p, &graph))
    };
    let policy = policy_for_run(st, &ir, org);
    let s1_backend = s1_backend_for(policy.as_ref());
    let mut routing = json!({ "policy_source": "default" });
    let mut fallback = Vec::new();
    let (pool, cfg) = match raw_pool {
        Ok(p) => {
            let (p, c) = constrain(p, &models);
            // Prod lanes (WP-P1): cooled-down backends out; subscription
            // lanes held back as the fallback under `api_first`.
            let (p, fb) = super::safety::split_lanes(p, super::safety::lane_mode());
            fallback = fb;
            match policy.as_ref().map(|(e, src)| apply_policy(p.clone(), c.clone(), e, src)) {
                None => (Some(p), c),
                Some(Ok((p, c, trace))) => { routing = trace; (Some(p), c) }
                Some(Err(why)) => {
                    // Fail closed: no candidate satisfies local_only. Attention, no effect.
                    tracing::warn!(run_id, %why, "routing policy cannot be satisfied; failing closed with attention");
                    h.block_on(s.park_attention(run_id, "routing_policy_unsatisfied", "No local model available", &why, json!({})))?;
                    return Ok(());
                }
            }
        }
        Err(e) => {
            tracing::warn!(run_id, error = %e, "model pool unavailable; cognitive steps will fail closed");
            (None, RouterConfig::default())
        }
    };
    let mut x = Exec {
        last_test_output: String::new(),
        mark: std::cell::Cell::new(Instant::now()), mark_at: std::cell::RefCell::new(super::store::now()),
        step_tokens: Default::default(), step_model_ms: Default::default(), step_tokens_in: Default::default(), step_tokens_out: Default::default(),
        pending_wait_ms: std::cell::Cell::new(attention_wait_ms(&rec.attention)), routing, s1_backend,
        h, st, s, run_id: run_id.to_string(), dag_id: ir["dag_id"].as_str().unwrap_or_default().to_string(), seq: 0,
        ws: Ws::new(run_id)?, graph, pool, cfg, attempts: Default::default(), limits: limits.clone(), org: org.to_string(),
        epoch, occ: Default::default(), stuck: super::safety::StuckDetector::default(), fallback,
        caps: super::safety::RunCaps::from_env().tightened(&super::safety::load_org(&st.db, org).unwrap_or_default()),
    };
    let goal = ir["goal"].as_str().unwrap_or_default().to_string();
    let repo = ir["workspace"]["repo"].as_str().unwrap_or_default().to_string();
    let git_ref = ir["workspace"]["ref"].as_str().map(str::to_string);
    let out = match kernel_type {
        None => x.run(&goal, &repo, git_ref.as_deref()),
        Some(t) => x.run_task_type(t, &goal, &write_set),
    };
    let result = match out {
        Ok(()) | Err(StepErr::Stop) => Ok(()),
        Err(StepErr::Fail(e)) => {
            let _ = x.run_receipt("failed", "");
            h.block_on(finish(s, run_id, "failed", &e.to_string(), None))
        }
    };
    // Disposable: drop the checkout once the run is terminal.
    if let Ok(Some(r)) = h.block_on(s.load_run(run_id)) {
        if r.run["terminal"] == true {
            let _ = std::fs::remove_dir_all(&x.ws.root);
        }
    }
    result
}

#[cfg(test)]
mod s1_shadow_tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Mock decision runtime: answers /v1/decision with a shadow result, records every request.
    async fn mock() -> (String, tokio::sync::mpsc::UnboundedReceiver<String>) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = l.accept().await else { return };
                let tx = tx.clone();
                tokio::spawn(async move {
                    let (mut buf, mut got) = (vec![0u8; 16384], Vec::new());
                    loop {
                        let n = s.read(&mut buf).await.unwrap_or(0);
                        if n == 0 { break; }
                        got.extend_from_slice(&buf[..n]);
                        let txt = String::from_utf8_lossy(&got).to_string();
                        if let Some(i) = txt.find("\r\n\r\n") {
                            let len = txt.lines().find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
                            if got.len() >= i + 4 + len {
                                let first = txt.lines().next().unwrap_or("").to_string();
                                let resp = if first.contains("/v1/decision/outcome") { "{}".to_string() } else {
                                    json!({ "confidence": 0.4, "confidence_semantics": "UNCALIBRATED", "calibration_level_served": "RAW",
                                        "threshold_action": "REVIEW", "extensions": { "x-decision_id": "dec-123" } }).to_string() };
                                let _ = tx.send(format!("{first}|{}", &txt[i + 4..]));
                                let _ = s.write_all(format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\ncontent-type: application/json\r\n\r\n{resp}", resp.len()).as_bytes()).await;
                                break;
                            }
                        }
                    }
                });
            }
        });
        (url, rx)
    }

    fn reporter(url: &str) -> OutcomeReporter {
        OutcomeReporter { base_url: url.to_string(), token: None, timeout: Duration::from_millis(800), enabled: true }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn agency_s1_shadow_requests_decision_and_reports_outcome() {
        let (url, mut rx) = mock().await;
        let h = Handle::current();
        let r = reporter(&url);
        let ev = tokio::task::spawn_blocking(move || {
            let mut ev = vec![];
            s1_shadow_classify(&h, &r, "laya_bundled", "run_1", "FAILED: AssertionError: expected 2 got 3", &mut ev);
            ev
        }).await.unwrap();
        assert_eq!(ev, vec!["s1-verify:dec-123".to_string()]);
        let first = rx.recv().await.unwrap();
        assert!(first.starts_with("POST /v1/decision "), "{first}");
        assert!(first.contains("error_ontology.v0.1") && first.contains("AssertionError"), "{first}");
        let second = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await.expect("outcome reported").unwrap();
        assert!(second.starts_with("POST /v1/decision/outcome"), "{second}");
        assert!(second.contains("dec-123") && second.contains("TEST_ASSERTION"), "{second}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn agency_s1_shadow_unreachable_runtime_changes_nothing() {
        let h = Handle::current();
        // Nothing listens on port 1.
        let r = reporter("http://127.0.0.1:1");
        let ev = tokio::task::spawn_blocking(move || {
            let mut ev = vec![];
            s1_shadow_classify(&h, &r, "laya_bundled", "run_1", "AssertionError", &mut ev);
            ev
        }).await.unwrap();
        assert!(ev.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn agency_s1_off_backend_skips_the_shadow_call() {
        let (url, mut rx) = mock().await;
        let h = Handle::current();
        let r = reporter(&url);
        let ev = tokio::task::spawn_blocking(move || {
            let mut ev = vec![];
            s1_shadow_classify(&h, &r, "off", "run_1", "AssertionError: expected 2 got 3", &mut ev);
            ev
        }).await.unwrap();
        assert!(ev.is_empty());
        assert!(tokio::time::timeout(Duration::from_millis(400), rx.recv()).await.is_err(), "no request reaches the runtime");
    }

    fn pool_of(entries: Vec<PoolEntry>) -> StaticModelPool { StaticModelPool { entries } }

    fn classed(id: &str, class: &str, res: Residency, caps: &[&str]) -> PoolEntry {
        let caps: Vec<String> = caps.iter().map(|c| c.to_string()).collect();
        let mut e = scripted_entry(id, Role::S2, Mode::M5Generative, res, &caps);
        e.extensions.as_mut().unwrap().insert("x-model_class".into(), json!(class));
        e
    }

    fn gen_node(id: &str, role: &str, cap: &str) -> allternit_commrails::kernel::graph::GraphNode {
        serde_json::from_value(json!({ "node_id": id, "primitive_id": "prim.t", "node_kind": "COMPUTE", "cognitive_role": role,
            "capability_request": { "capability": cap }, "on_failure": { "strategy": "fail" } })).unwrap()
    }

    fn route(pool: &StaticModelPool, cfg: &RouterConfig, n: &allternit_commrails::kernel::graph::GraphNode) -> ExecutionPlan {
        Router::new(pool, cfg).route(n, &BudgetLedger { remaining_cost_units: 1.0e9, remaining_wall_ms: None }).unwrap()
    }

    #[test]
    fn routing_policy_override_picks_the_preferred_class_per_capability() {
        let pool = pool_of(vec![
            classed("be.a", "mc.fast", Residency::Remote, &["cap.x", "cap.y"]),
            classed("be.b", "mc.deep", Residency::Remote, &["cap.x", "cap.y"]),
        ]);
        let eff = json!({ "s1_backend": "off", "s2": { "default": "mc.fast", "overrides": { "cap.y": "mc.deep" } },
            "s3": { "solver": "", "fallback": "" }, "retrieval": "mc.ret", "local_only": false });
        let (p, cfg, trace) = apply_policy(pool, RouterConfig::default(), &eff, &json!({ "s2.default": "org" })).unwrap();
        assert_eq!(route(&p, &cfg, &gen_node("N1", "S2", "cap.x")).backend_id, "be.a");
        let plan = route(&p, &cfg, &gen_node("N2", "S2", "cap.y"));
        assert_eq!(plan.backend_id, "be.b", "override beats the default");
        assert_eq!(plan.extensions.as_ref().unwrap()["x-route_class"], "mc.deep");
        assert_eq!(trace["policy_source"], "routing_policy");
        assert_eq!(trace["retrieval"]["applied"], false);
        assert_eq!(trace["retrieval"]["model"], "mc.ret");
    }

    #[test]
    fn routing_policy_s3_solver_then_fallback_orders_escalation() {
        let mut a = classed("be.a", "mc.fast", Residency::Remote, &["cap.x"]);
        let mut b = classed("be.b", "mc.deep", Residency::Remote, &["cap.x"]);
        a.cognitive_roles = vec![Role::S3]; a.modes = vec![Mode::M6DeepSolver];
        b.cognitive_roles = vec![Role::S3]; b.modes = vec![Mode::M6DeepSolver];
        let eff = json!({ "s2": { "default": "", "overrides": {} }, "s3": { "solver": "mc.deep", "fallback": "mc.fast" }, "local_only": false });
        let (mut p, cfg, _) = apply_policy(pool_of(vec![a, b]), RouterConfig::default(), &eff, &json!({})).unwrap();
        let n = gen_node("N17", "S3", "cap.x");
        assert_eq!(route(&p, &cfg, &n).backend_id, "be.b");
        p.entries.retain(|e| e.backend_id != "be.b");
        assert_eq!(route(&p, &cfg, &n).backend_id, "be.a", "falls back to the fallback class");
    }

    #[test]
    fn routing_policy_local_only_with_no_local_candidate_fails_closed() {
        let remote = pool_of(vec![classed("be.r", "mc.remote", Residency::Remote, &["cap.x"])]);
        let eff = json!({ "s2": { "default": "", "overrides": {} }, "s3": { "solver": "", "fallback": "" }, "local_only": true });
        assert!(apply_policy(remote, RouterConfig::default(), &eff, &json!({})).is_err());
        let mixed = pool_of(vec![classed("be.r", "mc.remote", Residency::Remote, &["cap.x"]), classed("be.l", "mc.local", Residency::Warm, &["cap.x"])]);
        let (p, cfg, _) = apply_policy(mixed, RouterConfig::default(), &eff, &json!({})).unwrap();
        assert_eq!(route(&p, &cfg, &gen_node("N1", "S2", "cap.x")).backend_id, "be.l");
        assert!(!cfg.policy.allow_remote);
    }


    #[test]
    fn patch_schema_validates_replies() {
        let ok = |v: Value| crate::structured_output::validate(&patch_schema(), &v).is_ok();
        assert!(ok(json!({ "path": "src/a.py", "content": "x = 1\n" })));
        assert!(!ok(json!({ "path": "src/a.py" })));
        assert!(!ok(json!({ "path": "", "content": "x" })));
        assert!(!ok(json!({ "path": "a", "content": 3 })));
        let fenced = "```json\n{\"path\":\"a.py\",\"content\":\"y\"}\n```";
        assert_eq!(crate::structured_output::parse_text(fenced, &patch_schema(), "t"), Some(json!({ "path": "a.py", "content": "y" })));
    }

    #[test]
    fn speed_split_is_recorded_and_rate_uses_output_tokens() {
        let v = speed_fields("t".into(), 900, 100, 50, 150, 1000, 250);
        assert_eq!((v["tokens_in"].as_u64(), v["tokens_out"].as_u64(), v["tokens"].as_u64(), v["wait_ms"].as_u64()), (Some(100), Some(50), Some(150), Some(250)));
        assert_eq!(v["tok_per_s"].as_f64(), Some(50.0));
        let none = speed_fields("t".into(), 10, 0, 0, 0, 0, 0);
        assert!(none["tokens_in"].is_null() && none["tok_per_s"].is_null());
        // total only (no split reported): rate falls back to the total
        assert_eq!(speed_fields("t".into(), 10, 0, 0, 40, 1000, 0)["tok_per_s"].as_f64(), Some(40.0));
        let u = crate::gizzi_completion::usage_from_info(&json!({ "tokens": { "input": 7, "output": 3, "reasoning": 2 }, "cost": 0.5 }));
        assert_eq!((u.tokens, u.tokens_in, u.tokens_out), (12, 7, 5));
    }

    #[test]
    fn attention_wait_is_summed_from_resolved_requests() {
        let a = json!([{ "created_at": "2026-09-30T10:00:00.000Z", "resolution": { "resolved_at": "2026-09-30T10:00:02.500Z" } },
                       { "created_at": "2026-09-30T10:00:00.000Z", "resolution": null }]);
        assert_eq!(attention_wait_ms(a.as_array().unwrap()), 2500);
    }

    #[test]
    fn agency_s0_error_code_known_and_unknown() {
        assert_eq!(s0_error_code("SyntaxError: bad"), "SYNTAX_ERROR");
        assert_eq!(s0_error_code("something odd"), "UNKNOWN");
    }
}
