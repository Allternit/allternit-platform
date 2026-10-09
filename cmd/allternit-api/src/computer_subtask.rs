//! `run_subtask` (allternit.computer.v2): computer use on typed decisions
//! (decision-runtime spec §2.2, phase E2).
//!
//! The planner model hands over one bounded subtask — a goal, the literal
//! inputs it may type, success checks in `verify` syntax, constraints and
//! limits — and this loop runs it without another planner call:
//!
//! 1. read the element map with `read_ui` (narrow: `query`, `max_elements`,
//!    then `since` diffs patched into the local copy);
//! 2. build the options: visible actionable elements x allowed ops, plus
//!    `type <input> into <field>` for each literal input, plus `done` and
//!    `escalate`;
//! 3. one `/v1/decisions` call (kind `element`) picks an option. Unfilled
//!    inputs first get one `field` decision each, concurrently;
//! 4. the choice runs through the same lease → policy → audit path as every
//!    toolset call (`computer_toolset::execute_step`). Predictable steps
//!    (typing a literal into a field: the outcome is the field's value) run
//!    as one `run_batch` with an `expect` per step, UFO2's speculative
//!    multi-action: the batch stops at the first failed check;
//! 5. the success checks run with `verify`; `{ask}` checks are `verify`-kind
//!    decisions over the screen;
//! 6. repeat until done, escalate, `max_steps` or `budget_ms`.
//!
//! A decision that abstains or falls under its kind's threshold hands
//! control back to the planner with a compact state — never a blind loop.
//! Every decision's outcome is written back (the `PATCH /v1/decisions/:id`
//! store) for the E5 flywheel.
//!
//! The person approves the subtask once (its goal and literal inputs, on
//! non-sandbox targets); its steps carry that grant and cannot type anything
//! that isn't one of the inputs.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use crate::auth::AuthUser;
use crate::computer_routes::ComputerResponse;
use crate::computer_toolset::{execute_step, screen_info, text, Fail, Target, ToolsetResult};
use crate::AppState;

const DEFAULT_MAX_STEPS: u64 = 12;
const MAX_MAX_STEPS: u64 = 40;
const DEFAULT_BUDGET_MS: u64 = 60_000;
const MAX_BUDGET_MS: u64 = 300_000;
const DEFAULT_MAX_ELEMENTS: u64 = 300;
/// Options per decision, leaving room for done / escalate (the runtime takes 255).
const MAX_ELEMENT_OPTIONS: usize = 240;
/// One decision's latency budget cap. The loop runs on the fast tiers only.
const DECISION_BUDGET_CAP_MS: u64 = 8_000;

/// The fast decision tiers: the configured chain without the oracle. The
/// oracle IS a planner-class model call; a subtask that would need it hands
/// control back to the planner instead (with a compact state), which is the
/// point of the loop. Empty when no fast backend is configured: then every
/// step hands back.
fn fast_backends() -> Vec<&'static str> {
    crate::agency_api::decisions::backends::chain().iter().map(|b| b.name()).filter(|n| *n != "oracle").collect()
}
/// Elements named like this are irreversible and hand back to the planner
/// unless `constraints.allow_irreversible` (driver spec D5).
const IRREVERSIBLE: [&str; 14] = [
    "delete", "remove", "erase", "send", "pay", "purchase", "buy", "place order", "publish", "transfer", "unsubscribe", "sign out",
    "log out", "empty trash",
];
const DEFAULT_OPS: [&str; 4] = ["click", "set_value", "select", "press"];

/// The subtask as the planner gave it.
struct Spec {
    goal: String,
    inputs: Vec<(String, String)>,
    exact: Vec<Value>,
    fuzzy: Vec<String>,
    ops: HashSet<String>,
    avoid: Vec<String>,
    allow_irreversible: bool,
    max_steps: u64,
    budget: Duration,
    scope: Map<String, Value>,
    query: Option<String>,
    max_elements: u64,
}

fn parse(input: &Value) -> Result<Spec, Fail> {
    let goal = input.get("goal").and_then(Value::as_str).map(str::trim).unwrap_or_default().to_string();
    if goal.is_empty() {
        return Err(Fail::from("run_subtask needs a goal"));
    }
    let inputs: Vec<(String, String)> = input
        .get("inputs")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|i| Some((i.get("name")?.as_str()?.trim().to_string(), i.get("value")?.as_str()?.to_string())))
                .filter(|(n, _)| !n.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let mut seen = HashSet::new();
    if let Some((dup, _)) = inputs.iter().find(|(n, _)| !seen.insert(n.to_lowercase())) {
        return Err(Fail::from(format!("run_subtask input names must be unique ({dup} repeats)")));
    }
    let (mut exact, mut fuzzy) = (Vec::new(), Vec::new());
    for check in input.get("success").and_then(Value::as_array).into_iter().flatten() {
        let mut c = check.as_object().cloned().unwrap_or_default();
        if let Some(ask) = c.remove("ask").and_then(|a| a.as_str().map(str::to_string)) {
            if !ask.trim().is_empty() {
                fuzzy.push(ask);
            }
        }
        if !c.is_empty() {
            exact.push(Value::Object(c));
        }
    }
    let constraints = input.get("constraints").cloned().unwrap_or_else(|| json!({}));
    let ops: HashSet<String> = match constraints.get("ops").and_then(Value::as_array) {
        Some(a) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        None => DEFAULT_OPS.iter().map(|s| s.to_string()).collect(),
    };
    let avoid = constraints
        .get("avoid")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();
    let mut scope = Map::new();
    for k in ["app", "pid", "window_id"] {
        if let Some(v) = input.get(k) {
            scope.insert(k.into(), v.clone());
        }
    }
    let num = |k: &str| input.get(k).and_then(Value::as_u64);
    Ok(Spec {
        goal,
        inputs,
        exact,
        fuzzy,
        ops,
        avoid,
        allow_irreversible: constraints.get("allow_irreversible").and_then(Value::as_bool).unwrap_or(false),
        max_steps: num("max_steps").unwrap_or(DEFAULT_MAX_STEPS).clamp(1, MAX_MAX_STEPS),
        budget: Duration::from_millis(num("budget_ms").unwrap_or(DEFAULT_BUDGET_MS).clamp(1_000, MAX_BUDGET_MS)),
        scope,
        query: input.get("query").and_then(Value::as_str).map(str::to_string).filter(|q| !q.trim().is_empty()),
        max_elements: num("max_elements").unwrap_or(DEFAULT_MAX_ELEMENTS).clamp(10, 1_000),
    })
}

// ---------------------------------------------------------------------------
// The element map, patched from `since` diffs.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Screen {
    version: Option<i64>,
    window: Value,
    order: Vec<String>,
    by_id: HashMap<String, Value>,
}

impl Screen {
    /// Fold one read_ui reply in: a full element list replaces the map, a
    /// `diff` patches it.
    fn apply(&mut self, reply: &Value) {
        if let Some(v) = reply.get("version").and_then(Value::as_i64) {
            self.version = Some(v);
        }
        if let Some(w) = reply.get("window").filter(|w| w.is_object()) {
            self.window = w.clone();
        }
        if let Some(elements) = reply.get("elements").and_then(Value::as_array) {
            self.order.clear();
            self.by_id.clear();
            for e in elements {
                self.put(e);
            }
        } else if let Some(diff) = reply.get("diff") {
            for id in diff.get("removed").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
                self.by_id.remove(id);
            }
            self.order.retain(|id| self.by_id.contains_key(id));
            for key in ["changed", "added"] {
                for e in diff.get(key).and_then(Value::as_array).into_iter().flatten() {
                    self.put(e);
                }
            }
        }
    }

    fn put(&mut self, e: &Value) {
        let Some(id) = e.get("id").and_then(Value::as_str) else { return };
        if self.by_id.insert(id.to_string(), e.clone()).is_none() {
            self.order.push(id.to_string());
        }
    }

    fn elements(&self) -> impl Iterator<Item = &Value> {
        self.order.iter().filter_map(|id| self.by_id.get(id))
    }

    fn pid(&self) -> Option<i64> {
        self.window.get("pid").and_then(Value::as_i64)
    }

    /// The compact state the planner gets back on escalation.
    fn compact(&self, limit: usize) -> Value {
        let mut out: Vec<Value> = Vec::new();
        for e in self.elements() {
            if out.len() >= limit {
                break;
            }
            let role = role_of(e);
            if !(is_text_entry(&role) || is_clickable(e, &role) || !name_of(e).is_empty() || !value_of(e).is_empty()) {
                continue;
            }
            let mut c = Map::new();
            for k in ["id", "role", "name", "enabled", "focused"] {
                if let Some(v) = e.get(k) {
                    c.insert(k.into(), v.clone());
                }
            }
            let v = value_of(e);
            if !v.is_empty() {
                c.insert("value".into(), json!(clip(&v, 120)));
            }
            out.push(Value::Object(c));
        }
        json!({ "window": self.window, "version": self.version, "elements": out, "total": self.order.len() })
    }
}

fn role_of(e: &Value) -> String {
    let r = e.get("role").and_then(Value::as_str).unwrap_or_default();
    r.strip_prefix("AX").unwrap_or(r).to_lowercase().replace([' ', '_', '-'], "")
}

fn name_of(e: &Value) -> String {
    e.get("name").and_then(Value::as_str).map(|s| s.split_whitespace().collect::<Vec<_>>().join(" ")).unwrap_or_default()
}

fn value_of(e: &Value) -> String {
    match e.get("value") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

fn enabled(e: &Value) -> bool {
    e.get("enabled").and_then(Value::as_bool) != Some(false)
}

fn focused(e: &Value) -> bool {
    e.get("focused").and_then(Value::as_bool) == Some(true)
}

/// Text-entry roles across AX (macOS), UIA (Windows) and AT-SPI (Linux).
/// Secure fields are left out: secrets go through use_credential.
fn is_text_entry(role: &str) -> bool {
    matches!(role, "textfield" | "textarea" | "searchfield" | "combobox" | "edit" | "entry" | "text" | "document" | "spinbutton")
}

fn is_clickable(e: &Value, role: &str) -> bool {
    const ROLES: [&str; 15] = [
        "button", "checkbox", "radiobutton", "menuitem", "menubutton", "popupbutton", "link", "tab", "disclosuretriangle", "switch",
        "togglebutton", "listitem", "cell", "row", "segmentedcontrol",
    ];
    if role.contains("closebutton") || role.contains("minimizebutton") || role.contains("zoombutton") || role.contains("fullscreenbutton") {
        return false;
    }
    ROLES.contains(&role)
        || e.get("actions").and_then(Value::as_array).is_some_and(|a| {
            a.iter().filter_map(Value::as_str).any(|x| matches!(x.to_lowercase().trim_start_matches("ax"), "press" | "click" | "pick" | "activate"))
        })
}

fn pretty_role(role: &str) -> &str {
    match role {
        "textfield" | "edit" | "entry" => "text field",
        "textarea" => "text area",
        "searchfield" => "search field",
        "combobox" => "combo box",
        "checkbox" => "checkbox",
        "radiobutton" => "radio button",
        "popupbutton" => "pop-up button",
        "menuitem" => "menu item",
        other => other,
    }
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}

/// How a person would name the element: its name, or a nearby label.
fn label(e: &Value) -> String {
    let n = name_of(e);
    if !n.is_empty() {
        return clip(&n, 80);
    }
    let v = value_of(e);
    if !v.is_empty() {
        return format!("showing {}", clip(&v, 40));
    }
    e.get("id").and_then(Value::as_str).unwrap_or("?").to_string()
}

// ---------------------------------------------------------------------------
// Options.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum Action {
    Click { id: String },
    Type { id: String, input: usize },
    Press { id: String, key: String },
    Done,
    Escalate,
}

struct Opt {
    id: String,
    text: String,
    action: Action,
}

impl Spec {
    fn blocked(&self, e: &Value) -> Option<&'static str> {
        let n = name_of(e).to_lowercase();
        if self.avoid.iter().any(|a| n.contains(a)) {
            return Some("avoid");
        }
        if !self.allow_irreversible && IRREVERSIBLE.iter().any(|w| n.contains(w)) {
            return Some("irreversible");
        }
        None
    }
}

/// The step's options: actionable elements x allowed ops, `type <input>`
/// into each text field, then done and escalate. Option ids are short and
/// stable (`o1`…); the action stays on our side, so a decision can only pick
/// what the host built.
fn options(spec: &Spec, screen: &Screen, typed: &HashMap<usize, String>, spent: &HashSet<String>) -> Vec<Opt> {
    let mut out = Vec::new();
    let can = |op: &str| spec.ops.contains(op);
    let query = spec.query.as_deref().map(str::to_lowercase);
    for e in screen.elements() {
        if out.len() >= MAX_ELEMENT_OPTIONS {
            break;
        }
        if !enabled(e) || spec.blocked(e).is_some() {
            continue;
        }
        if let Some(q) = &query {
            let hay = format!("{} {}", name_of(e), value_of(e)).to_lowercase();
            if !hay.contains(q) {
                continue;
            }
        }
        let Some(eid) = e.get("id").and_then(Value::as_str) else { continue };
        let role = role_of(e);
        let what = format!("{} \"{}\"", pretty_role(&role), label(e));
        if is_text_entry(&role) {
            if can("set_value") {
                let current = value_of(e);
                for (i, (name, value)) in spec.inputs.iter().enumerate() {
                    // A field already holding this input is done; an input
                    // already placed elsewhere isn't offered twice.
                    if current == *value || typed.contains_key(&i) {
                        continue;
                    }
                    out.push(Opt {
                        id: String::new(),
                        text: format!("type the {name} (\"{}\") into the {what}", clip(value, 60)),
                        action: Action::Type { id: eid.into(), input: i },
                    });
                }
            }
            if can("press") && focused(e) {
                out.push(Opt { id: String::new(), text: format!("press Return in the {what}"), action: Action::Press { id: eid.into(), key: "Return".into() } });
            }
        } else if can("click") && is_clickable(e, &role) && !(name_of(e).is_empty() && value_of(e).is_empty()) {
            // Unnamed controls (window chrome, icon-only buttons) can't be
            // chosen from text; they stay for the planner's own read_ui.
            out.push(Opt { id: String::new(), text: format!("click the {what}"), action: Action::Click { id: eid.into() } });
        }
    }
    out.retain(|o| !spent.contains(&action_key(&o.action)));
    out.push(Opt { id: String::new(), text: "done: the goal is met on this screen".into(), action: Action::Done });
    out.push(Opt { id: String::new(), text: "escalate: none of these moves the goal forward; hand back to the planner".into(), action: Action::Escalate });
    for (i, o) in out.iter_mut().enumerate() {
        o.id = format!("o{}", i + 1);
    }
    out
}

fn action_key(a: &Action) -> String {
    match a {
        Action::Click { id } => format!("click:{id}"),
        Action::Type { id, input } => format!("type:{id}:{input}"),
        Action::Press { id, key } => format!("press:{id}:{key}"),
        Action::Done => "done".into(),
        Action::Escalate => "escalate".into(),
    }
}

// ---------------------------------------------------------------------------
// The loop.
// ---------------------------------------------------------------------------

/// One decision as the trace shows it.
struct Decided {
    id: Option<String>,
    choice: Option<String>,
    abstained: bool,
    confidence: f64,
    threshold: f64,
    reply: Value,
}

struct Run<'a> {
    state: &'a Arc<AppState>,
    user: &'a AuthUser,
    computer: &'a ComputerResponse,
    target: &'a Target,
    run_id: &'a str,
    spec: Spec,
    started: Instant,
    trace: Vec<Value>,
    decisions: u32,
    decision_ms: f64,
    action_ms: f64,
    actions: u64,
    oracle_tokens: u64,
    oracle_cost: f64,
    /// Decisions whose outcome is known only at the end (verify "yes").
    pending: Vec<String>,
}

impl<'a> Run<'a> {
    fn left(&self) -> Duration {
        self.spec.budget.saturating_sub(self.started.elapsed())
    }

    /// Where every step goes: the caller's app/pid/window, pinned to the
    /// window the first read found, so a person switching windows in the
    /// same app doesn't move the subtask.
    fn scope(&self, screen: &Screen) -> Map<String, Value> {
        let mut m = self.spec.scope.clone();
        if !m.contains_key("window_id") {
            if let (Some(pid), Some(wid)) = (screen.pid(), screen.window.get("window_id").and_then(Value::as_i64)) {
                m.insert("pid".into(), json!(pid));
                m.insert("window_id".into(), json!(wid));
            }
        }
        m
    }

    async fn step(&mut self, member: &str, input: Value) -> Result<Value, Fail> {
        let t = Instant::now();
        let out = execute_step(self.state, self.user, self.computer, self.target, member, input, self.run_id).await;
        self.action_ms += t.elapsed().as_secs_f64() * 1000.0;
        out
    }

    async fn read(&mut self, screen: &mut Screen) -> Result<(), Fail> {
        let mut input = self.scope(screen);
        input.insert("max_elements".into(), json!(self.spec.max_elements));
        if let Some(v) = screen.version {
            input.insert("since".into(), json!(v));
        }
        // A busy app can miss one accessibility round (AX "cannot complete");
        // two short retries before the subtask gives up.
        let mut tries = 0;
        let reply = loop {
            match self.step("read_ui", Value::Object(input.clone())).await {
                Ok(r) => break r,
                Err(e) if tries < 2 && !self.left().is_zero() => {
                    tries += 1;
                    tracing::debug!("run_subtask: read_ui retry {tries}: {}", e.message);
                    tokio::time::sleep(Duration::from_millis(400)).await;
                }
                Err(e) => return Err(e),
            }
        };
        screen.apply(&reply);
        Ok(())
    }

    /// The text every decision of this subtask sees.
    fn context(&self, screen: &Screen, typed: &HashMap<usize, String>, history: &[String]) -> String {
        let mut s = format!("Goal: {}\n", self.spec.goal);
        if !self.spec.inputs.is_empty() {
            s.push_str("Inputs (the only text that may be typed):\n");
            for (i, (name, value)) in self.spec.inputs.iter().enumerate() {
                let state = if typed.contains_key(&i) { "typed" } else { "not typed yet" };
                s.push_str(&format!("- {name}: \"{}\" ({state})\n", clip(value, 80)));
            }
        }
        if !history.is_empty() {
            s.push_str("Done so far:\n");
            for h in history.iter().rev().take(8).rev() {
                s.push_str(&format!("- {h}\n"));
            }
        }
        let app = screen.window.get("app").and_then(Value::as_str).unwrap_or("");
        let title = screen.window.get("title").and_then(Value::as_str).unwrap_or("");
        s.push_str(&format!("Screen: {app} — {title}\n"));
        let mut shown = 0;
        for e in screen.elements() {
            if shown >= 25 {
                break;
            }
            let role = role_of(e);
            let (n, v) = (name_of(e), value_of(e));
            if n.is_empty() && v.is_empty() {
                continue;
            }
            let mut line = format!("- {}", pretty_role(&role));
            if !n.is_empty() {
                line.push_str(&format!(" \"{}\"", clip(&n, 60)));
            }
            if !v.is_empty() {
                line.push_str(&format!(" = \"{}\"", clip(&v, 60)));
            }
            if focused(e) {
                line.push_str(" (focused)");
            }
            s.push_str(&line);
            s.push('\n');
            shown += 1;
        }
        s
    }

    async fn decide(&mut self, kind: &str, context: &str, question: &str, opts: &[(String, String)]) -> Decided {
        let budget = (self.left().as_millis() as u64).clamp(50, DECISION_BUDGET_CAP_MS);
        let body = json!({
            "context": context,
            "options": opts.iter().map(|(id, text)| json!({ "id": id, "text": text })).collect::<Vec<_>>(),
            "kind": kind,
            "question": question,
            "allow_abstain": true,
            "backends": fast_backends(),
            "latency_budget_ms": budget,
            "session_id": self.run_id,
            "task": clip(&self.spec.goal, 200),
        });
        let t = Instant::now();
        let reply = crate::agency_api::decisions::decide_value(self.state, self.user, body).await;
        self.decision_ms += t.elapsed().as_secs_f64() * 1000.0;
        self.decisions += 1;
        let reply = reply.unwrap_or_else(|e| json!({ "abstained": true, "error": e }));
        for a in reply.get("attempts").and_then(Value::as_array).into_iter().flatten() {
            if let Some(d) = a.get("detail") {
                self.oracle_tokens += d.get("tokens").and_then(Value::as_u64).unwrap_or(0);
                self.oracle_cost += d.get("cost_usd").and_then(Value::as_f64).unwrap_or(0.0);
            }
        }
        Decided {
            id: reply.get("id").and_then(Value::as_str).map(str::to_string),
            choice: reply.get("choice").and_then(Value::as_str).map(str::to_string),
            abstained: reply.get("abstained").and_then(Value::as_bool).unwrap_or(true),
            confidence: reply.get("confidence").and_then(Value::as_f64).unwrap_or(0.0),
            threshold: reply.get("threshold").and_then(Value::as_f64).unwrap_or(0.0),
            reply,
        }
    }

    fn outcome(&self, d: &Decided, status: &str, detail: &str) {
        if let Some(id) = &d.id {
            if let Err(e) = crate::agency_api::decisions::record_outcome(self.state, self.user, id, status, d.choice.as_deref(), Some(detail)) {
                tracing::warn!("run_subtask: decision outcome not recorded: {e}");
            }
        }
    }

    fn trace_decision(&mut self, kind: &str, d: &Decided, option: Option<&str>, action: Value) {
        let r = &d.reply;
        self.trace.push(json!({
            "kind": kind,
            "decision_id": d.id,
            "choice": d.choice,
            "option": option,
            "backend": r.get("backend"),
            "latency_ms": r.get("latency_ms"),
            "confidence": r.get("confidence"),
            "abstained": d.abstained,
            "escalated": r.get("escalated"),
            "action": action,
        }));
    }

    /// The exact success checks, with `verify`. `None` when there are none.
    async fn check_exact(&mut self, screen: &Screen, timeout_ms: u64) -> Result<Option<(bool, Value)>, Fail> {
        if self.spec.exact.is_empty() {
            return Ok(None);
        }
        let mut input = self.scope(screen);
        input.insert("checks".into(), json!(self.spec.exact));
        if timeout_ms > 0 {
            input.insert("timeout_ms".into(), json!(timeout_ms));
        }
        let reply = self.step("verify", Value::Object(input)).await?;
        Ok(Some((reply.get("ok").and_then(Value::as_bool).unwrap_or(false), reply)))
    }

    /// The fuzzy `{ask}` checks: one `verify`-kind decision each.
    async fn check_fuzzy(&mut self, screen: &Screen, typed: &HashMap<usize, String>, history: &[String]) -> bool {
        let asks = self.spec.fuzzy.clone();
        for ask in asks {
            let ctx = self.context(screen, typed, history);
            let opts = [("yes".to_string(), format!("yes: {ask}")), ("no".to_string(), "no, not yet".to_string())];
            let d = self.decide("verify", &ctx, &ask, &opts).await;
            let yes = !d.abstained && d.choice.as_deref() == Some("yes");
            self.trace_decision("verify", &d, Some(&ask), json!(null));
            if yes {
                if let Some(id) = &d.id {
                    self.pending.push(id.clone());
                }
            } else {
                self.outcome(&d, "skipped", "not yet met at this step");
                return false;
            }
        }
        true
    }

    /// All success checks. `None` when the subtask has none (done comes from
    /// the decision then).
    async fn met(&mut self, screen: &Screen, typed: &HashMap<usize, String>, history: &[String], timeout_ms: u64) -> Result<Option<bool>, Fail> {
        if self.spec.exact.is_empty() && self.spec.fuzzy.is_empty() {
            return Ok(None);
        }
        if let Some((ok, _)) = self.check_exact(screen, timeout_ms).await? {
            if !ok {
                return Ok(Some(false));
            }
        }
        Ok(Some(self.check_fuzzy(screen, typed, history).await))
    }

    /// Typing phase, UFO2-style: one `field` decision per unfilled input (run
    /// concurrently), then every confident, distinct assignment in ONE
    /// run_batch with an `expect` per field. Returns how many inputs landed.
    async fn fill(&mut self, screen: &mut Screen, typed: &mut HashMap<usize, String>, history: &mut Vec<String>) -> Result<usize, Fail> {
        if !self.spec.ops.contains("set_value") {
            return Ok(0);
        }
        let fields: Vec<(String, String)> = screen
            .elements()
            .filter(|e| enabled(e) && is_text_entry(&role_of(e)) && self.spec.blocked(e).is_none())
            .filter_map(|e| Some((e.get("id")?.as_str()?.to_string(), format!("{} \"{}\"", pretty_role(&role_of(e)), label(e)))))
            .take(MAX_ELEMENT_OPTIONS)
            .collect();
        let todo: Vec<usize> = (0..self.spec.inputs.len()).filter(|i| !typed.contains_key(i)).collect();
        if fields.is_empty() || todo.is_empty() {
            return Ok(0);
        }
        let ctx = self.context(screen, typed, history);
        let mut opts: Vec<(String, String)> = fields.iter().enumerate().map(|(i, (_, t))| (format!("f{}", i + 1), t.clone())).collect();
        opts.push(("none".into(), "none of these fields: it isn't on this screen yet".into()));
        let questions: Vec<String> =
            todo.iter().map(|&i| format!("Which field takes the {} (\"{}\")?", self.spec.inputs[i].0, clip(&self.spec.inputs[i].1, 60))).collect();
        let t = Instant::now();
        let budget = (self.left().as_millis() as u64).clamp(50, DECISION_BUDGET_CAP_MS);
        let calls = questions.iter().map(|q| {
            let body = json!({
                "context": ctx,
                "options": opts.iter().map(|(id, text)| json!({ "id": id, "text": text })).collect::<Vec<_>>(),
                "kind": "element",
                "question": q,
                "allow_abstain": true,
            "backends": fast_backends(),
                "latency_budget_ms": budget,
                "session_id": self.run_id,
                "task": clip(&self.spec.goal, 200),
            });
            crate::agency_api::decisions::decide_value(self.state, self.user, body)
        });
        let replies = futures::future::join_all(calls).await;
        self.decision_ms += t.elapsed().as_secs_f64() * 1000.0;
        self.decisions += replies.len() as u32;

        // Confident, distinct assignments only; the rest go to the step loop.
        let mut taken: HashSet<String> = HashSet::new();
        let mut plan: Vec<(usize, String, Decided)> = Vec::new();
        for (&input, reply) in todo.iter().zip(replies) {
            let reply = reply.unwrap_or_else(|e| json!({ "abstained": true, "error": e }));
            for a in reply.get("attempts").and_then(Value::as_array).into_iter().flatten() {
                if let Some(d) = a.get("detail") {
                    self.oracle_tokens += d.get("tokens").and_then(Value::as_u64).unwrap_or(0);
                    self.oracle_cost += d.get("cost_usd").and_then(Value::as_f64).unwrap_or(0.0);
                }
            }
            let d = Decided {
                id: reply.get("id").and_then(Value::as_str).map(str::to_string),
                choice: reply.get("choice").and_then(Value::as_str).map(str::to_string),
                abstained: reply.get("abstained").and_then(Value::as_bool).unwrap_or(true),
                confidence: reply.get("confidence").and_then(Value::as_f64).unwrap_or(0.0),
                threshold: reply.get("threshold").and_then(Value::as_f64).unwrap_or(0.0),
                reply,
            };
            let field = d.choice.as_deref().and_then(|c| c.strip_prefix('f')).and_then(|n| n.parse::<usize>().ok()).and_then(|n| fields.get(n - 1));
            match field {
                Some((fid, _)) if !d.abstained && taken.insert(fid.clone()) => plan.push((input, fid.clone(), d)),
                _ => {
                    let why = if d.abstained { "unsure" } else { "not on this screen, or the field was already taken" };
                    self.trace_decision("field", &d, Some(&self.spec.inputs[input].0.clone()), json!({ "skipped": why }));
                    self.outcome(&d, "skipped", why);
                }
            }
        }
        if plan.is_empty() || self.actions + plan.len() as u64 > self.spec.max_steps {
            for (_, _, d) in &plan {
                self.outcome(d, "skipped", "over max_steps");
            }
            return Ok(0);
        }

        let steps: Vec<Value> = plan
            .iter()
            .map(|(i, fid, _)| {
                let v = &self.spec.inputs[*i].1;
                json!({ "act": { "id": fid, "op": "set_value", "value": v }, "expect": { "id": fid, "value": v } })
            })
            .collect();
        let mut input = self.scope(screen);
        input.insert("steps".into(), json!(steps));
        if let Some(v) = screen.version {
            input.insert("version".into(), json!(v));
        }
        let t = Instant::now();
        let reply = self.step("run_batch", Value::Object(input)).await;
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let reply = match reply {
            Ok(r) => r,
            Err(f) => {
                for (_, _, d) in &plan {
                    self.outcome(d, "error", &f.message);
                }
                return Err(f);
            }
        };
        let results = reply.get("steps").and_then(Value::as_array).cloned().unwrap_or_default();
        let stale = reply.get("status").and_then(Value::as_str) == Some("stale_version");
        let mut landed = 0;
        for (k, (i, fid, d)) in plan.iter().enumerate() {
            let r = results.get(k);
            let ok = r.and_then(|r| r.get("status")).and_then(Value::as_str) == Some("ok");
            let name = self.spec.inputs[*i].0.clone();
            let detail = r.and_then(|r| r.get("error")).and_then(Value::as_str).unwrap_or(if stale { "stale map" } else { "not run" }).to_string();
            if ok {
                typed.insert(*i, fid.clone());
                history.push(format!("typed the {name} into {fid}"));
                landed += 1;
                self.actions += 1;
                self.outcome(d, "success", "field value matched the input (run_batch expect)");
            } else if r.is_some() {
                self.actions += 1;
                self.outcome(d, "failure", &detail);
            } else {
                self.outcome(d, "skipped", &detail);
            }
            self.trace_decision("field", d, Some(&name), json!({ "member": "run_batch", "op": "set_value", "id": fid, "ok": ok, "detail": if ok { Value::Null } else { json!(detail) }, "batch_ms": ms }));
        }
        if let Some(changes) = reply.get("changes") {
            // run_batch ends with one observe: fold it into the map.
            let mut folded = changes.clone();
            if folded.get("elements").is_none() {
                folded = json!({ "diff": changes, "version": reply.get("version") });
            }
            screen.apply(&folded);
        }
        if let Some(map) = reply.get("map") {
            screen.apply(map);
        }
        Ok(landed)
    }

    /// Run one chosen action. Returns (ok, detail).
    async fn perform(&mut self, screen: &mut Screen, action: &Action) -> Result<(bool, String), Fail> {
        let version = screen.version;
        let reply = match action {
            Action::Click { id } => {
                let mut input = json!({ "id": id, "op": "click" });
                if let Some(v) = version {
                    input["version"] = json!(v);
                }
                self.step("act", input).await?
            }
            Action::Press { id, key } => {
                let mut input = json!({ "id": id, "op": "press", "key": key });
                if let Some(v) = version {
                    input["version"] = json!(v);
                }
                self.step("act", input).await?
            }
            Action::Type { id, input } => {
                // Predictable outcome: batch it with its expect check.
                let v = &self.spec.inputs[*input].1;
                let mut b = json!({ "steps": [{ "act": { "id": id, "op": "set_value", "value": v }, "expect": { "id": id, "value": v } }] });
                if let Some(ver) = version {
                    b["version"] = json!(ver);
                }
                for (k, v) in self.scope(screen) {
                    b[k] = v;
                }
                self.step("run_batch", b).await?
            }
            Action::Done | Action::Escalate => return Ok((true, String::new())),
        };
        self.actions += 1;
        if let Some(map) = reply.get("map") {
            screen.apply(map);
        }
        let status = reply.get("status").and_then(Value::as_str);
        let ok = match action {
            Action::Type { .. } => reply.get("ok").and_then(Value::as_bool).unwrap_or(false),
            _ => status == Some("done"),
        };
        let detail = if ok {
            String::new()
        } else {
            reply
                .get("steps")
                .and_then(Value::as_array)
                .and_then(|s| s.iter().find_map(|r| r.get("error").and_then(Value::as_str)))
                .map(str::to_string)
                .or_else(|| status.map(|s| format!("status {s}")))
                .unwrap_or_else(|| "the action didn't complete".into())
        };
        Ok((ok, detail))
    }

    fn finish(mut self, status: &str, reason: Option<String>, screen: &Screen, success: Value) -> ToolsetResult {
        // Verify decisions resolve now: "yes" was right iff the subtask is done.
        let pending = std::mem::take(&mut self.pending);
        for id in pending {
            let (st, detail) = if status == "done" { ("success", "the subtask completed") } else { ("skipped", "the subtask did not complete") };
            let _ = crate::agency_api::decisions::record_outcome(self.state, self.user, &id, st, Some("yes"), Some(detail));
        }
        let mut body = json!({
            "status": status,
            "goal": self.spec.goal,
            "decisions": self.decisions,
            "actions": self.actions,
            "elapsed_ms": (self.started.elapsed().as_secs_f64() * 1000.0).round(),
            "decision_ms": self.decision_ms.round(),
            "action_ms": self.action_ms.round(),
            "oracle": { "tokens": self.oracle_tokens, "cost_usd": (self.oracle_cost * 1e6).round() / 1e6 },
            "inputs_typed": self.spec.inputs.len(),
            "success": success,
            "steps": self.trace,
        });
        if let Some(r) = reason {
            body["reason"] = json!(r);
        }
        if status != "done" {
            body["screen"] = screen.compact(60);
            body["next"] = json!(match status {
                "escalated" => "Continue from this screen yourself (read_ui/act/run_batch), or call run_subtask again with a narrower goal.",
                _ => "The subtask stopped. Read the screen and decide how to recover.",
            });
        }
        ToolsetResult {
            is_error: false,
            content: vec![text(serde_json::to_string(&body).unwrap_or_else(|_| "{}".into()))],
            browser_state: None,
            screen: screen_info(None),
            error: None,
        }
    }
}

/// Run one `run_subtask` call (after the outer call's lease, policy,
/// approval and audit). Errors only when the driver itself is unreachable;
/// every other end is a status in the result.
pub async fn run(
    state: &Arc<AppState>,
    user: &AuthUser,
    computer: &ComputerResponse,
    target: &Target,
    input: &Value,
    run_id: &str,
) -> Result<ToolsetResult, Fail> {
    let spec = parse(input)?;
    let mut run = Run {
        state,
        user,
        computer,
        target,
        run_id,
        spec,
        started: Instant::now(),
        trace: Vec::new(),
        decisions: 0,
        decision_ms: 0.0,
        action_ms: 0.0,
        actions: 0,
        oracle_tokens: 0,
        oracle_cost: 0.0,
        pending: Vec::new(),
    };
    let mut screen = Screen::default();
    let mut typed: HashMap<usize, String> = HashMap::new();
    let mut history: Vec<String> = Vec::new();
    // Actions that ran without changing the screen: not offered again.
    let mut spent: HashSet<String> = HashSet::new();

    run.read(&mut screen).await?;
    // Already done? (A re-issued subtask, or a goal the screen already meets.)
    if let Some(true) = run.met(&screen, &typed, &history, 0).await? {
        return Ok(run.finish("done", Some("the success checks already held".into()), &screen, json!(true)));
    }

    loop {
        if run.left().is_zero() {
            let why = format!("budget_ms ({} ms) spent", run.spec.budget.as_millis());
            return Ok(run.finish("escalated", Some(why), &screen, json!(false)));
        }
        if run.actions >= run.spec.max_steps {
            let why = format!("max_steps ({}) reached", run.spec.max_steps);
            return Ok(run.finish("escalated", Some(why), &screen, json!(false)));
        }

        // Fill every input the screen can take in one speculative batch.
        if typed.len() < run.spec.inputs.len() {
            let landed = run.fill(&mut screen, &mut typed, &mut history).await?;
            if landed > 0 {
                if let Some(true) = run.met(&screen, &typed, &history, 0).await? {
                    return Ok(run.finish("done", None, &screen, json!(true)));
                }
                run.read(&mut screen).await?;
                continue;
            }
        }

        // One step decision.
        let opts = options(&run.spec, &screen, &typed, &spent);
        let ctx = run.context(&screen, &typed, &history);
        let pairs: Vec<(String, String)> = opts.iter().map(|o| (o.id.clone(), o.text.clone())).collect();
        let d = run.decide("element", &ctx, "Which one action moves the goal forward next?", &pairs).await;
        let chosen = d.choice.as_deref().and_then(|c| opts.iter().find(|o| o.id == c));
        let Some(opt) = chosen.filter(|_| !d.abstained) else {
            // Under threshold, abstained, or nothing answered: hand back.
            let why = match d.reply.get("error").and_then(Value::as_str) {
                Some(e) => format!("the decision runtime didn't answer: {e}"),
                None => format!("the next step was unclear (confidence {:.2} under the {:.2} threshold)", d.confidence, d.threshold),
            };
            run.trace_decision("element", &d, None, json!(null));
            run.outcome(&d, "skipped", "abstained: handed back to the planner");
            return Ok(run.finish("escalated", Some(why), &screen, json!(false)));
        };
        let (option_text, action) = (opt.text.clone(), opt.action.clone());

        match action {
            Action::Escalate => {
                run.trace_decision("element", &d, Some(&option_text), json!(null));
                run.outcome(&d, "skipped", "escalated to the planner");
                return Ok(run.finish("escalated", Some("the decision found no option that moves the goal forward".into()), &screen, json!(false)));
            }
            Action::Done => {
                // Trust, then verify (give a settling UI a moment).
                let met = run.met(&screen, &typed, &history, 2_000).await?;
                run.trace_decision("element", &d, Some(&option_text), json!(null));
                return Ok(match met {
                    Some(false) => {
                        run.outcome(&d, "failure", "picked done but the success checks failed");
                        run.finish("escalated", Some("the decision said done, but the success checks don't hold".into()), &screen, json!(false))
                    }
                    _ => {
                        run.outcome(&d, "success", "success checks held");
                        let unchecked = met.is_none();
                        run.finish("done", unchecked.then(|| "no success checks were given; the decision judged the goal met".to_string()), &screen, json!(!unchecked))
                    }
                });
            }
            _ => {}
        }

        let before = screen.version;
        let (ok, detail) = run.perform(&mut screen, &action).await?;
        let key = action_key(&action);
        if let Action::Type { input, id } = &action {
            if ok {
                typed.insert(*input, id.clone());
            }
        }
        history.push(format!("{option_text}{}", if ok { "" } else { " (failed)" }));
        run.read(&mut screen).await?;
        let changed = screen.version != before;
        if !changed || !ok {
            spent.insert(key);
        }
        let outcome_ok = ok && (changed || matches!(action, Action::Type { .. }));
        run.outcome(&d, if outcome_ok { "success" } else { "failure" }, if outcome_ok { "action ran and the screen changed" } else if ok { "action ran but nothing changed" } else { &detail });
        run.trace_decision("element", &d, Some(&option_text), json!({ "ok": ok, "changed": changed, "detail": if ok { Value::Null } else { json!(detail) } }));

        if let Some(true) = run.met(&screen, &typed, &history, 0).await? {
            return Ok(run.finish("done", None, &screen, json!(true)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(input: Value) -> Spec {
        parse(&input).expect("valid subtask")
    }

    fn form() -> Screen {
        let mut s = Screen::default();
        s.apply(&json!({
            "version": 3,
            "window": { "pid": 42, "app": "Safari", "title": "Sign up" },
            "elements": [
                { "id": "e1", "role": "AXTextField", "name": "Full name" },
                { "id": "e2", "role": "AXTextField", "name": "Email", "value": "ada@example.com" },
                { "id": "e3", "role": "AXButton", "name": "Submit" },
                { "id": "e4", "role": "AXButton", "name": "Delete account" },
                { "id": "e5", "role": "AXButton", "name": "Disabled", "enabled": false },
                { "id": "e6", "role": "AXStaticText", "name": "Welcome" },
            ],
        }));
        s
    }

    #[test]
    fn parse_splits_exact_and_fuzzy_checks_and_clamps_limits() {
        let s = spec(json!({
            "goal": "Fill the form",
            "inputs": [{ "name": "name", "value": "Ada" }],
            "success": [{ "text": "Thanks" }, { "ask": "Is the thank-you page showing?" }],
            "max_steps": 500,
            "budget_ms": 10,
        }));
        assert_eq!(s.exact, vec![json!({ "text": "Thanks" })]);
        assert_eq!(s.fuzzy, vec!["Is the thank-you page showing?".to_string()]);
        assert_eq!(s.max_steps, MAX_MAX_STEPS);
        assert_eq!(s.budget, Duration::from_millis(1_000));
        assert!(parse(&json!({ "goal": " " })).is_err());
        assert!(parse(&json!({ "goal": "x", "inputs": [{ "name": "a", "value": "1" }, { "name": "A", "value": "2" }] })).is_err());
    }

    #[test]
    fn options_offer_literal_inputs_safe_clicks_done_and_escalate() {
        let s = spec(json!({ "goal": "Sign up", "inputs": [{ "name": "full name", "value": "Ada" }, { "name": "email", "value": "ada@example.com" }] }));
        let opts = options(&s, &form(), &HashMap::new(), &HashSet::new());
        let actions: Vec<&Action> = opts.iter().map(|o| &o.action).collect();
        // Both inputs into the empty field; the email field already holds the email.
        assert!(actions.contains(&&Action::Type { id: "e1".into(), input: 0 }));
        assert!(actions.contains(&&Action::Type { id: "e1".into(), input: 1 }));
        assert!(!actions.contains(&&Action::Type { id: "e2".into(), input: 1 }));
        assert!(actions.contains(&&Action::Click { id: "e3".into() }));
        // Irreversible and disabled elements are never options; static text isn't actionable.
        assert!(!actions.iter().any(|a| matches!(a, Action::Click { id } if id == "e4" || id == "e5" || id == "e6")));
        assert_eq!(opts[opts.len() - 2].action, Action::Done);
        assert_eq!(opts[opts.len() - 1].action, Action::Escalate);
        assert!(opts.iter().enumerate().all(|(i, o)| o.id == format!("o{}", i + 1)));
        // Opting in to irreversible clicks, and spent actions dropping out.
        let s2 = spec(json!({ "goal": "x", "constraints": { "allow_irreversible": true, "avoid": ["submit"] } }));
        let spent: HashSet<String> = ["click:e4".to_string()].into();
        let acts: Vec<Action> = options(&s2, &form(), &HashMap::new(), &HashSet::new()).into_iter().map(|o| o.action).collect();
        assert!(acts.contains(&Action::Click { id: "e4".into() }));
        assert!(!acts.contains(&Action::Click { id: "e3".into() }), "avoid wins");
        let acts: Vec<Action> = options(&s2, &form(), &HashMap::new(), &spent).into_iter().map(|o| o.action).collect();
        assert!(!acts.contains(&Action::Click { id: "e4".into() }));
    }

    #[test]
    fn screen_patches_since_diffs() {
        let mut s = form();
        s.apply(&json!({
            "version": 4,
            "diff": {
                "since": 3,
                "added": [{ "id": "e7", "role": "AXStaticText", "name": "Thanks, Ada" }],
                "changed": [{ "id": "e1", "role": "AXTextField", "name": "Full name", "value": "Ada" }],
                "removed": ["e4"],
            },
        }));
        assert_eq!(s.version, Some(4));
        assert_eq!(s.order, vec!["e1", "e2", "e3", "e5", "e6", "e7"]);
        assert_eq!(value_of(&s.by_id["e1"]), "Ada");
        assert_eq!(s.pid(), Some(42));
        let c = s.compact(3);
        assert_eq!(c["elements"].as_array().map(Vec::len), Some(3));
    }
}
