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

use crate::phone_runs;

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
    let file = gateway_file().ok_or("no home directory")?;
    let raw = tokio::fs::read(&file)
        .await
        .map_err(|_| "Allternit Desktop's phone gateway isn't running on this computer".to_string())?;
    let gw: Value = serde_json::from_slice(&raw).map_err(|e| format!("phone gateway file unreadable: {e}"))?;
    let (url, token) = (
        gw.get("url").and_then(Value::as_str).ok_or("phone gateway file has no url")?,
        gw.get("token").and_then(Value::as_str).ok_or("phone gateway file has no token")?,
    );
    let res = reqwest::Client::new()
        .post(format!("{}/phone/call", url.trim_end_matches('/')))
        .header("x-allternit-phone-token", token)
        .json(&json!({ "name": name, "arguments": args }))
        .timeout(GATEWAY_TIMEOUT)
        .send()
        .await
        .map_err(|e| format!("phone gateway unreachable: {e}"))?;
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

pub fn store(data_dir: &FsPath, s: &Skill) -> Result<(), String> {
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
        steps.push(PStep { op: s.tool.clone(), args: Value::Object(args), locator, label, check });
    }
    if steps.is_empty() {
        return Err("that run has no successful phone actions to save (reads alone aren't a skill)".into());
    }
    if steps.len() > MAX_SKILL_STEPS {
        return Err(format!("a skill holds at most {MAX_SKILL_STEPS} steps; save a step range"));
    }
    let now = chrono::Utc::now().to_rfc3339();
    let created_at = load(data_dir, &name).map(|s| s.created_at).unwrap_or_else(|| now.clone());
    Ok(Skill {
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
    })
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

/// Replay a saved skill on a phone. `Ok` carries the per-step report (also
/// when the replay diverged); `Err` is a skill that couldn't start.
pub async fn run_skill(
    data_dir: &FsPath,
    name: &str,
    serial: Option<&str>,
    inputs: &[(String, String)],
    model: Option<(String, String)>,
    source: &str,
) -> Result<Value, String> {
    let mut skill = load(data_dir, name).ok_or_else(|| format!("no phone skill named \"{}\"", name.trim()))?;
    let missing: Vec<&String> = skill.inputs.iter().filter(|n| !inputs.iter().any(|(k, _)| k == *n)).collect();
    if !missing.is_empty() {
        return Err(format!("this skill needs inputs: {}", missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
    }
    let run_id = format!("skill-{}", uuid::Uuid::new_v4().simple());
    let started = Instant::now();
    let mut report = Vec::new();
    let mut model_calls = 0u32;
    let mut healed = 0u32;
    let mut status = "succeeded";
    let mut error: Option<String> = None;
    let mut dirty = false;

    for index in 0..skill.steps.len() {
        let step = skill.steps[index].clone();
        let t = Instant::now();
        let mut args: Map<String, Value> = substitute_json(&step.args, inputs).as_object().cloned().unwrap_or_default();
        let mut via: Option<&'static str> = None;
        let mut op = step.op.clone();
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
        // find_and_tap that no longer finds: one re-decision as a tap.
        let mut outcome = act(&op, args.clone(), serial, &run_id, source).await;
        if op == "phone.find_and_tap" && outcome.as_ref().err().is_some_and(|e| e.to_lowercase().contains("no element")) {
            if let Ok(screen) = read_screen(serial, &run_id, source).await {
                model_calls += 1;
                let image = screenshot(serial, &run_id, source).await.map(|(_, url)| url);
                if let Ok(Some(el)) = redecide(&step, &screen, image.as_deref(), model.as_ref()).await {
                    let (x, y) = el.center();
                    let mut tap = Map::new();
                    tap.insert("x".into(), json!(x));
                    tap.insert("y".into(), json!(y));
                    op = "phone.tap".into();
                    via = Some("model");
                    outcome = act(&op, tap, serial, &run_id, source).await;
                    if outcome.is_ok() && !el.id.is_empty() {
                        skill.steps[index].args["element_id"] = json!(el.id);
                        healed += 1;
                        dirty = true;
                    }
                }
            }
        }
        if let Err(e) = outcome {
            status = "failed";
            error = Some(format!("step {} ({}): {e}", index + 1, step.label));
            report.push(json!({ "index": index, "op": op, "label": step.label, "ok": false, "via": via, "error": e, "duration_ms": t.elapsed().as_millis() as u64 }));
            break;
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
        report.push(json!({ "index": index, "op": op, "label": step.label, "ok": checked != Some(false), "via": via, "checked": checked, "duration_ms": t.elapsed().as_millis() as u64 }));
        if status != "succeeded" {
            break;
        }
    }

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
        "duration_ms": started.elapsed().as_millis() as u64,
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
    Extension(_user): Extension<AuthUser>,
    Path(name): Path<String>,
    Json(body): Json<RunIn>,
) -> Response {
    let model = brain(&state.db, body.bot_id.as_deref(), body.brain.as_deref());
    let inputs = inputs_of(&body.inputs);
    match run_skill(&state.data_dir, &name, body.serial.as_deref(), &inputs, model, "skill").await {
        Ok(report) => Json(report).into_response(),
        Err(e) if e.starts_with("no phone skill") => bad(StatusCode::NOT_FOUND, e),
        Err(e) => bad(StatusCode::BAD_REQUEST, e),
    }
}

/// Merged under `/api/v1`.
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
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
}
