//! Cognitive Execution Router v1.1 (WP7): graph node → `ExecutionPlanV1`.
//!
//! Authority: Living Architecture §39/§44 (L3440–L3475), 02 §10–§11, ABI 1.0.0
//! `capability.schema.json` (`ExecutionPlanV1`, `ModelPoolEntryV1`).
//!
//! Algorithm (L3440): resolve role → enumerate legal modes → policy / trust /
//! locality filters (always BEFORE scoring, L1747) → budget (fail closed) →
//! score eligible backends → emit the plan with a fallback chain.
//!
//! Hard rules enforced here:
//! - Plans carry capability / role / mode / backend ids only. Concrete model
//!   identity is execution *evidence*, never plan semantics (L3591); vendor or
//!   model names never enter a plan (asserted in tests).
//! - S1 (System-1) is refused unless the primitive's S1 ModelPool entry lists
//!   a calibration manifest that passed the Q22 gate; S1 is SHADOW by default, mirroring the
//!   WP8 decision runtime (`tools/system-one-local`, `POST /v1/decision`).
//! - An exhausted budget fails closed: no plan, not even M0.
//! - The router is pure: no I/O. The ModelPool is injected; `fetch_model_pool`
//!   is the only network path and it is never used by unit tests.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::graph::GraphNode;

pub const PLAN_SCHEMA_ID: &str = "allternit.kernel.ExecutionPlanV1";
pub const POOL_ENTRY_SCHEMA_ID: &str = "allternit.kernel.ModelPoolEntryV1";
pub const SCHEMA_VERSION: &str = "1.0.0";
/// S0 nodes execute through the PrimitiveRegistry, not the ModelPool (L3586).
pub const S0_BACKEND_ID: &str = "backend.s0.primitive_registry";

// ---------------------------------------------------------------- enums

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Role {
    S0,
    S1,
    S2,
    S3,
}

impl Role {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "S0" => Some(Self::S0),
            "S1" => Some(Self::S1),
            "S2" => Some(Self::S2),
            "S3" => Some(Self::S3),
            _ => None,
        }
    }
}

/// M0–M6 (L3424–L3439). Declared cheapest-correct-first, so `Ord` is the
/// "pick the cheapest correct inference form" order (L3163).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Mode {
    #[serde(rename = "M0.DETERMINISTIC")]
    M0Deterministic,
    #[serde(rename = "M1.LOGIT_READOUT")]
    M1LogitReadout,
    #[serde(rename = "M2.CALIBRATED_READOUT")]
    M2CalibratedReadout,
    #[serde(rename = "M3.HIDDEN_HEAD")]
    M3HiddenHead,
    #[serde(rename = "M4.DEDICATED_DECIDER")]
    M4DedicatedDecider,
    #[serde(rename = "M5.GENERATIVE")]
    M5Generative,
    #[serde(rename = "M6.DEEP_SOLVER")]
    M6DeepSolver,
}

impl Mode {
    fn parse(s: &str) -> Option<Self> {
        serde_json::from_value(Value::String(s.to_string())).ok()
    }

    /// Legal modes per role. M1 (uncalibrated logit readout) is never legal
    /// for an authoritative S1 route: that is exactly "uncalibrated S1".
    pub fn legal_for(role: Role) -> &'static [Mode] {
        match role {
            Role::S0 => &[Mode::M0Deterministic],
            Role::S1 => &[Mode::M2CalibratedReadout, Mode::M3HiddenHead, Mode::M4DedicatedDecider],
            Role::S2 => &[Mode::M5Generative],
            Role::S3 => &[Mode::M6DeepSolver],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Residency {
    Pinned,
    Hot,
    Warm,
    Cold,
    Remote,
}

impl Residency {
    /// Residency is an accelerator only (02 §11): a small tie-break penalty.
    fn penalty(self) -> f64 {
        match self {
            Residency::Pinned | Residency::Hot => 0.0,
            Residency::Warm => 0.02,
            Residency::Remote => 0.05,
            Residency::Cold => 0.1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum TrustClass {
    Public,
    Internal,
    Restricted,
    Secret,
    Untrusted,
}

impl TrustClass {
    fn as_str(self) -> &'static str {
        match self {
            TrustClass::Public => "PUBLIC",
            TrustClass::Internal => "INTERNAL",
            TrustClass::Restricted => "RESTRICTED",
            TrustClass::Secret => "SECRET",
            TrustClass::Untrusted => "UNTRUSTED",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CalibrationLevel {
    #[serde(rename = "RAW")]
    Raw,
    L0,
    L1,
    L2,
}

// ---------------------------------------------------------------- contracts

/// `ModelPoolEntryV1` (ABI 1.0.0). Closed: unknown fields fail closed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoolEntry {
    pub schema_id: String,
    pub schema_version: String,
    pub backend_id: String,
    pub cognitive_roles: Vec<Role>,
    pub modes: Vec<Mode>,
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trust_tags: Vec<String>,
    pub confidence_estimate: f64,
    pub latency_ms: f64,
    pub cost: f64,
    pub residency: Residency,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backbone_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantization: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer_stop: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readout_head_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calibration_manifest_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_mb: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_latency_ms: Option<f64>,
    /// Registry data (e.g. `x-model_ref`) lives here and never reaches a plan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Map<String, Value>>,
}

/// `ExecutionPlanV1` (ABI 1.0.0). Only ids — no model identity fields are
/// populated by the router (backbone/revision/runtime/quantization are trace
/// evidence recorded at execution time).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionPlan {
    pub schema_id: String,
    pub schema_version: String,
    pub plan_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    pub cognitive_role: Role,
    pub capability_id: String,
    pub execution_mode: Mode,
    pub backend_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer_stop: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readout_head_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calibration_manifest_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calibration_level: Option<CalibrationLevel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_projection_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub residency_requirement: Option<Residency>,
    pub confidence_floor: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_budget_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_budget: Option<f64>,
    pub fallback_chain: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Map<String, Value>>,
}

// ---------------------------------------------------------------- inputs

/// Anything that can list ModelPool entries. gizzi-code owns the pool and
/// serves it over HTTP (`GET /model-pool`); the router only reads snapshots.
pub trait ModelPool {
    fn entries(&self) -> &[PoolEntry];
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StaticModelPool {
    pub entries: Vec<PoolEntry>,
}

impl ModelPool for StaticModelPool {
    fn entries(&self) -> &[PoolEntry] {
        &self.entries
    }
}

impl StaticModelPool {
    /// Parse a gizzi-code `GET /model-pool` body. Every entry must be a valid
    /// closed `ModelPoolEntryV1`; one bad entry rejects the snapshot.
    pub fn from_http_body(body: &str) -> Result<Self, RouteError> {
        let v: Value = serde_json::from_str(body).map_err(|e| RouteError::PoolUnavailable(e.to_string()))?;
        let entries = v.get("entries").cloned().ok_or_else(|| RouteError::PoolUnavailable("missing entries".into()))?;
        let entries: Vec<PoolEntry> =
            serde_json::from_value(entries).map_err(|e| RouteError::PoolUnavailable(e.to_string()))?;
        if let Some(bad) = entries.iter().find(|e| e.schema_id != POOL_ENTRY_SCHEMA_ID) {
            return Err(RouteError::PoolUnavailable(format!("bad schema_id on {}", bad.backend_id)));
        }
        Ok(Self { entries })
    }
}

/// Fetch a pool snapshot from gizzi-code over HTTP (allternit components never
/// call model providers directly). `capability` narrows server-side.
pub async fn fetch_model_pool(base_url: &str, capability: Option<&str>) -> Result<StaticModelPool, RouteError> {
    let mut url = format!("{}/model-pool", base_url.trim_end_matches('/'));
    if let Some(cap) = capability {
        url.push_str("?capability=");
        url.push_str(cap);
    }
    // Service login for a Clerk-protected gizzi-code: HTTP basic auth from
    // GIZZI_PASSWORD / GIZZI_SERVER_PASSWORD when set (loopback dev: none).
    let mut rq = reqwest::Client::new().get(&url);
    let password = std::env::var("GIZZI_PASSWORD")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var("GIZZI_SERVER_PASSWORD").ok().filter(|v| !v.is_empty()));
    if let Some(pw) = password {
        let user = std::env::var("GIZZI_USERNAME")
            .or_else(|_| std::env::var("GIZZI_SERVER_USERNAME"))
            .unwrap_or_else(|_| "gizzi".to_string());
        rq = rq.basic_auth(user, Some(pw));
    }
    let resp = rq.send().await.map_err(|e| RouteError::PoolUnavailable(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(RouteError::PoolUnavailable(format!("HTTP {}", resp.status())));
    }
    let body = resp.text().await.map_err(|e| RouteError::PoolUnavailable(e.to_string()))?;
    StaticModelPool::from_http_body(&body)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum S1Mode {
    /// Default. S1 answers are recorded, never acted on.
    #[default]
    Shadow,
    Live,
}

/// A calibration manifest bound to a primitive (WP8 `DecisionCalibrationManifestV1`).
#[derive(Debug, Clone, PartialEq)]
pub struct CalibrationBinding {
    pub manifest_id: String,
    pub level: CalibrationLevel,
    /// Q22 gate verdict. `false` = refused exactly like no manifest.
    pub gate_passed: bool,
}

#[derive(Debug, Clone)]
pub struct RouterPolicy {
    pub allow_remote: bool,
    pub denied_backends: HashSet<String>,
    /// Trust classes that must stay off REMOTE residency (locality).
    pub local_only_trust: Vec<TrustClass>,
}

impl Default for RouterPolicy {
    fn default() -> Self {
        Self {
            allow_remote: true,
            denied_backends: HashSet::new(),
            local_only_trust: vec![TrustClass::Restricted, TrustClass::Secret],
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RouterConfig {
    pub s1_mode: S1Mode,
    pub policy: RouterPolicy,
    /// Preferred model classes, best first. Key lookup order: the node's
    /// `<capability>@<role>`, then `role:<S0..S3>`, then `*`. A soft preference: it
    /// orders the eligible candidates (an empty or unmatched list changes
    /// nothing), so the list doubles as an escalation order.
    pub class_preference: HashMap<String, Vec<String>>,
}

/// Logical model class of a pool entry (`x-model_class`, else by residency).
pub fn model_class(e: &PoolEntry) -> String {
    e.extensions.as_ref().and_then(|x| x.get("x-model_class")).and_then(Value::as_str).map(str::to_string)
        .unwrap_or_else(|| if e.residency == Residency::Remote { "mc.remote".into() } else { "mc.local".into() })
}

/// Class match by dotted prefix in either direction (`mc.remote` ~ `mc.remote.fast`).
pub fn class_matches(entry: &str, c: &str) -> bool {
    entry == c || entry.starts_with(&format!("{c}.")) || c.starts_with(&format!("{entry}."))
}

/// Remaining run budget (resolved `AgentStateV1.budgets` minus spend).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BudgetLedger {
    pub remaining_cost_units: f64,
    pub remaining_wall_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RouteError {
    #[error("node {0}: unknown cognitive_role {1}")]
    BadRole(String, String),
    #[error("node {0}: unknown allowed mode {1}")]
    BadMode(String, String),
    #[error("node {0}: no legal mode for role (allowed_modes ∩ legal = ∅)")]
    NoLegalMode(String),
    #[error("node {0}: capability node without a capability id")]
    MissingCapability(String),
    #[error("node {node}: S1 refused — no gate-passing calibration manifest for primitive {primitive}")]
    UncalibratedS1 { node: String, primitive: String },
    #[error("node {node}: no pool backend offers {capability} for the legal modes")]
    NoEligibleBackend { node: String, capability: String },
    #[error("node {node}: every candidate backend rejected by policy: {reasons:?}")]
    PolicyRejected { node: String, reasons: Vec<String> },
    #[error("node {0}: budget exhausted (fail closed)")]
    BudgetExhausted(String),
    #[error("model pool unavailable: {0}")]
    PoolUnavailable(String),
}

// ---------------------------------------------------------------- router

pub struct Router<'a, P: ModelPool> {
    pub pool: &'a P,
    pub config: &'a RouterConfig,
}

struct NodeBudget {
    max_cost: Option<f64>,
    max_wall_ms: Option<u64>,
}

fn node_budget(node: &GraphNode) -> NodeBudget {
    let b = node.budget.as_ref();
    NodeBudget {
        max_cost: b.and_then(|b| b.get("max_cost_units")).and_then(Value::as_f64),
        max_wall_ms: b.and_then(|b| b.get("max_wall_ms")).and_then(Value::as_u64),
    }
}

/// Shared effective-role resolver for routing and static graph invariants.
pub(crate) fn resolve_role(node: &GraphNode) -> Result<Role, RouteError> {
    match node.cognitive_role.as_deref() {
        Some(r) => Role::parse(r).ok_or_else(|| RouteError::BadRole(node.node_id.clone(), r.to_string())),
        // POLICY / VERIFY / WAIT / CONTROL and plain compute without a
        // capability request are deterministic S0 work.
        None if node.capability_request.is_some() && node.node_kind == "COMPUTE" => Ok(Role::S2),
        None => Ok(Role::S0),
    }
}

/// A missing/empty restriction permits every legal mode; any unknown entry
/// in a nonempty restriction is an error, even alongside a valid entry.
pub(crate) fn resolve_modes(node: &GraphNode, role: Role) -> Result<Vec<Mode>, RouteError> {
    let allowed: Vec<Mode> = node.allowed_modes.iter()
        .map(|m| Mode::parse(m).ok_or_else(|| RouteError::BadMode(node.node_id.clone(), m.clone())))
        .collect::<Result<_, _>>()?;
    let modes: Vec<Mode> = Mode::legal_for(role).iter().copied()
        .filter(|m| node.allowed_modes.is_empty() || allowed.contains(m)).collect();
    if modes.is_empty() {
        return Err(RouteError::NoLegalMode(node.node_id.clone()));
    }
    Ok(modes)
}

impl<'a, P: ModelPool> Router<'a, P> {
    pub fn new(pool: &'a P, config: &'a RouterConfig) -> Self {
        Self { pool, config }
    }

    pub fn route(&self, node: &GraphNode, ledger: &BudgetLedger) -> Result<ExecutionPlan, RouteError> {
        let nid = node.node_id.clone();

        // Budget: fail closed before anything else.
        let nb = node_budget(node);
        if !(ledger.remaining_cost_units > 0.0) || ledger.remaining_wall_ms == Some(0) || nb.max_cost == Some(0.0) {
            return Err(RouteError::BudgetExhausted(nid));
        }
        let cost_cap = nb.max_cost.map_or(ledger.remaining_cost_units, |m| m.min(ledger.remaining_cost_units));
        let wall_cap = match (nb.max_wall_ms, ledger.remaining_wall_ms) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };

        // 1. role, 2. legal modes ∩ node.allowed_modes.
        let role = resolve_role(node)?;
        let modes = resolve_modes(node, role)?;

        let req = node.capability_request.as_ref();
        let capability = req
            .and_then(|r| r.get("capability"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let quality_floor = req.and_then(|r| r.get("quality_floor")).and_then(Value::as_f64).unwrap_or(0.0);
        let trust: Option<TrustClass> = req
            .and_then(|r| r.get("trust_requirement"))
            .and_then(|t| serde_json::from_value(t.clone()).ok());

        let ext = Map::new();

        // S0 → PrimitiveRegistry, not the pool.
        if role == Role::S0 {
            return Ok(self.plan(node, role, Mode::M0Deterministic, capability.unwrap_or_else(|| "cap.system.deterministic".into()),
                S0_BACKEND_ID.into(), None, 1.0, cost_cap, wall_cap, vec![], None, ext));
        }
        let capability = capability.ok_or_else(|| RouteError::MissingCapability(nid.clone()))?;

        // S1 calibration gate (Q22 / WP8), resolved THROUGH THE POOL: an S1
        // backend is eligible only if its pool entry lists a gate-passing
        // calibration manifest for this primitive. No out-of-band bindings.
        let mut s1_cal: HashMap<String, CalibrationBinding> = HashMap::new();
        if role == Role::S1 {
            for e in self.pool.entries().iter().filter(|e| {
                e.cognitive_roles.contains(&Role::S1) && e.capabilities.iter().any(|c| c == &capability)
            }) {
                if let Some(b) = s1_calibration(e, &node.primitive_id) {
                    s1_cal.insert(e.backend_id.clone(), b);
                }
            }
            if s1_cal.is_empty() {
                return Err(RouteError::UncalibratedS1 { node: nid, primitive: node.primitive_id.clone() });
            }
        }

        // 3. candidates by capability / role / mode.
        let candidates: Vec<&PoolEntry> = self
            .pool
            .entries()
            .iter()
            .filter(|e| e.capabilities.iter().any(|c| c == &capability))
            .filter(|e| e.cognitive_roles.contains(&role))
            .filter(|e| role != Role::S1 || s1_cal.contains_key(&e.backend_id))
            .filter(|e| e.modes.iter().any(|m| modes.contains(m)))
            .collect();
        if candidates.is_empty() {
            return Err(RouteError::NoEligibleBackend { node: nid, capability });
        }

        // 4. policy / trust / locality — before any scoring (L1747).
        let pol = &self.config.policy;
        let mut reasons = Vec::new();
        let passed: Vec<&PoolEntry> = candidates
            .into_iter()
            .filter(|e| {
                let why = if pol.denied_backends.contains(&e.backend_id) {
                    Some("denied backend")
                } else if e.residency == Residency::Remote && !pol.allow_remote {
                    Some("remote residency disallowed")
                } else if e.residency == Residency::Remote && trust.map_or(false, |t| pol.local_only_trust.contains(&t)) {
                    Some("trust class requires local residency")
                } else if trust.map_or(false, |t| !e.trust_tags.iter().any(|x| x == t.as_str())) {
                    Some("backend not cleared for trust class")
                } else if e.confidence_estimate < quality_floor {
                    Some("below quality floor")
                } else {
                    None
                };
                if let Some(w) = why {
                    reasons.push(format!("{}: {}", e.backend_id, w));
                }
                why.is_none()
            })
            .collect();
        if passed.is_empty() {
            return Err(RouteError::PolicyRejected { node: nid, reasons });
        }

        // 5. budget (fail closed if nothing fits).
        let mut fits: Vec<&PoolEntry> = passed
            .into_iter()
            .filter(|e| e.cost <= cost_cap && wall_cap.map_or(true, |w| e.latency_ms <= w as f64))
            .collect();
        if fits.is_empty() {
            return Err(RouteError::BudgetExhausted(nid));
        }

        // 6. score: cheapest correct mode first, then utility.
        let utility = |e: &PoolEntry| e.confidence_estimate - e.cost - e.latency_ms / 100_000.0 - e.residency.penalty();
        let best_mode = |e: &PoolEntry| *e.modes.iter().filter(|m| modes.contains(m)).min().expect("filtered");
        fits.sort_by(|a, b| {
            best_mode(a)
                .cmp(&best_mode(b))
                .then(utility(b).partial_cmp(&utility(a)).unwrap_or(std::cmp::Ordering::Equal))
                .then(a.backend_id.cmp(&b.backend_id))
        });
        let prefs: Vec<String> = [format!("{capability}@{role:?}"), format!("role:{role:?}"), "*".to_string()]
            .iter().find_map(|k| self.config.class_preference.get(k).filter(|v| !v.is_empty()).cloned()).unwrap_or_default();
        let rank = |e: &PoolEntry| {
            let c = model_class(e);
            prefs.iter().position(|p| class_matches(&c, p)).unwrap_or(prefs.len())
        };
        fits.sort_by_key(|e| rank(e)); // stable: the order above breaks ties
        let chosen = fits[0];
        let chosen_rank = rank(chosen);
        let fallback: Vec<String> = fits[1..].iter().map(|e| e.backend_id.clone()).collect();
        let calibration = s1_cal.get(&chosen.backend_id).cloned();
        let mut ext = ext;
        if !prefs.is_empty() {
            ext.insert("x-route_preference".into(), Value::from(prefs.clone()));
            ext.insert("x-route_class".into(), Value::String(model_class(chosen)));
            ext.insert("x-route_preference_hit".into(), Value::Bool(chosen_rank < prefs.len()));
        }
        if role == Role::S1 {
            // Live only when BOTH the router config and the pool entry say live.
            let live = self.config.s1_mode == S1Mode::Live && entry_s1_live(chosen);
            ext.insert("x-s1_mode".into(), Value::String(if live { "live" } else { "shadow" }.into()));
        }

        let mut plan = self.plan(node, role, best_mode(chosen), capability, chosen.backend_id.clone(),
            Some(chosen.residency), quality_floor, cost_cap, wall_cap, fallback, calibration.as_ref(), ext);
        plan.layer_stop = chosen.layer_stop;
        plan.readout_head_id = chosen.readout_head_id.clone();
        Ok(plan)
    }

    #[allow(clippy::too_many_arguments)]
    fn plan(
        &self,
        node: &GraphNode,
        role: Role,
        mode: Mode,
        capability_id: String,
        backend_id: String,
        residency: Option<Residency>,
        confidence_floor: f64,
        cost_cap: f64,
        wall_cap: Option<u64>,
        fallback_chain: Vec<String>,
        calibration: Option<&CalibrationBinding>,
        ext: Map<String, Value>,
    ) -> ExecutionPlan {
        ExecutionPlan {
            schema_id: PLAN_SCHEMA_ID.into(),
            schema_version: SCHEMA_VERSION.into(),
            plan_id: format!("plan.{}", node.node_id),
            node_id: Some(node.node_id.clone()),
            cognitive_role: role,
            capability_id,
            execution_mode: mode,
            backend_id,
            layer_stop: None,
            readout_head_id: None,
            calibration_manifest_id: calibration.map(|c| c.manifest_id.clone()),
            calibration_level: calibration.map(|c| c.level),
            context_projection_id: node
                .extensions
                .as_ref()
                .and_then(|x| x.get("x-context_projection_id"))
                .and_then(Value::as_str)
                .map(str::to_string),
            residency_requirement: residency,
            confidence_floor: confidence_floor.clamp(0.0, 1.0),
            latency_budget_ms: wall_cap.map(|w| w as f64),
            cost_budget: Some(cost_cap),
            fallback_chain,
            extensions: if ext.is_empty() { None } else { Some(ext) },
        }
    }
}

// ---------------------------------------------------------------- S1 outcome

/// The fields of WP8's `DecisionResultV1` the router needs. Unknown fields are
/// ignored here on purpose: this is a *view* of a result the decision runtime
/// already validated against the frozen contract.
#[derive(Debug, Clone, Deserialize)]
pub struct DecisionResultView {
    pub confidence: f64,
    pub confidence_semantics: String,
    pub calibration_level_served: String,
    #[serde(default)]
    pub calibration_id: Option<String>,
    pub threshold_action: String,
    #[serde(default)]
    pub extensions: Option<Map<String, Value>>,
}

impl DecisionResultView {
    /// `x-decision_id` the runtime assigned (shadow ledger join key).
    pub fn decision_id(&self) -> Option<String> {
        super::s1_outcome::decision_id(&self.extensions)
    }
    /// Evidence ref to record on the node/receipt for a completion (GATE/VERIFY) decision,
    /// so the verifier's CompletionDecision can later report ground truth.
    pub fn verify_evidence_ref(&self) -> Option<String> {
        super::s1_outcome::verify_evidence_ref(&self.extensions)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum S1Verdict {
    /// Live, calibrated, AUTO and above floor: the S1 answer is authoritative.
    Accept,
    /// Shadow (default): recorded for evaluation; authority goes to the fallback chain.
    Shadow { confidence: f64 },
    /// Calibrated but not confident enough / REVIEW / ESCALATE: take the fallback chain.
    Escalate { reason: String },
    /// Uncalibrated or mismatched calibration: the numbers are never trusted.
    Refused { reason: String },
}

/// `apply_s1_result` plus recording: when the runtime returned a decision id,
/// append its `s1-verify:<id>` ref to the node's evidence so the later
/// CompletionDecision can report ground truth. Idempotent.
pub fn apply_s1_result_recording(plan: &ExecutionPlan, result: &DecisionResultView, node_evidence: &mut Vec<String>) -> S1Verdict {
    if let Some(r) = result.verify_evidence_ref() {
        if !node_evidence.contains(&r) {
            node_evidence.push(r);
        }
    }
    apply_s1_result(plan, result)
}

fn ext_str<'e>(x: &'e Option<Map<String, Value>>, k: &str) -> Option<&'e str> {
    x.as_ref()?.get(k)?.as_str()
}

/// `x-s1_mode` on a pool entry; anything but "live" is shadow.
fn entry_s1_live(e: &PoolEntry) -> bool {
    ext_str(&e.extensions, "x-s1_mode") == Some("live")
}

/// A gate-passing, non-RAW calibration for `primitive` listed in the entry's
/// `x-calibrations` (published by gizzi-code's ModelPool from the decision
/// runtime's `DecisionCalibrationManifestV1`s).
pub fn s1_calibration(e: &PoolEntry, primitive: &str) -> Option<CalibrationBinding> {
    if ext_str(&e.extensions, "x-calibration_status") != Some("calibrated") {
        return None;
    }
    let cals = e.extensions.as_ref()?.get("x-calibrations")?.as_array()?;
    cals.iter().find_map(|c| {
        if c.get("primitive_id")?.as_str()? != primitive || !c.get("gate_passed")?.as_bool()? {
            return None;
        }
        let level: CalibrationLevel = serde_json::from_value(c.get("level")?.clone()).ok()?;
        if level == CalibrationLevel::Raw {
            return None;
        }
        Some(CalibrationBinding { manifest_id: c.get("manifest_id")?.as_str()?.to_string(), level, gate_passed: true })
    })
}

/// Apply a WP8 decision result to an S1 plan. The S1 mode comes from the plan
/// (`x-s1_mode`), which the router set from config AND the pool entry.
pub fn apply_s1_result(plan: &ExecutionPlan, result: &DecisionResultView) -> S1Verdict {
    let mode = if ext_str(&plan.extensions, "x-s1_mode") == Some("live") { S1Mode::Live } else { S1Mode::Shadow };
    let refused_flag = result
        .extensions
        .as_ref()
        .and_then(|x| x.get("x-refused_uncalibrated"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if result.confidence_semantics != "CALIBRATED"
        || matches!(result.calibration_level_served.as_str(), "NONE" | "RAW")
        || refused_flag
    {
        return S1Verdict::Refused { reason: "uncalibrated S1 result".into() };
    }
    if plan.calibration_manifest_id.is_none() || result.calibration_id != plan.calibration_manifest_id {
        return S1Verdict::Refused { reason: "calibration manifest mismatch".into() };
    }
    if mode == S1Mode::Shadow {
        return S1Verdict::Shadow { confidence: result.confidence };
    }
    if result.threshold_action != "AUTO" {
        return S1Verdict::Escalate { reason: format!("threshold_action {}", result.threshold_action) };
    }
    if result.confidence < plan.confidence_floor {
        return S1Verdict::Escalate { reason: "below confidence floor".into() };
    }
    S1Verdict::Accept
}

#[cfg(test)]
#[path = "router_tests.rs"]
mod tests;
