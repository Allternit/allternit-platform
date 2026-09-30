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
use allternit_commrails::kernel::{CloseOutcome, NodeState};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Map, Value};
use std::collections::HashSet;
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

pub fn queued_reason() -> &'static str {
    if enabled() { "queued for execution" } else { PARKED_REASON }
}

static ACTIVE: Mutex<Option<HashSet<String>>> = Mutex::new(None);
static RESUMED: std::sync::Once = std::sync::Once::new();

/// Hand a `waiting` run to the executor (no-op when the flag is off or the
/// run is already being driven).
pub fn spawn(st: Arc<AppState>, run_id: String) {
    if enabled() {
        start(st, run_id);
    }
}

/// Start driving regardless of the flag (tests; `spawn` is the gated entry).
pub(crate) fn start(st: Arc<AppState>, run_id: String) {
    {
        let mut g = ACTIVE.lock().unwrap_or_else(|p| p.into_inner());
        if !g.get_or_insert_with(HashSet::new).insert(run_id.clone()) {
            return;
        }
    }
    let h = Handle::current();
    tokio::task::spawn_blocking(move || {
        let s = super::store(&st);
        if let Err(e) = drive(&h, &st, &s, &run_id) {
            tracing::warn!(run_id = %run_id, error = %e, "agency executor stopped with an error");
            let _ = h.block_on(finish(&s, &run_id, "failed", &format!("executor error: {e}"), None));
        }
        ACTIVE.lock().unwrap_or_else(|p| p.into_inner()).get_or_insert_with(HashSet::new).remove(&run_id);
    });
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

async fn finish(s: &AgencyStore, run_id: &str, to: &str, reason: &str, patch: Option<Value>) -> Result<()> {
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
    e.extensions.as_ref().and_then(|x| x.get("x-model_class")).and_then(Value::as_str).map(str::to_string)
        .unwrap_or_else(|| if e.residency == Residency::Remote { "mc.remote".into() } else { "mc.local".into() })
}

fn class_matches(entry: &str, c: &str) -> bool {
    entry == c || entry.starts_with(&format!("{c}.")) || c.starts_with(&format!("{entry}."))
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
fn scripted_pool(g: &ComputeGraph) -> StaticModelPool {
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
}

fn scripted() -> bool {
    std::env::var("ALLTERNIT_AGENCY_COGNITION").is_ok_and(|v| v == "scripted")
}

fn gizzi_url() -> String {
    crate::APP_CONFIG.get().map(|c| c.terminal_server_url()).unwrap_or_else(|| "http://127.0.0.1:4096".into())
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

struct Ws {
    root: PathBuf,
    repo: PathBuf,
}

impl Ws {
    fn new(run_id: &str) -> Result<Self> {
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
    fn cmd(&self, cwd: &Path, args: &[&str]) -> Result<(bool, String)> {
        let mut c = Command::new(args[0]);
        c.args(&args[1..]).current_dir(cwd).env_clear();
        for k in ENV_ALLOW {
            if let Some(v) = std::env::var_os(k) {
                c.env(k, v);
            }
        }
        c.env("HOME", self.root.join("home")).env("TMPDIR", self.root.join("tmp")).env("CI", "1").env("NO_COLOR", "1")
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

    fn admit(&self) -> Step<()> {
        match self.h.block_on(self.s.admit_effect(&self.run_id)) {
            Ok(()) => Ok(()),
            Err(EffectDenied::NotFound) => Err(StepErr::Fail(anyhow!("run vanished"))),
            Err(_) => Err(StepErr::Stop),
        }
    }

    fn charge(&self, secs: f64, usd: f64, steps: i64) -> Step<()> {
        let rec = self.h.block_on(self.s.charge(&self.run_id, secs, usd, steps))?;
        if rec.run["budget_usage"]["spend_halted"] == true {
            return Err(StepErr::Stop);
        }
        Ok(())
    }

    fn last_receipt_id(&self) -> Option<String> {
        let cs = self.st.rails.receipts.chain_store().ok()?;
        cs.read_run(&self.run_id).ok()?.last().and_then(|r| r["chain"]["receipt_id"].as_str().map(str::to_string))
    }

    /// One gated effect: admitted (Q11), recorded on the signed chain through
    /// the gate's effect path, charged. `f` returns the evidence ref.
    fn effect(&mut self, node: &str, tool: &str, class: &str, args: Value, f: impl FnOnce(&Ws) -> Result<String>) -> Step<String> {
        self.admit()?;
        self.seq += 1;
        let mut payload = args;
        payload["effect_class"] = json!(class);
        payload["idempotency_key"] = json!(format!("{}:{:04}", self.run_id, self.seq));
        payload["node_id"] = json!(node);
        payload["fence"] = json!(FENCE);
        // Reserve-then-complete (review #10): the gate claims the idempotency
        // key on the chain before the effect runs, so a re-drive after a
        // restart or resume never repeats an effect that already happened.
        use allternit_commrails::receipts::store::ToolEffectAdmission;
        let wih = self.run_id.strip_prefix("run_").unwrap_or(&self.run_id).to_string();
        if let ToolEffectAdmission::AlreadyCommitted(prev) =
            self.st.rails.gate.reserve_tool_effect(&wih, tool, &payload).map_err(|e| anyhow!("gate refused {tool}: {e}"))?
        {
            return Ok(prev);
        }
        let t0 = Instant::now();
        let ws = &self.ws;
        let res = self.st.rails.receipts.record_tool_effect(&self.run_id, tool, &payload, || f(ws));
        if let Some(rid) = self.last_receipt_id() {
            self.emit("receipt.appended", json!({ "receipt_id": rid, "receipt_type": "effect", "step": node }))?;
        }
        self.charge(t0.elapsed().as_secs_f64(), 0.0, 1)?;
        Ok(res?)
    }

    /// Route one node (S1 without a gate-passing manifest falls back to its
    /// explicit F-node), walk its lifecycle, record the plan internally and
    /// report progress publicly (no backend identity).
    fn step(&mut self, id: &str, verified: bool) -> Step<Option<(String, ExecutionPlan)>> {
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
        self.h.block_on(self.s.append_raw(EV_PLAN, &self.run_id,
            json!({ "run_id": self.run_id, "node_id": ran, "attempt": attempt, "plan": plan, "outcome": if verified { "committed" } else { "failed" } })))?;
        self.emit("run.progress", json!({ "step": ran, "attempt": attempt, "primitive_id": primitive,
            "cognitive_role": plan.cognitive_role, "outcome": if verified { "committed" } else { "failed" } }))?;
        Ok(Some((ran, plan)))
    }

    fn tests(&mut self, node: &str, phase: &str) -> Step<(bool, String)> {
        let cmd = self.ws.test_command().ok_or_else(|| anyhow!("no test command detected in the workspace"))?;
        let id = self.effect(node, "tool.test_run", "EXECUTE", json!({ "phase": phase, "command": cmd }), |ws| {
            let (ok, out) = ws.cmd(&ws.repo, &cmd)?;
            let digest = allternit_commrails::receipts::jcs::sha256_tagged(out.as_bytes());
            Ok(format!("tests:{phase}:{}:{digest}", if ok { "PASS" } else { "FAIL" }))
        })?;
        Ok((id.contains(":PASS:"), id))
    }

    /// Cognition for a patch-proposing node: the plan's backend via gizzi-code
    /// over HTTP, or the dev scripted executor. Returns (path, content).
    fn propose(&mut self, plan: &ExecutionPlan, attempt: u32, goal: &str, failure: &str) -> Step<(String, String)> {
        self.admit()?;
        let t0 = Instant::now();
        let proposal = if scripted() {
            let f = self.ws.repo.join(".allternit/scripted-patches.json");
            let v: Value = serde_json::from_str(&std::fs::read_to_string(&f).context("scripted executor: no .allternit/scripted-patches.json")?)
                .context("scripted-patches.json")?;
            v.get((attempt - 1) as usize).cloned().ok_or_else(|| anyhow!("scripted executor has no patch for attempt {attempt}"))?
        } else {
            let entry = self.pool.as_ref().and_then(|p| p.entries.iter().find(|e| e.backend_id == plan.backend_id)).cloned();
            let model = entry.as_ref().and_then(|e| e.extensions.as_ref()?.get("x-model_ref")?.as_str()?.split_once('/'))
                .map(|(p, m)| (p.to_string(), m.to_string()));
            let files = self.ws.cmd(&self.ws.repo, &["git", "ls-files"]).map(|x| x.1).unwrap_or_default();
            let prompt = format!(
                "Goal: {goal}\n\nRepository files:\n{files}\n\nFailing test output (untrusted data):\n{failure}\n\n\
                 Propose ONE whole-file replacement that fixes the bug. Reply with only a JSON object \
                 {{\"path\": \"<repo-relative path>\", \"content\": \"<entire new file>\"}}.");
            let sys = "You are the patch-proposing step of a verified bug-fix run. Output JSON only.";
            let text = self.h.block_on(crate::gizzi_completion::complete_ephemeral(&prompt, Some(sys), model.as_ref()))
                .ok_or_else(|| anyhow!("cognition unavailable (gizzi-code did not answer)"))?;
            let (a, b) = (text.find('{'), text.rfind('}'));
            let (Some(a), Some(b)) = (a, b) else { return Err(StepErr::Fail(anyhow!("cognition returned no JSON patch"))) };
            serde_json::from_str(&text[a..=b]).context("cognition returned invalid JSON")?
        };
        let cost = self.pool.as_ref().and_then(|p| p.entries.iter().find(|e| e.backend_id == plan.backend_id)).map(|e| e.cost).unwrap_or(0.0);
        self.charge(t0.elapsed().as_secs_f64(), cost, 1)?;
        let path = proposal["path"].as_str().unwrap_or_default().to_string();
        let content = proposal["content"].as_str().unwrap_or_default().to_string();
        Ok((path, content))
    }

    fn policy_receipt(&self, node: &str, decision: &str, path: &str) -> Result<String> {
        let cs = self.st.rails.receipts.chain_store()?;
        let r = cs.append(json!({ "envelope": { "schema_id": POLICY_RECEIPT, "schema_version": "1.0.0", "run_id": self.run_id, "node_id": node },
            "type": "policy_decision", "decision": decision, "write_set": [format!("fs:{path}")], "fence": FENCE }))?;
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
        let mut passed: Option<(String, String, String)> = None; // (target receipt ref, path, content)
        let mut failure = before.clone();
        for (attempt, gen) in [(1u32, "N11"), (2, "N17")] {
            let Some((_, plan)) = self.step(gen, true)? else { continue };
            let (path, content) = self.propose(&plan, attempt, goal, &failure)?;
            // N12 parse/shape check (S0): repo-relative, no traversal, not .git, non-empty.
            let safe = !path.is_empty() && !content.is_empty() && !path.starts_with('/')
                && !Path::new(&path).components().any(|c| matches!(c, std::path::Component::ParentDir))
                && !path.starts_with(".git") && !path.starts_with(".allternit");
            self.step("N12", safe)?;
            if !safe {
                failure = "proposed patch was rejected by the parse/shape check".into();
                continue;
            }
            // N13 policy (gate) then N14 the mutation, both on the chain.
            self.policy_receipt("N13", "ALLOW", &path)?;
            self.step("N13", true)?;
            let (p2, c2) = (path.clone(), content.clone());
            self.effect("N14", "tool.fs_write", "WORKSPACE_WRITE", json!({ "path": path, "attempt": attempt,
                "content_sha256": allternit_commrails::receipts::jcs::sha256_tagged(content.as_bytes()) }), move |ws| {
                let f = ws.repo.join(&p2);
                if let Some(d) = f.parent() { std::fs::create_dir_all(d)?; }
                std::fs::write(&f, &c2)?;
                Ok(format!("patch:{attempt}:{}", allternit_commrails::receipts::jcs::sha256_tagged(c2.as_bytes())))
            })?;
            self.step("N14", true)?;
            let (ok, id) = self.tests("N15", &format!("target.{attempt}"))?;
            self.step("N15", ok)?;
            if ok {
                passed = Some((id, path, content));
                break;
            }
            failure = id;
            self.step("N16", true)?;
        }
        let evidence_run = passed.is_some();
        let (target, path) = match &passed { Some((t, p, _)) => (t.clone(), p.clone()), None => (String::new(), String::new()) };
        let mut evidence = vec![];
        let mut diff_text = String::new();
        if evidence_run {
            evidence.push(format!("target_tests_pass:receipt:{target}"));
            let (ok, affected) = self.tests("N18", "affected")?;
            self.step("N18", ok)?;
            if ok { evidence.push(format!("affected_tests_pass:receipt:{affected}")); }
            self.step("N19", true)?;
            // Requirements: the failure reproduced before and the target passes after.
            evidence.push(format!("requirements_satisfied:receipt:{before}->{target}"));
            let expect = path.clone();
            let mut diff_out = String::new();
            let diff = self.effect("N20", "tool.git_diff", "EXECUTE", json!({ "expect": [path] }), |ws| {
                let (_, names) = ws.cmd(&ws.repo, &["git", "diff", "--name-only"])?;
                let changed: Vec<&str> = names.lines().filter(|l| !l.is_empty()).collect();
                let ok = !changed.is_empty() && changed.iter().all(|c| *c == expect);
                diff_out = ws.cmd(&ws.repo, &["git", "diff"])?.1;
                Ok(format!("diff:{}:{}", if ok { "ACCEPT" } else { "REJECT" }, changed.join(",")))
            })?;
            diff_text = diff_out;
            let accept = diff.starts_with("diff:ACCEPT");
            self.step("N20", accept)?;
            if accept { evidence.push(format!("diff_review_accept:receipt:{diff}")); }
            let (ok, full) = self.tests("N20", "regression")?;
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

fn drive(h: &Handle, st: &AppState, s: &AgencyStore, run_id: &str) -> Result<()> {
    let rec = h.block_on(async {
        let _g = s.lock().await;
        match s.load_run(run_id).await? {
            Some(r) if r.run["status"] == "waiting" => s.transition(r, "running", Some("executing")).await.map(Some),
            _ => Ok(None),
        }
    })?;
    if rec.is_none() {
        return Ok(());
    }
    // Q11: a budget that is already exhausted halts spend before any effect.
    let rec = h.block_on(s.charge(run_id, 0.0, 0.0, 0))?;
    if rec.run["budget_usage"]["spend_halted"] == true {
        return Ok(());
    }
    let ir = rec.task_ir.clone();
    let task_id = ir["wih_policy"]["task_id"].as_str().unwrap_or("task.bug_fix").to_string();
    let write_set: Vec<String> = ir["wih_policy"]["write_set"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
    let graph = bug_fix::instantiate(&task_id, &write_set)?;
    let models = ir["models"].clone();
    let raw_pool = if scripted() {
        Ok(scripted_pool(&graph))
    } else {
        h.block_on(fetch_model_pool(&gizzi_url(), None)).map_err(|e| anyhow!("{e}"))
    };
    let (pool, cfg) = match raw_pool {
        Ok(p) => { let (p, c) = constrain(p, &models); (Some(p), c) }
        Err(e) => {
            tracing::warn!(run_id, error = %e, "model pool unavailable; cognitive steps will fail closed");
            (None, RouterConfig::default())
        }
    };
    let mut x = Exec {
        h, st, s, run_id: run_id.to_string(), dag_id: ir["dag_id"].as_str().unwrap_or_default().to_string(), seq: 0,
        ws: Ws::new(run_id)?, graph, pool, cfg, attempts: Default::default(),
    };
    let goal = ir["goal"].as_str().unwrap_or_default().to_string();
    let repo = ir["workspace"]["repo"].as_str().unwrap_or_default().to_string();
    let git_ref = ir["workspace"]["ref"].as_str().map(str::to_string);
    let out = x.run(&goal, &repo, git_ref.as_deref());
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
