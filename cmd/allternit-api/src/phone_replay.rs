//! Phone skills: record once, replay with no model (the `computer_replay`
//! design over phone screens).
//!
//! * **Record.** Every phone tool call is already in the run history
//!   (`phone_runs`). A run, or a step range of it, saved by name becomes a
//!   skill: each acting step (tap, type, swipe, key, back, home, open app,
//!   find-and-tap) with a semantic locator for its element — class, label
//!   (text or content-desc), resource id, screen-relative box and an 8x8
//!   pixel hash — and, when the next screen read shows it, a check of what
//!   the step changed (an element that appeared or went away).
//! * **Replay.** `POST /phones/skills/:name/run` drives the Desktop phone
//!   gateway (`~/.allternit/phone-gateway.json`) step by step with no model:
//!   read the screen, find the step's element again (same id + label, else
//!   the one element with that label, else with that resource id, else the
//!   one element in the same place that still looks the same), act, check.
//! * **Self-heal.** A step whose element can't be placed gets one
//!   re-decision through the same gizzi model path the decision runtime's
//!   oracle uses (`gizzi_completion::complete_decision`) — the bot's brain
//!   when the caller names one. A healed step is written back.
//! * **Secrets.** Typed text is never stored: history redacts it, and a type
//!   step stores only `{{input}}`; the value comes with each run. Skills
//!   never send texts or place calls.
//! * **Holds.** Saving marks each step the phone guard classes as sensitive
//!   (`sensitive`, `effect`; additive, old files load without them). On
//!   replay every acting step goes through [`phone_guard::gate`] before it
//!   acts, flagged or not (the flag is a hint, the guard decides): the run
//!   waits up to five minutes for the owner, the wait is left out of
//!   `duration_ms`, and the report carries `held_ms` and `holds`.
use crate::{auth::AuthUser, AppState};
use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{
    path::{Path as FsPath, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use crate::phone_guard::{self, CheckIn, El, Gate, Reread, TaskCtx};
use crate::phone_runs;

/// Tests answer phone tool calls with this instead of Desktop's gateway.
#[cfg(test)]
pub(crate) type FakeTool = Arc<dyn Fn(&str, &Value) -> Result<Value, String> + Send + Sync>;
#[cfg(test)]
tokio::task_local! {
    pub(crate) static FAKE_TOOL: FakeTool;
}

/// Pixel-hash distance (of 64 bits) that still counts as the same element.
const CROP_TOLERANCE: u32 = 10;
const MIN_IOU: f64 = 0.5;
/// A step's check (and a step's element) is waited for this long.
const STEP_TIMEOUT: Duration = Duration::from_secs(5);
const GATEWAY_TIMEOUT: Duration = Duration::from_secs(45);
const MAX_SKILL_STEPS: usize = 80;
const ABSTAIN: &str = "__abstain__";

/// Ops a skill can hold (reads are dropped; texts and calls are never replayed).
const ACTING: [&str; 8] = [
    "phone.tap",
    "phone.type",
    "phone.swipe",
    "phone.key",
    "phone.home",
    "phone.back",
    "phone.open_app",
    "phone.find_and_tap",
];

// ─── The Desktop phone gateway ─────────────────────────────────────────────

fn gateway_file() -> Option<PathBuf> {
    std::env::var_os("ALLTERNIT_PHONE_GATEWAY_FILE")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".allternit").join("phone-gateway.json")))
}

/// Whether Desktop's phone gateway is up on this machine.
pub fn gateway_available() -> bool {
    gateway_file().and_then(|p| std::fs::read(p).ok()).is_some()
}

/// One phone tool call through Desktop: `{name, arguments}` → the tool result.
pub async fn call_tool(name: &str, args: Value) -> Result<Value, String> {
    call_tool_within(name, args, GATEWAY_TIMEOUT).await
}

/// [`call_tool`] with its own time limit (a text or call waits on approval).
pub async fn call_tool_within(name: &str, args: Value, timeout: Duration) -> Result<Value, String> {
    #[cfg(test)]
    if let Ok(r) = FAKE_TOOL.try_with(|f| f(name, &args)) {
        return r;
    }
    let file = gateway_file().ok_or("no home directory")?;
    let raw = tokio::fs::read(&file)
        .await
        .map_err(|_| "Allternit Desktop's phone gateway isn't running on this computer".to_string())?;
    let gw: Value = serde_json::from_slice(&raw).map_err(|e| format!("phone gateway file unreadable: {e}"))?;
    let (url, token) = (
        gw.get("url").and_then(Value::as_str).ok_or("phone gateway file has no url")?,
        gw.get("token").and_then(Value::as_str).ok_or("phone gateway file has no token")?,
    );
    let res = crate::net_client::client()
        .post(format!("{}/phone/call", url.trim_end_matches('/')))
        .header("x-allternit-phone-token", token)
        .json(&json!({ "name": name, "arguments": args }))
        .timeout(timeout)
        .send()
        .await
        .map_err(|e| if e.is_timeout() { "phone gateway timed out".to_string() } else { format!("phone gateway unreachable: {e}") })?;
    res.json::<Value>().await.map_err(|e| format!("phone gateway answered badly: {e}"))
}

fn tool_error(v: &Value) -> Option<String> {
    if v.get("ok").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    Some(
        v.get("message")
            .or_else(|| v.get("error"))
            .and_then(Value::as_str)
            .unwrap_or("the phone tool failed")
            .to_string(),
    )
}

/// Call a tool, with `serial`, `_run_id` and `_source` added (Desktop records
/// the step under that run and strips the underscore keys).
pub async fn act(name: &str, mut args: Map<String, Value>, serial: Option<&str>, run_id: &str, source: &str) -> Result<Value, String> {
    if let Some(s) = serial {
        args.insert("serial".into(), json!(s));
    }
    args.insert("_run_id".into(), json!(run_id));
    args.insert("_source".into(), json!(source));
    let v = call_tool(name, Value::Object(args)).await?;
    match tool_error(&v) {
        Some(e) => Err(e),
        None => Ok(v),
    }
}

// ─── Screens ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiEl {
    pub class: String,
    pub label: String,
    pub id: String,
    pub rect: [f64; 4],
}

impl UiEl {
    pub fn center(&self) -> (i64, i64) {
        ((self.rect[0] + self.rect[2] / 2.0).round() as i64, (self.rect[1] + self.rect[3] / 2.0).round() as i64)
    }
    pub fn describe(&self) -> String {
        let short = self.class.rsplit('.').next().unwrap_or(&self.class);
        let mut s = short.to_string();
        if !self.label.is_empty() {
            s.push_str(&format!(" \"{}\"", self.label.chars().take(60).collect::<String>()));
        }
        if !self.id.is_empty() {
            s.push_str(&format!(" [{}]", self.id.rsplit('/').next().unwrap_or(&self.id)));
        }
        s
    }
}

/// One `read_ui` line: `class "label" [id] {{x, y}, {w, h}}` (empty parts omitted).
pub fn parse_line(line: &str) -> Option<UiEl> {
    let line = line.trim();
    let open = line.rfind("{{")?;
    let nums: Vec<f64> = line[open..]
        .split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();
    if nums.len() != 4 {
        return None;
    }
    let mut rest = line[..open].trim();
    let mut id = String::new();
    if rest.ends_with(']') {
        if let Some(i) = rest.rfind(" [") {
            id = rest[i + 2..rest.len() - 1].to_string();
            rest = rest[..i].trim();
        }
    }
    let (class, label) = match rest.find(' ') {
        Some(i) => {
            let l = rest[i + 1..].trim();
            (rest[..i].to_string(), l.strip_prefix('"').and_then(|l| l.strip_suffix('"')).unwrap_or(l).to_string())
        }
        None => (rest.to_string(), String::new()),
    };
    if class.is_empty() {
        return None;
    }
    Some(UiEl { class, label, id, rect: [nums[0], nums[1], nums[2], nums[3]] })
}

pub fn parse_ui(text: &str) -> Vec<UiEl> {
    text.lines().filter_map(parse_line).collect()
}

/// The screen's size: the extent of its elements.
pub fn screen_of(els: &[UiEl]) -> (f64, f64) {
    let w = els.iter().map(|e| e.rect[0] + e.rect[2]).fold(0.0, f64::max);
    let h = els.iter().map(|e| e.rect[1] + e.rect[3]).fold(0.0, f64::max);
    (w.max(1.0), h.max(1.0))
}

fn rel(e: &UiEl, screen: (f64, f64)) -> [f64; 4] {
    [e.rect[0] / screen.0, e.rect[1] / screen.1, e.rect[2] / screen.0, e.rect[3] / screen.1]
}

fn iou(a: [f64; 4], b: [f64; 4]) -> f64 {
    let (x1, y1) = (a[0].max(b[0]), a[1].max(b[1]));
    let (x2, y2) = ((a[0] + a[2]).min(b[0] + b[2]), (a[1] + a[3]).min(b[1] + b[3]));
    let inter = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
    let union = a[2] * a[3] + b[2] * b[3] - inter;
    if union <= 0.0 {
        0.0
    } else {
        inter / union
    }
}

/// 8x8 average hash (hex) of a screen-relative box of an image.
pub fn crop_hash(img: &image::DynamicImage, rel: [f64; 4]) -> Option<String> {
    let (w, h) = (img.width() as f64, img.height() as f64);
    let (x, y) = ((rel[0] * w).max(0.0) as u32, (rel[1] * h).max(0.0) as u32);
    let (cw, ch) = ((rel[2] * w) as u32, (rel[3] * h) as u32);
    if cw < 2 || ch < 2 || x + cw > img.width() || y + ch > img.height() {
        return None;
    }
    let small = img.crop_imm(x, y, cw, ch).resize_exact(8, 8, image::imageops::FilterType::Triangle).to_luma8();
    let px: Vec<u32> = small.pixels().map(|p| p.0[0] as u32).collect();
    let mean = px.iter().sum::<u32>() / 64;
    let bits = px.iter().enumerate().fold(0u64, |acc, (i, p)| if *p > mean { acc | (1 << i) } else { acc });
    Some(format!("{bits:016x}"))
}

fn crop_distance(a: &str, b: &str) -> Option<u32> {
    Some((u64::from_str_radix(a, 16).ok()? ^ u64::from_str_radix(b, 16).ok()?).count_ones())
}

// ─── Templates (values out, back in) ───────────────────────────────────────

fn placeholder(name: &str) -> String {
    format!("{{{{{name}}}}}")
}

pub fn substitute(s: &str, inputs: &[(String, String)]) -> String {
    let mut out = s.to_string();
    for (n, v) in inputs {
        out = out.replace(&placeholder(n), v);
    }
    out
}

fn substitute_json(v: &Value, inputs: &[(String, String)]) -> Value {
    match v {
        Value::String(s) => Value::String(substitute(s, inputs)),
        Value::Array(a) => Value::Array(a.iter().map(|x| substitute_json(x, inputs)).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), substitute_json(x, inputs))).collect()),
        other => other.clone(),
    }
}

// ─── Skills ────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Locator {
    pub class: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub resource_id: String,
    /// Screen-relative x, y, w, h (0..1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rel: Option<[f64; 4]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crop: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PStep {
    /// The phone tool (`phone.tap`, …).
    pub op: String,
    /// Arguments (no serial); type steps hold `{"text": "{{input}}"}`.
    #[serde(default)]
    pub args: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locator: Option<Locator>,
    pub label: String,
    /// `{label, gone?}`: what the step changed on screen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<Value>,
    /// The phone guard classed this step sensitive when it was saved. A hint:
    /// the guard classifies every step again at run time.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sensitive: bool,
    /// The effect behind `sensitive` (`pay`, `delete`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub inputs: Vec<String>,
    pub steps: Vec<PStep>,
    #[serde(default)]
    pub serial: Option<String>,
    #[serde(default)]
    pub source_run: Option<String>,
    #[serde(default)]
    pub runs: i64,
    #[serde(default)]
    pub heals: i64,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub last_used_at: Option<String>,
}

pub fn summary(s: &Skill) -> Value {
    json!({
        "name": s.name,
        "description": s.description,
        "inputs": s.inputs,
        "steps": s.steps.len(),
        "serial": s.serial,
        "source_run": s.source_run,
        "runs": s.runs,
        "healed": s.heals,
        "created_at": s.created_at,
        "updated_at": s.updated_at,
        "last_used_at": s.last_used_at,
    })
}

fn skills_dir(data_dir: &FsPath) -> PathBuf {
    data_dir.join("phone-skills")
}

fn skill_path(data_dir: &FsPath, name: &str) -> PathBuf {
    skills_dir(data_dir).join(format!("{}.json", hex::encode(Sha256::digest(name.trim().to_lowercase().as_bytes()))))
}

fn valid_name(name: &str) -> Result<String, String> {
    let n = name.trim();
    if n.is_empty() || n.chars().count() > 80 {
        return Err("a skill name is 1–80 characters".into());
    }
    Ok(n.to_string())
}

pub fn load(data_dir: &FsPath, name: &str) -> Option<Skill> {
    serde_json::from_slice(&std::fs::read(skill_path(data_dir, name)).ok()?).ok()
}

/// The element a step concerns: a tap's recorded element, a find-and-tap's
/// match, or (for type and key) the element tapped last.
fn step_el(step: &PStep, last: Option<&El>) -> Option<El> {
    match step.op.as_str() {
        "phone.tap" => step.locator.as_ref().map(|l| El { class: l.class.clone(), label: l.label.clone(), id: l.resource_id.clone() }),
        "phone.find_and_tap" => {
            let a = |k: &str| step.args.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
            Some(El { class: String::new(), label: a("match"), id: a("element_id") })
        }
        "phone.type" | "phone.key" => last.cloned(),
        _ => None,
    }
}

/// What the guard's rules say about one step. A find-and-tap is judged by the
/// label it looks for, like a tap on that element.
pub fn step_effect(op: &str, args: &Map<String, Value>, el: Option<&El>, app: Option<&str>) -> Option<&'static str> {
    if op == "phone.find_and_tap" {
        return el.and_then(phone_guard::label_effect);
    }
    phone_guard::rule_effect(&phone_guard::Action { tool: op, args, el, app })
}

/// Mark every step the guard classes as sensitive (`sensitive`, `effect`).
/// A flag is never removed here; the guard decides again at run time.
pub fn mark_sensitive(s: &mut Skill) {
    let (mut last, mut app): (Option<El>, Option<String>) = (None, None);
    for step in s.steps.iter_mut() {
        let el = step_el(step, last.as_ref());
        let args = step.args.as_object().cloned().unwrap_or_default();
        if let Some(effect) = step_effect(&step.op, &args, el.as_ref(), app.as_deref()) {
            step.sensitive = true;
            step.effect = Some(effect.to_string());
        }
        match step.op.as_str() {
            "phone.tap" | "phone.find_and_tap" => last = el,
            "phone.open_app" => app = args.get("app").and_then(Value::as_str).map(str::to_string),
            _ => {}
        }
    }
}

pub fn store(data_dir: &FsPath, s: &Skill) -> Result<(), String> {
    let mut s = s.clone();
    mark_sensitive(&mut s);
    let s = &s;
    let dir = skills_dir(data_dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    std::fs::write(skill_path(data_dir, &s.name), serde_json::to_vec_pretty(s).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

pub fn list(data_dir: &FsPath) -> Vec<Skill> {
    let mut out: Vec<Skill> = std::fs::read_dir(skills_dir(data_dir))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|e| serde_json::from_slice::<Skill>(&std::fs::read(e.path()).ok()?).ok())
        .collect();
    out.sort_by(|a, b| b.last_used_at.as_ref().unwrap_or(&b.updated_at).cmp(a.last_used_at.as_ref().unwrap_or(&a.updated_at)));
    out
}

pub fn forget(data_dir: &FsPath, name: &str) -> bool {
    std::fs::remove_file(skill_path(data_dir, name)).is_ok()
}

/// Which steps of a run: an index (0-based, in the run's order) or a step id.
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum StepRef {
    Index(usize),
    Id(String),
}

#[derive(Debug, Default, Deserialize)]
pub struct SaveIn {
    pub run_id: String,
    pub name: String,
    #[serde(default)]
    pub serial: Option<String>,
    #[serde(default)]
    pub from_step: Option<StepRef>,
    #[serde(default)]
    pub to_step: Option<StepRef>,
    #[serde(default)]
    pub description: Option<String>,
    /// Names for the typed inputs, in order (default `text1`, `text2`, …).
    #[serde(default)]
    pub input_names: Vec<String>,
}

fn position(steps: &[phone_runs::Step], r: &StepRef) -> Option<usize> {
    match r {
        StepRef::Index(i) => (*i < steps.len()).then_some(*i),
        StepRef::Id(id) => steps.iter().position(|s| &s.id == id),
    }
}

/// What a step changed, from the screens before and after it: an element that
/// appeared, else one that went away. Labels only, never field values of
/// password fields.
pub fn derive_check(before: &[UiEl], after: &[UiEl]) -> Option<Value> {
    let usable = |e: &UiEl| {
        let n = e.label.trim();
        !n.is_empty() && n.chars().count() <= 80 && !e.class.to_lowercase().contains("password") && !n.contains("{{")
    };
    let has = |set: &[UiEl], l: &str| set.iter().any(|e| e.label == l);
    if let Some(e) = after.iter().find(|e| usable(e) && !has(before, &e.label)) {
        return Some(json!({ "label": e.label }));
    }
    before.iter().find(|e| usable(e) && !has(after, &e.label)).map(|e| json!({ "label": e.label, "gone": true }))
}

fn frame_image(data_dir: &FsPath, step: &phone_runs::Step) -> Option<image::DynamicImage> {
    image::load_from_memory(&phone_runs::frame(data_dir, &step.serial, &step.id)?).ok()
}

/// Build a skill from history.
pub fn from_run(data_dir: &FsPath, body: &SaveIn) -> Result<Skill, String> {
    let name = valid_name(&body.name)?;
    let all: Vec<phone_runs::Step> = match body.serial.as_deref() {
        Some(s) => phone_runs::steps(data_dir, s, Some(&body.run_id)),
        None => std::fs::read_dir(phone_runs::root(data_dir))
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .flat_map(|d| phone_runs::prune(&d.path()).unwrap_or_default())
            .filter(|s| s.run_id == body.run_id)
            .collect(),
    };
    if all.is_empty() {
        return Err(format!("no steps for run {} in the phone history", body.run_id));
    }
    let from = body.from_step.as_ref().map(|r| position(&all, r).ok_or("from_step isn't a step of that run")).transpose()?.unwrap_or(0);
    let to = body.to_step.as_ref().map(|r| position(&all, r).ok_or("to_step isn't a step of that run")).transpose()?.unwrap_or(all.len() - 1);
    if from > to {
        return Err("from_step comes after to_step".into());
    }
    let mut steps = Vec::new();
    let mut inputs: Vec<String> = Vec::new();
    for i in from..=to {
        let s = &all[i];
        if !s.ok || !ACTING.contains(&s.tool.as_str()) {
            continue;
        }
        let mut args: Map<String, Value> = s.args.as_object().cloned().unwrap_or_default();
        args.remove("serial");
        let mut label = s.label.clone();
        if s.tool == "phone.type" {
            let name = match args.get("text").and_then(Value::as_str) {
                Some(t) if t.starts_with("{{") && t.ends_with("}}") => t[2..t.len() - 2].to_string(),
                _ => body.input_names.get(inputs.len()).cloned().unwrap_or_else(|| format!("text{}", inputs.len() + 1)),
            };
            args.insert("text".into(), json!(placeholder(&name)));
            label = format!("type {{{{{name}}}}}");
            if !inputs.contains(&name) {
                inputs.push(name);
            }
        }
        // The element a tap landed on (Desktop attaches it from the last read).
        let locator = s.target.as_ref().and_then(|t| {
            let rect: Vec<f64> = t.get("rect")?.as_array()?.iter().filter_map(Value::as_f64).collect();
            let screen: Vec<f64> = t.get("screen").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
            let el = UiEl {
                class: t.get("class").and_then(Value::as_str).unwrap_or_default().to_string(),
                label: t.get("label").and_then(Value::as_str).unwrap_or_default().to_string(),
                id: t.get("id").and_then(Value::as_str).unwrap_or_default().to_string(),
                rect: [*rect.first()?, *rect.get(1)?, *rect.get(2)?, *rect.get(3)?],
            };
            let size = if screen.len() == 2 { (screen[0].max(1.0), screen[1].max(1.0)) } else { s.ui.as_deref().map(|u| screen_of(&parse_ui(u))).unwrap_or((1.0, 1.0)) };
            let r = rel(&el, size);
            // The pixel hash from the last frame before this step in the run.
            let crop = all[..=i].iter().rev().find(|p| p.snapshot).and_then(|p| frame_image(data_dir, p)).and_then(|img| crop_hash(&img, r));
            Some(Locator { class: el.class, label: el.label, resource_id: el.id, rel: Some(r), crop })
        });
        // The check: the screen read right after this step, against the one before it.
        let before = s.ui.as_deref().map(parse_ui).unwrap_or_default();
        let after = all[i + 1..]
            .iter()
            .take_while(|n| !ACTING.contains(&n.tool.as_str()))
            .find(|n| n.tool == "phone.read_ui" && n.ok)
            .and_then(|n| n.ui.as_deref())
            .map(parse_ui);
        let check = match after {
            Some(after) if !before.is_empty() => derive_check(&before, &after),
            _ => None,
        };
        if let Some(l) = &locator {
            if !l.label.is_empty() && s.tool == "phone.tap" {
                label = format!("tap \"{}\"", l.label);
            }
        }
        steps.push(PStep { op: s.tool.clone(), args: Value::Object(args), locator, label, check, ..Default::default() });
    }
    if steps.is_empty() {
        return Err("that run has no successful phone actions to save (reads alone aren't a skill)".into());
    }
    if steps.len() > MAX_SKILL_STEPS {
        return Err(format!("a skill holds at most {MAX_SKILL_STEPS} steps; save a step range"));
    }
    let now = chrono::Utc::now().to_rfc3339();
    let created_at = load(data_dir, &name).map(|s| s.created_at).unwrap_or_else(|| now.clone());
    let mut skill = Skill {
        name,
        description: body.description.clone().unwrap_or_default().chars().take(500).collect(),
        inputs,
        steps,
        serial: Some(all[0].serial.clone()),
        source_run: Some(body.run_id.clone()),
        runs: 0,
        heals: 0,
        created_at,
        updated_at: now,
        last_used_at: None,
    };
    mark_sensitive(&mut skill);
    Ok(skill)
}

// ─── Replay ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    Id,
    Name,
    Resource,
    Pixels,
}

impl Via {
    pub fn as_str(self) -> &'static str {
        match self {
            Via::Id => "id",
            Via::Name => "name",
            Via::Resource => "resource",
            Via::Pixels => "pixels",
        }
    }
}

/// Find a locator's element on the current screen. Ambiguity is a miss.
pub fn resolve(loc: &Locator, els: &[UiEl], screen: (f64, f64), crop_of: &dyn Fn(&UiEl) -> Option<String>) -> Option<(UiEl, Via)> {
    let same: Vec<&UiEl> = els.iter().filter(|e| e.class == loc.class).collect();
    let one = |m: Vec<&UiEl>, via: Via| if m.len() == 1 { Some((m[0].clone(), via)) } else { None };
    if !loc.resource_id.is_empty() {
        if let Some(hit) = one(same.iter().copied().filter(|e| e.id == loc.resource_id && e.label == loc.label).collect(), Via::Id) {
            return Some(hit);
        }
    }
    if !loc.label.is_empty() {
        if let Some(hit) = one(same.iter().copied().filter(|e| e.label == loc.label).collect(), Via::Name) {
            return Some(hit);
        }
    }
    if !loc.resource_id.is_empty() {
        if let Some(hit) = one(same.iter().copied().filter(|e| e.id == loc.resource_id).collect(), Via::Resource) {
            return Some(hit);
        }
    }
    let b = loc.rel?;
    let near: Vec<&UiEl> = same
        .iter()
        .copied()
        .filter(|e| iou(rel(*e, screen), b) >= MIN_IOU)
        .filter(|e| {
            let renamed = e.label != loc.label;
            match loc.crop.as_deref() {
                Some(a) => crop_of(*e).and_then(|c| crop_distance(a, &c)).is_some_and(|d| d <= CROP_TOLERANCE),
                None => !renamed,
            }
        })
        .collect();
    one(near, Via::Pixels)
}

struct Screen {
    els: Vec<UiEl>,
    size: (f64, f64),
}

async fn read_screen(serial: Option<&str>, run_id: &str, source: &str) -> Result<Screen, String> {
    let mut a = Map::new();
    a.insert("max_elements".into(), json!(1000));
    let v = act("phone.read_ui", a, serial, run_id, source).await?;
    let els = parse_ui(v.get("text").and_then(Value::as_str).unwrap_or_default());
    Ok(Screen { size: screen_of(&els), els })
}

/// A screenshot as a decoded image and a data URL.
pub async fn screenshot(serial: Option<&str>, run_id: &str, source: &str) -> Option<(image::DynamicImage, String)> {
    let v = act("phone.screenshot", Map::new(), serial, run_id, source).await.ok()?;
    let data = v.get("dataBase64").and_then(Value::as_str)?;
    let mime = v.get("mimeType").and_then(Value::as_str).unwrap_or("image/png");
    let img = image::load_from_memory(&B64.decode(data).ok()?).ok()?;
    Some((img, format!("data:{mime};base64,{data}")))
}

fn check_holds(check: &Value, els: &[UiEl], inputs: &[(String, String)]) -> bool {
    let label = substitute(check.get("label").and_then(Value::as_str).unwrap_or_default(), inputs);
    let present = els.iter().any(|e| e.label == label);
    if check.get("gone").and_then(Value::as_bool).unwrap_or(false) {
        !present
    } else {
        present
    }
}

/// The one re-decision: which element on this screen is the step's element.
/// Ask the model (the bot's brain when given) with the screen outline and frame.
async fn redecide(step: &PStep, screen: &Screen, image: Option<&str>, model: Option<&(String, String)>) -> Result<Option<UiEl>, String> {
    let options: Vec<&UiEl> = screen.els.iter().filter(|e| !e.label.is_empty() || !e.id.is_empty()).take(150).collect();
    if options.is_empty() {
        return Ok(None);
    }
    let mut ids: Vec<String> = (0..options.len()).map(|i| format!("e{i}")).collect();
    let mut listing = String::new();
    for (i, e) in options.iter().enumerate() {
        listing.push_str(&format!("e{i}: {}\n", e.describe()));
    }
    let want = step
        .locator
        .as_ref()
        .map(|l| {
            let mut s = l.class.rsplit('.').next().unwrap_or(&l.class).to_string();
            if !l.label.is_empty() {
                s.push_str(&format!(" \"{}\"", l.label));
            }
            if !l.resource_id.is_empty() {
                s.push_str(&format!(" [{}]", l.resource_id));
            }
            s
        })
        .unwrap_or_else(|| step.label.clone());
    let prompt = format!(
        "A recorded phone step must run again but its element moved.\nStep: {}\nRecorded element: {want}\n\nElements on the screen now:\n{}\nWhich element is the recorded one? If none is, answer \"{ABSTAIN}\".",
        step.label,
        crate::computer_safety::spotlight("phone screen", &listing, &[]),
    );
    ids.push(ABSTAIN.into());
    let schema = json!({
        "type": "object",
        "properties": { "choice": { "type": "string", "enum": ids } },
        "required": ["choice"],
        "additionalProperties": false,
    });
    let images: Vec<String> = image.map(|i| vec![i.to_string()]).unwrap_or_default();
    let (text, _) = crate::gizzi_completion::complete_decision(
        &prompt,
        &images,
        Some("You place recorded phone steps on a changed screen. Pick exactly one of the given element ids. Never invent one."),
        model,
        &schema,
    )
    .await?;
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let choice = crate::agency_api::decisions::backends::parse_choice(&text, &refs);
    Ok(choice.and_then(|c| c.strip_prefix('e')).and_then(|n| n.parse::<usize>().ok()).and_then(|i| options.get(i).map(|e| (*e).clone())))
}

#[derive(Debug, Default, Deserialize)]
pub struct RunIn {
    #[serde(default)]
    pub serial: Option<String>,
    #[serde(default)]
    pub inputs: Map<String, Value>,
    /// The bot whose brain re-decides a moved step.
    #[serde(default)]
    pub bot_id: Option<String>,
    /// Or a brain as `provider/model`.
    #[serde(default)]
    pub brain: Option<String>,
    /// The conversation the run belongs to (its holds land there).
    #[serde(default)]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    /// Answer 202 `{run_id, status: "running"}` at once and replay in the
    /// background; the end is the `phone.run.completed` event (the Platform
    /// API uses this: a relayed request can't wait for a whole replay).
    #[serde(default)]
    pub background: bool,
}

pub fn inputs_of(map: &Map<String, Value>) -> Vec<(String, String)> {
    map.iter()
        .map(|(k, v)| (k.clone(), v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())))
        .collect()
}

/// The model a bot's turn uses (`agents.provider/model`), or a `provider/model` string.
pub fn brain(db: &crate::db::DbHandle, bot_id: Option<&str>, brain: Option<&str>) -> Option<(String, String)> {
    if let Some((p, m)) = brain.and_then(|b| b.split_once('/')) {
        if !p.is_empty() && !m.is_empty() {
            return Some((p.to_string(), m.to_string()));
        }
    }
    let id = bot_id?;
    let conn = db.connect().ok()?;
    let (p, m): (Option<String>, Option<String>) = conn
        .query_row("SELECT provider, model FROM agents WHERE id = ?1", rusqlite::params![id], |r| Ok((r.get(0)?, r.get(1)?)))
        .ok()?;
    match (p, m) {
        (Some(p), Some(m)) if !p.is_empty() && !m.is_empty() => {
            let m = m.strip_prefix(&format!("{p}/")).map(str::to_string).unwrap_or(m);
            Some((p, m))
        }
        _ => None,
    }
}

/// A fresh skill run id (`skill-<hex>`).
pub fn new_skill_run_id() -> String {
    format!("skill-{}", uuid::Uuid::new_v4().simple())
}

/// One step of a replay through the phone guard. `flag` is the step's saved
/// flag (`Some("")` = sensitive with no named effect); the guard's rules run
/// either way. `Ok` = the args to act with (`_pass`, and `_hold` when a hold
/// was used); `Err` = `(run status, why)` and the replay stops.
#[allow(clippy::too_many_arguments)]
async fn guard_step(
    db: &crate::db::DbHandle,
    data_dir: &FsPath,
    ctx: &TaskCtx,
    at: (Option<&str>, &str, &str),
    step: (usize, &str, &str),
    op: &str,
    args: &Map<String, Value>,
    el: Option<&El>,
    app: Option<&str>,
    flag: Option<String>,
    holds: &mut Vec<Value>,
    held: &mut Duration,
) -> Result<Map<String, Value>, (&'static str, String)> {
    let (serial, run_id, source) = at;
    let (index, label, goal) = step;
    let flag = step_effect(op, args, el, app).map(str::to_string).or(flag);
    // The owner sees the screen with the hold; a plain step needs no frame.
    let shot = match flag {
        Some(_) => screenshot(serial, run_id, source).await.map(|(_, url)| url),
        None => None,
    };
    let mut req = CheckIn {
        serial: serial.unwrap_or_default().to_string(),
        tool: op.to_string(),
        args: args.clone(),
        el: el.cloned(),
        app: app.map(str::to_string),
        ctx: ctx.clone(),
        run_id: Some(run_id.to_string()),
        step: Some(index as u32 + 1),
        intent: Some(label.to_string()),
        goal: Some(goal.to_string()),
        sensitive: flag.as_ref().map(|_| true),
        effect: flag.filter(|e| !e.is_empty()),
        screenshot: shot,
        ..Default::default()
    };
    // After Approve: read the screen again and find the tapped element.
    let reread = move |r: CheckIn| async move {
        let Some(want) = r.el.clone().filter(|_| r.tool == "phone.tap") else { return Reread::Same };
        match read_screen(serial, run_id, source).await {
            Ok(screen) => match screen.els.iter().find(|e| El::from(*e) == want) {
                Some(e) => {
                    let (x, y) = e.center();
                    let mut a = r.args.clone();
                    a.insert("x".into(), json!(x));
                    a.insert("y".into(), json!(y));
                    Reread::Moved(a)
                }
                None if screen.els.is_empty() => Reread::Same,
                None => Reread::Gone,
            },
            Err(e) if e.contains("device_locked") => Reread::Locked,
            Err(_) => Reread::Same,
        }
    };
    match phone_guard::gate(db, Some(data_dir), &mut req, reread, holds, held).await {
        Gate::Go { hold_id, pass } => {
            let mut a = req.args;
            // Desktop's dispatch check honours the pass once; `_hold` is for the run record.
            a.insert("_pass".into(), json!(pass));
            if let Some(h) = hold_id {
                a.insert("_hold".into(), json!(h));
            }
            Ok(a)
        }
        Gate::Denied(w) => Err(("denied", w)),
        Gate::Expired(w) => Err(("approval_timeout", w)),
        Gate::Locked => Err(("phone_locked", "the phone is locked".into())),
        // Replay has no model to pick again: the recording no longer fits.
        Gate::Changed(w) => Err(("diverged", w)),
        Gate::Failed(w) => Err(("failed", w)),
    }
}

/// Replay a saved skill on a phone. `Ok` carries the per-step report (also
/// when the replay diverged or stopped at a hold); `Err` is a skill that
/// couldn't start. Statuses: `succeeded`, `diverged`, `failed`, `denied`,
/// `approval_timeout`, `phone_locked` (live: `waiting_approval`, through
/// [`phone_guard::live_status`]). `ctx` is who the holds ask; with no thread
/// the guard uses `phone:<serial>`.
#[allow(clippy::too_many_arguments)]
pub async fn run_skill(
    db: &crate::db::DbHandle,
    data_dir: &FsPath,
    name: &str,
    serial: Option<&str>,
    inputs: &[(String, String)],
    model: Option<(String, String)>,
    source: &str,
    run_id: Option<String>,
    ctx: &TaskCtx,
) -> Result<Value, String> {
    let mut skill = load(data_dir, name).ok_or_else(|| format!("no phone skill named \"{}\"", name.trim()))?;
    let missing: Vec<&String> = skill.inputs.iter().filter(|n| !inputs.iter().any(|(k, _)| k == *n)).collect();
    if !missing.is_empty() {
        return Err(format!("this skill needs inputs: {}", missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
    }
    let run_id = run_id.unwrap_or_else(new_skill_run_id);
    let started = Instant::now();
    let mut report = Vec::new();
    let mut model_calls = 0u32;
    let mut healed = 0u32;
    let mut status = "succeeded";
    let mut error: Option<String> = None;
    let mut dirty = false;
    let (mut held, mut holds) = (Duration::ZERO, Vec::<Value>::new());
    let (mut last_el, mut app): (Option<El>, Option<String>) = (None, None);
    let goal = format!("run the phone skill \"{}\"", skill.name);

    for index in 0..skill.steps.len() {
        let step = skill.steps[index].clone();
        let t = Instant::now();
        let mut args: Map<String, Value> = substitute_json(&step.args, inputs).as_object().cloned().unwrap_or_default();
        let mut via: Option<&'static str> = None;
        let mut op = step.op.clone();
        // The element the guard judges: the recorded one until the screen says otherwise.
        let shown = PStep { args: Value::Object(args.clone()), locator: step.locator.clone().map(|l| Locator { label: substitute(&l.label, inputs), ..l }), ..step.clone() };
        let mut target = step_el(&shown, last_el.as_ref());
        let flag = step.sensitive.then(|| step.effect.clone().unwrap_or_default());
        // Taps with a locator: find the element again on the current screen.
        if let (Some(loc), true) = (step.locator.as_ref(), op == "phone.tap") {
            let loc = Locator { label: substitute(&loc.label, inputs), ..loc.clone() };
            let deadline = Instant::now() + STEP_TIMEOUT;
            let mut found = None;
            let mut last_screen = None;
            loop {
                match read_screen(serial, &run_id, source).await {
                    Ok(screen) => {
                        let frame = if loc.crop.is_some() { screenshot(serial, &run_id, source).await } else { None };
                        let size = screen.size;
                        let crop_of = |e: &UiEl| frame.as_ref().and_then(|(img, _)| crop_hash(img, rel(e, size)));
                        found = resolve(&loc, &screen.els, screen.size, &crop_of);
                        last_screen = Some(screen);
                    }
                    // A lane with no screen read (USB-Agent iPhone): the recorded point.
                    Err(_) => break,
                }
                if found.is_some() || Instant::now() >= deadline {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            match (found, last_screen) {
                (Some((el, how)), Some(screen)) => {
                    via = Some(how.as_str());
                    target = Some(El::from(&el));
                    let (x, y) = el.center();
                    args.insert("x".into(), json!(x));
                    args.insert("y".into(), json!(y));
                    if how != Via::Id {
                        // Self-heal: remember where it is now.
                        skill.steps[index].locator = Some(Locator { rel: Some(rel(&el, screen.size)), resource_id: el.id.clone(), ..skill.steps[index].locator.clone().unwrap_or_default() });
                        dirty = true;
                    }
                }
                (None, Some(screen)) => {
                    model_calls += 1;
                    let image = screenshot(serial, &run_id, source).await.map(|(_, url)| url);
                    match redecide(&step, &screen, image.as_deref(), model.as_ref()).await {
                        Ok(Some(el)) => {
                            via = Some("model");
                            target = Some(El::from(&el));
                            let (x, y) = el.center();
                            args.insert("x".into(), json!(x));
                            args.insert("y".into(), json!(y));
                            skill.steps[index].locator = Some(Locator {
                                class: el.class.clone(),
                                label: if el.label.contains("{{") { el.label.clone() } else { step.locator.as_ref().map(|l| l.label.clone()).filter(|l| l.contains("{{")).unwrap_or(el.label.clone()) },
                                resource_id: el.id.clone(),
                                rel: Some(rel(&el, screen.size)),
                                crop: None,
                            });
                            healed += 1;
                            dirty = true;
                        }
                        Ok(None) => {
                            status = "diverged";
                            error = Some(format!("step {} ({}) couldn't be placed on this screen", index + 1, step.label));
                        }
                        Err(e) => {
                            status = "diverged";
                            error = Some(format!("step {} ({}) moved and the re-decision failed: {e}", index + 1, step.label));
                        }
                    }
                }
                (_, None) => via = Some("coordinates"),
            }
        }
        if status != "succeeded" {
            report.push(json!({ "index": index, "op": op, "label": step.label, "ok": false, "error": error, "duration_ms": t.elapsed().as_millis() as u64 }));
            break;
        }
        // The phone guard: every acting step, flagged when saved or not.
        let at = (serial, run_id.as_str(), source);
        let acting = match guard_step(db, data_dir, ctx, at, (index, step.label.as_str(), goal.as_str()), &op, &args, target.as_ref(), app.as_deref(), flag.clone(), &mut holds, &mut held).await {
            Ok(a) => a,
            Err((st, why)) => {
                status = st;
                error = Some(format!("step {} ({}): {why}", index + 1, step.label));
                report.push(json!({ "index": index, "op": op, "label": step.label, "ok": false, "via": via, "error": why, "duration_ms": t.elapsed().as_millis() as u64 }));
                break;
            }
        };
        let mut hold = acting.get("_hold").cloned();
        // find_and_tap that no longer finds: one re-decision as a tap.
        let mut outcome = act(&op, acting, serial, &run_id, source).await;
        let mut stopped: Option<(&'static str, String)> = None;
        if op == "phone.find_and_tap" && outcome.as_ref().err().is_some_and(|e| e.to_lowercase().contains("no element")) {
            if let Ok(screen) = read_screen(serial, &run_id, source).await {
                model_calls += 1;
                let image = screenshot(serial, &run_id, source).await.map(|(_, url)| url);
                if let Ok(Some(el)) = redecide(&step, &screen, image.as_deref(), model.as_ref()).await {
                    let (x, y) = el.center();
                    let mut tap = Map::new();
                    tap.insert("x".into(), json!(x));
                    tap.insert("y".into(), json!(y));
                    target = Some(El::from(&el));
                    match guard_step(db, data_dir, ctx, at, (index, step.label.as_str(), goal.as_str()), "phone.tap", &tap, target.as_ref(), app.as_deref(), flag.clone(), &mut holds, &mut held).await {
                        Ok(tap) => {
                            hold = tap.get("_hold").cloned().or(hold);
                            op = "phone.tap".into();
                            via = Some("model");
                            outcome = act(&op, tap, serial, &run_id, source).await;
                            if outcome.is_ok() && !el.id.is_empty() {
                                skill.steps[index].args["element_id"] = json!(el.id);
                                healed += 1;
                                dirty = true;
                            }
                        }
                        Err(end) => stopped = Some(end),
                    }
                }
            }
        }
        if let Some((st, why)) = stopped {
            status = st;
            error = Some(format!("step {} ({}): {why}", index + 1, step.label));
            report.push(json!({ "index": index, "op": op, "label": step.label, "ok": false, "via": via, "error": why, "duration_ms": t.elapsed().as_millis() as u64 }));
            break;
        }
        if let Err(e) = outcome {
            status = if e.contains("device_locked") { "phone_locked" } else { "failed" };
            error = Some(format!("step {} ({}): {e}", index + 1, step.label));
            report.push(json!({ "index": index, "op": op, "label": step.label, "ok": false, "via": via, "error": e, "duration_ms": t.elapsed().as_millis() as u64 }));
            break;
        }
        match op.as_str() {
            "phone.tap" | "phone.find_and_tap" => last_el = target.clone(),
            "phone.open_app" => app = args.get("app").and_then(Value::as_str).map(str::to_string),
            _ => {}
        }
        // The step's check: what it changed on screen.
        let mut checked = None;
        if let Some(check) = &step.check {
            let deadline = Instant::now() + STEP_TIMEOUT;
            let ok = loop {
                let ok = match read_screen(serial, &run_id, source).await {
                    Ok(screen) => check_holds(check, &screen.els, inputs),
                    // No screen read on this lane: nothing to check against.
                    Err(_) => true,
                };
                if ok || Instant::now() >= deadline {
                    break ok;
                }
                tokio::time::sleep(Duration::from_millis(400)).await;
            };
            checked = Some(ok);
            if !ok {
                status = "diverged";
                error = Some(format!("after step {} ({}) the screen didn't show what the recording did", index + 1, step.label));
            }
        } else {
            tokio::time::sleep(Duration::from_millis(350)).await;
        }
        report.push(json!({ "index": index, "op": op, "label": step.label, "ok": checked != Some(false), "via": via, "checked": checked, "hold": hold, "duration_ms": t.elapsed().as_millis() as u64 }));
        if status != "succeeded" {
            break;
        }
    }

    phone_guard::set_live(&run_id, None);
    let now = chrono::Utc::now().to_rfc3339();
    skill.last_used_at = Some(now.clone());
    if status == "succeeded" {
        skill.runs += 1;
    }
    if dirty && status == "succeeded" {
        skill.heals += healed as i64;
        skill.updated_at = now;
    } else if dirty {
        // A diverged run keeps the recording as it was.
        if let Some(fresh) = load(data_dir, &skill.name) {
            skill.steps = fresh.steps;
        }
    }
    if let Err(e) = store(data_dir, &skill) {
        tracing::warn!("phone skill {} not updated: {e}", skill.name);
    }
    Ok(json!({
        "ok": status == "succeeded",
        "status": status,
        "skill": skill.name,
        "serial": serial,
        "run_id": run_id,
        "model_calls": model_calls,
        "healed": healed,
        "steps": report,
        "error": error,
        // The wait on the owner is not phone time.
        "duration_ms": started.elapsed().saturating_sub(held).as_millis() as u64,
        "held_ms": held.as_millis() as u64,
        "holds": holds,
    }))
}

// ─── Routes ────────────────────────────────────────────────────────────────

fn bad(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({ "message": message.into() }))).into_response()
}

async fn list_skills(State(state): State<Arc<AppState>>, Extension(_user): Extension<AuthUser>) -> Response {
    let dir = state.data_dir.clone();
    let skills = tokio::task::spawn_blocking(move || list(&dir)).await.unwrap_or_default();
    Json(json!({ "skills": skills.iter().map(summary).collect::<Vec<_>>() })).into_response()
}

async fn get_skill(State(state): State<Arc<AppState>>, Extension(_user): Extension<AuthUser>, Path(name): Path<String>) -> Response {
    match load(&state.data_dir, &name) {
        Some(s) => Json(json!({ "skill": s })).into_response(),
        None => bad(StatusCode::NOT_FOUND, format!("no phone skill named \"{name}\"")),
    }
}

async fn save_skill(State(state): State<Arc<AppState>>, Extension(_user): Extension<AuthUser>, Json(body): Json<SaveIn>) -> Response {
    let dir = state.data_dir.clone();
    let result = tokio::task::spawn_blocking(move || {
        let skill = from_run(&dir, &body)?;
        store(&dir, &skill)?;
        Ok::<_, String>(skill)
    })
    .await;
    match result {
        Ok(Ok(skill)) => Json(json!({ "ok": true, "skill": summary(&skill), "steps": skill.steps })).into_response(),
        Ok(Err(e)) => bad(StatusCode::BAD_REQUEST, e),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn delete_skill(State(state): State<Arc<AppState>>, Extension(_user): Extension<AuthUser>, Path(name): Path<String>) -> Response {
    if forget(&state.data_dir, &name) {
        Json(json!({ "ok": true })).into_response()
    } else {
        bad(StatusCode::NOT_FOUND, format!("no phone skill named \"{name}\""))
    }
}

async fn run_route(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(name): Path<String>,
    Json(body): Json<RunIn>,
) -> Response {
    let model = brain(&state.db, body.bot_id.as_deref(), body.brain.as_deref());
    let inputs = inputs_of(&body.inputs);
    // The signed-in caller owns the run: its holds ask them.
    let some = |v: &Option<String>| v.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let ctx = TaskCtx { owner: user.user_id.clone(), bot_id: some(&body.bot_id), thread_id: some(&body.thread_id), session_id: some(&body.session_id), source: "skill".into() };
    if body.background {
        let Some(skill) = load(&state.data_dir, &name) else {
            return bad(StatusCode::NOT_FOUND, format!("no phone skill named \"{}\"", name.trim()));
        };
        let missing: Vec<&String> = skill.inputs.iter().filter(|n| !inputs.iter().any(|(k, _)| k == *n)).collect();
        if !missing.is_empty() {
            return bad(StatusCode::BAD_REQUEST, format!("this skill needs inputs: {}", missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
        }
        if !gateway_available() {
            return bad(StatusCode::SERVICE_UNAVAILABLE, "Allternit Desktop's phone gateway isn't running on this computer");
        }
        let run_id = new_skill_run_id();
        let (st, id, serial) = (state.clone(), run_id.clone(), body.serial.clone());
        tokio::spawn(async move {
            let report = match run_skill(&st.db, &st.data_dir, &name, serial.as_deref(), &inputs, model, "skill", Some(id.clone()), &ctx).await {
                Ok(report) => report,
                Err(e) => json!({ "ok": false, "status": "failed", "skill": name, "serial": serial, "run_id": id, "error": e, "steps": [] }),
            };
            phone_runs::record_run_completed(&st.db, &report, "skill");
        });
        return (StatusCode::ACCEPTED, Json(json!({ "ok": true, "status": "running", "run_id": run_id, "skill": skill.name, "serial": body.serial }))).into_response();
    }
    match run_skill(&state.db, &state.data_dir, &name, body.serial.as_deref(), &inputs, model, "skill", None, &ctx).await {
        Ok(report) => {
            phone_runs::record_run_completed(&state.db, &report, "skill");
            Json(report).into_response()
        }
        Err(e) if e.starts_with("no phone skill") => bad(StatusCode::NOT_FOUND, e),
        Err(e) => bad(StatusCode::BAD_REQUEST, e),
    }
}

/// A sent text or a placed call waits on the person's approval on Desktop; the
/// Platform API relay gives up at 90 s, so the gateway call stops just before.
const TOOL_TIMEOUT: Duration = Duration::from_secs(80);

#[derive(Debug, Deserialize)]
pub struct ToolIn {
    pub name: String,
    #[serde(default)]
    pub arguments: Map<String, Value>,
    /// Groups the call into a run in the history (default: a new `api-…` run).
    #[serde(default)]
    pub run_id: Option<String>,
}

/// `POST /api/v1/phones/:serial/tool {name, arguments, run_id?}`: one phone
/// tool through the Desktop phone gateway, exactly as a bot calls it, so a
/// text or a call still waits for the person's approval on Desktop (the
/// gateway's gate; nothing here bypasses it). The step lands in the run
/// history (and `phone.step`) like any other. `serial` `-` = the one phone.
async fn tool_route(
    Extension(_user): Extension<AuthUser>,
    Path(serial): Path<String>,
    Json(body): Json<ToolIn>,
) -> Response {
    let raw = body.name.trim();
    let name = if raw.starts_with("phone.") { raw.to_string() } else { format!("phone.{raw}") };
    let known = ["devices", "screenshot", "tap", "type", "read_ui", "screen_text", "find_and_tap", "swipe", "home", "back", "key", "open_app", "sms.send", "sms.read", "call.dial",
        // iPhone app phones (`app:<id>`): the in-app browser. Desktop main sends
        // these to `phone_app`'s rpc route for the phone's control socket.
        "web.open", "web.snapshot", "web.click", "web.type", "web.scroll", "web.back", "web.wait"];
    if !known.iter().any(|k| name == format!("phone.{k}")) {
        return bad(StatusCode::BAD_REQUEST, format!("unknown phone tool \"{raw}\" (use the task or skills routes for phone.task and phone.run_skill)"));
    }
    if !gateway_available() {
        return bad(StatusCode::SERVICE_UNAVAILABLE, "Allternit Desktop's phone gateway isn't running on this computer");
    }
    let mut args = body.arguments;
    args.retain(|k, _| !k.starts_with('_'));
    if serial != "-" && serial != "default" {
        args.insert("serial".into(), json!(serial));
    }
    let run_id = body
        .run_id
        .map(|r| r.trim().chars().take(120).collect::<String>())
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| format!("api-{}", uuid::Uuid::new_v4().simple()));
    args.insert("_run_id".into(), json!(run_id));
    args.insert("_source".into(), json!("api"));
    match call_tool_within(&name, Value::Object(args), TOOL_TIMEOUT).await {
        Ok(mut result) => {
            if let Some(obj) = result.as_object_mut() {
                obj.insert("run_id".into(), json!(run_id));
                obj.insert("tool".into(), json!(name));
            }
            Json(result).into_response()
        }
        Err(e) if e.contains("timed out") => {
            bad(StatusCode::GATEWAY_TIMEOUT, "the phone didn't answer in time (a text or call may still be waiting for approval on Desktop)")
        }
        Err(e) => bad(StatusCode::BAD_GATEWAY, e),
    }
}

/// Merged under `/api/v1`.
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/phones/:serial/tool", post(tool_route))
        .route("/phones/skills", get(list_skills).post(save_skill))
        .route("/phones/skills/:name", get(get_skill).delete(delete_skill))
        .route("/phones/skills/:name/run", post(run_route))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(class: &str, label: &str, id: &str, r: [f64; 4]) -> UiEl {
        UiEl { class: class.into(), label: label.into(), id: id.into(), rect: r }
    }

    #[test]
    fn parses_the_read_ui_text_form() {
        let e = parse_line("android.widget.Button \"Turn on\" [com.x:id/go] {{10, 20}, {100, 40}}").unwrap();
        assert_eq!(e, el("android.widget.Button", "Turn on", "com.x:id/go", [10.0, 20.0, 100.0, 40.0]));
        let bare = parse_line("android.widget.FrameLayout {{0, 0}, {1080, 2400}}").unwrap();
        assert_eq!(bare.label, "");
        assert_eq!(screen_of(&[bare]), (1080.0, 2400.0));
        assert!(parse_line("garbage").is_none());
    }

    #[test]
    fn resolve_tiers_and_never_guesses() {
        let screen = (1000.0, 2000.0);
        let go = el("Button", "Go", "app:id/go", [100.0, 100.0, 200.0, 100.0]);
        let loc = Locator { class: "Button".into(), label: "Go".into(), resource_id: "app:id/go".into(), rel: Some(rel(&go, screen)), crop: None };
        let none = |_: &UiEl| None;
        assert_eq!(resolve(&loc, &[go.clone()], screen, &none).unwrap().1, Via::Id);
        let moved = el("Button", "Go", "app:id/other", [0.0, 900.0, 200.0, 100.0]);
        assert_eq!(resolve(&loc, &[moved], screen, &none).unwrap().1, Via::Name);
        let renamed = el("Button", "Start", "app:id/go", [0.0, 900.0, 200.0, 100.0]);
        assert_eq!(resolve(&loc, &[renamed], screen, &none).unwrap().1, Via::Resource);
        let twins = [el("Button", "Go", "", [0.0; 4]), el("Button", "Go", "", [1.0; 4])];
        let loose = Locator { resource_id: String::new(), rel: None, ..loc.clone() };
        assert!(resolve(&loose, &twins, screen, &none).is_none());
        // Same place, other name, a pixel hash that matches.
        let look = Locator { crop: Some("ffff0000ffff0000".into()), ..loose.clone() };
        let there = el("Button", "Begin", "", [100.0, 100.0, 200.0, 100.0]);
        let crop = |_: &UiEl| Some("ffff0000ffff0001".to_string());
        assert_eq!(resolve(&Locator { label: "x".into(), rel: Some(rel(&go, screen)), ..look }, &[there], screen, &crop).unwrap().1, Via::Pixels);
    }

    #[test]
    fn checks_come_from_what_appeared_or_went_away() {
        let a = [el("T", "Settings", "", [0.0; 4])];
        let b = [el("T", "Settings", "", [0.0; 4]), el("T", "Dark theme", "", [0.0; 4])];
        assert_eq!(derive_check(&a, &b), Some(json!({ "label": "Dark theme" })));
        assert_eq!(derive_check(&b, &a), Some(json!({ "label": "Dark theme", "gone": true })));
        assert!(check_holds(&json!({ "label": "{{who}}" }), &[el("T", "Ada", "", [0.0; 4])], &[("who".into(), "Ada".into())]));
    }

    #[test]
    fn saving_a_run_templates_typed_text_and_skips_reads() {
        let tmp = tempfile::tempdir().unwrap();
        let ui = "Button \"Search\" [s] {{0, 0}, {100, 50}}\nEditText \"\" [q] {{0, 100}, {500, 50}}";
        let post = |tool: &str, args: Value, ui: Option<&str>, target: Option<Value>| {
            phone_runs::ingest(tmp.path(), "link:p", phone_runs::StepIn {
                run_id: Some("r".into()),
                tool: tool.into(),
                args,
                ok: true,
                ui: ui.map(str::to_string),
                target,
                ..Default::default()
            })
            .unwrap();
        };
        post("phone.read_ui", json!({}), Some(ui), None);
        post("phone.tap", json!({ "x": 50, "y": 25 }), Some(ui), Some(json!({ "class": "Button", "label": "Search", "id": "s", "rect": [0, 0, 100, 50], "screen": [500, 150] })));
        post("phone.type", json!({ "text": "secret words" }), Some(ui), None);
        let skill = from_run(tmp.path(), &SaveIn { run_id: "r".into(), name: "search".into(), input_names: vec!["query".into()], ..Default::default() }).unwrap();
        assert_eq!(skill.steps.len(), 2);
        assert_eq!(skill.inputs, vec!["query".to_string()]);
        assert_eq!(skill.steps[1].args["text"], "{{query}}");
        assert_eq!(skill.steps[0].locator.as_ref().unwrap().label, "Search");
        assert!(!serde_json::to_string(&skill).unwrap().contains("secret words"));
    }

    // ─── Holds ──────────────────────────────────────────────────────────

    fn tap_step(label: &str, id: &str) -> PStep {
        PStep {
            op: "phone.tap".into(),
            args: json!({ "x": 50, "y": 25 }),
            locator: Some(Locator { class: "Button".into(), label: label.into(), resource_id: id.into(), ..Default::default() }),
            label: format!("tap \"{label}\""),
            ..Default::default()
        }
    }

    fn skill_of(name: &str, steps: Vec<PStep>) -> Skill {
        let now = chrono::Utc::now().to_rfc3339();
        Skill { name: name.into(), steps, created_at: now.clone(), updated_at: now, ..Default::default() }
    }

    /// A phone whose screen shows `ui`; every tap is recorded with its args.
    fn fake_phone(ui: &'static str) -> (FakeTool, Arc<std::sync::Mutex<Vec<Value>>>) {
        let taps = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = taps.clone();
        let fake: FakeTool = Arc::new(move |name: &str, args: &Value| match name {
            "phone.read_ui" => Ok(json!({ "ok": true, "text": ui })),
            "phone.tap" => {
                seen.lock().unwrap().push(args.clone());
                Ok(json!({ "ok": true }))
            }
            _ => Err("not on this phone".to_string()),
        });
        (fake, taps)
    }

    async fn hold_state() -> (Arc<AppState>, TaskCtx) {
        let dir = std::env::temp_dir().join(format!("phone-replay-hold-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        (crate::test_helpers::app_state(&dir).await, TaskCtx { owner: "user-a".into(), source: "skill".into(), ..Default::default() })
    }

    /// The one pending hold (fails when there are none, or more than one).
    async fn the_pending_hold(st: &AppState) -> String {
        for _ in 0..200 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let ids: Vec<String> = {
                let c = st.db.connect().unwrap();
                let mut q = c.prepare("SELECT id FROM gateway_approvals WHERE state = 'pending'").unwrap();
                let ids = q.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
                ids
            };
            match ids.len() {
                0 => continue,
                1 => return ids[0].clone(),
                n => panic!("expected one pending hold, found {n}"),
            }
        }
        panic!("the replay never made a hold");
    }

    fn answer(st: &AppState, id: &str, to: &str) {
        st.db
            .connect()
            .unwrap()
            .execute("UPDATE gateway_approvals SET state = ?2, resolved_at = ?3, actor_type = 'user', actor_id = 'user-a' WHERE id = ?1", rusqlite::params![id, to, chrono::Utc::now().to_rfc3339()])
            .unwrap();
    }

    #[test]
    fn storing_a_skill_flags_its_delete_step() {
        let tmp = tempfile::tempdir().unwrap();
        store(tmp.path(), &skill_of("tidy", vec![tap_step("Search", "s"), tap_step("Delete", "del")])).unwrap();
        let back = load(tmp.path(), "tidy").unwrap();
        assert!(!back.steps[0].sensitive && back.steps[0].effect.is_none());
        assert!(back.steps[1].sensitive);
        assert_eq!(back.steps[1].effect.as_deref(), Some("delete"));
        // Additive: a plain step is written without the keys.
        let raw: Value = serde_json::from_slice(&std::fs::read(skill_path(tmp.path(), "tidy")).unwrap()).unwrap();
        assert!(raw["steps"][0].get("sensitive").is_none() && raw["steps"][1]["sensitive"] == json!(true));
    }

    #[tokio::test]
    async fn replay_waits_on_one_hold_and_acts_once_after_approval() {
        let (st, ctx) = hold_state().await;
        store(&st.data_dir, &skill_of("clear inbox", vec![tap_step("Delete", "del")])).unwrap();
        let (fake, taps) = fake_phone("Button \"Delete\" [del] {{0, 0}, {100, 50}}");
        let approver = async {
            let id = the_pending_hold(&st).await;
            // Nothing acts while the hold waits.
            assert!(taps.lock().unwrap().is_empty());
            answer(&st, &id, "approved");
            id
        };
        let run = run_skill(&st.db, &st.data_dir, "clear inbox", Some("link:1"), &[], None, "skill", Some("skill-h1".into()), &ctx);
        let (report, id) = FAKE_TOOL.scope(fake, async { tokio::join!(run, approver) }).await;
        let report = report.unwrap();
        assert_eq!(report["status"], "succeeded", "{report}");
        let taps = taps.lock().unwrap().clone();
        assert_eq!(taps.len(), 1, "acts exactly once");
        assert!(taps[0]["_pass"].as_str().is_some_and(|p| p.starts_with("pass_")), "{}", taps[0]);
        assert_eq!(taps[0]["_hold"], json!(id));
        assert_eq!(report["holds"], json!([{ "approvalId": id, "effect": "delete", "state": "approved" }]));
        assert!(report["held_ms"].as_u64().unwrap() > 0);
        assert_eq!(report["steps"][0]["hold"], json!(id));
        assert!(phone_guard::live_status("skill-h1").is_none(), "the live status is cleared at the end");
    }

    #[tokio::test]
    async fn an_old_skill_file_without_flags_still_holds_on_pay() {
        let (st, ctx) = hold_state().await;
        // Written before flags existed: no `sensitive` key anywhere.
        let old = json!({
            "name": "checkout", "inputs": [], "created_at": "2026-10-01T00:00:00Z", "updated_at": "2026-10-01T00:00:00Z",
            "steps": [{ "op": "phone.tap", "args": { "x": 50, "y": 25 }, "label": "tap \"Pay\"", "locator": { "class": "Button", "label": "Pay", "resource_id": "pay" } }],
        });
        std::fs::create_dir_all(skills_dir(&st.data_dir)).unwrap();
        std::fs::write(skill_path(&st.data_dir, "checkout"), serde_json::to_vec(&old).unwrap()).unwrap();
        assert!(!load(&st.data_dir, "checkout").unwrap().steps[0].sensitive);
        let (fake, taps) = fake_phone("Button \"Pay\" [pay] {{0, 0}, {100, 50}}");
        let denier = async {
            let id = the_pending_hold(&st).await;
            answer(&st, &id, "denied");
        };
        let run = run_skill(&st.db, &st.data_dir, "checkout", Some("link:1"), &[], None, "skill", None, &ctx);
        let (report, ()) = FAKE_TOOL.scope(fake, async { tokio::join!(run, denier) }).await;
        let report = report.unwrap();
        assert_eq!(report["status"], "denied", "{report}");
        assert!(taps.lock().unwrap().is_empty());
        assert_eq!(report["holds"][0]["effect"], "pay");
    }
}
