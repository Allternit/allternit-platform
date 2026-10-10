//! Parallel subtasks and Behavior Best-of-N (Driver phase D7).
//!
//! Two fan-outs over `run_subtask` (`computer_subtask`):
//!
//! - **`run_parallel`**: independent subtasks, one per computer, run at the
//!   same time. Each subtask names its computer (default: the route's) and
//!   goes through that computer's own target, control lease, audit row and
//!   subtask loop. Results come back in input order.
//! - **`run_subtask` with `best_of: N`** (Agent S3's Behavior Best-of-N,
//!   Apache-2.0): N rollouts of one subtask on N sandbox computers at once.
//!   Each rollout's step trace becomes a narrative; when more than one
//!   rollout reached the goal, a `judge` decision (`/v1/decisions`, fast
//!   tiers first, the planner-class oracle only when they abstain and the
//!   cost cap allows) picks one by those narratives. Rollouts run only on
//!   sandbox/cloud computers, never as N copies on the person's own machine.
//!
//! **One input owner per computer.** A computer appearing twice in one call
//! is refused before anything runs, and a process-wide registry
//! (`claim_input`) refuses a subtask loop on a computer another loop is
//! already driving, across calls. Guest leases are per run id, so a second
//! agent on a guest is also refused by `lease_gate`.
//!
//! **Budgets.** `max_parallel` (default 4, at most 8) subtasks at once; one
//! wall-clock `budget_ms` for the whole call (each subtask's own budget is
//! cut to what is left when it starts); `max_cost_usd` on decision spend
//! (subtasks not yet started once it is reached come back `skipped`). Each
//! rollout keeps run_subtask's own `max_steps` / `budget_ms`.
//!
//! **Approval.** One approval covers the whole call when any subtask targets
//! a non-sandbox computer (the same rule as run_subtask); the subtasks'
//! steps carry it. Best-of-N is sandbox-only, so it never asks.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::http::{HeaderMap, StatusCode};
use futures::stream::{self, StreamExt};
use once_cell::sync::Lazy;
use serde_json::{json, Value};

use crate::auth::AuthUser;
use crate::computer_routes::ComputerResponse;
use crate::computer_toolset::{
    approval_gate, audit, build_target, emit_action, error_result, lease_check, needs_approval, policy_check, resolve_computer,
    unsupported_reason, MemberSpec, Target, Toolset, ToolsetRequest,
};
use crate::AppState;

const DEFAULT_MAX_PARALLEL: u64 = 4;
const MAX_PARALLEL: u64 = 8;
const DEFAULT_TOTAL_BUDGET_MS: u64 = 120_000;
const MAX_TOTAL_BUDGET_MS: u64 = 600_000;
const DEFAULT_COST_CAP_USD: f64 = 0.25;
const MAX_COST_CAP_USD: f64 = 5.0;
const MIN_BEST_OF: u64 = 2;
const MAX_BEST_OF: u64 = 5;
/// A subtask that would start with less than this left is skipped.
const MIN_START_MS: u64 = 1_000;
/// run_subtask's own default budget, for cutting to the call's remainder.
const SUBTASK_DEFAULT_BUDGET_MS: u64 = 60_000;
/// The judge's latency budget (fast tiers; the oracle gets the same).
const JUDGE_BUDGET_MS: u64 = 8_000;

/// Is this call a fan-out handled here?
pub fn is_fanout(member: &str, input: &Value) -> bool {
    member == "run_parallel" || (member == "run_subtask" && input.get("best_of").is_some_and(|n| !n.is_null()))
}

// ---------------------------------------------------------------------------
// One input owner per computer.
// ---------------------------------------------------------------------------

static OWNERS: Lazy<Mutex<HashMap<String, String>>> = Lazy::new(|| Mutex::new(HashMap::new()));

/// Holds a computer's input for one subtask loop; released on drop.
pub struct InputOwner {
    computer: String,
}

impl Drop for InputOwner {
    fn drop(&mut self) {
        if let Ok(mut m) = OWNERS.lock() {
            m.remove(&self.computer);
        }
    }
}

/// Claim `computer_id`'s input for `run_id`. `Err(holder run id)` when another
/// subtask loop is driving it.
pub fn claim_input(computer_id: &str, run_id: &str) -> Result<InputOwner, String> {
    let mut m = OWNERS.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(holder) = m.get(computer_id) {
        return Err(holder.clone());
    }
    m.insert(computer_id.to_string(), run_id.to_string());
    Ok(InputOwner { computer: computer_id.to_string() })
}

pub fn busy_text(holder: &str) -> String {
    format!("Another subtask ({holder}) is driving this computer. One computer runs one subtask at a time; wait for it or use another computer.")
}

// ---------------------------------------------------------------------------
// Plan: the jobs a call runs, and its limits.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Limits {
    pub max_parallel: usize,
    pub budget: Duration,
    pub cost_cap: f64,
}

pub(crate) fn limits(input: &Value) -> Limits {
    let num = |k: &str| input.get(k).and_then(Value::as_u64);
    Limits {
        max_parallel: num("max_parallel").unwrap_or(DEFAULT_MAX_PARALLEL).clamp(1, MAX_PARALLEL) as usize,
        budget: Duration::from_millis(num("budget_ms").unwrap_or(DEFAULT_TOTAL_BUDGET_MS).clamp(MIN_START_MS, MAX_TOTAL_BUDGET_MS)),
        cost_cap: input.get("max_cost_usd").and_then(Value::as_f64).unwrap_or(DEFAULT_COST_CAP_USD).clamp(0.0, MAX_COST_CAP_USD),
    }
}

/// One subtask: the computer it asks for and its run_subtask input.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Job {
    pub computer: String,
    pub input: Value,
}

/// `run_parallel`'s jobs; a subtask without `computer` runs on the route's.
pub(crate) fn parallel_jobs(input: &Value, route_id: &str) -> Result<Vec<Job>, String> {
    let subtasks = input.get("subtasks").and_then(Value::as_array).filter(|a| !a.is_empty()).ok_or("run_parallel needs at least one subtask")?;
    if subtasks.len() as u64 > MAX_PARALLEL {
        return Err(format!("run_parallel takes at most {MAX_PARALLEL} subtasks"));
    }
    Ok(subtasks
        .iter()
        .map(|s| {
            let mut sub = s.as_object().cloned().unwrap_or_default();
            let computer = sub.remove("computer").and_then(|c| c.as_str().map(str::to_string)).filter(|c| !c.trim().is_empty());
            Job { computer: computer.unwrap_or_else(|| route_id.to_string()), input: Value::Object(sub) }
        })
        .collect())
}

/// Best-of-N's rollouts: `best_of` copies of the subtask, one per listed computer.
pub(crate) fn best_of_jobs(input: &Value) -> Result<Vec<Job>, String> {
    let n = input.get("best_of").and_then(Value::as_u64).filter(|n| (MIN_BEST_OF..=MAX_BEST_OF).contains(n));
    let n = n.ok_or(format!("best_of must be a whole number from {MIN_BEST_OF} to {MAX_BEST_OF}"))? as usize;
    let computers: Vec<String> = input
        .get("computers")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::trim).filter(|c| !c.is_empty()).map(str::to_string).collect())
        .unwrap_or_default();
    let mut seen = HashSet::new();
    let distinct: Vec<String> = computers.into_iter().filter(|c| seen.insert(c.clone())).collect();
    if distinct.len() < n {
        return Err(format!(
            "best_of {n} needs {n} distinct sandbox computers in computers (got {}). Rollouts run one per sandbox/cloud computer, never several on one.",
            distinct.len()
        ));
    }
    let mut sub = input.as_object().cloned().unwrap_or_default();
    for k in ["best_of", "computers", "max_cost_usd"] {
        sub.remove(k);
    }
    Ok(distinct.into_iter().take(n).map(|computer| Job { computer, input: Value::Object(sub.clone()) }).collect())
}

/// One computer never gets two subtasks in one call (resolved ids, so
/// `this-device` and its own id count as one).
pub(crate) fn one_owner_each(resolved: &[String]) -> Result<(), String> {
    let mut seen = HashSet::new();
    match resolved.iter().find(|id| !seen.insert(id.as_str())) {
        Some(dup) => Err(format!("Computer {dup} appears more than once. One computer runs one subtask at a time; give each subtask its own computer.")),
        None => Ok(()),
    }
}

/// Best-of-N runs only on sandbox/cloud computers.
pub(crate) fn sandbox_only(labels: &[(String, &'static str)]) -> Result<(), String> {
    match labels.iter().find(|(_, label)| !matches!(*label, "guest_linux" | "guest_windows")) {
        Some((id, _)) => Err(format!(
            "best_of runs its rollouts only on sandbox/cloud computers, never as copies on the person's own machine; {id} isn't one. List cloud computers in computers."
        )),
        None => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// The fan-out.
// ---------------------------------------------------------------------------

fn cost_of(body: &Value) -> f64 {
    body.pointer("/oracle/cost_usd").and_then(Value::as_f64).unwrap_or(0.0)
}

fn skipped(why: &str) -> Value {
    json!({ "status": "skipped", "reason": why })
}

/// Run `n` jobs, at most `max_parallel` at once, inside the call's budget and
/// cost cap. `run(i, left)` runs job `i` with `left` of the call's budget.
/// Results come back in job order.
pub(crate) async fn fan_out<F, Fut>(n: usize, limits: &Limits, run: F) -> Vec<Value>
where
    F: Fn(usize, Duration) -> Fut,
    Fut: Future<Output = Value>,
{
    let started = Instant::now();
    let spent = Mutex::new(0.0_f64);
    let (run, spent) = (&run, &spent);
    let mut out: Vec<(usize, Value)> = stream::iter(0..n)
        .map(|i| async move {
            let left = limits.budget.saturating_sub(started.elapsed());
            if left < Duration::from_millis(MIN_START_MS) {
                return (i, skipped("the call's budget_ms ran out before this subtask could start"));
            }
            if *spent.lock().unwrap_or_else(|p| p.into_inner()) >= limits.cost_cap && limits.cost_cap > 0.0 {
                return (i, skipped("max_cost_usd was reached before this subtask could start"));
            }
            let body = run(i, left).await;
            *spent.lock().unwrap_or_else(|p| p.into_inner()) += cost_of(&body);
            (i, body)
        })
        .buffer_unordered(limits.max_parallel.max(1))
        .collect()
        .await;
    out.sort_by_key(|(i, _)| *i);
    out.into_iter().map(|(_, v)| v).collect()
}

/// A subtask input with its budget cut to what the call has left.
fn within_budget(input: &Value, left: Duration) -> Value {
    let mut v = input.clone();
    let own = v.get("budget_ms").and_then(Value::as_u64).unwrap_or(SUBTASK_DEFAULT_BUDGET_MS);
    v["budget_ms"] = json!(own.min(left.as_millis() as u64));
    v
}

// ---------------------------------------------------------------------------
// Best-of-N: narratives and the judge.
// ---------------------------------------------------------------------------

/// A rollout's step narrative, the text the judge reads (Agent S3's
/// "behavior narrative": what each step did and what the screen did after).
pub(crate) fn narrative(label: &str, computer: &str, body: &Value) -> String {
    let status = body.get("status").and_then(Value::as_str).unwrap_or("failed");
    let mut s = format!("Rollout {label} on computer {computer}: ended {status}");
    if let Some(r) = body.get("reason").and_then(Value::as_str) {
        s.push_str(&format!(" ({r})"));
    }
    s.push_str(".\n");
    let steps = body.get("steps").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut n = 0;
    for st in &steps {
        let Some(opt) = st.get("option").and_then(Value::as_str) else { continue };
        n += 1;
        let a = st.get("action").cloned().unwrap_or(Value::Null);
        let how = match (a.get("ok").and_then(Value::as_bool), a.get("changed").and_then(Value::as_bool)) {
            (Some(false), _) => format!(" -> failed: {}", a.get("detail").and_then(Value::as_str).unwrap_or("the action didn't complete")),
            (Some(true), Some(false)) => " -> ran, the screen didn't change".to_string(),
            (Some(true), _) => " -> ran, the screen changed".to_string(),
            _ => String::new(),
        };
        if n <= 20 {
            s.push_str(&format!("{n}. {opt}{how}\n"));
        }
    }
    if n > 20 {
        s.push_str(&format!("... {} more steps\n", n - 20));
    }
    match body.get("success") {
        Some(Value::Bool(true)) => s.push_str("Success checks: held.\n"),
        Some(Value::Bool(false)) if status == "done" => s.push_str("Success checks: none given; the loop judged the goal met.\n"),
        _ => {}
    }
    if let Some(screen) = body.get("screen") {
        let texts: Vec<String> = screen
            .get("elements")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|e| {
                let pick = |k: &str| e.get(k).and_then(Value::as_str).filter(|t| !t.is_empty());
                pick("name").or_else(|| pick("value")).map(str::to_string)
            })
            .take(12)
            .collect();
        if !texts.is_empty() {
            s.push_str(&format!("Final screen shows: {}\n", texts.join(" | ")));
        }
    }
    s
}

/// Which rollouts the judge chooses among: the ones whose success checks
/// held; failing that, the ones that ended done. Empty = none reached the goal.
pub(crate) fn shortlist(bodies: &[Value]) -> Vec<usize> {
    let st = |b: &Value| b.get("status").and_then(Value::as_str) == Some("done");
    let verified: Vec<usize> = (0..bodies.len()).filter(|&i| st(&bodies[i]) && bodies[i].get("success") == Some(&json!(true))).collect();
    if !verified.is_empty() {
        return verified;
    }
    (0..bodies.len()).filter(|&i| st(&bodies[i])).collect()
}

/// The judge's `/v1/decisions` body over the candidates' narratives.
pub(crate) fn judge_request(goal: &str, narratives: &[String], candidates: &[usize], backends: &[&str]) -> Value {
    let mut context = format!("Goal: {goal}\nThese rollouts each tried the same goal on a separate computer. Their step narratives:\n\n");
    for &i in candidates {
        context.push_str(&narratives[i]);
        context.push('\n');
    }
    json!({
        "kind": "judge",
        "context": context,
        "question": "Which rollout achieved the goal most correctly and completely, with no stray or harmful steps?",
        "options": candidates.iter().map(|&i| json!({ "id": format!("r{}", i + 1), "text": format!("rollout r{}", i + 1) })).collect::<Vec<_>>(),
        "allow_abstain": true,
        "backends": backends,
        "latency_budget_ms": JUDGE_BUDGET_MS,
        "task": goal.chars().take(200).collect::<String>(),
    })
}

/// The rollout a judge reply picked, if it picked one of the candidates.
pub(crate) fn picked(reply: &Value, candidates: &[usize]) -> Option<usize> {
    if reply.get("abstained").and_then(Value::as_bool).unwrap_or(true) {
        return None;
    }
    let c = reply.get("choice").and_then(Value::as_str)?;
    candidates.iter().copied().find(|&i| c == format!("r{}", i + 1))
}

/// Choose the winning rollout. `decide(body)` runs one judge decision.
/// Returns (winner, why, the judge replies).
pub(crate) async fn choose<D, Fut>(goal: &str, narratives: &[String], bodies: &[Value], fast: &[&str], allow_oracle: bool, mut decide: D) -> (Option<usize>, String, Vec<Value>)
where
    D: FnMut(Value) -> Fut,
    Fut: Future<Output = Value>,
{
    let candidates = shortlist(bodies);
    match candidates.as_slice() {
        [] => return (None, "no rollout reached the goal".into(), vec![]),
        [only] => {
            let why = if bodies[*only].get("success") == Some(&json!(true)) {
                "the only rollout whose success checks held"
            } else {
                "the only rollout that ended done"
            };
            return (Some(*only), why.into(), vec![]);
        }
        _ => {}
    }
    let mut replies = Vec::new();
    let mut tiers: Vec<Vec<&str>> = vec![];
    if !fast.is_empty() {
        tiers.push(fast.to_vec());
    }
    if allow_oracle {
        tiers.push(vec!["oracle"]);
    }
    for tier in tiers {
        let reply = decide(judge_request(goal, narratives, &candidates, &tier)).await;
        let pick = picked(&reply, &candidates);
        let conf = reply.get("confidence").and_then(Value::as_f64).unwrap_or(0.0);
        let backend = reply.get("backend").and_then(Value::as_str).unwrap_or("?").to_string();
        replies.push(reply);
        if let Some(w) = pick {
            let why = format!("the judge ({backend}, confidence {conf:.2}) picked r{} by the step narratives among {} rollouts that reached the goal", w + 1, candidates.len());
            return (Some(w), why, replies);
        }
    }
    (None, format!("the judge couldn't separate the {} rollouts that reached the goal; compare their narratives", candidates.len()), replies)
}

// ---------------------------------------------------------------------------
// The executor.
// ---------------------------------------------------------------------------

/// One planned subtask: its resolved computer and target.
struct Planned {
    computer: ComputerResponse,
    target: Target,
    input: Value,
}

/// Run one subtask on its computer: support check, input owner, lease,
/// audit, then the subtask loop. Every end is a JSON body with a status.
async fn run_one(state: &Arc<AppState>, user: &AuthUser, p: &Planned, left: Duration, run_id: &str, parent: &str) -> Value {
    if let Some(reason) = unsupported_reason(p.target.label(), Toolset::Computer, "run_subtask") {
        return json!({ "status": "failed", "error": "unimplemented", "reason": reason });
    }
    let _owner = match claim_input(&p.computer.id, run_id) {
        Ok(g) => g,
        Err(holder) => return json!({ "status": "refused", "error": "computer_busy", "reason": busy_text(&holder) }),
    };
    if let Err((_, code, message)) = lease_check(state, user, &p.computer, &p.target, Some(run_id)).await {
        return json!({ "status": "refused", "error": code, "reason": message });
    }
    let input = within_budget(&p.input, left);
    let req = ToolsetRequest {
        toolset: Toolset::Computer,
        member: "run_subtask".into(),
        input: input.clone(),
        run_id: Some(run_id.to_string()),
        turn_id: None,
        call_index: None,
        model_frame: None,
        coordinate_space: None,
        approval_grant: None,
        browser_session_id: None,
        enable: vec![],
        within_subtask: Some(parent.to_string()),
        project_id: None,
    };
    let spec = crate::computer_toolset::contract(Toolset::Computer).member("run_subtask").expect("run_subtask is in the contract");
    let (desc, verdict) = match policy_check(user, &p.computer, &req) {
        Ok(pv) => pv,
        Err(reason) => return json!({ "status": "refused", "error": "policy_denied", "reason": reason }),
    };
    if audit(user, &p.computer, &p.target, spec, &req, &desc, verdict.as_ref(), None).is_err() {
        return json!({ "status": "refused", "error": "audit_unavailable", "reason": "The action log couldn't be written, so the subtask did not run." });
    }
    crate::computer_routes::touch_computer_activity(&state.db, &p.computer.id);
    let out = crate::computer_subtask::run_body(state, user, &p.computer, &p.target, &input, run_id).await;
    emit_action(&p.computer.id, Toolset::Computer, "run_subtask", None, None, Some(run_id), out.is_ok());
    out.unwrap_or_else(|f| json!({ "status": "failed", "error": "action_failed", "reason": f.message }))
}

fn reply(status: StatusCode, body: Value) -> (StatusCode, Value) {
    (status, body)
}

fn refuse(status: StatusCode, code: &str, message: impl Into<String>) -> (StatusCode, Value) {
    reply(status, serde_json::to_value(error_result(code, message, None)).unwrap_or_default())
}

fn ok_result(body: &Value) -> (StatusCode, Value) {
    reply(StatusCode::OK, serde_json::to_value(crate::computer_subtask::as_result(body)).unwrap_or_default())
}

/// `run_parallel` or `run_subtask` + `best_of`, after contract validation.
pub async fn execute(state: &Arc<AppState>, user: &AuthUser, headers: &HeaderMap, route: &ComputerResponse, spec: &MemberSpec, req: &ToolsetRequest) -> (StatusCode, Value) {
    let best_of = req.member == "run_subtask";
    let planned_jobs = if best_of { best_of_jobs(&req.input) } else { parallel_jobs(&req.input, &route.id) };
    let jobs = match planned_jobs {
        Ok(j) => j,
        Err(e) => return refuse(StatusCode::BAD_REQUEST, "invalid_input", e),
    };
    let mut lim = limits(&req.input);
    if best_of {
        // Rollouts all run at once; the call's budget is the rollout budget.
        lim.max_parallel = jobs.len();
        let own = req.input.get("budget_ms").and_then(Value::as_u64).unwrap_or(SUBTASK_DEFAULT_BUDGET_MS);
        lim.budget = Duration::from_millis(own.clamp(MIN_START_MS, MAX_TOTAL_BUDGET_MS) + JUDGE_BUDGET_MS);
    }

    // Resolve every computer and its target up front: sandboxing decides
    // the approval, and a duplicate computer refuses the whole call.
    let mut planned = Vec::with_capacity(jobs.len());
    for job in &jobs {
        let computer = match resolve_computer(state, user, &job.computer, headers).await {
            Ok(c) => c,
            Err((status, m)) => return refuse(status, "computer_unavailable", format!("{}: {m}", job.computer)),
        };
        let target = match build_target(state, user, &computer, Toolset::Computer, None).await {
            Ok(t) => t,
            Err((status, m)) => return refuse(status, "target_unavailable", format!("{}: {m}", job.computer)),
        };
        planned.push(Planned { computer, target, input: job.input.clone() });
    }
    if let Err(e) = one_owner_each(&planned.iter().map(|p| p.computer.id.clone()).collect::<Vec<_>>()) {
        return refuse(StatusCode::CONFLICT, "computer_conflict", e);
    }
    if best_of {
        let labels: Vec<(String, &'static str)> = planned.iter().map(|p| (p.computer.id.clone(), p.target.label())).collect();
        if let Err(e) = sandbox_only(&labels) {
            return refuse(StatusCode::CONFLICT, "sandbox_required", e);
        }
    }

    // Policy, one approval for the whole call (when any target is the
    // person's own machine), and the call's audit row.
    let (desc, verdict) = match policy_check(user, route, req) {
        Ok(pv) => pv,
        Err(reason) => return refuse(StatusCode::FORBIDDEN, "policy_denied", reason),
    };
    let all_sandboxed = planned.iter().all(|p| p.target.sandboxed());
    if needs_approval(spec, all_sandboxed) {
        if let Err(refusal) = approval_gate(state, user, &route.id, spec, req) {
            return refusal;
        }
    }
    // `planned` is never empty (both planners refuse an empty call).
    if audit(user, route, &planned[0].target, spec, req, &desc, verdict.as_ref(), None).is_err() {
        return refuse(StatusCode::INTERNAL_SERVER_ERROR, "audit_unavailable", "The action log couldn't be written, so nothing ran.");
    }

    let parent = req.run_id.clone().unwrap_or_else(|| format!("toolset-{}", uuid::Uuid::new_v4().simple()));
    let started = Instant::now();
    let ids: Vec<String> = (0..planned.len()).map(|i| format!("{parent}:{}{}", if best_of { "r" } else { "s" }, i + 1)).collect();
    let (planned_ref, ids_ref, parent_ref) = (&planned, &ids, parent.as_str());
    let bodies = fan_out(planned.len(), &lim, |i, left| run_one(state, user, &planned_ref[i], left, &ids_ref[i], parent_ref)).await;
    let spent: f64 = bodies.iter().map(cost_of).sum();
    let elapsed = (started.elapsed().as_secs_f64() * 1000.0).round();
    let count = |s: &str| bodies.iter().filter(|b| b.get("status").and_then(Value::as_str) == Some(s)).count();

    if !best_of {
        let done = count("done");
        let status = if done == bodies.len() { "done" } else if done > 0 { "partial" } else { "failed" };
        let results: Vec<Value> = bodies
            .iter()
            .zip(&planned)
            .enumerate()
            .map(|(i, (b, p))| {
                let mut r = b.clone();
                r["subtask"] = json!(i + 1);
                r["computer"] = json!(p.computer.id);
                r
            })
            .collect();
        return ok_result(&json!({
            "status": status,
            "done": done,
            "subtasks": bodies.len(),
            "max_parallel": lim.max_parallel,
            "elapsed_ms": elapsed,
            "cost_usd": (spent * 1e6).round() / 1e6,
            "results": results,
            "next": "Each result is one run_subtask result on its computer. Continue the ones that escalated from the screen they return.",
        }));
    }

    // Best-of-N: judge the rollouts by their narratives.
    let goal = req.input.get("goal").and_then(Value::as_str).unwrap_or("").to_string();
    let narratives: Vec<String> = bodies.iter().zip(&planned).enumerate().map(|(i, (b, p))| narrative(&format!("r{}", i + 1), &p.computer.id, b)).collect();
    let fast = crate::computer_subtask::fast_backends();
    let allow_oracle = spent < lim.cost_cap;
    let (winner, why, judged) = choose(&goal, &narratives, &bodies, &fast, allow_oracle, |mut body| {
        body["session_id"] = json!(parent_ref);
        async move { crate::agency_api::decisions::decide_value(state, user, body).await.unwrap_or_else(|e| json!({ "abstained": true, "error": e })) }
    })
    .await;
    let judge_cost: f64 = judged
        .iter()
        .flat_map(|r| r.get("attempts").and_then(Value::as_array).cloned().unwrap_or_default())
        .filter_map(|a| a.pointer("/detail/cost_usd").and_then(Value::as_f64))
        .sum();
    // The judge's outcome is known now: its pick ended done.
    for r in &judged {
        if let (Some(id), Some(choice)) = (r.get("id").and_then(Value::as_str), r.get("choice").and_then(Value::as_str)) {
            let st = if winner.is_some_and(|w| choice == format!("r{}", w + 1)) { "success" } else { "skipped" };
            let _ = crate::agency_api::decisions::record_outcome(state, user, id, st, Some(choice), Some("best-of-N judge"));
        }
    }
    let rollouts: Vec<Value> = bodies
        .iter()
        .zip(&planned)
        .zip(&narratives)
        .enumerate()
        .map(|(i, ((b, p), n))| json!({ "rollout": format!("r{}", i + 1), "computer": p.computer.id, "status": b.get("status"), "narrative": n, "result": b }))
        .collect();
    let chosen = winner.map(|w| json!({ "rollout": format!("r{}", w + 1), "computer": planned[w].computer.id, "why": why }));
    ok_result(&json!({
        "status": if winner.is_some() { "done" } else { "escalated" },
        "best_of": bodies.len(),
        "chosen": chosen,
        "reason": if winner.is_some() { Value::Null } else { json!(why) },
        "judge_decisions": judged.len(),
        "elapsed_ms": elapsed,
        "cost_usd": ((spent + judge_cost) * 1e6).round() / 1e6,
        "rollouts": rollouts,
        "next": if winner.is_some() {
            "Continue on the chosen rollout's computer; the others are left as they ended."
        } else {
            "No rollout was chosen. Read the narratives and continue on the computer whose rollout got closest, or retry with a narrower goal."
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn two_subtasks_on_two_computers_run_concurrently() {
        let input = json!({ "subtasks": [{ "computer": "cloud-a", "goal": "a" }, { "computer": "cloud-b", "goal": "b" }] });
        let jobs = parallel_jobs(&input, "this-device").unwrap();
        assert_eq!(jobs.iter().map(|j| j.computer.as_str()).collect::<Vec<_>>(), ["cloud-a", "cloud-b"]);
        assert_eq!(jobs[0].input, json!({ "goal": "a" }), "computer is not a run_subtask field");
        let (live, peak) = (AtomicUsize::new(0), AtomicUsize::new(0));
        let lim = limits(&input);
        let started = Instant::now();
        let out = fan_out(jobs.len(), &lim, |i, _left| {
            let (live, peak, job) = (&live, &peak, &jobs[i]);
            async move {
                // Each "subtask" holds its computer's input while it runs.
                let _owner = claim_input(&format!("test-par-{}", job.computer), "r").expect("distinct computers");
                let now = live.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(150)).await;
                live.fetch_sub(1, Ordering::SeqCst);
                json!({ "status": "done", "goal": job.input["goal"], "oracle": { "cost_usd": 0.0 } })
            }
        })
        .await;
        assert_eq!(peak.load(Ordering::SeqCst), 2, "both ran at the same time");
        assert!(started.elapsed() < Duration::from_millis(290), "concurrent, not sequential: {:?}", started.elapsed());
        assert_eq!(out[0]["goal"], "a");
        assert_eq!(out[1]["goal"], "b");
    }

    #[tokio::test]
    async fn fan_out_honours_max_parallel_and_the_cost_cap() {
        let lim = Limits { max_parallel: 1, budget: Duration::from_secs(30), cost_cap: 0.05 };
        let peak = AtomicUsize::new(0);
        let live = AtomicUsize::new(0);
        let out = fan_out(3, &lim, |_, _| {
            let (live, peak) = (&live, &peak);
            async move {
                let now = live.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                tokio::task::yield_now().await;
                live.fetch_sub(1, Ordering::SeqCst);
                json!({ "status": "done", "oracle": { "cost_usd": 0.06 } })
            }
        })
        .await;
        assert_eq!(peak.load(Ordering::SeqCst), 1);
        assert_eq!(out[0]["status"], "done");
        assert_eq!(out[1]["status"], "skipped", "the first one spent past the cap");
        assert_eq!(out[2]["status"], "skipped");
        // A spent call budget skips what hasn't started.
        let none = Limits { max_parallel: 2, budget: Duration::from_millis(0), cost_cap: 1.0 };
        let out = fan_out(1, &none, |_, _| async { json!({ "status": "done" }) }).await;
        assert_eq!(out[0]["status"], "skipped");
        // Subtask budgets are cut to what's left.
        assert_eq!(within_budget(&json!({ "goal": "x" }), Duration::from_millis(5_000))["budget_ms"], 5_000);
        assert_eq!(within_budget(&json!({ "goal": "x", "budget_ms": 2_000 }), Duration::from_secs(90))["budget_ms"], 2_000);
    }

    #[test]
    fn one_computer_never_gets_two_input_owners() {
        // In one call: a repeated (resolved) computer refuses the whole call.
        assert!(one_owner_each(&["c1".into(), "c2".into()]).is_ok());
        let e = one_owner_each(&["c1".into(), "c1".into()]).unwrap_err();
        assert!(e.contains("c1") && e.contains("one subtask at a time"));
        // Across calls: the registry refuses a second loop until the first ends.
        let first = claim_input("test-lease-c1", "run-a").unwrap();
        assert_eq!(claim_input("test-lease-c1", "run-b").err().as_deref(), Some("run-a"));
        assert!(claim_input("test-lease-c2", "run-b").is_ok(), "another computer is free");
        drop(first);
        assert!(claim_input("test-lease-c1", "run-b").is_ok(), "released on drop");
    }

    #[test]
    fn guest_leases_refuse_a_second_agent() {
        use crate::computer_control_lease::{Holder, HolderKind};
        use crate::computer_toolset::{lease_gate, LeaseRefusal};
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE computer_control_leases (computer_id TEXT PRIMARY KEY, holder_kind TEXT NOT NULL, holder_id TEXT NOT NULL,
               holder_label TEXT, device_id TEXT, acquired_at TEXT NOT NULL, expires_at TEXT NOT NULL);",
        )
        .unwrap();
        let agent = |id: &str| Holder { kind: HolderKind::Agent, id: id.into(), label: Some("Agent".into()), device_id: None };
        let now = chrono::Utc::now().timestamp();
        assert_eq!(lease_gate(&conn, "cloud-a", false, "u1", &agent("p:s1"), now), Ok(()));
        assert!(matches!(lease_gate(&conn, "cloud-a", false, "u1", &agent("p:s2"), now), Err(LeaseRefusal::Held(_))));
        assert_eq!(lease_gate(&conn, "cloud-b", false, "u1", &agent("p:s2"), now), Ok(()));
    }

    #[test]
    fn best_of_needs_distinct_sandbox_computers() {
        let jobs = best_of_jobs(&json!({ "goal": "g", "best_of": 2, "computers": ["a", "b", "c"], "max_cost_usd": 1.0 })).unwrap();
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[1].input, json!({ "goal": "g" }));
        assert!(best_of_jobs(&json!({ "goal": "g", "best_of": 3, "computers": ["a", "a", "b"] })).unwrap_err().contains("distinct"));
        assert!(best_of_jobs(&json!({ "goal": "g", "best_of": 9, "computers": ["a"] })).is_err());
        assert!(best_of_jobs(&json!({ "goal": "g", "best_of": 1, "computers": ["a"] })).is_err());
        // Never on the person's own machine.
        assert!(sandbox_only(&[("a".into(), "guest_linux"), ("b".into(), "guest_windows")]).is_ok());
        let e = sandbox_only(&[("a".into(), "guest_linux"), ("mine".into(), "this_device")]).unwrap_err();
        assert!(e.contains("mine") && e.contains("sandbox"));
        assert!(is_fanout("run_subtask", &json!({ "best_of": 2 })));
        assert!(!is_fanout("run_subtask", &json!({ "goal": "x" })));
        assert!(is_fanout("run_parallel", &json!({})));
    }

    fn rollout(status: &str, success: bool, steps: &[(&str, bool, bool)], shows: &str) -> Value {
        json!({
            "status": status,
            "success": success,
            "steps": steps.iter().map(|(o, ok, ch)| json!({ "option": o, "action": { "ok": ok, "changed": ch } })).collect::<Vec<_>>(),
            "screen": { "elements": [{ "name": shows }] },
        })
    }

    #[tokio::test]
    async fn judge_picks_by_the_step_narratives() {
        let bodies = vec![
            rollout("done", true, &[("click the button \"Submit\"", true, true)], "Error: email missing"),
            rollout("done", true, &[("type the email into the text field \"Email\"", true, true), ("click the button \"Submit\"", true, true)], "Thanks, Ada"),
            rollout("escalated", false, &[("click the button \"Help\"", true, false)], "Help"),
        ];
        let narratives: Vec<String> = bodies.iter().enumerate().map(|(i, b)| narrative(&format!("r{}", i + 1), &format!("cloud-{i}"), b)).collect();
        assert!(narratives[1].contains("2. click the button \"Submit\" -> ran, the screen changed"));
        assert!(narratives[1].contains("Final screen shows: Thanks, Ada"));
        assert!(narratives[2].contains("the screen didn't change"));
        assert_eq!(shortlist(&bodies), vec![0, 1], "the escalated rollout isn't a candidate");
        // A mock judge that reads the narratives in the context and picks
        // the rollout whose final screen shows the thank-you text.
        let mut seen = Vec::new();
        let (w, why, replies) = choose("Sign up as Ada", &narratives, &bodies, &["local"], true, |body| {
            seen.push(body.clone());
            async move {
                let ctx = body["context"].as_str().unwrap().to_string();
                let pick = ctx.split("Rollout ").skip(1).find(|n| n.contains("Thanks, Ada")).and_then(|n| n.split_whitespace().next()).unwrap().to_string();
                json!({ "id": "d1", "choice": pick, "abstained": false, "confidence": 0.91, "backend": "local" })
            }
        })
        .await;
        assert_eq!(w, Some(1));
        assert!(why.contains("r2") && why.contains("narratives"), "{why}");
        assert_eq!(replies.len(), 1, "fast tier answered; no oracle call");
        assert_eq!(seen[0]["kind"], "judge");
        assert_eq!(seen[0]["options"].as_array().unwrap().len(), 2);
        assert!(!seen[0]["context"].as_str().unwrap().contains("Help"), "only candidates are judged");
    }

    #[tokio::test]
    async fn judge_escalates_to_the_planner_tier_then_hands_back() {
        let bodies = vec![rollout("done", true, &[], "A"), rollout("done", true, &[], "B")];
        let narratives: Vec<String> = bodies.iter().enumerate().map(|(i, b)| narrative(&format!("r{}", i + 1), "c", b)).collect();
        let mut tiers = Vec::new();
        let (w, _, _) = choose("g", &narratives, &bodies, &["local"], true, |body| {
            tiers.push(body["backends"].clone());
            let oracle = body["backends"] == json!(["oracle"]);
            async move {
                if oracle {
                    json!({ "choice": "r1", "abstained": false, "confidence": 0.8, "backend": "oracle" })
                } else {
                    json!({ "abstained": true })
                }
            }
        })
        .await;
        assert_eq!(w, Some(0));
        assert_eq!(tiers, vec![json!(["local"]), json!(["oracle"])]);
        // Over the cost cap: no oracle; nobody picked, the planner decides.
        let (w, why, _) = choose("g", &narratives, &bodies, &["local"], false, |_| async { json!({ "abstained": true }) }).await;
        assert_eq!(w, None);
        assert!(why.contains("couldn't separate"));
        // One verified rollout wins with no judge call at all.
        let one = vec![rollout("done", true, &[], "A"), rollout("escalated", false, &[], "B")];
        let (w, why, replies) = choose("g", &narratives, &one, &["local"], true, |_: Value| std::future::ready::<Value>(unreachable!("one verified rollout needs no judge"))).await;
        assert_eq!((w, replies.len()), (Some(0), 0));
        assert!(why.contains("only rollout"));
    }
}
