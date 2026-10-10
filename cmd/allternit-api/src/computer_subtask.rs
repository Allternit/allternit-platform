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
//!
//! Replay cache (driver spec D4, `computer_replay`): a subtask seen before
//! replays its recorded steps with no decisions, heals a moved step, and a
//! new one is recorded once its exact success checks hold. A recording saved
//! by name is a skill (`run_skill`, `skills`).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use crate::auth::AuthUser;
use crate::computer_replay as replay;
use crate::computer_routes::ComputerResponse;
use crate::computer_toolset::{execute_step, screen_info, text, Fail, Target, ToolsetResult};
use crate::AppState;

/// Everything the loop does outside itself: driver steps (through the
/// lease → policy → audit path), decisions, decision outcomes and the replay
/// cache. Production is `Live`; the tests drive a simulated app.
#[async_trait::async_trait]
pub(crate) trait Io: Send + Sync {
    async fn step(&self, member: &str, input: Value) -> Result<Value, Fail>;
    async fn decide(&self, body: Value) -> Result<Value, String>;
    fn outcome(&self, id: &str, status: &str, label: Option<&str>, detail: Option<&str>);
    fn cached(&self, key: &str) -> Option<replay::Entry>;
    fn save(&self, entry: &replay::Entry) -> Result<(), String>;
    fn hit(&self, id: &str);
}

struct Live<'a> {
    state: &'a Arc<AppState>,
    user: &'a AuthUser,
    computer: &'a ComputerResponse,
    target: &'a Target,
    run_id: &'a str,
}

#[async_trait::async_trait]
impl Io for Live<'_> {
    async fn step(&self, member: &str, input: Value) -> Result<Value, Fail> {
        execute_step(self.state, self.user, self.computer, self.target, member, input, self.run_id).await
    }
    async fn decide(&self, body: Value) -> Result<Value, String> {
        crate::agency_api::decisions::decide_value(self.state, self.user, body).await
    }
    fn outcome(&self, id: &str, status: &str, label: Option<&str>, detail: Option<&str>) {
        if let Err(e) = crate::agency_api::decisions::record_outcome(self.state, self.user, id, status, label, detail) {
            tracing::warn!("run_subtask: decision outcome not recorded: {e}");
        }
    }
    fn cached(&self, key: &str) -> Option<replay::Entry> {
        let conn = self.state.db.connect().ok()?;
        replay::by_key(&conn, &self.user.user_id, key).unwrap_or_else(|e| {
            tracing::warn!("run_subtask: replay cache read failed: {e}");
            None
        })
    }
    fn save(&self, entry: &replay::Entry) -> Result<(), String> {
        let mut conn = self.state.db.connect().map_err(|e| e.to_string())?;
        replay::save(&mut conn, &self.user.user_id, entry).map_err(|e| e.to_string())
    }
    fn hit(&self, id: &str) {
        let now = chrono::Utc::now().to_rfc3339();
        if let Err(e) = self.state.db.connect().and_then(|c| replay::touch(&c, &self.user.user_id, id, &now)) {
            tracing::warn!("run_subtask: replay hit not counted: {e}");
        }
    }
}

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
/// step hands back. Shared with the safety monitor.
fn fast_backends() -> Vec<&'static str> {
    crate::computer_safety::fast_backends()
}
const DEFAULT_OPS: [&str; 4] = ["click", "set_value", "select", "press"];
/// A replayed step waits this long for its element and for its check (a
/// page load between steps fits; a moved element costs it once, then heals).
const REPLAY_STEP_TIMEOUT_MS: u64 = 5_000;

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
    cache: replay::Mode,
    save_as: Option<String>,
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
        cache: replay::Mode::parse(input.get("cache").and_then(Value::as_str)),
        save_as: input.get("save_as").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty()).map(|n| clip(n, 80)),
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

    fn origin(&self) -> (f64, f64) {
        replay::origin(self.elements())
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

pub(crate) fn role_of(e: &Value) -> String {
    let r = e.get("role").and_then(Value::as_str).unwrap_or_default();
    r.strip_prefix("AX").unwrap_or(r).to_lowercase().replace([' ', '_', '-'], "")
}

pub(crate) fn name_of(e: &Value) -> String {
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
        // One classifier for the whole executor (computer_safety): with
        // allow_irreversible such an element is offered, and choosing it
        // ends the subtask with `needs_confirmation`, since the person
        // confirms every irreversible step.
        if !self.allow_irreversible && crate::computer_safety::classify_element(e.get("role").and_then(Value::as_str).unwrap_or(""), &name_of(e)).is_some() {
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

/// The replay cache's view of this run.
struct Cache {
    mode: replay::Mode,
    key: String,
    window: String,
    app: String,
    /// The entry this run replays (its id is kept when it is rewritten).
    entry: Option<replay::Entry>,
    /// pending (before the first read) | off | uncached | miss | hit |
    /// healed | diverged
    status: &'static str,
    replayed: usize,
    healed: Vec<Value>,
    reason: Option<String>,
    stored: bool,
}

struct Run<'a> {
    io: &'a dyn Io,
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
    /// The steps that moved the UI, for the replay cache.
    recording: Vec<replay::Step>,
    cache: Cache,
    /// The step the safety layer refused (hand-back for the planner).
    held: Option<Value>,
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
        let out = self.io.step(member, input.clone()).await;
        self.action_ms += t.elapsed().as_secs_f64() * 1000.0;
        if let Err(f) = &out {
            if f.code.is_some() {
                // The safety layer refused this step: the planner gets it back.
                self.held = Some(json!({ "member": member, "input": input }));
            }
        }
        out
    }

    async fn read(&mut self, screen: &mut Screen) -> Result<(), Fail> {
        let mut input = self.scope(screen);
        input.insert("max_elements".into(), json!(self.spec.max_elements));
        if let Some(v) = screen.version {
            input.insert("since".into(), json!(v));
        }
        // Recording: tree paths (cheap) and pixel hashes (a fresh walk) for
        // the locators. Replaying: paths only.
        if self.caching() {
            input.insert("paths".into(), json!(true));
            if matches!(self.cache.status, "miss" | "diverged") {
                input.insert("crops".into(), json!(true));
            }
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
        let mut seen = format!("Screen: {app} — {title}\n");
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
            seen.push_str(&line);
            seen.push('\n');
            shown += 1;
        }
        // Screen text is untrusted: the decision model reads it as data.
        s.push_str(&crate::computer_safety::spotlight("screen", &seen, &[]));
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
        let reply = self.io.decide(body).await;
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
            self.io.outcome(id, status, d.choice.as_deref(), Some(detail));
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
            self.io.decide(body)
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

        // Locators before the batch changes the screen.
        let locs: Vec<Option<(replay::Locator, String)>> = plan
            .iter()
            .map(|(_, fid, _)| {
                let what = fields.iter().find(|(id, _)| id == fid).map(|(_, w)| w.clone()).unwrap_or_default();
                self.locate(screen, fid).map(|l| (l, what))
            })
            .collect();
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
                if let Some((loc, what)) = locs[k].clone() {
                    self.record(loc, "set_value", Some(*i), None, &format!("type the {name} into the {what}"), None);
                }
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
        // run_batch ends with one observe: fold it into the map.
        Self::fold(screen, &reply);
        Ok(landed)
    }

    // ---- replay cache ------------------------------------------------------------

    /// Is this run read from / written to the cache? Only subtasks with an
    /// exact success check: a replay is judged by verify, never a decision.
    fn caching(&self) -> bool {
        self.cache.mode != replay::Mode::Off && !self.spec.exact.is_empty()
    }

    fn locate(&self, screen: &Screen, id: &str) -> Option<replay::Locator> {
        let e = screen.by_id.get(id)?;
        Some(replay::locator(e, screen.origin(), &self.spec.inputs))
    }

    fn record(&mut self, locator: replay::Locator, op: &str, input: Option<usize>, key: Option<&str>, label: &str, check: Option<Value>) {
        if !self.caching() {
            return;
        }
        self.recording.push(replay::Step {
            op: op.into(),
            input: input.map(|i| self.spec.inputs[i].0.clone()),
            key: key.map(str::to_string),
            locator,
            label: replay::template(label, &self.spec.inputs),
            check,
        });
    }

    fn diverge(&mut self, why: String) -> bool {
        self.cache.status = "diverged";
        self.cache.reason = Some(why);
        false
    }

    /// One recorded step as a run_batch step on element `id`: the act, then
    /// its check (the field's value for set_value, the recorded effect
    /// otherwise). `None` when the step can't run under this subtask.
    fn batch_step(&self, s: &replay::Step, id: &str) -> Option<Value> {
        let probe = json!({ "name": replay::substitute(&s.locator.name, &self.spec.inputs) });
        let op_ok = match s.op.as_str() {
            "click" => self.spec.ops.contains("click"),
            "press" => self.spec.ops.contains("press"),
            "set_value" => self.spec.ops.contains("set_value"),
            _ => false,
        };
        if !op_ok || self.spec.blocked(&probe).is_some() {
            return None;
        }
        let mut step = match s.op.as_str() {
            "set_value" => {
                let name = s.input.as_deref()?;
                let (_, v) = self.spec.inputs.iter().find(|(n, _)| n.eq_ignore_ascii_case(name))?;
                json!({ "act": { "id": id, "op": "set_value", "value": v }, "expect": { "id": id, "value": v } })
            }
            "press" => json!({ "act": { "id": id, "op": "press", "key": s.key.clone().unwrap_or_else(|| "Return".into()) } }),
            _ => json!({ "act": { "id": id, "op": "click" } }),
        };
        if let Some(check) = &s.check {
            if step.get("expect").is_none() {
                step["expect"] = replay::substitute_json(check, &self.spec.inputs);
            }
        }
        step["timeout_ms"] = json!(REPLAY_STEP_TIMEOUT_MS);
        Some(step)
    }

    /// Fold a run_batch reply's closing observe into the map.
    fn fold(screen: &mut Screen, reply: &Value) {
        if let Some(changes) = reply.get("changes") {
            let mut folded = changes.clone();
            if folded.get("elements").is_none() {
                folded = json!({ "diff": changes, "version": reply.get("version") });
            }
            screen.apply(&folded);
        }
        if let Some(map) = reply.get("map") {
            screen.apply(map);
        }
    }

    fn mark_typed(&self, s: &replay::Step, id: &str, typed: &mut HashMap<usize, String>) {
        if let Some(i) = s.input.as_deref().and_then(|n| self.spec.inputs.iter().position(|(m, _)| m.eq_ignore_ascii_case(n))) {
            typed.insert(i, id.to_string());
        }
    }

    /// Re-infer one stale step: ONE decision over the current screen's
    /// options of the same kind (click, press, or typing the same input).
    async fn reinfer(&mut self, s: &replay::Step, screen: &Screen, typed: &HashMap<usize, String>, history: &[String]) -> Option<(String, Decided)> {
        let all = options(&self.spec, screen, typed, &HashSet::new());
        let fits = |a: &Action| match (s.op.as_str(), a) {
            ("click", Action::Click { .. }) => true,
            ("press", Action::Press { key, .. }) => s.key.as_deref().unwrap_or("Return") == key,
            ("set_value", Action::Type { input, .. }) => s.input.as_deref().is_some_and(|n| self.spec.inputs[*input].0.eq_ignore_ascii_case(n)),
            _ => false,
        };
        let mut cands: Vec<(String, String, Option<String>)> = all
            .iter()
            .filter(|o| fits(&o.action))
            .map(|o| match &o.action {
                Action::Click { id } | Action::Press { id, .. } | Action::Type { id, .. } => (o.text.clone(), String::new(), Some(id.clone())),
                _ => (o.text.clone(), String::new(), None),
            })
            .collect();
        if cands.is_empty() {
            return None;
        }
        cands.push(("escalate: none of these does that step".into(), String::new(), None));
        for (i, c) in cands.iter_mut().enumerate() {
            c.1 = format!("o{}", i + 1);
        }
        let label = replay::substitute(&s.label, &self.spec.inputs);
        let ctx = self.context(screen, typed, history);
        let pairs: Vec<(String, String)> = cands.iter().map(|(t, id, _)| (id.clone(), t.clone())).collect();
        let q = format!("A recorded step can't find its element any more. Which option does the same thing as: {label}?");
        let d = self.decide("element", &ctx, &q, &pairs).await;
        let picked = d.choice.as_deref().and_then(|c| cands.iter().find(|(_, id, _)| id == c)).and_then(|(_, _, el)| el.clone());
        match picked.filter(|_| !d.abstained) {
            Some(id) => Some((id, d)),
            None => {
                self.trace_decision("reinfer", &d, Some(&label), json!(null));
                self.outcome(&d, "skipped", "re-inference found no element for the recorded step");
                None
            }
        }
    }

    /// Replay a cached entry with no decisions. `Ok(true)`: every step ran,
    /// its check held, and the exact success checks hold. `Ok(false)`: the
    /// replay diverged (`cache.reason`); the loop continues from this screen,
    /// with what replayed so far already in the recording.
    async fn replay(&mut self, entry: &replay::Entry, screen: &mut Screen, typed: &mut HashMap<usize, String>, history: &mut Vec<String>) -> Result<bool, Fail> {
        let steps = &entry.steps;
        let mut i = 0;
        while i < steps.len() {
            if self.left().is_zero() {
                return Ok(self.diverge(format!("budget spent at step {}", i + 1)));
            }
            // 1. Speculative: every remaining step in ONE batch, each waiting
            //    for its recorded element (UFO2's multi-action; stops at the
            //    first failed check).
            let mut batch = Vec::new();
            for s in &steps[i..] {
                let Some(mut b) = self.batch_step(s, &s.locator.id) else {
                    return Ok(self.diverge(format!("step {} ({}) isn't allowed by this subtask's inputs or constraints", i + batch.len() + 1, s.op)));
                };
                b["wait_for"] = json!({ "id": s.locator.id });
                batch.push(b);
            }
            let mut input = self.scope(screen);
            input.insert("steps".into(), json!(batch));
            let reply = self.step("run_batch", Value::Object(input)).await?;
            Self::fold(screen, &reply);
            let results = reply.get("steps").and_then(Value::as_array).cloned().unwrap_or_default();
            let ran = results.iter().take_while(|r| r.get("status").and_then(Value::as_str) == Some("ok")).count();
            for s in &steps[i..i + ran] {
                self.mark_typed(s, &s.locator.id, typed);
                history.push(replay::substitute(&s.label, &self.spec.inputs));
                self.recording.push(s.clone());
                self.trace.push(json!({ "kind": "replay", "op": s.op, "via": "id", "label": s.label }));
            }
            self.actions += ran as u64;
            self.cache.replayed += ran;
            i += ran;
            if i >= steps.len() {
                break;
            }
            let failed = results.get(ran);
            let code = failed.and_then(|r| r.get("code")).and_then(Value::as_str).unwrap_or("");
            if code != "wait_for_failed" {
                let why = failed.and_then(|r| r.get("error")).and_then(Value::as_str).unwrap_or("the batch stopped");
                return Ok(self.diverge(format!("step {} ran but didn't do what it did when recorded: {why}", i + 1)));
            }

            // 2. Its element moved: find it again by name, path or pixels;
            //    else re-infer that one step.
            let s = &steps[i];
            self.read(screen).await?;
            let loc = replay::substitute_locator(&s.locator, &self.spec.inputs);
            let mut found = {
                let els: Vec<&Value> = screen.elements().collect();
                replay::resolve(&loc, &els, screen.origin()).map(|(id, via)| (id, via.as_str()))
            };
            if found.is_none() && loc.crop.is_some() {
                let mut input = self.scope(screen);
                input.insert("max_elements".into(), json!(self.spec.max_elements));
                input.insert("paths".into(), json!(true));
                input.insert("crops".into(), json!(true));
                if let Ok(full) = self.step("read_ui", Value::Object(input)).await {
                    screen.apply(&full);
                    let els: Vec<&Value> = screen.elements().collect();
                    found = replay::resolve(&loc, &els, screen.origin()).map(|(id, via)| (id, via.as_str()));
                }
            }
            let mut decided = None;
            if found.is_none() {
                if let Some((id, d)) = self.reinfer(s, screen, typed, history).await {
                    found = Some((id, "decision"));
                    decided = Some(d);
                }
            }
            let Some((id, via)) = found else {
                return Ok(self.diverge(format!("step {} ({}): its element is gone and re-inference couldn't place it", i + 1, s.label)));
            };
            let healed_loc = self.locate(screen, &id);
            let Some(b) = self.batch_step(s, &id) else {
                return Ok(self.diverge(format!("step {} isn't allowed by this subtask's constraints", i + 1)));
            };
            let mut input = self.scope(screen);
            input.insert("steps".into(), json!([b]));
            let reply = self.step("run_batch", Value::Object(input)).await?;
            Self::fold(screen, &reply);
            let ok = reply.get("ok").and_then(Value::as_bool).unwrap_or(false);
            if let Some(d) = &decided {
                self.outcome(d, if ok { "success" } else { "failure" }, "re-inferred a stale replay step");
                self.trace_decision("reinfer", d, Some(&s.label), json!({ "id": id, "ok": ok }));
            }
            if !ok {
                return Ok(self.diverge(format!("step {} found its element by {via}, but its check failed", i + 1)));
            }
            self.actions += 1;
            self.mark_typed(s, &id, typed);
            history.push(replay::substitute(&s.label, &self.spec.inputs));
            self.recording.push(replay::Step { locator: healed_loc.unwrap_or_else(|| s.locator.clone()), ..s.clone() });
            self.cache.healed.push(json!({ "step": i + 1, "via": via }));
            self.trace.push(json!({ "kind": "replay", "op": s.op, "via": via, "label": s.label }));
            i += 1;
        }
        match self.check_exact(screen, 0).await? {
            Some((true, _)) => Ok(true),
            _ => Ok(self.diverge("every step replayed, but the success checks don't hold".into())),
        }
    }

    /// Write the recording once the subtask succeeded (exact checks held).
    fn store(&mut self) {
        if !self.caching() || self.recording.is_empty() {
            return;
        }
        let clean_hit = self.cache.status == "hit";
        if let Some(e) = &self.cache.entry {
            if clean_hit && self.spec.save_as.is_none() {
                self.io.hit(&e.id);
                self.cache.stored = true;
                return;
            }
        }
        let now = chrono::Utc::now().to_rfc3339();
        let prev = self.cache.entry.clone();
        let inputs = &self.spec.inputs;
        let entry = replay::Entry {
            id: prev.as_ref().map(|e| e.id.clone()).unwrap_or_else(|| format!("rpl_{}", uuid::Uuid::new_v4().simple())),
            key: self.cache.key.clone(),
            name: self.spec.save_as.clone().or_else(|| prev.as_ref().and_then(|e| e.name.clone())),
            goal: replay::template(&self.spec.goal, inputs),
            app: self.cache.app.clone(),
            window: self.cache.window.clone(),
            variables: inputs.iter().map(|(n, _)| n.clone()).collect(),
            success: self.spec.exact.iter().map(|c| replay::template_json(c, inputs)).collect(),
            steps: std::mem::take(&mut self.recording),
            hits: prev.as_ref().map_or(0, |e| e.hits) + i64::from(self.cache.replayed > 0),
            heals: prev.as_ref().map_or(0, |e| e.heals) + self.cache.healed.len() as i64,
            created_at: prev.as_ref().map(|e| e.created_at.clone()).unwrap_or_else(|| now.clone()),
            updated_at: now.clone(),
            last_used_at: Some(now),
        };
        match replay::guard(&entry, inputs).and_then(|_| self.io.save(&entry)) {
            Ok(()) => self.cache.stored = true,
            Err(e) => {
                tracing::warn!("run_subtask: recording not stored: {e}");
                self.cache.reason.get_or_insert(format!("not stored: {e}"));
            }
        }
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
            self.io.outcome(&id, st, Some("yes"), Some(detail));
        }
        if status == "done" && success == json!(true) {
            self.store();
        }
        let mut cache = json!({
            "status": self.cache.status,
            "replayed_steps": self.cache.replayed,
            "healed_steps": self.cache.healed,
            "stored": self.cache.stored,
        });
        if self.caching() {
            cache["key"] = json!(self.cache.key);
        }
        if let Some(name) = self.spec.save_as.clone().or_else(|| self.cache.entry.as_ref().and_then(|e| e.name.clone())) {
            cache["skill"] = json!(name);
        }
        if let Some(r) = &self.cache.reason {
            cache["reason"] = json!(r);
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
            "cache": cache,
        });
        if let Some(r) = reason {
            body["reason"] = json!(r);
        }
        if let Some(step) = self.held.take() {
            body["held_step"] = step;
        }
        if status != "done" {
            body["screen"] = screen.compact(60);
            body["next"] = json!(match status {
                "escalated" => "Continue from this screen yourself (read_ui/act/run_batch), or call run_subtask again with a narrower goal.",
                "needs_confirmation" => "The next step (held_step) needs the person's confirmation. If it is what the person asked for, run it yourself with act or run_batch: that call asks the person to approve it.",
                "paused" => "The safety monitor paused the subtask. Stop and call request_human so a person can look at the screen; don't retry the step on your own.",
                "denied" => "The next step isn't allowed on this computer (its app/domain lists or a credential binding). Find another way, or ask the person.",
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
    let io = Live { state, user, computer, target, run_id };
    drive(&io, run_id, spec, None).await
}

/// Run a saved skill (`run_skill`): its recording replays with the given
/// input values, through the same loop (and the same single approval) as
/// run_subtask.
pub async fn run_skill(
    state: &Arc<AppState>,
    user: &AuthUser,
    computer: &ComputerResponse,
    target: &Target,
    input: &Value,
    run_id: &str,
) -> Result<ToolsetResult, Fail> {
    let name = input.get("name").and_then(Value::as_str).map(str::trim).unwrap_or_default();
    let entry = {
        let conn = state.db.connect().map_err(|e| Fail::from(format!("the skill store isn't available: {e}")))?;
        replay::by_name(&conn, &user.user_id, name).map_err(|e| Fail::from(format!("the skill store isn't available: {e}")))?
    }
    .ok_or_else(|| Fail::from(format!("There is no saved skill named \"{name}\". The skills member lists them; run_subtask with save_as teaches one.")))?;
    let io = Live { state, user, computer, target, run_id };
    let spec = skill_spec(&entry, input)?;
    drive(&io, run_id, spec, Some(entry)).await
}

/// The subtask a skill runs as: its goal and success checks with this call's
/// input values, its app unless the caller names one.
fn skill_spec(entry: &replay::Entry, input: &Value) -> Result<Spec, Fail> {
    let given: Vec<(String, String)> = input
        .get("inputs")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|i| Some((i.get("name")?.as_str()?.trim().to_string(), i.get("value")?.as_str()?.to_string()))).collect())
        .unwrap_or_default();
    let has = |n: &str| given.iter().any(|(g, _)| g.eq_ignore_ascii_case(n));
    let missing: Vec<&str> = entry.variables.iter().map(String::as_str).filter(|v| !has(v)).collect();
    let extra: Vec<&str> = given.iter().map(|(g, _)| g.as_str()).filter(|g| !entry.variables.iter().any(|v| v.eq_ignore_ascii_case(g))).collect();
    if !missing.is_empty() || !extra.is_empty() {
        let name = entry.name.as_deref().unwrap_or("this skill");
        return Err(Fail::from(format!(
            "{name} takes the inputs [{}] (missing: [{}], unknown: [{}])",
            entry.variables.join(", "),
            missing.join(", "),
            extra.join(", ")
        )));
    }
    // The skill's own names, in its order, with this call's values.
    let inputs: Vec<Value> = entry
        .variables
        .iter()
        .map(|v| {
            let value = given.iter().find(|(g, _)| g.eq_ignore_ascii_case(v)).map(|(_, x)| x.clone()).unwrap_or_default();
            json!({ "name": v, "value": value })
        })
        .collect();
    let pairs: Vec<(String, String)> =
        inputs.iter().map(|i| (i["name"].as_str().unwrap_or_default().to_string(), i["value"].as_str().unwrap_or_default().to_string())).collect();
    let mut spec = json!({
        "goal": replay::substitute(&entry.goal, &pairs),
        "inputs": inputs,
        "success": replay::substitute_json(&json!(entry.success), &pairs),
        "cache": "auto",
    });
    for k in ["pid", "window_id", "max_steps", "budget_ms", "app"] {
        if let Some(v) = input.get(k) {
            spec[k] = v.clone();
        }
    }
    if spec.get("app").is_none() && spec.get("pid").is_none() && !entry.app.is_empty() {
        spec["app"] = json!(entry.app);
    }
    parse(&spec)
}

/// `skills`: the saved skills, after an optional `forget`.
pub fn skills(state: &Arc<AppState>, user: &AuthUser, input: &Value) -> Result<ToolsetResult, Fail> {
    let conn = state.db.connect().map_err(|e| Fail::from(format!("the skill store isn't available: {e}")))?;
    let mut forgot = None;
    if let Some(name) = input.get("forget").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty()) {
        let gone = replay::forget(&conn, &user.user_id, name).map_err(|e| Fail::from(e.to_string()))?;
        if !gone {
            return Err(Fail::from(format!("There is no saved skill named \"{name}\".")));
        }
        forgot = Some(name.to_string());
    }
    let list = replay::skills(&conn, &user.user_id).map_err(|e| Fail::from(e.to_string()))?;
    let mut body = json!({ "skills": list.iter().map(replay::skill_summary).collect::<Vec<_>>() });
    if let Some(f) = forgot {
        body["forgot"] = json!(f);
    }
    Ok(ToolsetResult {
        is_error: false,
        content: vec![text(serde_json::to_string(&body).unwrap_or_else(|_| "{}".into()))],
        browser_state: None,
        screen: screen_info(None),
        error: None,
    })
}

/// The loop: first read, the replay cache, then decisions until done,
/// escalate, `max_steps` or `budget_ms`. `pinned`: a skill's entry, replayed
/// whatever this screen's key.
async fn drive(io: &dyn Io, run_id: &str, spec: Spec, pinned: Option<replay::Entry>) -> Result<ToolsetResult, Fail> {
    let mode = spec.cache;
    let mut run = Run {
        io,
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
        recording: Vec::new(),
        cache: Cache {
            mode,
            key: String::new(),
            window: String::new(),
            app: String::new(),
            entry: None,
            status: "pending",
            replayed: 0,
            healed: Vec::new(),
            reason: None,
            stored: false,
        },
        held: None,
    };
    let mut screen = Screen::default();
    match drive_loop(&mut run, &mut screen, pinned, mode).await {
        Ok((status, reason, success)) => Ok(run.finish(status, reason, &screen, success)),
        Err(f) => {
            // A step the safety layer refused ends the subtask with a status
            // the planner can act on (held_step says which step).
            let status = match f.code {
                Some("safety_paused") => "paused",
                Some("needs_confirmation") => "needs_confirmation",
                Some("safety_denied") => "denied",
                _ => return Err(f),
            };
            Ok(run.finish(status, Some(f.message), &screen, json!(false)))
        }
    }
}

/// How a subtask ended: status, reason, success.
type Ending = (&'static str, Option<String>, Value);

fn ending(status: &'static str, reason: Option<String>, success: Value) -> Ending {
    (status, reason, success)
}

/// The loop itself: replay a cached recording when there is one, else
/// decide step by step. A step the safety layer refuses ends it with that
/// step's code (`drive` turns it into a status).
async fn drive_loop(run: &mut Run<'_>, screen: &mut Screen, pinned: Option<replay::Entry>, mode: replay::Mode) -> Result<Ending, Fail> {
    let mut typed: HashMap<usize, String> = HashMap::new();
    let mut history: Vec<String> = Vec::new();
    // Actions that ran without changing the screen: not offered again.
    let mut spent: HashSet<String> = HashSet::new();

    run.read(screen).await?;

    // The replay cache: key on this window, then replay a recording.
    if !run.caching() {
        run.cache.status = if mode == replay::Mode::Off { "off" } else { "uncached" };
    } else {
        let app = run.spec.scope.get("app").and_then(Value::as_str).filter(|a| !a.is_empty()).map(str::to_string);
        let app = app.unwrap_or_else(|| screen.window.get("app").and_then(Value::as_str).unwrap_or_default().to_string());
        let title = screen.window.get("title").and_then(Value::as_str).unwrap_or_default().to_string();
        let (key, window) = replay::cache_key(&run.spec.goal, &app, &title, &run.spec.inputs);
        let entry = match (pinned, mode) {
            (Some(e), _) => Some(e),
            (None, replay::Mode::Auto) => run.io.cached(&key),
            _ => None,
        };
        run.cache.key = key;
        run.cache.window = window;
        run.cache.app = app;
        run.cache.status = "miss";
        if let Some(entry) = entry {
            run.cache.status = "hit";
            run.cache.entry = Some(entry.clone());
            // Already done? Exact checks only: a replay makes no decisions.
            if let Some((true, _)) = run.check_exact(screen, 0).await? {
                return Ok(ending("done", Some("the success checks already held".into()), json!(true)));
            }
            if run.replay(&entry, screen, &mut typed, &mut history).await? {
                if !run.cache.healed.is_empty() {
                    run.cache.status = "healed";
                }
                return Ok(ending("done", None, json!(true)));
            }
            // Diverged: the decision loop takes over from this screen.
            run.read(screen).await?;
        }
    }
    // Already done? (A re-issued subtask, or a goal the screen already meets.)
    if let Some(true) = run.met(screen, &typed, &history, 0).await? {
        return Ok(ending("done", Some("the success checks already held".into()), json!(true)));
    }

    loop {
        if run.left().is_zero() {
            let why = format!("budget_ms ({} ms) spent", run.spec.budget.as_millis());
            return Ok(ending("escalated", Some(why), json!(false)));
        }
        if run.actions >= run.spec.max_steps {
            let why = format!("max_steps ({}) reached", run.spec.max_steps);
            return Ok(ending("escalated", Some(why), json!(false)));
        }

        // Fill every input the screen can take in one speculative batch.
        if typed.len() < run.spec.inputs.len() {
            let landed = run.fill(screen, &mut typed, &mut history).await?;
            if landed > 0 {
                if let Some(true) = run.met(screen, &typed, &history, 0).await? {
                    return Ok(ending("done", None, json!(true)));
                }
                run.read(screen).await?;
                continue;
            }
        }

        // One step decision.
        let opts = options(&run.spec, screen, &typed, &spent);
        let ctx = run.context(screen, &typed, &history);
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
            return Ok(ending("escalated", Some(why), json!(false)));
        };
        let (option_text, action) = (opt.text.clone(), opt.action.clone());

        match action {
            Action::Escalate => {
                run.trace_decision("element", &d, Some(&option_text), json!(null));
                run.outcome(&d, "skipped", "escalated to the planner");
                return Ok(ending("escalated", Some("the decision found no option that moves the goal forward".into()), json!(false)));
            }
            Action::Done => {
                // Trust, then verify (give a settling UI a moment).
                let met = run.met(screen, &typed, &history, 2_000).await?;
                run.trace_decision("element", &d, Some(&option_text), json!(null));
                return Ok(match met {
                    Some(false) => {
                        run.outcome(&d, "failure", "picked done but the success checks failed");
                        ending("escalated", Some("the decision said done, but the success checks don't hold".into()), json!(false))
                    }
                    _ => {
                        run.outcome(&d, "success", "success checks held");
                        let unchecked = met.is_none();
                        ending("done", unchecked.then(|| "no success checks were given; the decision judged the goal met".to_string()), json!(!unchecked))
                    }
                });
            }
            _ => {}
        }

        let before = screen.version;
        // For the recording: the element's locator and the screen before.
        let acted = match &action {
            Action::Click { id } | Action::Press { id, .. } | Action::Type { id, .. } => run.locate(screen, id),
            _ => None,
        };
        let before_els: Vec<Value> = if run.caching() { screen.elements().cloned().collect() } else { Vec::new() };
        let (ok, detail) = run.perform(screen, &action).await?;
        let key = action_key(&action);
        if let Action::Type { input, id } = &action {
            if ok {
                typed.insert(*input, id.clone());
            }
        }
        history.push(format!("{option_text}{}", if ok { "" } else { " (failed)" }));
        run.read(screen).await?;
        let changed = screen.version != before;
        if !changed || !ok {
            spent.insert(key);
        }
        let outcome_ok = ok && (changed || matches!(action, Action::Type { .. }));
        if let (true, Some(loc)) = (outcome_ok, acted) {
            match &action {
                Action::Type { input, .. } => run.record(loc, "set_value", Some(*input), None, &option_text, None),
                Action::Click { .. } | Action::Press { .. } => {
                    let b: Vec<&Value> = before_els.iter().collect();
                    let a: Vec<&Value> = screen.elements().collect();
                    let check = replay::derive_check(&b, &a, &run.spec.inputs);
                    let (op, key) = match &action {
                        Action::Press { key, .. } => ("press", Some(key.clone())),
                        _ => ("click", None),
                    };
                    run.record(loc, op, None, key.as_deref(), &option_text, check);
                }
                _ => {}
            }
        }
        run.outcome(&d, if outcome_ok { "success" } else { "failure" }, if outcome_ok { "action ran and the screen changed" } else if ok { "action ran but nothing changed" } else { &detail });
        run.trace_decision("element", &d, Some(&option_text), json!({ "ok": ok, "changed": changed, "detail": if ok { Value::Null } else { json!(detail) } }));

        if let Some(true) = run.met(screen, &typed, &history, 0).await? {
            return Ok(ending("done", None, json!(true)));
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

    // -----------------------------------------------------------------------
    // The replay cache over a simulated app: a sign-up form whose Submit
    // button shows "Thanks, <email>".
    // -----------------------------------------------------------------------

    use std::sync::Mutex;

    struct App {
        version: i64,
        email: String,
        thanks: bool,
        /// The submit button's (id, name): renamed to test self-healing.
        button: (String, String),
        title: String,
        typed: Vec<String>,
        decisions: Vec<String>,
        cache: HashMap<String, replay::Entry>,
        saved: u32,
        hits: u32,
    }

    struct FakeIo(Mutex<App>);

    impl FakeIo {
        fn new() -> Self {
            FakeIo(Mutex::new(App {
                version: 1,
                email: String::new(),
                thanks: false,
                button: ("b1".into(), "Submit".into()),
                title: "Sign up".into(),
                typed: vec![],
                decisions: vec![],
                cache: HashMap::new(),
                saved: 0,
                hits: 0,
            }))
        }
        /// A fresh form (the cache survives).
        fn reset(&self, button: (&str, &str), title: &str) {
            let mut a = self.0.lock().unwrap();
            a.version += 1;
            a.email.clear();
            a.thanks = false;
            a.button = (button.0.into(), button.1.into());
            a.title = title.into();
            a.typed.clear();
            a.decisions.clear();
        }
    }

    fn elements(a: &App) -> Vec<Value> {
        let mut v = vec![
            json!({ "id": "w", "role": "AXWindow", "name": a.title }),
            json!({ "id": "f1", "role": "AXTextField", "name": "Email", "value": a.email }),
            json!({ "id": a.button.0, "role": "AXButton", "name": a.button.1 }),
        ];
        if a.thanks {
            v.push(json!({ "id": "t1", "role": "AXStaticText", "name": format!("Thanks, {}", a.email) }));
        }
        v
    }

    fn holds(els: &[Value], c: &Value) -> bool {
        let mut m: Vec<&Value> = els.iter().collect();
        if let Some(id) = c.get("id").and_then(Value::as_str) {
            m.retain(|e| e["id"] == id);
        }
        if let Some(r) = c.get("role").and_then(Value::as_str) {
            m.retain(|e| e["role"].as_str().unwrap_or("").trim_start_matches("AX").eq_ignore_ascii_case(r.trim_start_matches("AX")));
        }
        if let Some(n) = c.get("name").and_then(Value::as_str) {
            m.retain(|e| e["name"].as_str().unwrap_or("").to_lowercase().contains(&n.to_lowercase()));
        }
        if c.get("gone").and_then(Value::as_bool) == Some(true) {
            return m.is_empty();
        }
        if m.is_empty() {
            return false;
        }
        match c.get("value").and_then(Value::as_str) {
            Some(v) => m.iter().any(|e| e["value"].as_str() == Some(v)),
            None => true,
        }
    }

    impl App {
        fn act(&mut self, a: &Value) -> Result<(), String> {
            let id = a["id"].as_str().unwrap_or("");
            match (a["op"].as_str().unwrap_or("click"), id) {
                ("set_value", "f1") => {
                    self.email = a["value"].as_str().unwrap_or("").to_string();
                    self.typed.push(self.email.clone());
                }
                ("click", b) if b == self.button.0 => self.thanks = !self.email.is_empty(),
                _ => return Err(format!("{id} isn't in the map")),
            }
            self.version += 1;
            Ok(())
        }
        fn map(&self) -> Value {
            json!({
                "version": self.version,
                "window": { "pid": 1, "window_id": 2, "app": "Safari", "title": self.title },
                "elements": elements(self),
            })
        }
    }

    #[async_trait::async_trait]
    impl Io for FakeIo {
        async fn step(&self, member: &str, input: Value) -> Result<Value, Fail> {
            let mut a = self.0.lock().unwrap();
            match member {
                "read_ui" => Ok(a.map()),
                "verify" => {
                    let els = elements(&a);
                    let ok = input["checks"].as_array().unwrap().iter().all(|c| holds(&els, c));
                    Ok(json!({ "ok": ok }))
                }
                "act" => {
                    a.act(&input).map_err(Fail::from)?;
                    Ok(json!({ "status": "done", "version": a.version }))
                }
                "run_batch" => {
                    let mut results = vec![];
                    let mut ok = true;
                    for (i, st) in input["steps"].as_array().unwrap().iter().enumerate() {
                        let fail = |code: &str, why: String| json!({ "i": i, "status": "failed", "code": code, "error": why });
                        if let Some(w) = st.get("wait_for") {
                            if !holds(&elements(&a), w) {
                                results.push(fail("wait_for_failed", "no element matches".into()));
                                ok = false;
                                break;
                            }
                        }
                        if let Err(e) = a.act(&st["act"]) {
                            results.push(fail("element_gone", e));
                            ok = false;
                            break;
                        }
                        if let Some(x) = st.get("expect") {
                            if !holds(&elements(&a), x) {
                                results.push(fail("expect_failed", "no element matches".into()));
                                ok = false;
                                break;
                            }
                        }
                        results.push(json!({ "i": i, "status": "ok" }));
                    }
                    Ok(json!({ "ok": ok, "steps": results, "version": a.version, "changes": a.map() }))
                }
                other => Err(Fail::from(format!("{other} isn't simulated"))),
            }
        }
        async fn decide(&self, body: Value) -> Result<Value, String> {
            // A deterministic decider: the email field, else Submit, else Join.
            let mut a = self.0.lock().unwrap();
            let opts = body["options"].as_array().cloned().unwrap_or_default();
            let pick = ["text field \"Email\"", "click the button \"Submit\"", "click the button \"Join\""]
                .iter()
                .find_map(|want| opts.iter().find(|o| o["text"].as_str().unwrap_or("").contains(want)))
                .and_then(|o| o["id"].as_str().map(str::to_string));
            a.decisions.push(body["question"].as_str().unwrap_or("").to_string());
            let n = a.decisions.len();
            Ok(json!({ "id": format!("d{n}"), "choice": pick, "abstained": pick.is_none(), "confidence": 0.9, "threshold": 0.5 }))
        }
        fn outcome(&self, _id: &str, _status: &str, _label: Option<&str>, _detail: Option<&str>) {}
        fn cached(&self, key: &str) -> Option<replay::Entry> {
            self.0.lock().unwrap().cache.get(key).cloned()
        }
        fn save(&self, entry: &replay::Entry) -> Result<(), String> {
            let mut a = self.0.lock().unwrap();
            a.cache.retain(|_, e| e.id != entry.id);
            a.cache.insert(entry.key.clone(), entry.clone());
            a.saved += 1;
            Ok(())
        }
        fn hit(&self, _id: &str) {
            self.0.lock().unwrap().hits += 1;
        }
    }

    fn signup(email: &str) -> Spec {
        spec(json!({
            "goal": "Sign up",
            "inputs": [{ "name": "email", "value": email }],
            "success": [{ "role": "StaticText", "name": format!("Thanks, {email}") }],
        }))
    }

    async fn go(io: &FakeIo, s: Spec, pinned: Option<replay::Entry>) -> Value {
        let r = drive(io, "run-test", s, pinned).await.expect("subtask ran");
        serde_json::from_str(r.content[0]["text"].as_str().unwrap()).unwrap()
    }

    #[tokio::test]
    async fn record_then_replay_with_zero_decisions_and_new_values() {
        let io = FakeIo::new();
        let first = go(&io, signup("ada@example.com"), None).await;
        assert_eq!(first["status"], "done", "{first}");
        assert_eq!(first["cache"]["status"], "miss");
        assert_eq!(first["cache"]["stored"], true);
        assert!(first["decisions"].as_u64().unwrap() >= 2, "the first run decides");
        let entry = io.0.lock().unwrap().cache.values().next().cloned().expect("recorded");
        assert_eq!(entry.steps.iter().map(|s| s.op.as_str()).collect::<Vec<_>>(), ["set_value", "click"]);
        assert_eq!(entry.steps[1].check, Some(json!({ "role": "StaticText", "name": "Thanks, {{email}}" })));
        // Secrets: no input value anywhere in what was stored.
        let stored = serde_json::to_string(&entry).unwrap();
        assert!(!stored.contains("ada@example.com"), "{stored}");
        assert!(stored.contains("{{email}}"));

        // Same subtask, new value: replays with no decisions, the new value typed.
        io.reset(("b1", "Submit"), "Sign up");
        let again = go(&io, signup("bo@example.org"), None).await;
        assert_eq!(again["status"], "done", "{again}");
        assert_eq!(again["decisions"], 0);
        assert_eq!(again["cache"]["status"], "hit");
        assert_eq!(again["cache"]["replayed_steps"], 2);
        let a = io.0.lock().unwrap();
        assert!(a.decisions.is_empty());
        assert_eq!(a.typed, ["bo@example.org"]);
        assert_eq!(a.hits, 1);
    }

    #[tokio::test]
    async fn a_stale_step_is_reinferred_once_and_the_entry_rewritten() {
        let io = FakeIo::new();
        go(&io, signup("ada@example.com"), None).await;
        let before = io.0.lock().unwrap().cache.values().next().cloned().unwrap();
        // The button moved and was renamed: no id, name or path finds it.
        io.reset(("b2", "Join"), "Sign up");
        let r = go(&io, signup("bo@example.org"), None).await;
        assert_eq!(r["status"], "done", "{r}");
        assert_eq!(r["decisions"], 1, "one decision, for that step only");
        assert_eq!(r["cache"]["status"], "healed");
        assert_eq!(r["cache"]["healed_steps"][0]["via"], "decision");
        let after = io.0.lock().unwrap().cache.values().next().cloned().unwrap();
        assert_eq!(after.id, before.id, "the same entry, rewritten");
        assert_eq!(after.steps[1].locator.name, "Join");
        assert_eq!(after.steps[1].locator.id, "b2");
        assert_eq!(after.heals, 1);
        // The healed entry now replays with no decisions.
        io.reset(("b2", "Join"), "Sign up");
        let r = go(&io, signup("cy@example.net"), None).await;
        assert_eq!((r["decisions"].as_u64(), r["cache"]["status"].as_str()), (Some(0), Some("hit")), "{r}");
    }

    #[tokio::test]
    async fn another_window_or_input_name_is_a_miss_and_off_skips_the_cache() {
        let io = FakeIo::new();
        go(&io, signup("ada@example.com"), None).await;
        io.reset(("b1", "Submit"), "Settings");
        let r = go(&io, signup("bo@example.org"), None).await;
        assert_eq!(r["cache"]["status"], "miss", "another window shape");
        assert!(r["decisions"].as_u64().unwrap() > 0);
        io.reset(("b1", "Submit"), "Sign up");
        let renamed = spec(json!({
            "goal": "Sign up",
            "inputs": [{ "name": "mail", "value": "bo@example.org" }],
            "success": [{ "role": "StaticText", "name": "Thanks, bo@example.org" }],
        }));
        assert_eq!(go(&io, renamed, None).await["cache"]["status"], "miss", "another input name");
        io.reset(("b1", "Submit"), "Sign up");
        let saved = io.0.lock().unwrap().saved;
        let mut off = signup("bo@example.org");
        off.cache = replay::Mode::Off;
        let r = go(&io, off, None).await;
        assert_eq!(r["cache"]["status"], "off");
        assert_eq!(io.0.lock().unwrap().saved, saved, "off never writes");
        // No exact success check: never cached.
        io.reset(("b1", "Submit"), "Sign up");
        let fuzzy = spec(json!({ "goal": "Sign up", "inputs": [{ "name": "email", "value": "x@y.z" }], "success": [{ "ask": "Thanked?" }] }));
        assert_eq!(go(&io, fuzzy, None).await["cache"]["status"], "uncached");
    }

    #[tokio::test]
    async fn a_saved_skill_runs_by_name_with_its_inputs() {
        let io = FakeIo::new();
        let mut taught = signup("ada@example.com");
        taught.save_as = Some("signup".into());
        go(&io, taught, None).await;
        let entry = io.0.lock().unwrap().cache.values().next().cloned().unwrap();
        assert_eq!(entry.name.as_deref(), Some("signup"));
        // Inputs are checked against the skill's names.
        assert!(skill_spec(&entry, &json!({ "name": "signup" })).is_err());
        assert!(skill_spec(&entry, &json!({ "name": "signup", "inputs": [{ "name": "email", "value": "a@b.c" }, { "name": "x", "value": "1" }] })).is_err());
        let s = skill_spec(&entry, &json!({ "name": "signup", "inputs": [{ "name": "email", "value": "dee@example.com" }] })).unwrap();
        assert_eq!(s.exact, vec![json!({ "role": "StaticText", "name": "Thanks, dee@example.com" })]);
        assert_eq!(s.scope.get("app"), Some(&json!("Safari")));
        // Pinned: runs even in another window shape, with no decisions.
        io.reset(("b1", "Submit"), "Sign up — draft 2");
        let r = go(&io, s, Some(entry)).await;
        assert_eq!((r["status"].as_str(), r["decisions"].as_u64()), (Some("done"), Some(0)), "{r}");
        assert_eq!(r["cache"]["skill"], "signup");
        assert_eq!(io.0.lock().unwrap().typed, ["dee@example.com"]);
    }
}
