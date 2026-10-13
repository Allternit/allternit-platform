//! Trajectory graders: deterministic checks over what a run (or a replay of
//! it, or a scripted walk) did, step by step. No model: every grader is a
//! pure function of the [`Trajectory`]. A grader is built from a JSON spec
//! `{ "type": "<grader>", ...params }` ([`parse`]).

use serde_json::{json, Value};
use std::collections::HashSet;

/// One step of a trajectory (a visited node).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Step {
    pub node: String,
    pub primitive: String,
    pub ok: bool,
}

/// One effect the trajectory performed (or, in a replay, was served from the recording).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Effect {
    pub node: String,
    pub tool: String,
    pub committed: bool,
}

/// What a run did, in the order it did it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Trajectory {
    /// `completed` | `failed` | `partial` | `cancelled` | `waiting` | `needs_evidence` | `refused` | ...
    pub status: String,
    /// The verifier's verdict (None: never reached).
    pub verifier: Option<bool>,
    pub steps: Vec<Step>,
    pub effects: Vec<Effect>,
    /// Text the run produced (model outputs, document contents), for citation checks.
    pub texts: Vec<String>,
    pub stopped_at: Option<String>,
    /// Completion criteria the verifier found unsupported.
    pub missing: Vec<String>,
    pub cost_usd: f64,
    pub tokens: u64,
    pub latency_ms: f64,
}

impl Trajectory {
    fn step_matches(s: &Step, spec: &str) -> bool {
        s.node == spec || s.primitive == spec
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Grade {
    pub grader: &'static str,
    pub pass: bool,
    pub detail: String,
}

impl Grade {
    pub fn json(&self) -> Value {
        json!({ "grader": self.grader, "pass": self.pass, "detail": self.detail })
    }
}

pub trait Grader: Send + Sync {
    fn name(&self) -> &'static str;
    fn grade(&self, t: &Trajectory) -> Grade;
}

fn grade(name: &'static str, pass: bool, detail: impl Into<String>) -> Grade {
    Grade { grader: name, pass, detail: detail.into() }
}

/// The steps occur in this order (a subsequence of the committed steps; a
/// step id matches a node id or a primitive id): "did it look the order up
/// before it refunded?".
pub struct StepOrder(pub Vec<String>);
impl Grader for StepOrder {
    fn name(&self) -> &'static str {
        "step_order"
    }
    fn grade(&self, t: &Trajectory) -> Grade {
        let mut want = self.0.iter().peekable();
        for s in t.steps.iter().filter(|s| s.ok) {
            if want.peek().is_some_and(|w| Trajectory::step_matches(s, w)) {
                want.next();
            }
        }
        match want.next() {
            None => grade(self.name(), true, format!("{} in order", self.0.join(" → "))),
            Some(w) => grade(self.name(), false, format!("{w} did not run (committed) in the required order {}", self.0.join(" → "))),
        }
    }
}

/// Every one of these steps committed, in any order.
pub struct RequiredSteps(pub Vec<String>);
impl Grader for RequiredSteps {
    fn name(&self) -> &'static str {
        "required_steps"
    }
    fn grade(&self, t: &Trajectory) -> Grade {
        let missing: Vec<&String> = self.0.iter().filter(|w| !t.steps.iter().any(|s| Trajectory::step_matches(s, w))).collect();
        if missing.is_empty() {
            grade(self.name(), true, "all required steps ran")
        } else {
            grade(self.name(), false, format!("never ran: {}", missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")))
        }
    }
}

/// None of these steps ran (any outcome).
pub struct ForbiddenStep(pub Vec<String>);
impl Grader for ForbiddenStep {
    fn name(&self) -> &'static str {
        "forbidden_step"
    }
    fn grade(&self, t: &Trajectory) -> Grade {
        let hit: Vec<&str> = t.steps.iter().filter(|s| self.0.iter().any(|w| Trajectory::step_matches(s, w))).map(|s| s.node.as_str()).collect();
        if hit.is_empty() {
            grade(self.name(), true, "no forbidden step ran")
        } else {
            grade(self.name(), false, format!("forbidden step(s) ran: {}", hit.join(", ")))
        }
    }
}

/// Committed effects (optionally of one tool) within `[min, max]`.
pub struct EffectCount {
    pub min: Option<u64>,
    pub max: Option<u64>,
    pub tool: Option<String>,
}
impl Grader for EffectCount {
    fn name(&self) -> &'static str {
        "effect_count"
    }
    fn grade(&self, t: &Trajectory) -> Grade {
        let n = t.effects.iter().filter(|e| e.committed && self.tool.as_ref().map_or(true, |x| &e.tool == x)).count() as u64;
        let ok = self.min.map_or(true, |m| n >= m) && self.max.map_or(true, |m| n <= m);
        grade(self.name(), ok, format!("{n} committed effect(s){}; allowed {}..{}", self.tool.as_ref().map(|t| format!(" of {t}")).unwrap_or_default(),
            self.min.map_or("0".into(), |m| m.to_string()), self.max.map_or("∞".into(), |m| m.to_string())))
    }
}

/// At least `min` distinct citations (`[src:<id>]` by default) in the output.
pub struct MustCite {
    pub min: usize,
    pub marker: String,
}
impl Grader for MustCite {
    fn name(&self) -> &'static str {
        "must_cite"
    }
    fn grade(&self, t: &Trajectory) -> Grade {
        let mut cites = HashSet::new();
        for text in &t.texts {
            for (i, _) in text.match_indices(self.marker.as_str()) {
                if let Some(c) = text[i + self.marker.len()..].split(']').next().filter(|c| !c.trim().is_empty() && c.len() <= 256) {
                    cites.insert(c.trim().to_string());
                }
            }
        }
        grade(self.name(), cites.len() >= self.min, format!("{} distinct citation(s), {} required", cites.len(), self.min))
    }
}

/// The verifier passed and the run completed.
pub struct VerifierPass;
impl Grader for VerifierPass {
    fn name(&self) -> &'static str {
        "verifier_pass"
    }
    fn grade(&self, t: &Trajectory) -> Grade {
        let ok = t.verifier == Some(true) && t.status == "completed";
        grade(self.name(), ok, format!("verifier {}, status {}", t.verifier.map_or("not reached", |v| if v { "pass" } else { "fail" }), t.status))
    }
}

pub struct CostCeiling {
    pub max_usd: Option<f64>,
    pub max_tokens: Option<u64>,
}
impl Grader for CostCeiling {
    fn name(&self) -> &'static str {
        "cost_ceiling"
    }
    fn grade(&self, t: &Trajectory) -> Grade {
        let ok = self.max_usd.map_or(true, |m| t.cost_usd <= m) && self.max_tokens.map_or(true, |m| t.tokens <= m);
        grade(self.name(), ok, format!("${:.4} / {} tokens", t.cost_usd, t.tokens))
    }
}

pub struct LatencyCeiling {
    pub max_ms: f64,
}
impl Grader for LatencyCeiling {
    fn name(&self) -> &'static str {
        "latency_ceiling"
    }
    fn grade(&self, t: &Trajectory) -> Grade {
        grade(self.name(), t.latency_ms <= self.max_ms, format!("{:.0} ms, ceiling {:.0} ms", t.latency_ms, self.max_ms))
    }
}

/// The trajectory ended in this status.
pub struct Status(pub String);
impl Grader for Status {
    fn name(&self) -> &'static str {
        "status"
    }
    fn grade(&self, t: &Trajectory) -> Grade {
        grade(self.name(), t.status == self.0, format!("status {} (want {})", t.status, self.0))
    }
}

/// The trajectory stopped at this step.
pub struct StoppedAt(pub String);
impl Grader for StoppedAt {
    fn name(&self) -> &'static str {
        "stopped_at"
    }
    fn grade(&self, t: &Trajectory) -> Grade {
        grade(self.name(), t.stopped_at.as_deref() == Some(self.0.as_str()), format!("stopped at {:?} (want {})", t.stopped_at, self.0))
    }
}

/// The verifier found exactly these criteria missing.
pub struct MissingEvidence(pub Vec<String>);
impl Grader for MissingEvidence {
    fn name(&self) -> &'static str {
        "missing_evidence"
    }
    fn grade(&self, t: &Trajectory) -> Grade {
        grade(self.name(), t.missing == self.0, format!("missing {:?} (want {:?})", t.missing, self.0))
    }
}

pub const GRADERS: &[&str] = &[
    "step_order", "required_steps", "forbidden_step", "effect_count", "must_cite", "verifier_pass", "cost_ceiling",
    "latency_ceiling", "status", "stopped_at", "missing_evidence",
];

fn strings(v: &Value, key: &str, ty: &str) -> Result<Vec<String>, String> {
    let a = v[key].as_array().filter(|a| !a.is_empty() && a.len() <= 64).ok_or_else(|| format!("{ty}.{key} must list 1-64 step ids"))?;
    a.iter()
        .map(|s| s.as_str().filter(|s| !s.is_empty() && s.len() <= 128).map(String::from).ok_or_else(|| format!("{ty}.{key} entries must be step ids")))
        .collect()
}

fn known(v: &Value, ty: &str, keys: &[&str]) -> Result<(), String> {
    match v.as_object().and_then(|o| o.keys().find(|k| *k != "type" && !keys.contains(&k.as_str()))) {
        Some(k) => Err(format!("{ty}: unknown field `{k}`")),
        None => Ok(()),
    }
}

fn num(v: &Value, key: &str, ty: &str) -> Result<Option<f64>, String> {
    match &v[key] {
        Value::Null => Ok(None),
        x => x.as_f64().filter(|n| *n >= 0.0 && n.is_finite()).map(Some).ok_or_else(|| format!("{ty}.{key} must be a non-negative number")),
    }
}

/// Build a grader from its JSON spec.
pub fn parse(v: &Value) -> Result<Box<dyn Grader>, String> {
    let ty = v["type"].as_str().ok_or("a grader needs a type")?;
    Ok(match ty {
        "step_order" => {
            known(v, ty, &["steps"])?;
            Box::new(StepOrder(strings(v, "steps", ty)?))
        }
        "required_steps" => {
            known(v, ty, &["steps"])?;
            Box::new(RequiredSteps(strings(v, "steps", ty)?))
        }
        "forbidden_step" => {
            known(v, ty, &["steps"])?;
            Box::new(ForbiddenStep(strings(v, "steps", ty)?))
        }
        "effect_count" => {
            known(v, ty, &["min", "max", "tool"])?;
            let (min, max) = (num(v, "min", ty)?.map(|n| n as u64), num(v, "max", ty)?.map(|n| n as u64));
            if min.is_none() && max.is_none() {
                return Err("effect_count needs min or max".into());
            }
            if min.zip(max).is_some_and(|(a, b)| a > b) {
                return Err("effect_count.min is above max".into());
            }
            Box::new(EffectCount { min, max, tool: v["tool"].as_str().map(String::from) })
        }
        "must_cite" => {
            known(v, ty, &["min", "marker"])?;
            let marker = v["marker"].as_str().unwrap_or("[src:").to_string();
            if marker.is_empty() || marker.len() > 32 {
                return Err("must_cite.marker must be 1-32 characters".into());
            }
            Box::new(MustCite { min: num(v, "min", ty)?.unwrap_or(1.0) as usize, marker })
        }
        "verifier_pass" => {
            known(v, ty, &[])?;
            Box::new(VerifierPass)
        }
        "cost_ceiling" => {
            known(v, ty, &["max_usd", "max_tokens"])?;
            let (max_usd, max_tokens) = (num(v, "max_usd", ty)?, num(v, "max_tokens", ty)?.map(|n| n as u64));
            if max_usd.is_none() && max_tokens.is_none() {
                return Err("cost_ceiling needs max_usd or max_tokens".into());
            }
            Box::new(CostCeiling { max_usd, max_tokens })
        }
        "latency_ceiling" => {
            known(v, ty, &["max_ms"])?;
            Box::new(LatencyCeiling { max_ms: num(v, "max_ms", ty)?.ok_or("latency_ceiling needs max_ms")? })
        }
        "status" => {
            known(v, ty, &["equals"])?;
            Box::new(Status(v["equals"].as_str().filter(|s| !s.is_empty()).ok_or("status needs equals")?.to_string()))
        }
        "stopped_at" => {
            known(v, ty, &["step"])?;
            Box::new(StoppedAt(v["step"].as_str().filter(|s| !s.is_empty()).ok_or("stopped_at needs step")?.to_string()))
        }
        "missing_evidence" => {
            known(v, ty, &["equals"])?;
            let a = v["equals"].as_array().ok_or("missing_evidence needs equals (a list of criteria)")?;
            Box::new(MissingEvidence(a.iter().map(|s| s.as_str().map(String::from).ok_or("missing_evidence.equals lists criteria ids")).collect::<Result<_, _>>()?))
        }
        other => return Err(format!("unknown grader `{other}` (known: {})", GRADERS.join(", "))),
    })
}

/// Grade `t` with every grader; the attempt passes when all pass.
pub fn grade_all(graders: &[Box<dyn Grader>], t: &Trajectory) -> (bool, Vec<Grade>) {
    let g: Vec<Grade> = graders.iter().map(|x| x.grade(t)).collect();
    (g.iter().all(|x| x.pass), g)
}
