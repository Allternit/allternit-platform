//! Agency request compiler (WP11).
//!
//! `AgencyRequest` (only `goal` required, Q9) → resolved defaults + TaskIR for
//! one run template. Defaults come from the versioned agent bundle manifest
//! (`bundles/allternit-code.v1.json`, CL-147) and the authority-profile /
//! completion-criteria registries in `catalog`. Nothing permissive is implied:
//! anything that can't be resolved safely is `ERR_DEFAULTS_UNRESOLVABLE`.
//!
//! Invariants enforced here (not by the caller):
//! * judge fail-closed + verifier-owned completion, `origin: agency` (Q18);
//! * the emitted WIH policy always has `requires_lease_for_write: true`
//!   (08 residual gap) — even if a template asks for `false`.
//!
//! Templates are looked up through [`TemplateRegistry`]. BUG_FIX is the
//! kernel's WP10 graph (`commrails::kernel::bug_fix::agency_graph`); it fails
//! closed without a declared workspace/write set, which maps to a 422.

use super::catalog;
use allternit_commrails::judge::policy::{CloseBy, Fence, JudgePolicy, PolicyOrigin, VerifyMode};
use serde_json::{json, Map, Value};
use std::sync::Arc;

/// Typed compiler failure mapped to the API error envelope by the router.
#[derive(Debug, Clone, PartialEq)]
pub struct CompileError {
    pub status: u16,
    pub family: &'static str,
    pub code: &'static str,
    pub message: String,
    pub param: Option<String>,
}

impl CompileError {
    fn new(status: u16, family: &'static str, code: &'static str, msg: impl Into<String>, param: Option<&str>) -> Self {
        Self { status, family, code, message: msg.into(), param: param.map(str::to_string) }
    }
    fn unsupported(param: &str, msg: impl Into<String>) -> Self {
        Self::new(400, "INPUT", "ERR_INPUT_UNSUPPORTED", msg, Some(param))
    }
    fn unresolvable(param: &str, msg: impl Into<String>) -> Self {
        Self::new(422, "INPUT", "ERR_DEFAULTS_UNRESOLVABLE", msg, Some(param))
    }
}

/// Graph produced by a template for one goal.
#[derive(Debug, Clone)]
pub struct TemplateGraph {
    pub nodes: Vec<Value>,
    pub edges: Vec<Value>,
    /// WIH policy the template proposes. The compiler overrides
    /// `requires_lease_for_write` to `true` whatever this says.
    pub wih_policy: Value,
}

/// A run template (BUG_FIX today). WP10 provides the real BUG_FIX graph.
pub trait RunTemplate: Send + Sync {
    fn id(&self) -> &'static str;
    fn version(&self) -> u32;
    /// `stub` until the real template is registered, then e.g. `kernel`.
    fn source(&self) -> &'static str;
    fn completion_policy(&self) -> &'static str;
    /// Fails (→ 422) when the template can't be instantiated safely, e.g. no
    /// declared workspace or write set.
    fn instantiate(&self, goal: &str, params: &Value) -> Result<TemplateGraph, String>;
}

/// The kernel BUG_FIX graph template (WP10).
pub struct KernelBugFixTemplate;

impl RunTemplate for KernelBugFixTemplate {
    fn id(&self) -> &'static str { "BUG_FIX" }
    fn version(&self) -> u32 { 1 }
    fn source(&self) -> &'static str { "kernel" }
    fn completion_policy(&self) -> &'static str { "completion.bug_fix" }
    fn instantiate(&self, goal: &str, params: &Value) -> Result<TemplateGraph, String> {
        let g = allternit_commrails::kernel::bug_fix::agency_graph(goal, params).map_err(|e| e.to_string())?;
        Ok(TemplateGraph { nodes: g.nodes, edges: g.edges, wih_policy: g.wih_policy })
    }
}

/// Template lookup by task type. `with_template` lets WP10 swap the stub out.
#[derive(Clone)]
pub struct TemplateRegistry {
    bug_fix: Arc<dyn RunTemplate>,
    /// WP-X2: task types this server runs (`ALLTERNIT_AGENCY_TASK_TYPES`).
    enabled: Vec<String>,
}

impl Default for TemplateRegistry {
    fn default() -> Self { Self { bug_fix: Arc::new(KernelBugFixTemplate), enabled: super::task_types::enabled_from_env() } }
}

impl TemplateRegistry {
    pub fn with_bug_fix(t: Arc<dyn RunTemplate>) -> Self { Self { bug_fix: t, ..Self::default() } }
    /// WP-X2: same templates, explicit enable list (tests; env is the prod path).
    pub fn with_enabled(mut self, ids: &[&str]) -> Self { self.enabled = ids.iter().map(|s| s.to_string()).collect(); self }
    pub fn is_enabled(&self, id: &str) -> bool { self.enabled.iter().any(|e| e == id) }
    pub fn get(&self, id: &str) -> Option<Arc<dyn RunTemplate>> {
        if id == "BUG_FIX" {
            return Some(self.bug_fix.clone());
        }
        // WP-X1 hook: kernel task types beyond BUG_FIX.
        super::task_types::template(id)
    }
}

/// Output of a successful compile.
#[derive(Debug, Clone)]
pub struct Compiled {
    pub agent: String,
    pub goal: String,
    /// `Run.resolved` (ResolvedDefaults) minus `resolved_at` (stamped by the store).
    pub resolved: Value,
    pub budget: Value,
    pub task_ir: Value,
    pub judge_policy: JudgePolicy,
    pub metadata: Value,
    pub thread_id: Option<String>,
}

const ALLOWED_FIELDS: &[&str] = &[
    "agent", "goal", "workspace", "context", "authority", "budget", "completion", "capabilities",
    "models", "graph", "runtime", "stream", "thread_id", "metadata", "task_type",
];

fn is_extension(k: &str) -> bool {
    k.len() > 2 && k.starts_with("x-") && k[2..].chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Rejects unknown top-level fields (incl. any attempt to switch off the
/// judge or verifier-owned completion) and validates `goal`.
fn validate_shape(req: &Value) -> Result<&Map<String, Value>, CompileError> {
    let obj = req.as_object().ok_or_else(|| CompileError::unsupported("body", "request body must be a JSON object"))?;
    for k in obj.keys() {
        if !ALLOWED_FIELDS.contains(&k.as_str()) && !is_extension(k) {
            return Err(CompileError::unsupported(k, format!("unknown field `{k}`")));
        }
    }
    match obj.get("goal").and_then(Value::as_str) {
        Some(g) if !g.trim().is_empty() && g.len() <= 20_000 => Ok(obj),
        Some(_) => Err(CompileError::new(400, "INPUT", "ERR_INPUT_INVALID", "goal must be 1..20000 chars", Some("goal"))),
        None => Err(CompileError::new(400, "INPUT", "ERR_INPUT_INVALID", "goal is required", Some("goal"))),
    }
}

fn resolve_agent(obj: &Map<String, Value>) -> Result<(Value, String), CompileError> {
    let raw = obj.get("agent").and_then(Value::as_str).unwrap_or("allternit-code");
    let (id, ver) = raw.split_once('@').unwrap_or((raw, "v1"));
    let manifest = catalog::bundle_manifest();
    if id != manifest["agent_id"] || ver != manifest["version"] {
        return Err(CompileError::unresolvable("agent", format!("unknown agent `{raw}`")));
    }
    Ok((manifest, format!("{id}@{ver}")))
}

fn resolve_budget(obj: &Map<String, Value>, m: &Value) -> Result<Value, CompileError> {
    let defaults = m["defaults"]["budget"].clone();
    let Some(req) = obj.get("budget") else { return Ok(defaults) };
    let r = req.as_object().ok_or_else(|| CompileError::unsupported("budget", "budget must be an object"))?;
    let mut out = defaults.as_object().cloned().unwrap_or_default();
    for (k, v) in r {
        let limit = m["hard_limits"].get(k).and_then(Value::as_f64);
        match k.as_str() {
            "max_seconds" | "max_cost_usd" | "max_steps" | "max_attention_requests" => {
                let n = v.as_f64().filter(|n| *n >= 0.0).ok_or_else(|| CompileError::unsupported(k, "must be a non-negative number"))?;
                if limit.is_some_and(|l| n > l) {
                    return Err(CompileError::new(422, "POLICY", "ERR_POLICY_DENIED", format!("budget.{k} exceeds the hard limit"), Some(k)));
                }
            }
            "on_exhaustion" if matches!(v.as_str(), Some("request_attention" | "stop")) => {}
            _ => return Err(CompileError::unsupported(k, format!("unsupported budget field `{k}`"))),
        }
        out.insert(k.clone(), v.clone());
    }
    Ok(Value::Object(out))
}

fn resolve_authority(obj: &Map<String, Value>, m: &Value) -> Result<Value, CompileError> {
    let default_id = m["defaults"]["authority_profile"]["id"].as_str().unwrap_or("code-safe");
    let id = match obj.get("authority") {
        None => default_id.to_string(),
        Some(a) => {
            let a = a.as_object().ok_or_else(|| CompileError::unsupported("authority", "authority must be an object"))?;
            if let Some(k) = a.keys().find(|k| *k != "profile") {
                // Explicit grants are Level B; not in the alpha.
                return Err(CompileError::unsupported(k, "only authority.profile is supported in the alpha"));
            }
            a.get("profile").and_then(Value::as_str).unwrap_or(default_id).to_string()
        }
    };
    let allowed = m["authority_profiles"].as_array().is_some_and(|p| p.iter().any(|x| x == id.as_str()));
    let profile = catalog::authority_profile(&id).filter(|_| allowed).ok_or_else(|| {
        CompileError::new(403, "POLICY", "ERR_AUTHORITY_PROFILE_FORBIDDEN", format!("authority profile `{id}` is not available to this agent"), Some("authority.profile"))
    })?;
    Ok(json!({
        "profile": { "id": profile["id"], "version": profile["version"] },
        "grants_summary": profile["grants_summary"],
        "interaction_mode": profile["interaction_mode"],
        "expires_at": null
    }))
}

fn resolve_completion(obj: &Map<String, Value>, m: &Value, template_cid: &str) -> Result<Value, CompileError> {
    let default_cid = m["defaults"]["completion_contract"]["id"].as_str().unwrap_or("completion.bug_fix");
    // WP-X2: a non-default task type always uses its own contract.
    let cid = if template_cid == "completion.bug_fix" { default_cid } else { template_cid };
    let contract = catalog::completion_contract(cid).ok_or_else(|| CompileError::unresolvable("completion", "no completion contract"))?;
    let mut require: Vec<Value> = contract["require"].as_array().cloned().unwrap_or_default();
    if let Some(c) = obj.get("completion") {
        let c = c.as_object().ok_or_else(|| CompileError::unsupported("completion", "completion must be an object"))?;
        for (k, v) in c {
            match k.as_str() {
                // Callers may add criteria; the contract's blocking ones always stay (no weakening).
                "require" => {
                    for crit in v.as_array().ok_or_else(|| CompileError::unsupported("completion.require", "must be an array"))? {
                        let id = crit.as_str().or_else(|| crit.get("id").and_then(Value::as_str)).unwrap_or_default();
                        let known = catalog::criterion(id).ok_or_else(|| {
                            CompileError::unresolvable("completion.require", format!("unknown completion criterion `{id}`"))
                        })?;
                        let r = json!({ "id": known["id"], "version": known["version"] });
                        if !require.contains(&r) {
                            require.push(r);
                        }
                    }
                }
                "allow_partial" if v == &Value::Bool(false) => {}
                "allow_partial" => return Err(CompileError::new(422, "POLICY", "ERR_POLICY_DENIED", "this contract does not allow partial completion", Some("completion.allow_partial"))),
                _ => return Err(CompileError::unsupported(k, format!("unsupported completion field `{k}`"))),
            }
        }
    }
    Ok(json!({ "contract_id": contract["id"], "contract_version": contract["version"], "require": require, "allow_partial": false }))
}

fn resolve_workspace(obj: &Map<String, Value>, m: &Value) -> Result<Value, CompileError> {
    match obj.get("workspace") {
        None => Ok(m["defaults"]["workspace"].clone()),
        Some(w) => {
            let o = w.as_object().filter(|o| !o.is_empty()).ok_or_else(|| CompileError::unsupported("workspace", "workspace must be a non-empty object"))?;
            if let Some(k) = o.keys().find(|k| !matches!(k.as_str(), "repo" | "ref" | "resources")) {
                return Err(CompileError::unsupported(k, format!("unknown workspace field `{k}`")));
            }
            Ok(w.clone())
        }
    }
}

/// Q25: Agency API (hosted, customer) runs always use the strict fence
/// profile: commrails `JudgePolicy.fence = strict` (#1045), which makes the
/// gate deny unresolved write effects; the executor adds the disposable
/// workspace, env allowlist and egress check.
pub const FENCE: &str = "strict";

/// The TaskIR's WIH policy. Always requires a lease for writes and always
/// runs under the strict fence; this is the only place the compiler emits a
/// WIH policy.
pub fn enforce_wih_policy(proposed: &Value) -> Value {
    let mut p = proposed.as_object().cloned().unwrap_or_default();
    p.insert("requires_lease_for_write".into(), Value::Bool(true));
    p.insert("fence".into(), Value::String(FENCE.into()));
    Value::Object(p)
}

/// The run's `JudgePolicySet` payload (carries `fence: strict`).
pub fn judge_policy_json(p: &JudgePolicy) -> Value {
    serde_json::to_value(p).unwrap_or_else(|_| json!({}))
}

const VENDOR_WORDS: &[&str] = &["openai", "anthropic", "claude", "codex", "gpt", "gemini", "llama", "mistral",
    "deepseek", "qwen", "sonnet", "opus", "haiku", "kimi", "grok"];

/// `models` (ModelConstraints, Level B): logical model classes and locality
/// only, passed to the router as a preference. Never a model or vendor name.
fn resolve_models(obj: &Map<String, Value>) -> Result<Value, CompileError> {
    let Some(m) = obj.get("models") else { return Ok(Value::Null) };
    let m = m.as_object().ok_or_else(|| CompileError::unsupported("models", "models must be an object"))?;
    for (k, v) in m {
        match k.as_str() {
            "allow_classes" | "deny_classes" => {
                let arr = v.as_array().ok_or_else(|| CompileError::unsupported(k, "must be an array of model classes"))?;
                for c in arr {
                    let c = c.as_str().unwrap_or_default();
                    let shape = c.len() > 3 && c.starts_with("mc.")
                        && c[3..].chars().all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '_' | '.' | '-'));
                    if !shape || VENDOR_WORDS.iter().any(|w| c.contains(w)) {
                        return Err(CompileError::new(400, "INPUT", "ERR_INPUT_INVALID", "model classes are logical `mc.*` ids; model or vendor names are rejected", Some(k)));
                    }
                    if !(c == "mc.local" || c == "mc.remote" || c.starts_with("mc.local.") || c.starts_with("mc.remote.")) {
                        return Err(CompileError::unresolvable(k, format!("unknown model class `{c}`")));
                    }
                }
            }
            "locality" if matches!(v.as_str(), Some("local_only" | "local_preferred" | "any")) => {}
            "max_data_trust_class_to_remote" if v.is_string() => {}
            _ => return Err(CompileError::unsupported(k, format!("unsupported models field `{k}`"))),
        }
    }
    Ok(Value::Object(m.clone()))
}

/// Compile a raw request body. `run_id` names the DAG (`agency-<run_id>`).
pub fn compile(req: &Value, run_id: &str, templates: &TemplateRegistry) -> Result<Compiled, CompileError> {
    let obj = validate_shape(req)?;
    for lvl in ["capabilities", "graph", "runtime"] {
        if obj.contains_key(lvl) {
            return Err(CompileError::unsupported(lvl, format!("`{lvl}` (advanced tier) is not available in the alpha")));
        }
    }
    let goal = obj["goal"].as_str().unwrap_or_default().to_string();
    let (manifest, agent) = resolve_agent(obj)?;
    let workspace = resolve_workspace(obj, &manifest)?;
    let authority = resolve_authority(obj, &manifest)?;
    let budget = resolve_budget(obj, &manifest)?;
    // WP-X2: explicit `task_type`, validated against the catalog and the
    // server's enable list; default stays the agent's template (BUG_FIX).
    let tname = match obj.get("task_type") {
        None => manifest["default_template"].as_str().unwrap_or("BUG_FIX").to_string(),
        Some(v) => v.as_str().filter(|s| !s.is_empty()).map(str::to_string)
            .ok_or_else(|| CompileError::unsupported("task_type", "task_type must be a string"))?,
    };
    let template = templates.get(&tname).ok_or_else(|| CompileError::unresolvable("task_type", format!("unknown task type `{tname}`")))?;
    if !templates.is_enabled(&tname) {
        return Err(CompileError::new(422, "POLICY", "ERR_POLICY_DENIED", format!("task type `{tname}` is not enabled on this server"), Some("task_type")));
    }
    let completion = resolve_completion(obj, &manifest, template.completion_policy())?;
    let models = resolve_models(obj)?;

    let omitted: Vec<&str> = ["workspace", "authority", "budget", "completion"].into_iter().filter(|k| !obj.contains_key(*k)).collect();
    let defaults_source = if omitted.is_empty() {
        "request".to_string()
    } else {
        format!("agent:{agent}/profile:{}@{}", authority["profile"]["id"].as_str().unwrap_or(""), authority["profile"]["version"])
    };

    // The template takes the workspace as one locator string (repo, else ref);
    // task types beyond BUG_FIX take the declared writable resources.
    let ws_locator = workspace.get("repo").or_else(|| workspace.get("ref")).and_then(Value::as_str).unwrap_or_default();
    let mut params = json!({ "workspace": ws_locator, "task_id": format!("task.{}.{run_id}", tname.to_ascii_lowercase()) });
    if tname != "BUG_FIX" {
        params["writable_resources"] = obj.get("workspace").and_then(|w| w.get("resources")).cloned().unwrap_or(json!([]));
    }
    let graph = template
        .instantiate(&goal, &params)
        .map_err(|e| CompileError::new(422, "INPUT", "ERR_INPUT_INVALID", format!("cannot instantiate {}: {e}", template.id()), Some("workspace")))?;
    let dag_id = format!("agency-{run_id}");
    let judge_policy = JudgePolicy {
        verify: Some(VerifyMode::Judge),
        close_by: Some(CloseBy::Verifier),
        tool_judge: Some(true),
        max_continuations: None,
        origin: Some(PolicyOrigin::Agency),
        completion_policy: Some(template.completion_policy().to_string()),
        // Q25: hosted / Agency API runs always use the strict fence (#1045).
        fence: Some(Fence::Strict),
        allow_credential_read: None,
        ..Default::default()
    };
    let task_ir = json!({
        "schema_id": "allternit.agency.TaskIR",
        "schema_version": "0.1.0",
        "dag_id": dag_id,
        "task_type": template.id(),
        "template": { "id": template.id(), "version": template.version(), "source": template.source() },
        "goal": goal,
        "workspace": workspace,
        "authority_profile": authority["profile"],
        "budget": budget,
        "completion": completion,
        "nodes": graph.nodes,
        "edges": graph.edges,
        "wih_policy": enforce_wih_policy(&graph.wih_policy),
        "judge_policy": judge_policy_json(&judge_policy),
        "models": models,
    });
    Ok(Compiled {
        agent,
        goal,
        resolved: json!({
            "workspace": workspace,
            "authority": authority,
            "budget": budget,
            "completion": completion,
            "defaults_source": defaults_source,
            "defaults_version": catalog::DEFAULTS_VERSION,
            "enforcement": { "judge_fail_closed": true, "verifier_owned_completion": true, "fence": FENCE },
            "models": models,
        }),
        budget,
        task_ir,
        judge_policy,
        metadata: obj.get("metadata").cloned().unwrap_or_else(|| json!({})),
        thread_id: obj.get("thread_id").and_then(Value::as_str).map(str::to_string),
    })
}
