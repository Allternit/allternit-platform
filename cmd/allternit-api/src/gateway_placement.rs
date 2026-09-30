//! Capability-aware Coordinator placement (spec agent-gateway.md,
//! "Coordinator placement" + "Context isolation is a routing rule").
//!
//! No vendor-name branches: everything is decided from the Bot's execution
//! binding state, its capability snapshot (`capabilities_json`, incl.
//! `context.maxParallel` / `context.isolation`), the count of ACTIVE remote
//! contexts, and policy flags.
//!
//! candidates = status permits work, binding READY (or native),
//!   required capabilities subset of capabilities, parallel capacity greater
//!   than active remote contexts, policy / kill switch allow.
//! score = roleFit + capabilityFit + availableCapacity - degradedLanePenalty
//!   (historicalPerformance, locality, costFit, vendorPreference, riskPenalty
//!   are stubbed at 0 below with TODO names).
//!
//! The planner's own choice is kept when it is a candidate; otherwise the best
//! scoring alternate is used; if the only problem is capacity/isolation the
//! step is serialized behind the step that holds the context. Every rejection
//! carries a plain-language reason ("why not chosen").

use std::collections::{HashMap, HashSet};

use rusqlite::params;
use serde_json::{json, Value};

use crate::coordinator_routes::{TeamBot, ValidStep};
use crate::db::DbHandle;

/// Default parallel capacity of a bot with no declared `context.maxParallel`.
const NATIVE_CAPACITY: usize = 64;
const VENDOR_DEFAULT_CAPACITY: usize = 1;

#[derive(Debug, Clone, PartialEq)]
pub struct BotFacts {
    pub bot_id: String,
    pub name: String,
    pub about: String,
    /// agents.status
    pub status: String,
    /// `None` = native bot with no execution binding.
    pub binding_state: Option<String>,
    pub degraded: bool,
    pub capabilities: Value,
    /// ACTIVE remote_thread_bindings for this bot.
    pub active_contexts: usize,
    pub killed: bool,
}

impl BotFacts {
    fn max_parallel(&self) -> usize {
        let declared = self.capabilities.pointer("/context/maxParallel").and_then(Value::as_u64).map(|n| n as usize);
        match (declared, self.binding_state.is_some()) {
            (Some(n), _) => n,
            (None, true) => VENDOR_DEFAULT_CAPACITY,
            (None, false) => NATIVE_CAPACITY,
        }
    }
    /// `shared`: every Thread of the bot would land in one remote context.
    fn shared(&self) -> bool {
        self.capabilities.pointer("/context/isolation").and_then(Value::as_str) == Some("shared")
    }
    /// Effective simultaneous capacity, after the isolation rule.
    pub fn capacity(&self) -> usize {
        if self.shared() { self.max_parallel().min(1) } else { self.max_parallel() }
    }
}

/// True when the capability snapshot offers `name`: `{name: true}`,
/// `{name: {supported: true}}`, or an entry in a `features` array.
pub fn has_capability(caps: &Value, name: &str) -> bool {
    match caps.get(name) {
        Some(Value::Bool(b)) => return *b,
        Some(Value::Object(o)) => return o.get("supported").and_then(Value::as_bool).unwrap_or(true),
        Some(Value::Null) | None => {}
        Some(_) => return true,
    }
    caps.get("features").and_then(Value::as_array).map(|a| a.iter().any(|v| v.as_str() == Some(name))).unwrap_or(false)
}

fn words(s: &str) -> HashSet<String> {
    s.split(|c: char| !c.is_alphanumeric()).map(|w| w.to_lowercase()).filter(|w| w.len() > 3).collect()
}

/// Fraction of the step's words that appear in the bot's name/role text.
fn role_fit(step: &ValidStep, b: &BotFacts) -> f64 {
    let want = words(&format!("{} {}", step.title, step.objective));
    if want.is_empty() {
        return 0.0;
    }
    let have = words(&format!("{} {}", b.name, b.about));
    want.intersection(&have).count() as f64 / want.len() as f64
}

/// Share of the bot's declared capabilities that this step touches (0 when it
/// asks for nothing). A bot that covers every requirement scores 1.
fn capability_fit(step: &ValidStep, b: &BotFacts) -> f64 {
    if step.requires.is_empty() {
        return 0.0;
    }
    step.requires.iter().filter(|r| has_capability(&b.capabilities, r)).count() as f64 / step.requires.len() as f64
}

pub fn score(step: &ValidStep, b: &BotFacts, in_flight: usize) -> f64 {
    let available = b.capacity().saturating_sub(b.active_contexts + in_flight) as f64;
    let available_capacity = (available / b.capacity().max(1) as f64).min(1.0) * 0.5;
    let degraded_lane_penalty = if b.degraded { 0.5 } else { 0.0 };
    let historical_performance = 0.0; // TODO(historicalPerformance)
    let locality_latency = 0.0; // TODO(localityLatency)
    let cost_fit = 0.0; // TODO(costFit)
    let vendor_preference = 0.0; // TODO(vendorPreference)
    let risk_penalty = 0.0; // TODO(riskPenalty)
    role_fit(step, b) + capability_fit(step, b) + historical_performance + locality_latency + cost_fit + vendor_preference
        + available_capacity
        - risk_penalty
        - degraded_lane_penalty
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Verdict {
    Ok,
    /// Only capacity/isolation stands in the way: serializing fixes it.
    Busy,
    Hard,
}

/// Filter. Returns the verdict and the "why not chosen" reasons.
fn check(step: &ValidStep, b: &BotFacts, in_flight: usize) -> (Verdict, Vec<String>) {
    let mut hard = Vec::new();
    if b.killed {
        hard.push("kill switch or policy blocks this bot".to_string());
    }
    if matches!(b.status.as_str(), "disabled" | "archived" | "paused" | "error") {
        hard.push(format!("bot status is {}", b.status));
    }
    match b.binding_state.as_deref() {
        None | Some("READY") => {}
        Some("DEGRADED") if !step.requires.iter().any(|r| r == "consequential") => {}
        Some(other) => hard.push(format!("execution binding is {other}, not READY")),
    }
    for r in step.requires.iter().filter(|r| !has_capability(&b.capabilities, r)) {
        hard.push(format!("missing capability {r}"));
    }
    if !hard.is_empty() {
        return (Verdict::Hard, hard);
    }
    let used = b.active_contexts + in_flight;
    if used >= b.capacity() {
        let why = if b.shared() && b.max_parallel() > 1 {
            format!("isolation is shared: one remote context, already in use ({used} active); Threads would contaminate each other")
        } else {
            format!("parallel capacity {} reached ({used} active remote contexts)", b.capacity())
        };
        return (Verdict::Busy, vec![why]);
    }
    (Verdict::Ok, vec![])
}

#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub key: String,
    pub bot_id: String,
    /// Planner's original pick when the step was moved.
    pub moved_from: Option<String>,
    /// Step key it now waits for (serialized behind a busy context).
    pub serialized_behind: Option<String>,
    pub placed: bool,
    pub score: f64,
    /// bot id -> reasons it was not chosen.
    pub not_chosen: Vec<(String, Vec<String>)>,
}

impl Note {
    pub fn to_json(&self) -> Value {
        json!({
            "key": self.key, "botId": self.bot_id, "movedFrom": self.moved_from,
            "serializedBehind": self.serialized_behind, "placed": self.placed, "score": self.score,
            "notChosen": self.not_chosen.iter().map(|(b, r)| json!({"botId": b, "reasons": r})).collect::<Vec<_>>(),
        })
    }
}

fn ancestors(steps: &[ValidStep], key: &str) -> HashSet<String> {
    let by: HashMap<&str, &ValidStep> = steps.iter().map(|s| (s.key.as_str(), s)).collect();
    let mut out = HashSet::new();
    let mut stack = vec![key.to_string()];
    while let Some(k) = stack.pop() {
        if let Some(s) = by.get(k.as_str()) {
            for d in &s.depends_on {
                if out.insert(d.clone()) {
                    stack.push(d.clone());
                }
            }
        }
    }
    out
}

/// Place every step (in plan order) on a team bot. Mutates `bot_id` and
/// `depends_on`; never fails - a step nothing can host stays on the planner's
/// bot with `placed: false` and reasons, so the runner surfaces it honestly.
pub fn place_plan(steps: &mut [ValidStep], team: &[BotFacts]) -> Vec<Note> {
    let mut notes = Vec::new();
    for i in 0..steps.len() {
        let key = steps[i].key.clone();
        let anc = ancestors(steps, &key);
        // Steps already placed that may run at the same time as this one.
        let concurrent = |bot: &str, steps: &[ValidStep]| -> Vec<String> {
            steps[..i].iter().filter(|p| p.bot_id == bot && !anc.contains(&p.key)).map(|p| p.key.clone()).collect()
        };
        let planned = steps[i].bot_id.clone();
        let mut ok: Vec<(&BotFacts, f64)> = Vec::new();
        let mut busy: Vec<(&BotFacts, Vec<String>)> = Vec::new();
        let mut not_chosen: Vec<(String, Vec<String>)> = Vec::new();
        for b in team {
            let inflight = concurrent(&b.bot_id, steps).len();
            match check(&steps[i], b, inflight) {
                (Verdict::Ok, _) => ok.push((b, score(&steps[i], b, inflight))),
                (Verdict::Busy, why) => busy.push((b, why)),
                (Verdict::Hard, why) => not_chosen.push((b.bot_id.clone(), why)),
            }
        }
        let pick = ok
            .iter()
            .find(|(b, _)| b.bot_id == planned)
            .or_else(|| ok.iter().max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)))
            .map(|(b, s)| (b.bot_id.clone(), *s));
        let mut note = Note { key: key.clone(), bot_id: planned.clone(), moved_from: None, serialized_behind: None, placed: true, score: 0.0, not_chosen: vec![] };
        if let Some((bot, s)) = pick {
            for (b, _) in ok.iter().filter(|(b, _)| b.bot_id != bot) {
                not_chosen.push((b.bot_id.clone(), vec!["scored lower".into()]));
            }
            for (b, why) in &busy {
                not_chosen.push((b.bot_id.clone(), why.clone()));
            }
            if bot != planned {
                note.moved_from = Some(planned.clone());
                steps[i].bot_id = bot.clone();
            }
            note.bot_id = bot;
            note.score = s;
        } else if let Some((b, why)) = busy
            .iter()
            .max_by(|a, b| {
                let sa = score(&steps[i], a.0, 0);
                let sb = score(&steps[i], b.0, 0);
                // Prefer the planner's bot, then the better score.
                (a.0.bot_id == planned).cmp(&(b.0.bot_id == planned)).then(sa.partial_cmp(&sb).unwrap_or(std::cmp::Ordering::Equal))
            })
            .cloned()
        {
            // Serialize behind the earliest concurrent step on that bot, if the
            // limit comes from this plan; a context busy from elsewhere just waits.
            let blocker = concurrent(&b.bot_id, steps).into_iter().next();
            if b.bot_id != planned {
                note.moved_from = Some(planned.clone());
                steps[i].bot_id = b.bot_id.clone();
            }
            note.bot_id = b.bot_id.clone();
            if let Some(bk) = blocker {
                if !steps[i].depends_on.contains(&bk) {
                    steps[i].depends_on.push(bk.clone());
                }
                note.serialized_behind = Some(bk);
            }
            note.not_chosen.push((b.bot_id.clone(), why));
        } else {
            note.placed = false;
        }
        note.not_chosen.extend(not_chosen);
        notes.push(note);
    }
    notes
}

/// Load the facts for the team's bots from the gateway tables.
pub fn load_facts(db: &DbHandle, user_id: &str, team: &[TeamBot]) -> Vec<BotFacts> {
    let Ok(conn) = db.connect() else { return vec![] };
    team.iter()
        .map(|t| {
            let (status, native_caps, cfg): (String, Option<String>, Option<String>) = conn
                .query_row("SELECT COALESCE(status,'idle'), capabilities, config FROM agents WHERE id = ?1", params![t.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .unwrap_or_else(|_| ("idle".into(), None, None));
            let cfg: Value = cfg.and_then(|c| serde_json::from_str(&c).ok()).unwrap_or(json!({}));
            let binding: Option<(String, String, String)> = conn
                .query_row(
                    "SELECT state, capabilities_json, health_json FROM bot_execution_bindings WHERE bot_id = ?1 AND owner = ?2",
                    params![t.id, user_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .ok();
            let active: i64 = conn
                .query_row("SELECT COUNT(*) FROM remote_thread_bindings WHERE bot_id = ?1 AND state = 'ACTIVE'", params![t.id], |r| r.get(0))
                .unwrap_or(0);
            let (binding_state, capabilities, health) = match binding {
                Some((s, c, h)) => (
                    Some(s),
                    serde_json::from_str(&c).unwrap_or(json!({})),
                    serde_json::from_str::<Value>(&h).unwrap_or(json!({})),
                ),
                None => (None, native_caps.and_then(|c| serde_json::from_str(&c).ok()).unwrap_or(json!({})), json!({})),
            };
            let killed = cfg.pointer("/gatewayPolicy/killSwitch").and_then(Value::as_bool).unwrap_or(false)
                || health.get("killSwitch").and_then(Value::as_bool).unwrap_or(false);
            let degraded = health.get("status").and_then(Value::as_str) == Some("degraded")
                || binding_state.as_deref() == Some("DEGRADED");
            BotFacts {
                bot_id: t.id.clone(),
                name: t.name.clone(),
                about: t.about.clone(),
                status,
                binding_state,
                degraded,
                capabilities,
                active_contexts: active as usize,
                killed,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bot(id: &str, about: &str, state: Option<&str>, caps: Value) -> BotFacts {
        BotFacts { bot_id: id.into(), name: id.into(), about: about.into(), status: "idle".into(), binding_state: state.map(Into::into), degraded: false, capabilities: caps, active_contexts: 0, killed: false }
    }
    fn step(key: &str, bot: &str, deps: &[&str], requires: &[&str]) -> ValidStep {
        ValidStep {
            key: key.into(), title: format!("research {key}"), objective: String::new(), bot_id: bot.into(),
            depends_on: deps.iter().map(|s| s.to_string()).collect(), todo: vec![], budget_usd: None,
            requires: requires.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn keeps_planner_pick_when_it_qualifies() {
        let team = [bot("a", "", Some("READY"), json!({"context": {"maxParallel": 3, "isolation": "isolated"}})), bot("b", "research", None, json!({}))];
        let mut s = vec![step("x", "a", &[], &[])];
        let n = place_plan(&mut s, &team);
        assert_eq!(s[0].bot_id, "a");
        assert!(n[0].placed && n[0].moved_from.is_none());
    }

    #[test]
    fn missing_capability_and_unready_binding_move_the_step_with_reasons() {
        let team = [bot("a", "", Some("AUTH_EXPIRED"), json!({"web": true})), bot("b", "", Some("READY"), json!({"web": false})), bot("c", "", Some("READY"), json!({"web": {"supported": true}}))];
        let mut s = vec![step("x", "a", &[], &["web"])];
        let n = place_plan(&mut s, &team);
        assert_eq!(s[0].bot_id, "c");
        assert_eq!(n[0].moved_from.as_deref(), Some("a"));
        let why = |id: &str| n[0].not_chosen.iter().find(|(b, _)| b == id).unwrap().1.join(";");
        assert!(why("a").contains("AUTH_EXPIRED"));
        assert!(why("b").contains("missing capability web"));
    }

    #[test]
    fn kill_switch_and_status_block() {
        let mut a = bot("a", "", None, json!({}));
        a.killed = true;
        let mut b = bot("b", "", None, json!({}));
        b.status = "paused".into();
        let mut s = vec![step("x", "a", &[], &[])];
        let n = place_plan(&mut s, &[a, b]);
        assert!(!n[0].placed);
        assert_eq!(s[0].bot_id, "a");
        assert_eq!(n[0].not_chosen.len(), 2);
    }

    #[test]
    fn active_remote_contexts_count_against_max_parallel() {
        let mut a = bot("a", "", Some("READY"), json!({"context": {"maxParallel": 2, "isolation": "isolated"}}));
        a.active_contexts = 2;
        let b = bot("b", "", None, json!({}));
        let mut s = vec![step("x", "a", &[], &[])];
        let n = place_plan(&mut s, &[a, b]);
        assert_eq!(s[0].bot_id, "b");
        assert!(n[0].not_chosen.iter().any(|(id, r)| id == "a" && r[0].contains("capacity 2")));
    }

    #[test]
    fn parallel_steps_beyond_capacity_serialize_when_no_alternate() {
        let team = [bot("a", "", Some("READY"), json!({"context": {"maxParallel": 2, "isolation": "isolated"}}))];
        let mut s = vec![step("1", "a", &[], &[]), step("2", "a", &[], &[]), step("3", "a", &[], &[])];
        let n = place_plan(&mut s, &team);
        assert!(s[1].depends_on.is_empty());
        assert_eq!(s[2].depends_on, vec!["1".to_string()]);
        assert_eq!(n[2].serialized_behind.as_deref(), Some("1"));
    }

    #[test]
    fn shared_isolation_never_runs_two_threads_in_one_context() {
        // maxParallel says 5, but shared isolation means one context.
        let shared = bot("s", "", Some("READY"), json!({"context": {"maxParallel": 5, "isolation": "shared"}}));
        let mut s = vec![step("1", "s", &[], &[]), step("2", "s", &[], &[])];
        let n = place_plan(&mut s, &[shared.clone()]);
        assert_eq!(s[1].depends_on, vec!["1".to_string()]);
        assert!(n[1].not_chosen[0].1[0].contains("isolation is shared"));
        // With an alternate, the second thread moves instead of waiting.
        let alt = bot("n", "", None, json!({}));
        let mut s = vec![step("1", "s", &[], &[]), step("2", "s", &[], &[])];
        place_plan(&mut s, &[shared, alt]);
        assert_eq!((s[0].bot_id.as_str(), s[1].bot_id.as_str()), ("s", "n"));
        assert!(s[1].depends_on.is_empty());
    }

    #[test]
    fn dependent_steps_reuse_the_context_sequentially() {
        let shared = bot("s", "", Some("READY"), json!({"context": {"isolation": "shared"}}));
        let mut s = vec![step("1", "s", &[], &[]), step("2", "s", &["1"], &[])];
        let n = place_plan(&mut s, &[shared]);
        assert!(n.iter().all(|x| x.serialized_behind.is_none()));
    }

    #[test]
    fn score_prefers_role_fit_and_penalizes_degraded_lane() {
        let mut deg = bot("d", "research analyst", Some("DEGRADED"), json!({}));
        deg.degraded = true;
        let good = bot("g", "research analyst", Some("READY"), json!({"context": {"maxParallel": 4}}));
        let st = step("x", "d", &[], &[]);
        assert!(score(&st, &good, 0) > score(&st, &deg, 0));
    }
}
