//! The computer / browser toolset executor (`allternit.computer.v2`,
//! `allternit.browser.v1`). One executor behind every model adapter: gizzi's
//! Claude-native, OpenAI, Gemini and JSON-function adapters, the Python engine
//! and the hosted driver all send contract calls here. The computer contract
//! is v2: the 17 pixel members of v1 (Anthropic `computer_toolset_20260801`
//! shape, unchanged) plus the 6 structured members (`computer_v2`): read_ui,
//! act, run_batch, verify through the Allternit Driver sidecar, request_human
//! and use_credential.
//!
//! `POST /computers/:id/toolset` runs one call. `:id` is a computer id or
//! `this-device`. Checks run in this order:
//!
//! 1. contract validation (member exists, enabled on this target, input matches
//!    the member schema; the batch rule: a call after a failed or unresolved
//!    call of the same turn answers with the contract's halt text),
//! 2. control lease (`423` when someone else holds control; this-device needs
//!    its owner to have taken control),
//! 3. declarative policy (deny = `403`), then risk and approval (`409
//!    approval_required` with a single-use action-hash grant id),
//! 4. the audit row (written and fsynced BEFORE dispatch),
//! 5. dispatch: Cua Driver (this-device), xdotool/scrot in the guest
//!    (cloud/bot computers), or the ACU gateway's Playwright session (browser).
//!
//! Coordinate scaling between the model frame and the screen, and screenshot
//! downscaling (long edge <= 1568, <= 1.15 MP), happen ONLY here.
//!
//! Every executed action emits `{type:"computer.action", data:{...}}` on the
//! computer's events channel (the `/computers/:id/events` websocket and the
//! `/computers/:id/toolset/events` SSE stream).

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Extension, Json, Router,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::warn;

use crate::aci_safety::ConfirmationClass;
use crate::auth::AuthUser;
use crate::computer_control_lease::{self as lease, Holder, HolderKind};
use crate::computer_routes::{ComputerKind, ComputerResponse};
use crate::AppState;

// ---------------------------------------------------------------------------
// Contract (read straight from the JSON source of truth; nothing to drift).
// ---------------------------------------------------------------------------

const COMPUTER_CONTRACT_JSON: &str =
    include_str!("../../../contracts/computer-toolset/allternit-computer-v2.json");
const BROWSER_CONTRACT_JSON: &str =
    include_str!("../../../contracts/computer-toolset/allternit-browser-v1.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Toolset {
    Computer,
    Browser,
}

impl Toolset {
    pub fn as_str(self) -> &'static str {
        match self {
            Toolset::Computer => "computer",
            Toolset::Browser => "browser",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct MemberSpec {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub risk: String,
    pub default_enabled: bool,
    pub needs_confirm: bool,
    pub confirm: String,
    pub result_kind: String,
    #[serde(default)]
    pub ack_text: Option<String>,
    #[serde(default)]
    pub scale_fields: BTreeMap<String, String>,
    pub input_schema: Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelFrameRule {
    pub max_long_edge: u32,
    pub max_pixels: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Upstream {
    pub anthropic_type: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Contract {
    pub id: String,
    pub upstream: Upstream,
    pub batch_halt_text: String,
    pub model_frame: ModelFrameRule,
    pub members: Vec<MemberSpec>,
}

impl Contract {
    pub fn member(&self, name: &str) -> Option<&MemberSpec> {
        self.members.iter().find(|m| m.name == name)
    }
}

static CONTRACTS: Lazy<(Contract, Contract)> = Lazy::new(|| {
    (
        serde_json::from_str(COMPUTER_CONTRACT_JSON).expect("allternit-computer-v1.json is valid"),
        serde_json::from_str(BROWSER_CONTRACT_JSON).expect("allternit-browser-v1.json is valid"),
    )
});

pub fn contract(toolset: Toolset) -> &'static Contract {
    match toolset {
        Toolset::Computer => &CONTRACTS.0,
        Toolset::Browser => &CONTRACTS.1,
    }
}

fn risk_class(spec: &MemberSpec) -> ConfirmationClass {
    match spec.risk.as_str() {
        "reversible" => ConfirmationClass::Reversible,
        "irreversible" => ConfirmationClass::Irreversible,
        _ => ConfirmationClass::Risky,
    }
}

/// Whether a call needs a human approval grant on this target.
pub fn needs_approval(spec: &MemberSpec, sandboxed: bool) -> bool {
    spec.risk == "irreversible"
        || spec.confirm == "always"
        || (spec.confirm == "non_sandbox" && !sandboxed)
}

/// Minimal JSON-schema check for the subset the contract uses (object,
/// required, additionalProperties:false, string/number/boolean/array, const,
/// enum, anyOf, min/maxItems). Returns a model-readable reason.
pub fn validate(schema: &Value, value: &Value, path: &str) -> Result<(), String> {
    let here = if path.is_empty() { "input".to_string() } else { path.to_string() };
    if let Some(any) = schema.get("anyOf").and_then(Value::as_array) {
        if any.iter().any(|s| validate(s, value, path).is_ok()) {
            return Ok(());
        }
        return Err(format!("{here} does not match any allowed shape"));
    }
    if let Some(c) = schema.get("const") {
        if value != c {
            return Err(format!("{here} must be {c}"));
        }
    }
    if let Some(options) = schema.get("enum").and_then(Value::as_array) {
        if !options.contains(value) {
            return Err(format!("{here} must be one of {}", Value::Array(options.clone())));
        }
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("object") => {
            let Some(obj) = value.as_object() else {
                return Err(format!("{here} must be an object"));
            };
            let props = schema.get("properties").and_then(Value::as_object);
            for req in schema.get("required").and_then(Value::as_array).into_iter().flatten() {
                let key = req.as_str().unwrap_or_default();
                if obj.get(key).map_or(true, Value::is_null) {
                    return Err(format!("{here}.{key} is required"));
                }
            }
            for (k, v) in obj {
                match props.and_then(|p| p.get(k)) {
                    // null is accepted for any optional field (the SDK types allow it).
                    Some(_) if v.is_null() => {}
                    Some(s) => validate(s, v, &format!("{here}.{k}"))?,
                    None if schema.get("additionalProperties") == Some(&Value::Bool(false)) => {
                        return Err(format!("{here}.{k} is not a field of this member"));
                    }
                    None => {}
                }
            }
        }
        Some("string") if !value.is_string() => return Err(format!("{here} must be a string")),
        Some("number") if !value.is_number() => return Err(format!("{here} must be a number")),
        Some("integer") if !(value.is_i64() || value.is_u64()) => {
            return Err(format!("{here} must be an integer"))
        }
        Some("boolean") if !value.is_boolean() => return Err(format!("{here} must be a boolean")),
        Some("array") => {
            let Some(items) = value.as_array() else {
                return Err(format!("{here} must be an array"));
            };
            let len = items.len() as u64;
            if let Some(min) = schema.get("minItems").and_then(Value::as_u64) {
                if len < min {
                    return Err(format!("{here} needs at least {min} items"));
                }
            }
            if let Some(max) = schema.get("maxItems").and_then(Value::as_u64) {
                if len > max {
                    return Err(format!("{here} takes at most {max} items"));
                }
            }
            if let Some(item_schema) = schema.get("items") {
                for (i, item) in items.iter().enumerate() {
                    validate(item_schema, item, &format!("{here}[{i}]"))?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Model frame and coordinate scaling (the only place scaling happens).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordinateSpace {
    #[default]
    Pixels,
    /// 0..1000 on both axes over the whole screen (UI-TARS / Qwen-VL style).
    Normalized1000,
}

/// The frame a screen of `screen` size is shown to the model in: downscaled
/// (never upscaled), aspect kept, long edge <= max_long_edge and area <=
/// max_pixels.
pub fn default_frame(screen: Frame, rule: &ModelFrameRule) -> Frame {
    let (w, h) = (screen.width.max(1) as f64, screen.height.max(1) as f64);
    let by_edge = rule.max_long_edge as f64 / w.max(h);
    let by_area = (rule.max_pixels as f64 / (w * h)).sqrt();
    let s = by_edge.min(by_area).min(1.0);
    Frame {
        width: ((w * s).floor() as u32).max(1),
        height: ((h * s).floor() as u32).max(1),
    }
}

/// Maps model-frame coordinates to screen pixels and back.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mapping {
    pub screen: Frame,
    pub frame: Frame,
    pub space: CoordinateSpace,
}

impl Mapping {
    pub fn new(screen: Frame, requested: Option<Frame>, space: CoordinateSpace, rule: &ModelFrameRule) -> Self {
        let frame = requested
            .filter(|f| f.width > 0 && f.height > 0)
            .unwrap_or_else(|| default_frame(screen, rule));
        Mapping { screen, frame, space }
    }

    fn denominators(&self) -> (f64, f64) {
        match self.space {
            CoordinateSpace::Pixels => (self.frame.width as f64, self.frame.height as f64),
            CoordinateSpace::Normalized1000 => (1000.0, 1000.0),
        }
    }

    /// Model coordinate -> screen pixel, clamped onto the screen.
    pub fn to_screen(&self, x: f64, y: f64) -> (i32, i32) {
        let (dx, dy) = self.denominators();
        let sx = (x * self.screen.width as f64 / dx).round();
        let sy = (y * self.screen.height as f64 / dy).round();
        (
            sx.clamp(0.0, (self.screen.width.max(1) - 1) as f64) as i32,
            sy.clamp(0.0, (self.screen.height.max(1) - 1) as f64) as i32,
        )
    }

    /// Screen pixel -> model coordinate (cursor_position).
    pub fn to_model(&self, sx: f64, sy: f64) -> (i64, i64) {
        let (dx, dy) = self.denominators();
        (
            (sx * dx / self.screen.width.max(1) as f64).round() as i64,
            (sy * dy / self.screen.height.max(1) as f64).round() as i64,
        )
    }

    /// Screen px per model-frame px.
    pub fn scale(&self) -> f64 {
        self.screen.width as f64 / self.frame.width.max(1) as f64
    }
}

/// Rewrite a member input's coordinate fields (per the contract's
/// `scale_fields`) from the model frame into screen pixels.
pub fn scale_input(spec: &MemberSpec, input: &Value, map: &Mapping) -> Value {
    let mut out = input.clone();
    for (field, kind) in &spec.scale_fields {
        let Some(v) = out.get_mut(field) else { continue };
        match kind.as_str() {
            "point" => {
                if let Some([x, y]) = v.as_array().and_then(|a| <&[Value; 2]>::try_from(a.as_slice()).ok()) {
                    let (sx, sy) = map.to_screen(x.as_f64().unwrap_or(0.0), y.as_f64().unwrap_or(0.0));
                    *v = json!([sx, sy]);
                }
            }
            "rect" => {
                if let Some([x0, y0, x1, y1]) = v.as_array().and_then(|a| <&[Value; 4]>::try_from(a.as_slice()).ok()) {
                    let (a, b) = map.to_screen(x0.as_f64().unwrap_or(0.0), y0.as_f64().unwrap_or(0.0));
                    let (c, d) = map.to_screen(x1.as_f64().unwrap_or(0.0), y1.as_f64().unwrap_or(0.0));
                    *v = json!([a, b, c, d]);
                }
            }
            "target" => {
                if v.get("type").and_then(Value::as_str) == Some("coordinate") {
                    let x = v.get("x").and_then(Value::as_f64).unwrap_or(0.0);
                    let y = v.get("y").and_then(Value::as_f64).unwrap_or(0.0);
                    let (sx, sy) = map.to_screen(x, y);
                    v["x"] = json!(sx);
                    v["y"] = json!(sy);
                }
            }
            _ => {}
        }
    }
    out
}

/// First screen point an input acts on (for the cursor overlay event).
fn primary_point(spec: &MemberSpec, scaled: &Value) -> Option<(i64, i64)> {
    for key in ["coordinate", "target", "start_coordinate", "region"] {
        if !spec.scale_fields.contains_key(key) {
            continue;
        }
        let v = scaled.get(key)?;
        if let Some(a) = v.as_array() {
            if a.len() >= 2 {
                return Some((a[0].as_i64()?, a[1].as_i64()?));
            }
        }
        if v.get("type").and_then(Value::as_str) == Some("coordinate") {
            return Some((v.get("x")?.as_i64()?, v.get("y")?.as_i64()?));
        }
    }
    None
}

fn png_size(png: &[u8]) -> Option<Frame> {
    image::io::Reader::new(std::io::Cursor::new(png))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
        .map(|(width, height)| Frame { width, height })
}

/// Resize an image (PNG/JPEG bytes) to exactly `to`, re-encoded as PNG.
/// When `crop` is set (screen px of the full image), crop first and fit the
/// crop inside `to` keeping its aspect (zoom).
pub fn render_for_model(image_bytes: &[u8], to: Frame, crop: Option<(u32, u32, u32, u32)>) -> Result<(Vec<u8>, Frame), String> {
    let img = image::load_from_memory(image_bytes).map_err(|e| format!("couldn't decode screenshot: {e}"))?;
    let (img, target) = match crop {
        Some((x0, y0, x1, y1)) => {
            let (iw, ih) = (img.width(), img.height());
            let (x0, x1) = (x0.min(x1).min(iw.saturating_sub(1)), x0.max(x1).min(iw));
            let (y0, y1) = (y0.min(y1).min(ih.saturating_sub(1)), y0.max(y1).min(ih));
            let (cw, ch) = ((x1 - x0).max(1), (y1 - y0).max(1));
            let s = (to.width as f64 / cw as f64).min(to.height as f64 / ch as f64);
            let fit = Frame {
                width: ((cw as f64 * s).round() as u32).max(1),
                height: ((ch as f64 * s).round() as u32).max(1),
            };
            (img.crop_imm(x0, y0, cw, ch), fit)
        }
        None => (img, to),
    };
    let out = if img.width() == target.width && img.height() == target.height {
        img
    } else {
        img.resize_exact(target.width, target.height, image::imageops::FilterType::Triangle)
    };
    let mut buf = Vec::new();
    out.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageOutputFormat::Png)
        .map_err(|e| format!("couldn't encode screenshot: {e}"))?;
    Ok((buf, target))
}

// ---------------------------------------------------------------------------
// Request / result shapes.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct ToolsetRequest {
    pub toolset: Toolset,
    pub member: String,
    #[serde(default = "empty_object")]
    pub input: Value,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub turn_id: Option<String>,
    #[serde(default)]
    pub call_index: Option<u32>,
    #[serde(default)]
    pub model_frame: Option<Frame>,
    #[serde(default)]
    pub coordinate_space: Option<CoordinateSpace>,
    #[serde(default)]
    pub approval_grant: Option<String>,
    #[serde(default)]
    pub browser_session_id: Option<String>,
    /// Off-by-default members (`file_upload`, `read_console`, `read_network`,
    /// `javascript_exec`) the caller turns on for this call. Others are ignored.
    #[serde(default)]
    pub enable: Vec<String>,
    /// Set (to the subtask's run id) on the steps an approved `run_subtask`
    /// runs; never read from the wire.
    #[serde(skip)]
    pub within_subtask: Option<String>,
    /// The cowork project this call runs in. Not trusted on its own: it is
    /// checked against the owner's projects and the run's own project before
    /// it can change which safety settings apply (see
    /// `computer_safety::resolve_project_scope`).
    #[serde(default)]
    pub project_id: Option<String>,
}

fn empty_object() -> Value {
    json!({})
}

#[derive(Debug, Clone, Serialize)]
pub struct ScreenInfo {
    pub width: u32,
    pub height: u32,
    pub scale: f64,
    pub frame_width: u32,
    pub frame_height: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolsetResult {
    pub is_error: bool,
    pub content: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub browser_state: Option<Value>,
    pub screen: ScreenInfo,
    /// Machine-readable error code on refusals (not part of the model result).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub(crate) fn text(t: impl Into<String>) -> Value {
    json!({ "type": "text", "text": t.into() })
}

fn image_block(png: &[u8]) -> Value {
    json!({ "type": "image", "media_type": "image/png", "data": B64.encode(png) })
}

pub(crate) fn screen_info(map: Option<&Mapping>) -> ScreenInfo {
    match map {
        Some(m) => ScreenInfo {
            width: m.screen.width,
            height: m.screen.height,
            scale: (m.scale() * 1000.0).round() / 1000.0,
            frame_width: m.frame.width,
            frame_height: m.frame.height,
        },
        None => ScreenInfo { width: 0, height: 0, scale: 1.0, frame_width: 0, frame_height: 0 },
    }
}

pub(crate) fn error_result(code: &str, message: impl Into<String>, map: Option<&Mapping>) -> ToolsetResult {
    ToolsetResult {
        is_error: true,
        content: vec![text(message)],
        browser_state: None,
        screen: screen_info(map),
        error: Some(code.to_string()),
    }
}

/// Fill an ack template like "Scrolled {scroll_direction}." from the input.
fn ack(spec: &MemberSpec, input: &Value) -> String {
    let Some(template) = spec.ack_text.as_deref() else {
        return "OK".to_string();
    };
    let mut out = template.to_string();
    for key in ["text", "duration", "scroll_direction", "ref"] {
        let needle = format!("{{{key}}}");
        if !out.contains(&needle) {
            continue;
        }
        let value = input
            .get(key)
            .or_else(|| input.get("target").and_then(|t| t.get(key)))
            .map(|v| match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .unwrap_or_default();
        out = out.replace(&needle, &value);
    }
    out
}

// ---------------------------------------------------------------------------
// Batch rule: per (user, computer, turn), stop after the first failure.
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct BatchState {
    /// call_index -> settled ok (true) / failed or awaiting approval (false).
    calls: BTreeMap<u32, bool>,
    touched: Option<Instant>,
}

const BATCH_TTL: Duration = Duration::from_secs(15 * 60);

pub struct Batches {
    inner: Mutex<HashMap<String, BatchState>>,
}

impl Batches {
    pub fn new() -> Self {
        Batches { inner: Mutex::new(HashMap::new()) }
    }

    /// True when an earlier call of this turn failed or never completed.
    pub fn halted(&self, key: &str, index: u32) -> bool {
        let mut map = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        map.retain(|_, s| s.touched.map_or(true, |t| t.elapsed() < BATCH_TTL));
        map.get(key).is_some_and(|s| s.calls.range(..index).any(|(_, ok)| !ok))
    }

    pub fn record(&self, key: &str, index: u32, ok: bool) {
        let mut map = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let state = map.entry(key.to_string()).or_default();
        state.calls.insert(index, ok);
        state.touched = Some(Instant::now());
    }
}

impl Default for Batches {
    fn default() -> Self {
        Self::new()
    }
}

static BATCHES: Lazy<Batches> = Lazy::new(Batches::new);

// ---------------------------------------------------------------------------
// Screen sizes and action events.
// ---------------------------------------------------------------------------

/// Last known input-space size per target (computer id, or browser session).
static SCREENS: Lazy<Mutex<HashMap<String, (Frame, Instant)>>> = Lazy::new(|| Mutex::new(HashMap::new()));
const SCREEN_TTL: Duration = Duration::from_secs(60);

fn cached_screen(key: &str) -> Option<Frame> {
    let map = SCREENS.lock().unwrap_or_else(|p| p.into_inner());
    map.get(key).filter(|(_, at)| at.elapsed() < SCREEN_TTL).map(|(f, _)| *f)
}

fn remember_screen(key: &str, frame: Frame) {
    SCREENS.lock().unwrap_or_else(|p| p.into_inner()).insert(key.to_string(), (frame, Instant::now()));
}

/// `computer.action` events: (computer id, event). Consumed by the events
/// websocket (computer_ws) and the toolset SSE stream (P5 cursor overlay).
pub static ACTION_EVENTS: Lazy<tokio::sync::broadcast::Sender<(String, Value)>> =
    Lazy::new(|| tokio::sync::broadcast::channel(256).0);

pub(crate) fn emit_action(computer_id: &str, toolset: Toolset, member: &str, point: Option<(i64, i64)>, map: Option<&Mapping>, run_id: Option<&str>, ok: bool) {
    let screen = map.map(|m| m.screen);
    let event = json!({
        "type": "computer.action",
        "ts": chrono::Utc::now().to_rfc3339(),
        "data": {
            "toolset": toolset.as_str(),
            "member": member,
            "x": point.map(|p| p.0),
            "y": point.map(|p| p.1),
            "screen_w": screen.map(|s| s.width),
            "screen_h": screen.map(|s| s.height),
            "run_id": run_id,
            "ok": ok,
        }
    });
    let _ = ACTION_EVENTS.send((computer_id.to_string(), event));
}

// ---------------------------------------------------------------------------
// Targets.
// ---------------------------------------------------------------------------

/// Where a call runs.
pub enum Target {
    /// The Mac running this API, through Cua Driver.
    ThisDevice,
    /// A cloud/bot computer: shell commands in the guest via the VM driver.
    Guest {
        driver: Arc<dyn allternit_driver_interface::ExecutionDriver>,
        handle: allternit_driver_interface::ExecutionHandle,
        os: String,
        display: &'static str,
    },
    /// A Playwright session in the ACU gateway. `public_only`: the browser
    /// runs for a cloud target, so loopback/private/metadata hosts are refused.
    Browser { base: String, session_id: String, public_only: bool },
}

impl Target {
    pub fn sandboxed(&self) -> bool {
        !matches!(self, Target::ThisDevice)
    }

    pub fn label(&self) -> &'static str {
        match self {
            Target::ThisDevice => "this_device",
            Target::Guest { os, .. } if os == "windows" => "guest_windows",
            Target::Guest { .. } => "guest_linux",
            Target::Browser { .. } => "browser_gateway",
        }
    }
}

/// Members a target can run, or why not. `None` = supported.
pub fn unsupported_reason(target_label: &str, toolset: Toolset, member: &str) -> Option<&'static str> {
    match (toolset, target_label) {
        (Toolset::Computer, "guest_linux" | "guest_windows") => {
            if crate::computer_v2::is_v2_member(member) {
                // D1b: the guest image doesn't carry the Allternit Driver yet.
                Some("The Allternit Driver doesn't run on this computer yet, and the structured members need it. It arrives with the guest driver image.")
            } else {
                None
            }
        }
        (Toolset::Computer, "this_device") => match member {
            _ => None,
        },
        (Toolset::Computer, _) => Some("The computer toolset doesn't run on this target."),
        (Toolset::Browser, "browser_gateway") => None,
        (Toolset::Browser, _) => Some("The browser toolset doesn't run on this target."),
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn xdotool_modifier(m: &str) -> Option<&'static str> {
    match m.trim().to_lowercase().as_str() {
        "ctrl" | "control" => Some("ctrl"),
        "shift" => Some("shift"),
        "alt" | "option" | "opt" => Some("alt"),
        "cmd" | "command" | "super" | "meta" | "win" => Some("super"),
        _ => None,
    }
}

/// Valid xdotool key spec (e.g. `ctrl+a`, `Return`, `ctrl+shift+t Page_Down`).
fn key_spec_ok(k: &str) -> bool {
    !k.trim().is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || "+_-. ".contains(c))
}

fn point_of(input: &Value, key: &str) -> Option<(i64, i64)> {
    let a = input.get(key)?.as_array()?;
    Some((a.first()?.as_i64()?, a.get(1)?.as_i64()?))
}

/// The guest shell script (xdotool) for one computer member. Coordinates are
/// already screen pixels. `None` = handled by the executor itself.
pub fn xdotool_script(member: &str, input: &Value) -> Result<Option<String>, String> {
    let mods: Vec<&str> = match input.get("text").and_then(Value::as_str) {
        Some(t) if member.ends_with("click") || member == "scroll" || member == "left_click_drag" => t
            .split('+')
            .filter(|p| !p.trim().is_empty())
            .map(|p| xdotool_modifier(p).ok_or_else(|| format!("unknown modifier key: {p}")))
            .collect::<Result<_, _>>()?,
        _ => vec![],
    };
    let down = if mods.is_empty() { String::new() } else { format!("keydown {} ", mods.join(" keydown ")) };
    let up = if mods.is_empty() { String::new() } else { format!(" keyup {}", mods.join(" keyup ")) };
    let move_to = |key: &str| point_of(input, key).map(|(x, y)| format!("mousemove --sync {x} {y} "));
    let click = |button: u8, repeat: u8| {
        let r = if repeat > 1 { format!("--repeat {repeat} --delay 80 ") } else { String::new() };
        format!("xdotool {}{down}click {r}{button}{up}", move_to("coordinate").unwrap_or_default())
    };
    Ok(Some(match member {
        "left_click" => click(1, 1),
        "right_click" => click(3, 1),
        "middle_click" => click(2, 1),
        "double_click" => click(1, 2),
        "triple_click" => click(1, 3),
        "mouse_move" => format!("xdotool {}", move_to("coordinate").ok_or("coordinate is required")?),
        "left_click_drag" => {
            let (sx, sy) = point_of(input, "start_coordinate").ok_or("start_coordinate is required")?;
            let (ex, ey) = point_of(input, "coordinate").ok_or("coordinate is required")?;
            format!("xdotool {down}mousemove --sync {sx} {sy} mousedown 1 mousemove --sync {ex} {ey} mouseup 1{up}")
        }
        "left_mouse_down" => "xdotool mousedown 1".to_string(),
        "left_mouse_up" => "xdotool mouseup 1".to_string(),
        "scroll" => {
            let button = match input.get("scroll_direction").and_then(Value::as_str) {
                Some("up") => 4,
                Some("down") => 5,
                Some("left") => 6,
                Some("right") => 7,
                _ => return Err("scroll_direction must be up, down, left or right".into()),
            };
            let amount = input.get("scroll_amount").and_then(Value::as_f64).unwrap_or(3.0).round().clamp(1.0, 50.0) as u32;
            format!("xdotool {}{down}click --repeat {amount} --delay 30 {button}{up}", move_to("coordinate").unwrap_or_default())
        }
        "type" => {
            let t = input.get("text").and_then(Value::as_str).ok_or("text is required")?;
            format!("xdotool type --delay 12 -- {}", shell_quote(t))
        }
        "key" => {
            let k = input.get("text").and_then(Value::as_str).ok_or("text is required")?;
            if !key_spec_ok(k) {
                return Err(format!("not a key name xdotool understands: {k}"));
            }
            let repeat = input.get("repeat").and_then(Value::as_f64).unwrap_or(1.0).round().clamp(1.0, 100.0) as u32;
            let keys: Vec<String> = k.split_whitespace().map(shell_quote).collect();
            format!("xdotool key --repeat {repeat} --delay 40 -- {}", keys.join(" "))
        }
        "hold_key" => {
            let k = input.get("text").and_then(Value::as_str).ok_or("text is required")?;
            if !key_spec_ok(k) || k.contains(' ') {
                return Err(format!("not a single key xdotool understands: {k}"));
            }
            let secs = input.get("duration").and_then(Value::as_f64).unwrap_or(1.0).clamp(0.0, 30.0);
            format!("xdotool keydown -- {q} && sleep {secs} && xdotool keyup -- {q}", q = shell_quote(k))
        }
        "cursor_position" => "xdotool getmouselocation --shell".to_string(),
        "screenshot" | "zoom" | "wait" => return Ok(None),
        other => return Err(format!("unknown computer member: {other}")),
    }))
}

pub(crate) async fn guest_exec(target: &Target, script: &str) -> Result<String, String> {
    let Target::Guest { driver, handle, display, .. } = target else {
        return Err("not a guest target".into());
    };
    let mut env_vars = HashMap::new();
    env_vars.insert("DISPLAY".to_string(), display.to_string());
    let spec = allternit_driver_interface::CommandSpec {
        command: vec!["sh".into(), "-c".into(), format!("export DISPLAY={display}; {script}")],
        env_vars,
        working_dir: None,
        stdin_data: None,
        capture_stdout: true,
        capture_stderr: true,
    };
    let out = tokio::time::timeout(Duration::from_secs(45), driver.exec(handle, spec))
        .await
        .map_err(|_| "the computer didn't answer in time".to_string())?
        .map_err(|e| format!("couldn't reach the computer: {e}"))?;
    let stdout = String::from_utf8_lossy(out.stdout.as_deref().unwrap_or(&[])).to_string();
    if out.exit_code != 0 {
        let stderr = String::from_utf8_lossy(out.stderr.as_deref().unwrap_or(&[]));
        return Err(format!("the action failed on the computer (exit {}): {}", out.exit_code, stderr.trim().chars().take(300).collect::<String>()));
    }
    Ok(stdout)
}

/// One contract member on the ACU gateway's browser toolset endpoint
/// (`domains/computer-use/core/gateway/toolset_browser.py`). The reply carries
/// `is_error`, optional `text` / `image` (base64 PNG) and `browser_state`.
async fn browser_call(target: &Target, member: &str, input: &Value, run_id: &str) -> Result<Value, String> {
    let Target::Browser { base, session_id, public_only } = target else {
        return Err("not a browser target".into());
    };
    let body = json!({ "session_id": session_id, "member": member, "input": input, "run_id": run_id, "public_only": public_only });
    let resp = reqwest::Client::new()
        .post(format!("{base}/v1/toolset/browser"))
        .timeout(Duration::from_secs(90))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("the browser driver isn't reachable: {e}"))?;
    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err("the browser driver is too old for the browser toolset; update Allternit Desktop".into());
    }
    let value: Value = resp.json().await.map_err(|e| format!("the browser driver sent a bad reply: {e}"))?;
    if !status.is_success() {
        return Err(value.get("detail").map(|d| d.to_string()).unwrap_or_else(|| format!("browser driver HTTP {status}")));
    }
    Ok(value)
}

/// Public internet address (not loopback, private, CGNAT/mesh, link-local
/// incl. cloud metadata, documentation, benchmark, reserved or multicast).
pub fn ip_is_public(ip: &std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(a) => {
            let o = a.octets();
            !(a.is_loopback()
                || a.is_private()
                || a.is_link_local()
                || a.is_unspecified()
                || a.is_broadcast()
                || a.is_documentation()
                || a.is_multicast()
                || o[0] == 0
                || (o[0] == 100 && (64..128).contains(&o[1]))
                || (o[0] == 192 && o[1] == 0 && o[2] == 0)
                || (o[0] == 198 && (o[1] == 18 || o[1] == 19))
                || o[0] >= 240)
        }
        IpAddr::V6(a) => {
            if let Some(v4) = a.to_ipv4_mapped() {
                return ip_is_public(&IpAddr::V4(v4));
            }
            let s = a.segments();
            !(a.is_loopback()
                || a.is_unspecified()
                || a.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00
                || (s[0] & 0xffc0) == 0xfe80
                || (s[0] == 0x2001 && s[1] == 0x0db8))
        }
    }
}

/// Why a browser may not open `url`, if it may not. Only http(s) and
/// about:blank are navigable; cloud targets (`public_only`) also refuse
/// hosts that resolve to non-public addresses. The gateway re-checks every
/// request (redirects, subresources) for cloud sessions.
pub async fn browser_url_refusal(url: &str, public_only: bool) -> Option<String> {
    let u = url.trim();
    if u.eq_ignore_ascii_case("about:blank") {
        return None;
    }
    let Ok(parsed) = reqwest::Url::parse(u) else {
        return Some(format!("{u} isn't a valid URL."));
    };
    if !matches!(parsed.scheme(), "http" | "https") {
        return Some(format!("{u} isn't allowed: only http(s) pages and about:blank can be opened."));
    }
    if !public_only {
        return None;
    }
    let refuse = || Some(format!("{u} isn't allowed: cloud browsers can't open loopback, private-network or metadata addresses."));
    let host = parsed.host_str().unwrap_or_default().trim_matches(|c| c == '[' || c == ']').trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty()
        || host == "localhost"
        || [".localhost", ".local", ".internal"].iter().any(|s| host.ends_with(s))
        || ["metadata", "instance-data"].contains(&host.as_str())
    {
        return refuse();
    }
    let addrs: Vec<std::net::IpAddr> = match host.parse::<std::net::IpAddr>() {
        Ok(ip) => vec![ip],
        Err(_) => match tokio::net::lookup_host((host.as_str(), parsed.port_or_known_default().unwrap_or(443))).await {
            Ok(it) => it.map(|sa| sa.ip()).collect(),
            Err(_) => return Some(format!("{u} isn't reachable: {host} doesn't resolve.")),
        },
    };
    if addrs.is_empty() || addrs.iter().any(|a| !ip_is_public(a)) {
        return refuse();
    }
    None
}

// ---------------------------------------------------------------------------
// Windows guests: PowerShell + user32 through the VM driver's exec.
// ---------------------------------------------------------------------------

const WIN_INPUT_PREAMBLE: &str = r#"$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class AltInput {
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X; public int Y; }
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] static extern bool GetCursorPos(out POINT p);
  [DllImport("user32.dll")] static extern void mouse_event(uint f, uint dx, uint dy, int data, UIntPtr extra);
  [DllImport("user32.dll")] static extern void keybd_event(byte vk, byte scan, uint f, UIntPtr extra);
  public static void M(uint f, int d) { mouse_event(f, 0, 0, d, UIntPtr.Zero); }
  public static void K(byte vk, bool up) { uint ext = ((vk >= 0x21 && vk <= 0x2E) || vk == 0x5B) ? 1u : 0u; keybd_event(vk, 0, (up ? 2u : 0u) | ext, UIntPtr.Zero); }
  public static string Pos() { POINT p; GetCursorPos(out p); return "X=" + p.X + "\nY=" + p.Y; }
}
'@
"#;

/// Windows virtual-key code for an xdotool-style key name.
fn win_vk(name: &str) -> Option<u8> {
    let n = name.trim().to_lowercase();
    Some(match n.as_str() {
        "ctrl" | "control" | "control_l" | "control_r" => 0x11,
        "shift" | "shift_l" | "shift_r" => 0x10,
        "alt" | "option" | "opt" | "alt_l" | "alt_r" => 0x12,
        "cmd" | "command" | "super" | "super_l" | "win" | "meta" => 0x5B,
        "return" | "enter" | "kp_enter" => 0x0D,
        "tab" => 0x09,
        "escape" | "esc" => 0x1B,
        "backspace" => 0x08,
        "delete" | "del" => 0x2E,
        "insert" => 0x2D,
        "space" => 0x20,
        "home" => 0x24,
        "end" => 0x23,
        "page_up" | "pageup" | "prior" => 0x21,
        "page_down" | "pagedown" | "next" => 0x22,
        "up" => 0x26,
        "down" => 0x28,
        "left" => 0x25,
        "right" => 0x27,
        "minus" | "-" => 0xBD,
        "equal" | "=" => 0xBB,
        "comma" | "," => 0xBC,
        "period" | "." => 0xBE,
        "slash" | "/" => 0xBF,
        "semicolon" | ";" => 0xBA,
        "apostrophe" | "'" => 0xDE,
        "grave" | "`" => 0xC0,
        "bracketleft" | "[" => 0xDB,
        "bracketright" | "]" => 0xDD,
        "backslash" | "\\" => 0xDC,
        _ => {
            if let Some(f) = n.strip_prefix('f').and_then(|d| d.parse::<u8>().ok()).filter(|f| (1..=24).contains(f)) {
                return Some(0x6F + f);
            }
            let mut chars = n.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if c.is_ascii_alphanumeric() => c.to_ascii_uppercase() as u8,
                _ => return None,
            }
        }
    })
}

fn win_chord(spec: &str) -> Result<Vec<u8>, String> {
    spec.split('+').filter(|p| !p.trim().is_empty()).map(|p| win_vk(p).ok_or_else(|| format!("not a key name this Windows computer understands: {p}"))).collect()
}

/// The PowerShell script for one computer member on a Windows guest.
/// Coordinates are already screen pixels. `None` = handled by the executor.
pub fn windows_script(member: &str, input: &Value) -> Result<Option<String>, String> {
    let mods: Vec<u8> = match input.get("text").and_then(Value::as_str) {
        Some(t) if member.ends_with("click") || member == "scroll" || member == "left_click_drag" => win_chord(t)?,
        _ => vec![],
    };
    let keys_down = |vks: &[u8]| vks.iter().map(|v| format!("[AltInput]::K({v},$false)")).collect::<Vec<_>>();
    let keys_up = |vks: &[u8]| vks.iter().rev().map(|v| format!("[AltInput]::K({v},$true)")).collect::<Vec<_>>();
    let move_to = |key: &str| point_of(input, key).map(|(x, y)| format!("[AltInput]::SetCursorPos({x},{y}) | Out-Null; Start-Sleep -Milliseconds 40"));
    let with_mods = |body: Vec<String>| {
        let mut lines = keys_down(&mods);
        lines.extend(body);
        lines.extend(keys_up(&mods));
        lines
    };
    let click = |down: u32, up: u32, repeat: u32| {
        let mut body: Vec<String> = move_to("coordinate").into_iter().collect();
        for _ in 0..repeat {
            body.push(format!("[AltInput]::M({down},0); [AltInput]::M({up},0); Start-Sleep -Milliseconds 70"));
        }
        with_mods(body)
    };
    let lines: Vec<String> = match member {
        "left_click" => click(0x2, 0x4, 1),
        "right_click" => click(0x8, 0x10, 1),
        "middle_click" => click(0x20, 0x40, 1),
        "double_click" => click(0x2, 0x4, 2),
        "triple_click" => click(0x2, 0x4, 3),
        "mouse_move" => vec![move_to("coordinate").ok_or("coordinate is required")?],
        "left_click_drag" => {
            let (sx, sy) = point_of(input, "start_coordinate").ok_or("start_coordinate is required")?;
            let (ex, ey) = point_of(input, "coordinate").ok_or("coordinate is required")?;
            let mut body = vec![format!("[AltInput]::SetCursorPos({sx},{sy}) | Out-Null; Start-Sleep -Milliseconds 40"), "[AltInput]::M(2,0)".to_string()];
            for i in 1..=10 {
                let (x, y) = (sx + (ex - sx) * i / 10, sy + (ey - sy) * i / 10);
                body.push(format!("[AltInput]::SetCursorPos({x},{y}) | Out-Null; Start-Sleep -Milliseconds 25"));
            }
            body.push("[AltInput]::M(4,0)".to_string());
            with_mods(body)
        }
        "left_mouse_down" => vec!["[AltInput]::M(2,0)".to_string()],
        "left_mouse_up" => vec!["[AltInput]::M(4,0)".to_string()],
        "scroll" => {
            let (flag, delta) = match input.get("scroll_direction").and_then(Value::as_str) {
                Some("up") => (0x800, 120),
                Some("down") => (0x800, -120),
                Some("left") => (0x1000, -120),
                Some("right") => (0x1000, 120),
                _ => return Err("scroll_direction must be up, down, left or right".into()),
            };
            let amount = input.get("scroll_amount").and_then(Value::as_f64).unwrap_or(3.0).round().clamp(1.0, 50.0) as u32;
            let mut body: Vec<String> = move_to("coordinate").into_iter().collect();
            body.push(format!("1..{amount} | ForEach-Object {{ [AltInput]::M({flag},{delta}); Start-Sleep -Milliseconds 30 }}"));
            with_mods(body)
        }
        "type" => {
            let t = input.get("text").and_then(Value::as_str).ok_or("text is required")?;
            vec![
                format!("$t = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{}'))", B64.encode(t.as_bytes())),
                "$t = [regex]::Replace($t, '[+^%~(){}\\[\\]]', '{$0}')".to_string(),
                "$t = $t -replace \"`r?`n\", '{ENTER}'".to_string(),
                "Add-Type -AssemblyName System.Windows.Forms".to_string(),
                "[System.Windows.Forms.SendKeys]::SendWait($t)".to_string(),
            ]
        }
        "key" => {
            let k = input.get("text").and_then(Value::as_str).ok_or("text is required")?;
            let repeat = input.get("repeat").and_then(Value::as_f64).unwrap_or(1.0).round().clamp(1.0, 100.0) as u32;
            let mut body = vec![];
            for _ in 0..repeat {
                for combo in k.split_whitespace() {
                    let vks = win_chord(combo)?;
                    body.extend(keys_down(&vks));
                    body.extend(keys_up(&vks));
                    body.push("Start-Sleep -Milliseconds 30".to_string());
                }
            }
            body
        }
        "hold_key" => {
            let vks = win_chord(input.get("text").and_then(Value::as_str).ok_or("text is required")?)?;
            let ms = (input.get("duration").and_then(Value::as_f64).unwrap_or(1.0).clamp(0.0, 30.0) * 1000.0).round() as u64;
            let mut body = keys_down(&vks);
            body.push(format!("Start-Sleep -Milliseconds {ms}"));
            body.extend(keys_up(&vks));
            body
        }
        "cursor_position" => vec!["[AltInput]::Pos()".to_string()],
        "screenshot" | "zoom" | "wait" => return Ok(None),
        other => return Err(format!("unknown computer member: {other}")),
    };
    Ok(Some(format!("{WIN_INPUT_PREAMBLE}{}\n", lines.join("\n"))))
}

pub(crate) async fn windows_exec(target: &Target, script: &str) -> Result<String, String> {
    let Target::Guest { driver, handle, .. } = target else {
        return Err("not a guest target".into());
    };
    let out = tokio::time::timeout(Duration::from_secs(60), driver.exec(handle, crate::bot_desktop_windows::shell_command(script)))
        .await
        .map_err(|_| "the computer didn't answer in time".to_string())?
        .map_err(|e| format!("couldn't reach the computer: {e}"))?;
    if out.exit_code != 0 {
        let stderr = String::from_utf8_lossy(out.stderr.as_deref().unwrap_or(&[]));
        return Err(format!("the action failed on the computer (exit {}): {}", out.exit_code, stderr.trim().chars().take(300).collect::<String>()));
    }
    Ok(String::from_utf8_lossy(out.stdout.as_deref().unwrap_or(&[])).to_string())
}

/// `X=..` / `Y=..` lines (xdotool --shell, and the Windows helper).
fn parse_xy_lines(out: &str) -> Option<(f64, f64)> {
    let mut x = None;
    let mut y = None;
    for line in out.lines() {
        if let Some(v) = line.trim().strip_prefix("X=") {
            x = v.trim().parse::<f64>().ok();
        }
        if let Some(v) = line.trim().strip_prefix("Y=") {
            y = v.trim().parse::<f64>().ok();
        }
    }
    Some((x?, y?))
}

// ---------------------------------------------------------------------------
// This device (macOS): the Allternit Driver sidecar's pixel ops (Cua
// Driver 0.34 desktop scope underneath); hold_key posts CoreGraphics events.
// ---------------------------------------------------------------------------

/// Cua Driver modifier names for a contract chord (`ctrl+shift`).
pub fn cua_modifiers(spec: &str) -> Result<Vec<&'static str>, String> {
    spec.split('+')
        .filter(|p| !p.trim().is_empty())
        .map(|p| match p.trim().to_lowercase().as_str() {
            "ctrl" | "control" => Ok("ctrl"),
            "shift" => Ok("shift"),
            "alt" | "option" | "opt" => Ok("option"),
            "cmd" | "command" | "super" | "meta" | "win" => Ok("cmd"),
            other => Err(format!("unknown modifier key: {other}")),
        })
        .collect()
}

/// First `{x, y}` number pair anywhere in a driver reply.
fn find_xy(v: &Value) -> Option<(f64, f64)> {
    match v {
        Value::Object(m) => {
            if let (Some(x), Some(y)) = (m.get("x").and_then(Value::as_f64), m.get("y").and_then(Value::as_f64)) {
                return Some((x, y));
            }
            m.values().find_map(find_xy)
        }
        Value::Array(a) => a.iter().find_map(find_xy),
        Value::String(s) => serde_json::from_str::<Value>(s).ok().as_ref().and_then(find_xy),
        _ => None,
    }
}

/// The real cursor in screen pixels (Cua Driver reports points; its desktop
/// scope takes native screenshot pixels, which differ on Retina).
async fn this_device_cursor_px(map: &Mapping) -> Result<(i64, i64), String> {
    let reply = crate::this_device_input::call_driver("get_cursor_position", json!({})).await.map_err(String::from)?;
    let (px, py) = find_xy(&reply).ok_or("the driver didn't report a cursor position")?;
    let ratio = match crate::computer_routes::this_device_screen_points().await {
        Some(points) if points.width > 0 => map.screen.width as f64 / points.width as f64,
        _ => 1.0,
    };
    Ok(((px * ratio).round() as i64, (py * ratio).round() as i64))
}

/// The X display a Linux computer's own screen is on: `DISPLAY`, else `:0`.
fn local_display() -> String {
    std::env::var("DISPLAY").ok().map(|d| d.trim().to_string()).filter(|d| d.starts_with(':')).unwrap_or_else(|| ":0".to_string())
}

/// Make sure the Linux computer has an X screen. A headless computer (the
/// hosted-driver image) boots without one; the first computer member starts
/// `Xvfb` on `DISPLAY` (size `ALLTERNIT_HEADLESS_SCREEN`, default 1280x800)
/// and it stays up for the computer's life. A computer with a desktop
/// session already has the socket, so this is a no-op there.
async fn ensure_local_display() -> Result<String, String> {
    static START: Lazy<tokio::sync::Mutex<()>> = Lazy::new(|| tokio::sync::Mutex::new(()));
    let display = local_display();
    let number = display.trim_start_matches(':').split('.').next().unwrap_or("0").to_string();
    let socket = std::path::PathBuf::from(format!("/tmp/.X11-unix/X{number}"));
    if socket.exists() {
        return Ok(display);
    }
    let _guard = START.lock().await;
    if socket.exists() {
        return Ok(display);
    }
    let size = std::env::var("ALLTERNIT_HEADLESS_SCREEN").ok().filter(|s| s.split_once('x').is_some_and(|(w, h)| w.parse::<u32>().is_ok() && h.parse::<u32>().is_ok())).unwrap_or_else(|| "1280x800".to_string());
    std::process::Command::new("setsid")
        .args(["-f", "Xvfb", &display, "-screen", "0", &format!("{size}x24"), "-nolisten", "tcp"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("couldn't start the virtual screen (Xvfb): {e}"))?;
    for _ in 0..50 {
        if socket.exists() {
            return Ok(display);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err("the virtual screen (Xvfb) didn't start".into())
}

/// Run an X shell command (xdotool / scrot) on this Linux computer.
async fn local_x_exec(script: &str) -> Result<String, String> {
    let display = ensure_local_display().await?;
    let out = tokio::time::timeout(
        Duration::from_secs(45),
        tokio::process::Command::new("sh").arg("-c").arg(script).env("DISPLAY", &display).kill_on_drop(true).output(),
    )
    .await
    .map_err(|_| "the action didn't finish in time".to_string())?
    .map_err(|e| format!("couldn't run the action: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!("the action failed (exit {}): {}", out.status.code().unwrap_or(-1), stderr.trim().chars().take(300).collect::<String>()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Pending `left_mouse_down` per computer: Cua Driver 0.34 has no separate
/// press/release on macOS, so the press is held here and sent with the
/// release as one click (same spot) or press-drag-release gesture.
static PRESSES: Lazy<Mutex<HashMap<String, (i64, i64)>>> = Lazy::new(|| Mutex::new(HashMap::new()));

/// Capture the target's screen at full resolution.
async fn capture(target: &Target, run_id: &str) -> Result<Vec<u8>, String> {
    match target {
        Target::ThisDevice if cfg!(target_os = "linux") => {
            let out = local_x_exec("scrot -z -o /tmp/allternit-toolset.png && base64 -w0 /tmp/allternit-toolset.png").await?;
            B64.decode(out.trim()).map_err(|e| format!("invalid screenshot output: {e}"))
        }
        Target::ThisDevice => crate::computer_routes::capture_this_device_png().await,
        Target::Guest { os, .. } if os == "windows" => {
            let Target::Guest { driver, handle, .. } = target else { unreachable!() };
            let out = driver
                .exec(handle, crate::bot_desktop_windows::screenshot_command())
                .await
                .map_err(|e| format!("couldn't capture the screen: {e}"))?;
            let s = String::from_utf8_lossy(out.stdout.as_deref().unwrap_or(&[])).trim().to_string();
            B64.decode(s).map_err(|e| format!("invalid screenshot output: {e}"))
        }
        Target::Guest { .. } => {
            let out = guest_exec(target, "scrot -z -o /tmp/allternit-toolset.png && base64 -w0 /tmp/allternit-toolset.png").await?;
            B64.decode(out.trim()).map_err(|e| format!("invalid screenshot output: {e}"))
        }
        Target::Browser { .. } => browser_shot(target, &json!({}), run_id).await.map(|(png, _)| png).map_err(|f| f.message),
    }
}

/// A browser screenshot plus the tab inventory it was taken in.
async fn browser_shot(target: &Target, input: &Value, run_id: &str) -> Result<(Vec<u8>, Option<Value>), Fail> {
    let mut args = json!({});
    if let Some(tab) = input.get("tab_id") {
        args["tab_id"] = tab.clone();
    }
    let reply = browser_call(target, "screenshot", &args, run_id).await?;
    let state = reply.get("browser_state").cloned();
    if reply.get("is_error").and_then(Value::as_bool).unwrap_or(false) {
        let message = reply.get("text").and_then(Value::as_str).unwrap_or("the browser couldn't take a screenshot").to_string();
        return Err(Fail { message, browser_state: state, code: None });
    }
    let png = reply
        .get("image")
        .and_then(Value::as_str)
        .and_then(|d| B64.decode(d.trim()).ok())
        .ok_or_else(|| Fail { message: "the browser driver returned no screenshot".into(), browser_state: state.clone(), code: None })?;
    Ok((png, state))
}

/// The input-space size of a target: the coordinate space its input calls
/// use, which is the native screenshot size everywhere (Cua Driver 0.34's
/// desktop scope takes native screenshot pixels, not points, on a Retina Mac).
async fn input_space(target: &Target, key: &str, run_id: &str) -> Result<Frame, String> {
    if let Some(f) = cached_screen(key) {
        return Ok(f);
    }
    let png = capture(target, run_id).await?;
    let frame = png_size(&png).ok_or_else(|| "couldn't read the screenshot size".to_string())?;
    remember_screen(key, frame);
    Ok(frame)
}

fn this_device_mouse(action: &str, xy: Option<(i64, i64)>, end: Option<(i64, i64)>, button: Option<&str>, amount: Option<i32>) -> crate::bot_desktop_input::MouseInput {
    crate::bot_desktop_input::MouseInput {
        action: action.to_string(),
        x: xy.map(|p| p.0 as i32),
        y: xy.map(|p| p.1 as i32),
        button: button.map(str::to_string),
        end_x: end.map(|p| p.0 as i32),
        end_y: end.map(|p| p.1 as i32),
        amount,
    }
}

/// A failed dispatch: the model-facing message, plus the browser's tab
/// inventory when the failure happened in a browser.
#[derive(Debug)]
pub struct Fail {
    pub message: String,
    pub browser_state: Option<Value>,
    /// Machine-readable code for refusals that aren't driver failures
    /// (`safety_denied`, `safety_paused`, `needs_confirmation`).
    pub code: Option<&'static str>,
}

impl Fail {
    pub fn coded(code: &'static str, message: impl Into<String>) -> Self {
        Fail { message: message.into(), browser_state: None, code: Some(code) }
    }
}

impl From<String> for Fail {
    fn from(message: String) -> Self {
        Fail { message, browser_state: None, code: None }
    }
}

impl From<&str> for Fail {
    fn from(message: &str) -> Self {
        Fail { message: message.to_string(), browser_state: None, code: None }
    }
}

/// What screenshot redaction needs: the computer's safety settings and the
/// person's vault secrets (values never leave this process), plus the data
/// dir the server OCR engine caches its models in.
pub struct Redactor<'a> {
    pub settings: &'a crate::computer_safety::SafetySettings,
    pub secrets: &'a [String],
    pub data_dir: std::path::PathBuf,
}

/// Dispatch one action (coordinates already in screen px). Returns the
/// content blocks for a successful call.
#[allow(clippy::too_many_arguments)]
async fn dispatch(
    target: &Target,
    toolset: Toolset,
    spec: &MemberSpec,
    scaled: &Value,
    map: &Mapping,
    screen_key: &str,
    run_id: &str,
    redactor: &Redactor<'_>,
) -> Result<(Vec<Value>, Option<Value>), Fail> {
    let member = spec.name.as_str();
    // Members the executor runs the same way on every target.
    match member {
        "wait" => {
            let secs = scaled.get("duration").and_then(Value::as_f64).unwrap_or(1.0).clamp(0.0, 60.0);
            tokio::time::sleep(Duration::from_secs_f64(secs)).await;
            return Ok((vec![text(ack(spec, scaled))], None));
        }
        "screenshot" | "zoom" => {
            let (png, state) = match target {
                Target::Browser { .. } => browser_shot(target, scaled, run_id).await?,
                _ => (capture(target, run_id).await?, None),
            };
            let full = png_size(&png).ok_or("couldn't read the screenshot size")?;
            // Personal data is blacked out on the full-size capture, before
            // any copy of it leaves the executor (D5).
            let (png, report) = crate::computer_safety::redact_png(&png, redactor.settings, target, redactor.secrets, &redactor.data_dir)
                .await
                .map_err(|m| Fail::coded("redaction_unavailable", m))?;
            // Crop region arrives in input-space px; map it to image px.
            let crop = if member == "zoom" {
                let r = scaled.get("region").and_then(Value::as_array).ok_or("region is required")?;
                let sx = full.width as f64 / map.screen.width.max(1) as f64;
                let sy = full.height as f64 / map.screen.height.max(1) as f64;
                let v = |i: usize, s: f64| (r.get(i).and_then(Value::as_f64).unwrap_or(0.0) * s).round().max(0.0) as u32;
                Some((v(0, sx), v(1, sy), v(2, sx), v(3, sy)))
            } else {
                None
            };
            let (out, _) = render_for_model(&png, map.frame, crop)?;
            return Ok((vec![image_block(&out), text(crate::computer_safety::screenshot_note(&report))], state));
        }
        _ => {}
    }

    match (toolset, target) {
        (Toolset::Computer, Target::Guest { os, .. }) => {
            let out = if os == "windows" {
                let script = windows_script(member, scaled)?.ok_or("nothing to run")?;
                windows_exec(target, &script).await?
            } else {
                let script = xdotool_script(member, scaled)?.ok_or("nothing to run")?;
                guest_exec(target, &script).await?
            };
            if member == "cursor_position" {
                let (x, y) = parse_xy_lines(&out).ok_or("no cursor position")?;
                let (mx, my) = map.to_model(x, y);
                return Ok((vec![text(format!("X={mx},Y={my}"))], None));
            }
            Ok((vec![text(ack(spec, scaled))], None))
        }
        (Toolset::Computer, Target::ThisDevice) if cfg!(target_os = "linux") => {
            let script = xdotool_script(member, scaled)?.ok_or("nothing to run")?;
            let out = local_x_exec(&script).await?;
            if member == "cursor_position" {
                let (x, y) = parse_xy_lines(&out).ok_or("no cursor position")?;
                let (mx, my) = map.to_model(x, y);
                return Ok((vec![text(format!("X={mx},Y={my}"))], None));
            }
            Ok((vec![text(ack(spec, scaled))], None))
        }
        (Toolset::Computer, Target::ThisDevice) => {
            use crate::this_device_input as td;
            let mods = match scaled.get("text").and_then(Value::as_str) {
                Some(t) if member.ends_with("click") || member == "scroll" || member == "left_click_drag" => cua_modifiers(t)?,
                _ => vec![],
            };
            if member == "scroll" && !mods.is_empty() {
                return Err("Cua Driver 0.34 can't hold modifier keys while scrolling on macOS; scroll without text, or use key for a shortcut.".into());
            }
            let at = point_of(scaled, "coordinate");
            let need_at = || at.ok_or_else(|| format!("{member} on this device needs a coordinate"));
            match member {
                "cursor_position" => {
                    let (x, y) = this_device_cursor_px(map).await?;
                    let (mx, my) = map.to_model(x as f64, y as f64);
                    return Ok((vec![text(format!("X={mx},Y={my}"))], None));
                }
                "hold_key" => {
                    let k = scaled.get("text").and_then(Value::as_str).ok_or("text is required")?;
                    let secs = scaled.get("duration").and_then(Value::as_f64).unwrap_or(1.0);
                    td::hold_key(k, secs).await?;
                    return Ok((vec![text(ack(spec, scaled))], None));
                }
                "left_mouse_down" => {
                    let here = this_device_cursor_px(map).await?;
                    PRESSES.lock().unwrap_or_else(|p| p.into_inner()).insert(screen_key.to_string(), here);
                    return Ok((vec![text("Left mouse button pressed. On this Mac the press is sent together with left_mouse_up, as one click or drag.")], None));
                }
                "left_mouse_up" => {
                    let pressed = PRESSES.lock().unwrap_or_else(|p| p.into_inner()).remove(screen_key);
                    let Some(from) = pressed else {
                        return Err("The left mouse button isn't down; call left_mouse_down first.".into());
                    };
                    let to = this_device_cursor_px(map).await?;
                    let call = if (from.0 - to.0).abs() <= 2 && (from.1 - to.1).abs() <= 2 {
                        td::mouse_call(&this_device_mouse("click", Some(from), None, Some("left"), None))?
                    } else {
                        td::mouse_call(&this_device_mouse("drag", Some(from), Some(to), None, None))?
                    };
                    td::call_driver(call.0, call.1).await.map_err(String::from)?;
                    return Ok((vec![text(ack(spec, scaled))], None));
                }
                _ => {}
            }
            let mut call = match member {
                "left_click" => td::mouse_call(&this_device_mouse("click", Some(need_at()?), None, Some("left"), None)),
                "middle_click" => td::mouse_call(&this_device_mouse("click", Some(need_at()?), None, Some("middle"), None)),
                "right_click" => td::mouse_call(&this_device_mouse("rightclick", Some(need_at()?), None, None, None)),
                "double_click" => td::mouse_call(&this_device_mouse("doubleclick", Some(need_at()?), None, None, None)),
                "triple_click" => td::mouse_call(&this_device_mouse("doubleclick", Some(need_at()?), None, None, None)).map(|(tool, mut args)| {
                    args["count"] = json!(3);
                    (tool, args)
                }),
                "mouse_move" => td::mouse_call(&this_device_mouse("move", Some(need_at()?), None, None, None)),
                "left_click_drag" => {
                    let start = point_of(scaled, "start_coordinate").ok_or("start_coordinate is required")?;
                    td::mouse_call(&this_device_mouse("drag", Some(start), Some(need_at()?), None, None))
                }
                "scroll" => {
                    let dir = scaled.get("scroll_direction").and_then(Value::as_str);
                    let amount = scaled.get("scroll_amount").and_then(Value::as_f64).map(|a| a.round() as i32);
                    td::mouse_call(&this_device_mouse("scroll", at, None, dir, amount))
                }
                "type" => td::keyboard_call(&crate::bot_desktop_input::KeyboardInput {
                    action: "type".into(),
                    text: scaled.get("text").and_then(Value::as_str).map(str::to_string),
                    key: None,
                }),
                "key" => td::keyboard_call(&crate::bot_desktop_input::KeyboardInput {
                    action: "key".into(),
                    text: None,
                    key: scaled.get("text").and_then(Value::as_str).map(str::to_string),
                }),
                other => Err(format!("{other} isn't supported on this device")),
            }?;
            if !mods.is_empty() {
                // click / double_click / right_click / drag take `modifier`.
                call.1["modifier"] = json!(mods);
            }
            let repeat = if member == "key" {
                scaled.get("repeat").and_then(Value::as_f64).unwrap_or(1.0).round().clamp(1.0, 100.0) as u32
            } else {
                1
            };
            for _ in 0..repeat {
                td::call_driver(call.0, call.1.clone()).await.map_err(String::from)?;
            }
            Ok((vec![text(ack(spec, scaled))], None))
        }
        (Toolset::Browser, Target::Browser { public_only, .. }) => {
            if member == "navigate" {
                let url = scaled.get("url").and_then(Value::as_str).unwrap_or_default();
                if !matches!(url, "back" | "forward" | "reload") {
                    if let Some(why) = browser_url_refusal(url, *public_only).await {
                        return Err(why.into());
                    }
                    if !crate::aci_safety::HOST_POLICY.allows(url) {
                        return Err(format!("Navigation to {url} is blocked by this workspace's host policy.").into());
                    }
                }
            }
            let reply = browser_call(target, member, scaled, run_id).await?;
            let state = reply.get("browser_state").cloned();
            let said = reply.get("text").and_then(Value::as_str).filter(|t| !t.is_empty()).map(str::to_string);
            if reply.get("is_error").and_then(Value::as_bool).unwrap_or(false) {
                return Err(Fail { message: said.unwrap_or_else(|| format!("{member} failed in the browser")), browser_state: state, code: None });
            }
            if matches!(member, "navigate" | "new_tab" | "switch_tab" | "close_tab") {
                // A different page or tab can change the viewport; re-learn it.
                SCREENS.lock().unwrap_or_else(|p| p.into_inner()).remove(screen_key);
            }
            Ok((vec![text(said.unwrap_or_else(|| ack(spec, scaled)))], state))
        }
        _ => Err("this toolset doesn't run on this target".into()),
    }
}

// ---------------------------------------------------------------------------
// Target resolution, lease and approval.
// ---------------------------------------------------------------------------

pub const THIS_DEVICE_ID: &str = "this-device";

/// Resolve `:id` (`this-device` or a computer id) to the caller's computer.
pub(crate) async fn resolve_computer(state: &Arc<AppState>, user: &AuthUser, id: &str, headers: &HeaderMap) -> Result<ComputerResponse, (StatusCode, String)> {
    let id = if id == THIS_DEVICE_ID {
        let db = state.db.clone();
        let owner = user.user_id.clone();
        let device = headers
            .get(crate::cowork_devices_routes::DEVICE_ID_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let found = tokio::task::spawn_blocking(move || -> rusqlite::Result<Option<String>> {
            let conn = db.connect()?;
            use rusqlite::OptionalExtension;
            conn.query_row(
                "SELECT id FROM computers WHERE kind = 'local' AND provider = ?1 AND owner_id = ?2
                   AND status != 'deleted' ORDER BY (native_id = ?3) DESC, last_activity_at DESC LIMIT 1",
                rusqlite::params![crate::computer_routes::THIS_DEVICE_PROVIDER, owner, device.unwrap_or_default()],
                |r| r.get(0),
            )
            .optional()
        })
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "internal error".to_string()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("database error: {e}")))?;
        found.ok_or((StatusCode::NOT_FOUND, "This device isn't registered as a computer here. Open Computers in Allternit Desktop to add it.".to_string()))?
    } else {
        id.to_string()
    };
    match crate::computer_routes::fetch_computer(state, user, &id).await {
        Ok(Some(c)) => Ok(c),
        Ok(None) => Err((StatusCode::NOT_FOUND, "computer not found".into())),
        Err(resp) => Err((resp.status(), "failed to load computer".into())),
    }
}

pub(crate) async fn build_target(state: &Arc<AppState>, user: &AuthUser, computer: &ComputerResponse, toolset: Toolset, browser_session_id: Option<&str>) -> Result<Target, (StatusCode, String)> {
    if toolset == Toolset::Browser {
        let session_id = browser_session_id
            .map(str::to_string)
            .unwrap_or_else(|| format!("toolset-{}-{}", user.user_id, computer.id));
        // The browser may reach this computer's own network only when it runs
        // on the person's own machine; cloud computers are public-only.
        let public_only = !(crate::computer_routes::is_this_device(computer) && !crate::cloud_computer_peer::running_on_cloud_computer());
        return Ok(Target::Browser { base: state.config.acu_url().trim_end_matches('/').to_string(), session_id, public_only });
    }
    if crate::computer_routes::is_this_device(computer) {
        return Ok(Target::ThisDevice);
    }
    if computer.provider == crate::mesh_bridge::FABRIC_PROVIDER {
        return Err((StatusCode::CONFLICT, "This is a paired computer. Agents don't send it input; view it live over the mesh instead.".into()));
    }
    if computer.kind == ComputerKind::Local {
        return Err((StatusCode::CONFLICT, "This computer doesn't accept agent input.".into()));
    }
    let record = crate::computer_routes::computer_sandbox(state, computer)
        .map_err(|r| (r.status(), "couldn't find this computer's desktop".to_string()))?
        .ok_or((StatusCode::NOT_FOUND, "This computer has no desktop to drive.".to_string()))?;
    let driver = state
        .vm_driver
        .clone()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "No VM driver is configured on this host".to_string()))?;
    let handle = crate::bot_desktop_routes::build_handle(&record.sandbox_id, Some(&record.os), Some(&record.provider));
    Ok(Target::Guest {
        driver,
        handle,
        os: record.os.clone(),
        display: crate::bot_desktop_input::desktop_display(&record.provider),
    })
}

/// The agent asking for control: one holder per run.
pub(crate) fn agent_holder(user: &AuthUser, run_id: Option<&str>) -> Holder {
    Holder {
        kind: HolderKind::Agent,
        id: run_id.map(str::to_string).unwrap_or_else(|| format!("toolset:{}", user.user_id)),
        label: Some("Agent".to_string()),
        device_id: None,
    }
}

#[derive(Debug, PartialEq)]
pub enum LeaseRefusal {
    /// Someone else holds control.
    Held(String),
    /// This device needs its owner to take control first.
    TakeControlFirst,
    Db(String),
}

/// Lease gate. This device: only while its owner (or this agent) holds
/// control, since holding control is how a person lets agents drive their
/// real screen. Other computers: the agent takes (or renews) the lease, so
/// viewers see who is driving and a person can Take over; a lease held by
/// anyone it may not preempt refuses.
pub fn lease_gate(conn: &rusqlite::Connection, computer_id: &str, this_device: bool, owner_id: &str, caller: &Holder, now: i64) -> Result<(), LeaseRefusal> {
    let label = |l: &lease::Lease| l.holder.label.clone().unwrap_or_else(|| "Someone else".to_string());
    if this_device {
        return match lease::current(conn, computer_id, now).map_err(|e| LeaseRefusal::Db(e.to_string()))? {
            Some(l) if l.holder == *caller => Ok(()),
            Some(l) if l.holder.kind == HolderKind::User && l.holder.id == owner_id => Ok(()),
            Some(l) => Err(LeaseRefusal::Held(label(&l))),
            None => Err(LeaseRefusal::TakeControlFirst),
        };
    }
    match lease::take(conn, computer_id, caller, now) {
        Ok(_) => Ok(()),
        Err(lease::TakeError::Held(l)) => Err(LeaseRefusal::Held(label(&l))),
        Err(lease::TakeError::Db(e)) => Err(LeaseRefusal::Db(e)),
    }
}

/// The canonical action payload a grant is bound to.
pub fn action_descriptor(computer_id: &str, req: &ToolsetRequest) -> Value {
    json!({
        "route": "computer.toolset",
        "computer_id": computer_id,
        "toolset": req.toolset.as_str(),
        "member": req.member,
        "input": req.input,
    })
}

// ---------------------------------------------------------------------------
// The executor.
// ---------------------------------------------------------------------------

/// Run one toolset call. Returns the HTTP status and the result body.
pub async fn execute(state: &Arc<AppState>, user: &AuthUser, id: &str, headers: &HeaderMap, req: ToolsetRequest) -> (StatusCode, Value) {
    let c = contract(req.toolset);
    let respond = |status: StatusCode, r: ToolsetResult| (status, serde_json::to_value(r).unwrap_or_default());

    // 1. Contract validation.
    let Some(spec) = c.member(&req.member) else {
        return respond(StatusCode::BAD_REQUEST, error_result("unknown_member", format!("{} is not a member of {}", req.member, c.id), None));
    };
    if let Err(reason) = validate(&spec.input_schema, &req.input, "") {
        return respond(StatusCode::BAD_REQUEST, error_result("invalid_input", reason, None));
    }
    if !spec.default_enabled && !req.enable.iter().any(|m| m == &spec.name) {
        return respond(
            StatusCode::FORBIDDEN,
            error_result("member_disabled", format!("{} is off by default; turn it on for this call with \"enable\": [\"{}\"].", spec.name, spec.name), None),
        );
    }
    let computer = match resolve_computer(state, user, id, headers).await {
        Ok(c) => c,
        Err((status, message)) => return respond(status, error_result("computer_unavailable", message, None)),
    };
    let batch_key = req.turn_id.as_deref().map(|t| format!("{}:{}:{}", user.user_id, computer.id, t));
    let index = req.call_index.unwrap_or(0);
    if let Some(key) = &batch_key {
        if BATCHES.halted(key, index) {
            return respond(StatusCode::OK, error_result("not_executed", c.batch_halt_text.clone(), None));
        }
    }
    let settle = |ok: bool| {
        if let Some(key) = &batch_key {
            BATCHES.record(key, index, ok);
        }
    };

    // Fan-out (run_parallel, run_subtask with best_of): its subtasks name
    // their own computers, so their targets, leases, approval and audit are
    // gated per computer there, not on this route's computer.
    if req.toolset == Toolset::Computer && crate::computer_parallel::is_fanout(&req.member, &req.input) {
        let (status, body) = crate::computer_parallel::execute(state, user, headers, &computer, spec, &req).await;
        settle(status == StatusCode::OK && !body.get("is_error").and_then(Value::as_bool).unwrap_or(false));
        return (status, body);
    }

    let target = match build_target(state, user, &computer, req.toolset, req.browser_session_id.as_deref()).await {
        Ok(t) => t,
        Err((status, message)) => {
            settle(false);
            return respond(status, error_result("target_unavailable", message, None));
        }
    };
    if let Some(reason) = unsupported_reason(target.label(), req.toolset, &req.member) {
        settle(false);
        return respond(StatusCode::OK, error_result("unimplemented", format!("{} is not available on this computer: {reason}", req.member), None));
    }

    // 2. Control lease (computers only; a gateway browser session is not a
    // shared screen).
    if req.toolset == Toolset::Computer {
        if let Err((status, code, message)) = lease_check(state, user, &computer, &target, req.run_id.as_deref()).await {
            settle(false);
            return respond(status, error_result(code, message, None));
        }
    }

    // 3. Declarative policy, then risk and approval.
    let (policy_desc, verdict) = match policy_check(user, &computer, &req) {
        Ok(pv) => pv,
        Err(reason) => {
            settle(false);
            return respond(StatusCode::FORBIDDEN, error_result("policy_denied", reason, None));
        }
    };
    // 3b. Pixel members: coordinates into screen px now, so the safety layer
    // can hit-test the element under the point (the screen size is cached).
    let v2 = crate::computer_v2::is_v2_member(&req.member);
    let screen_key = match &target {
        Target::Browser { session_id, .. } => format!("browser:{session_id}"),
        _ => computer.id.clone(),
    };
    let run_id = req.run_id.clone().unwrap_or_else(|| format!("toolset-{}", uuid::Uuid::new_v4().simple()));
    let map = if v2 {
        None
    } else {
        match input_space(&target, &screen_key, &run_id).await {
            Ok(screen) => Some(Mapping::new(screen, req.model_frame, req.coordinate_space.unwrap_or_default(), &c.model_frame)),
            Err(e) => {
                settle(false);
                return respond(StatusCode::OK, error_result("driver_failed", e, None));
            }
        }
    };
    let scaled = match &map {
        Some(m) => scale_input(spec, &req.input, m),
        None => req.input.clone(),
    };

    // 3c. The safety layer (D5/E3): lists, secret binding, irreversible
    // classes, watch mode and the per-step monitor. The project scope the
    // settings resolve through comes from the run's own session project; a
    // caller-supplied project_id must match it (or, with no run project, name
    // one of the owner's projects) — never trusted blindly from the wire.
    let project_scope = match crate::computer_safety::resolve_project_scope(state, &user.user_id, req.project_id.as_deref(), req.run_id.as_deref()).await {
        Ok(p) => p,
        Err(message) => {
            settle(false);
            return respond(StatusCode::FORBIDDEN, error_result("safety_denied", message, None));
        }
    };
    let safety = crate::computer_safety::assess(state, user, &computer, &target, req.toolset, spec, &scaled, req.run_id.as_deref(), project_scope.as_deref()).await;
    let watching = crate::computer_safety::watch_note(&computer.id, &safety, spec);
    match &safety.verdict {
        crate::computer_safety::Verdict::Deny(reason) => {
            settle(false);
            let _ = audit(user, &computer, &target, spec, &req, &policy_desc, verdict.as_ref(), Some(&safety));
            crate::computer_safety::emit(&computer.id, "denied", Some(&safety), &req.member, req.run_id.as_deref(), json!({}));
            return respond(StatusCode::FORBIDDEN, error_result("safety_denied", reason.clone(), map.as_ref()));
        }
        crate::computer_safety::Verdict::Pause(reason) => {
            settle(false);
            let _ = audit(user, &computer, &target, spec, &req, &policy_desc, verdict.as_ref(), Some(&safety));
            crate::computer_safety::emit(&computer.id, "paused", Some(&safety), &req.member, req.run_id.as_deref(), json!({}));
            crate::computer_safety::record_monitor_outcome(state, user, &safety, "skipped", "paused before running");
            return respond(
                StatusCode::OK,
                error_result(
                    "safety_paused",
                    format!("{reason} The step did not run. Stop here: call request_human so a person can look at the screen, and don't retry this step on your own."),
                    map.as_ref(),
                ),
            );
        }
        _ => {}
    }
    let safety_confirm = matches!(safety.verdict, crate::computer_safety::Verdict::Confirm(_));

    // Steps inside an approved run_subtask carry that subtask's grant: the
    // person approved the bounded goal and its literal inputs once, and the
    // loop can only type those inputs. Lease, policy and audit still run.
    // Irreversible, watch-mode and monitor confirmations always ask.
    if safety_confirm
        || (req.within_subtask.is_none()
            && (needs_approval(spec, target.sandboxed())
                || crate::computer_v2::v2_member_needs_approval(&req.member, &req.input, target.sandboxed())))
    {
        let class = if safety.irreversible.is_some() {
            ConfirmationClass::Irreversible
        } else {
            match risk_class(spec) {
                ConfirmationClass::Reversible => ConfirmationClass::Risky,
                other => other,
            }
        };
        if let Err(denial) = crate::aci_safety::enforce_confirmation(
            &state.approval_store,
            &user.user_id,
            "computer.toolset",
            class,
            &action_descriptor(&computer.id, &req),
            req.approval_grant.as_deref(),
        ) {
            // Unresolved until the retry with a grant settles it.
            settle(false);
            let mut body = denial.body;
            let code = body.get("error").and_then(Value::as_str).unwrap_or("approval_required").to_string();
            let (status, code) = if code == "confirmation_required" {
                (StatusCode::CONFLICT, "approval_required".to_string())
            } else {
                (denial.status, code)
            };
            let why = match &safety.verdict {
                crate::computer_safety::Verdict::Confirm(r) => format!(": {r}"),
                _ => String::new(),
            };
            if code == "approval_required" {
                crate::computer_safety::emit(&computer.id, "confirm", Some(&safety), &req.member, req.run_id.as_deref(), json!({ "approval_id": body.get("approval_id") }));
            }
            body["error"] = json!(code);
            body["is_error"] = json!(true);
            body["member"] = json!(req.member);
            body["toolset"] = json!(req.toolset.as_str());
            body["risk"] = json!(if safety.irreversible.is_some() { "irreversible" } else { spec.risk.as_str() });
            body["safety"] = serde_json::to_value(&safety).unwrap_or_default();
            body["content"] = json!([text(format!("{} needs a person's approval before it runs{why}.", req.member))]);
            body["screen"] = serde_json::to_value(screen_info(None)).unwrap_or_default();
            return (status, body);
        }
    }

    // 4. Audit row, durable before the action.
    if let Err(e) = audit(user, &computer, &target, spec, &req, &policy_desc, verdict.as_ref(), Some(&safety)) {
        warn!(error = %e, "toolset audit write failed; refusing to dispatch");
        settle(false);
        return respond(StatusCode::INTERNAL_SERVER_ERROR, error_result("audit_unavailable", "The action log couldn't be written, so the action did not run.", None));
    }
    if watching {
        crate::computer_safety::emit(&computer.id, "watch", Some(&safety), &req.member, req.run_id.as_deref(), json!({}));
    }

    // 4b. Rollback point on cloud computers before a risky step.
    let mut notes: Vec<Value> = Vec::new();
    if safety.snapshot {
        match crate::computer_safety::snapshot_before(&target, &computer.id, req.run_id.as_deref()).await {
            Ok(Some(snap)) => {
                crate::computer_safety::emit(&computer.id, "snapshot", Some(&safety), &req.member, req.run_id.as_deref(), json!({ "snapshot_id": snap }));
                notes.push(text(format!("Rollback point {snap} was saved before this step (restore it from the computer's snapshots).")));
            }
            Ok(None) => {}
            Err(e) => {
                warn!(computer_id = %computer.id, error = %e, "safety rollback snapshot failed; the confirmed step runs without one");
                crate::computer_safety::emit(&computer.id, "snapshot_failed", Some(&safety), &req.member, req.run_id.as_deref(), json!({ "error": e }));
            }
        }
    }

    // 5. Dispatch.
    let secrets: Vec<String> = crate::aci_credentials::CREDENTIALS.screening_secrets(&user.user_id).into_iter().map(|(_, v, _)| v).collect();
    crate::computer_routes::touch_computer_activity(&state.db, &computer.id);
    if req.member == "run_subtask" || req.member == "run_skill" || v2 {
        // run_subtask / run_skill: the bounded decision loop (a skill is a
        // saved recording of one); each of its steps, replayed ones too, goes
        // back through `execute_step` (lease, policy, safety, audit, dispatch,
        // event). One subtask loop per computer at a time: a second one is
        // refused. Structured v2 members: no screenshot and no coordinate
        // scaling — the driver answers read_ui/act/run_batch/verify, the human
        // gate pauses the lease, the credential backends type the value.
        let _owner = if req.member == "run_subtask" || req.member == "run_skill" {
            match crate::computer_parallel::claim_input(&computer.id, &run_id) {
                Ok(g) => Some(g),
                Err(holder) => {
                    settle(false);
                    return respond(StatusCode::LOCKED, error_result("computer_busy", crate::computer_parallel::busy_text(&holder), None));
                }
            }
        } else {
            None
        };
        let outcome = match req.member.as_str() {
            "run_subtask" => crate::computer_subtask::run(state, user, &computer, &target, &req.input, &run_id).await,
            "run_skill" => crate::computer_subtask::run_skill(state, user, &computer, &target, &req.input, &run_id).await,
            _ => crate::computer_v2::execute_v2(state, user, &computer, &target, &req.member, &req.input, &run_id).await,
        };
        emit_action(&computer.id, req.toolset, &req.member, None, None, req.run_id.as_deref(), outcome.is_ok());
        crate::computer_safety::record_monitor_outcome(state, user, &safety, if outcome.is_ok() { "success" } else { "error" }, "step ran");
        return match outcome {
            Ok(mut result) => {
                settle(!result.is_error);
                if crate::computer_safety::observation_member(&req.member) {
                    crate::computer_safety::mark_untrusted(&req.member, &mut result.content, &secrets);
                }
                result.content.extend(notes);
                (StatusCode::OK, serde_json::to_value(result).unwrap_or_default())
            }
            Err(fail) => {
                settle(false);
                respond(StatusCode::OK, error_result(fail.code.unwrap_or("action_failed"), fail.message, None))
            }
        };
    }
    let map = map.expect("pixel members have a mapping");
    let point = primary_point(spec, &scaled);
    let redactor = Redactor { settings: &safety.settings, secrets: &secrets, data_dir: state.data_dir.clone() };
    let outcome = dispatch(&target, req.toolset, spec, &scaled, &map, &screen_key, &run_id, &redactor).await;
    emit_action(&computer.id, req.toolset, &req.member, point, Some(&map), req.run_id.as_deref(), outcome.is_ok());
    crate::computer_safety::record_monitor_outcome(state, user, &safety, if outcome.is_ok() { "success" } else { "error" }, "step ran");
    match outcome {
        Ok((mut content, browser_state)) => {
            settle(true);
            if let Target::Browser { session_id, .. } = &target {
                let said = content.iter().find_map(|b| b.get("text").and_then(Value::as_str)).map(str::to_string);
                crate::computer_safety::remember_browser(session_id, &req.member, said.as_deref(), browser_state.as_ref());
            }
            if crate::computer_safety::observation_member(&req.member) {
                crate::computer_safety::mark_untrusted(&req.member, &mut content, &secrets);
            }
            content.extend(notes);
            respond(StatusCode::OK, ToolsetResult { is_error: false, content, browser_state, screen: screen_info(Some(&map)), error: None })
        }
        Err(fail) => {
            settle(false);
            if let (Target::Browser { session_id, .. }, Some(state)) = (&target, fail.browser_state.as_ref()) {
                crate::computer_safety::remember_browser(session_id, &req.member, None, Some(state));
            }
            let mut r = error_result(fail.code.unwrap_or("action_failed"), fail.message, Some(&map));
            r.browser_state = fail.browser_state;
            respond(StatusCode::OK, r)
        }
    }
}

/// The person's approval for one call: `Err((status, body))` with the
/// approval-required refusal until the call carries a valid single-use grant
/// bound to `action_descriptor(computer_id, req)`.
pub(crate) fn approval_gate(state: &Arc<AppState>, user: &AuthUser, computer_id: &str, spec: &MemberSpec, req: &ToolsetRequest) -> Result<(), (StatusCode, Value)> {
    if let Err(denial) = crate::aci_safety::enforce_confirmation(
        &state.approval_store,
        &user.user_id,
        "computer.toolset",
        match risk_class(spec) {
            ConfirmationClass::Reversible => ConfirmationClass::Risky,
            other => other,
        },
        &action_descriptor(computer_id, req),
        req.approval_grant.as_deref(),
    ) {
        let mut body = denial.body;
        let code = body.get("error").and_then(Value::as_str).unwrap_or("approval_required").to_string();
        let (status, code) = if code == "confirmation_required" {
            (StatusCode::CONFLICT, "approval_required".to_string())
        } else {
            (denial.status, code)
        };
        body["error"] = json!(code);
        body["is_error"] = json!(true);
        body["member"] = json!(req.member);
        body["toolset"] = json!(req.toolset.as_str());
        body["risk"] = json!(spec.risk);
        body["content"] = json!([text(format!("{} needs a person's approval before it runs.", req.member))]);
        body["screen"] = serde_json::to_value(screen_info(None)).unwrap_or_default();
        return Err((status, body));
    }
    Ok(())
}

/// The control-lease gate for one call: `Err((status, code, message))`
/// when someone else holds control or this device's owner hasn't taken it.
pub(crate) async fn lease_check(
    state: &Arc<AppState>,
    user: &AuthUser,
    computer: &ComputerResponse,
    target: &Target,
    run_id: Option<&str>,
) -> Result<(), (StatusCode, &'static str, String)> {
    let caller = agent_holder(user, run_id);
    let (db, cid, owner, this_device) = (state.db.clone(), computer.id.clone(), user.user_id.clone(), matches!(target, Target::ThisDevice));
    let verdict = tokio::task::spawn_blocking(move || {
        let conn = db.connect().map_err(|e| LeaseRefusal::Db(e.to_string()))?;
        lease_gate(&conn, &cid, this_device, &owner, &caller, chrono::Utc::now().timestamp())
    })
    .await
    .unwrap_or_else(|_| Err(LeaseRefusal::Db("internal error".into())));
    match verdict {
        Ok(()) => Ok(()),
        Err(LeaseRefusal::Held(who)) => Err((StatusCode::LOCKED, "computer_controlled_elsewhere", format!("{who} is controlling this computer. Ask them to hand over control."))),
        Err(LeaseRefusal::TakeControlFirst) => Err((
            StatusCode::LOCKED,
            "take_control_first",
            "Take control of this computer first (Computers > This computer > Take control) so the agent can use its mouse and keyboard.".to_string(),
        )),
        Err(LeaseRefusal::Db(e)) => Err((StatusCode::INTERNAL_SERVER_ERROR, "lease_unavailable", e)),
    }
}

/// Declarative policy for one call: the descriptor and verdict, or the
/// refusal reason when the policy denies it (the denial is recorded).
pub(crate) fn policy_check(
    user: &AuthUser,
    computer: &ComputerResponse,
    req: &ToolsetRequest,
) -> Result<(crate::policy_config::PolicyDescriptor, Option<crate::permission_policy::PolicyVerdict>), String> {
    let policy_desc = crate::policy_config::PolicyDescriptor {
        tool: format!("computer.toolset.{}.{}", req.toolset.as_str(), req.member),
        intent: Some(req.member.clone()),
        bot_id: computer.bot_id.clone(),
        session_id: computer.session_id.clone(),
        network_host: req.input.get("url").and_then(Value::as_str).and_then(|u| reqwest::Url::parse(u).ok()).and_then(|u| u.host_str().map(str::to_string)),
        ..Default::default()
    };
    let verdict = crate::policy_config::evaluate_descriptor(&policy_desc);
    if let Some(v) = &verdict {
        if v.action == crate::permission_policy::PermissionAction::Deny {
            let _ = crate::policy_config::record_decision(&policy_desc, v, Some(&user.user_id), req.run_id.as_deref());
            return Err(crate::policy_config::refusal_json(v)["reason"].as_str().unwrap_or("denied by policy").to_string());
        }
    }
    Ok((policy_desc, verdict))
}

/// The audit row, written (and fsynced) before the action runs.
#[allow(clippy::too_many_arguments)]
pub(crate) fn audit(
    user: &AuthUser,
    computer: &ComputerResponse,
    target: &Target,
    spec: &MemberSpec,
    req: &ToolsetRequest,
    policy_desc: &crate::policy_config::PolicyDescriptor,
    verdict: Option<&crate::permission_policy::PolicyVerdict>,
    safety: Option<&crate::computer_safety::Assessment>,
) -> Result<(), String> {
    // A safety refusal is a denied row of its own; an allowed step carries
    // the safety note in its intent.
    let refused = safety.is_some_and(|a| matches!(a.verdict, crate::computer_safety::Verdict::Deny(_) | crate::computer_safety::Verdict::Pause(_)));
    let note = safety.map(|a| a.audit_note()).unwrap_or_default();
    let audited = match verdict {
        Some(v) if !refused => {
            let mut desc = policy_desc.clone();
            desc.intent = Some(format!("{}{note}", desc.intent.as_deref().unwrap_or(&req.member)));
            crate::policy_config::record_decision(&desc, v, Some(&user.user_id), req.run_id.as_deref())
        }
        _ => {
            let decision = if refused { crate::policy_audit::PolicyDecision::Denied } else { crate::policy_audit::PolicyDecision::Allowed };
            let mut row = crate::policy_audit::PolicyAuditRow::new(decision, policy_desc.tool.clone());
            row.actor = Some(user.user_id.clone());
            row.bot_id = computer.bot_id.clone();
            row.session_id = computer.session_id.clone();
            let within = req.within_subtask.as_deref().map(|p| format!(" subtask={p}")).unwrap_or_default();
            row.intent = Some(format!("risk={} target={} computer={}{within}{note}", spec.risk, target.label(), computer.id));
            row.host = policy_desc.network_host.clone();
            row.run_id = req.run_id.clone();
            crate::policy_audit::record(&row)
        }
    };
    audited.map(|_| ()).map_err(|e| e.to_string())
}

/// One step of an approved `run_subtask` (read_ui, act, run_batch, verify):
/// contract validation, the lease, policy and audit, then the driver, and the
/// action event — the same path as `execute`, minus the per-step approval the
/// subtask's own grant covers. Returns the driver's JSON reply.
pub(crate) async fn execute_step(
    state: &Arc<AppState>,
    user: &AuthUser,
    computer: &ComputerResponse,
    target: &Target,
    member: &str,
    input: Value,
    run_id: &str,
) -> Result<Value, Fail> {
    if !matches!(member, "read_ui" | "act" | "run_batch" | "verify") || !matches!(target, Target::ThisDevice) {
        return Err(Fail::from(format!("{member} can't run inside a subtask on this target")));
    }
    let c = contract(Toolset::Computer);
    let spec = c.member(member).ok_or_else(|| Fail::from(format!("{member} is not a member of {}", c.id)))?;
    validate(&spec.input_schema, &input, "").map_err(Fail::from)?;
    let req = ToolsetRequest {
        toolset: Toolset::Computer,
        member: member.to_string(),
        input,
        run_id: Some(run_id.to_string()),
        turn_id: None,
        call_index: None,
        model_frame: None,
        coordinate_space: None,
        approval_grant: None,
        browser_session_id: None,
        enable: vec![],
        within_subtask: Some(run_id.to_string()),
        project_id: None,
    };
    lease_check(state, user, computer, target, Some(run_id)).await.map_err(|(_, _, m)| Fail::from(m))?;
    let (policy_desc, verdict) = policy_check(user, computer, &req).map_err(Fail::from)?;
    // The safety layer runs on every step. The subtask's one approval can't
    // cover an irreversible, watch-mode or monitor-flagged step: those end
    // the subtask and hand the step back to the planner, whose own call of
    // it goes through the person's approval. The project scope comes from
    // the run itself, like every other step of this subtask.
    let project_scope = crate::computer_safety::resolve_project_scope(state, &user.user_id, None, Some(run_id))
        .await
        .map_err(|m| Fail::coded("safety_denied", m))?;
    let safety = crate::computer_safety::assess(state, user, computer, target, Toolset::Computer, spec, &req.input, Some(run_id), project_scope.as_deref()).await;
    let watching = crate::computer_safety::watch_note(&computer.id, &safety, spec);
    let refusal = match &safety.verdict {
        crate::computer_safety::Verdict::Deny(r) => Some(("safety_denied", "denied", r.clone())),
        crate::computer_safety::Verdict::Pause(r) => Some(("safety_paused", "paused", r.clone())),
        crate::computer_safety::Verdict::Confirm(r) => Some(("needs_confirmation", "confirm", r.clone())),
        crate::computer_safety::Verdict::Allow => None,
    };
    if let Some((code, phase, reason)) = refusal {
        if code != "needs_confirmation" {
            let _ = audit(user, computer, target, spec, &req, &policy_desc, verdict.as_ref(), Some(&safety));
        }
        crate::computer_safety::emit(&computer.id, phase, Some(&safety), member, Some(run_id), json!({ "within_subtask": true }));
        crate::computer_safety::record_monitor_outcome(state, user, &safety, "skipped", "handed back to the planner");
        return Err(Fail::coded(code, reason));
    }
    audit(user, computer, target, spec, &req, &policy_desc, verdict.as_ref(), Some(&safety))
        .map_err(|_| Fail::from("The action log couldn't be written, so the action did not run."))?;
    if watching {
        crate::computer_safety::emit(&computer.id, "watch", Some(&safety), member, Some(run_id), json!({ "within_subtask": true }));
    }
    // Straight to the driver: a subtask step needs the driver's JSON, not the
    // model-facing result (whose screen block costs a screen-size query).
    let outcome = crate::computer_v2::driver_op(member, &req.input).await;
    emit_action(&computer.id, Toolset::Computer, member, None, None, Some(run_id), outcome.is_ok());
    crate::computer_safety::record_monitor_outcome(state, user, &safety, if outcome.is_ok() { "success" } else { "error" }, "step ran");
    outcome
}

// ---------------------------------------------------------------------------
// Routes.
// ---------------------------------------------------------------------------

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/computers/:id/toolset", post(post_toolset))
        .route("/computers/:id/toolset/schema", get(get_toolset_schema))
        .route("/computers/:id/toolset/events", get(get_toolset_events))
        .route("/computers/:id/human-done", post(post_human_done))
        .route("/computers/:id/safety", get(crate::computer_safety::get_safety).put(crate::computer_safety::put_safety))
        .route(
            "/projects/:project_id/safety",
            get(crate::computer_safety::get_project_safety).put(crate::computer_safety::put_project_safety),
        )
}

/// The person signals the human window is complete (CAPTCHA done, 2FA
/// answered). Wakes the pending `request_human` call, if any; the agent
/// session resumes on the same run.
async fn post_human_done(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let computer = match resolve_computer(&state, &user, &id, &headers).await {
        Ok(c) => c,
        Err((status, message)) => return (status, Json(json!({ "error": "computer_unavailable", "message": message }))).into_response(),
    };
    let found = crate::computer_v2::human_gate_done(&computer.id);
    Json(json!({ "ok": true, "window_open": found })).into_response()
}

async fn post_toolset(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(req): Json<ToolsetRequest>,
) -> Response {
    let (status, body) = execute(&state, &user, &id, &headers, req).await;
    (status, Json(body)).into_response()
}

#[derive(Debug, Deserialize)]
pub struct SchemaQuery {
    #[serde(default)]
    toolset: Option<Toolset>,
    /// Comma-separated off-by-default members to report as on (the caller
    /// will send them with `enable`).
    #[serde(default)]
    enable: Option<String>,
}

async fn gateway_healthy(base: &str) -> bool {
    reqwest::Client::new()
        .get(format!("{base}/health"))
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .is_ok_and(|r| r.status().is_success())
}

/// The members a target offers: contract metadata plus `enabled` (default on
/// AND implemented here AND the driver is reachable) and, when not, `reason`.
async fn get_toolset_schema(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Query(q): Query<SchemaQuery>,
    headers: HeaderMap,
) -> Response {
    let toolset = q.toolset.unwrap_or(Toolset::Computer);
    let c = contract(toolset);
    let computer = match resolve_computer(&state, &user, &id, &headers).await {
        Ok(c) => c,
        Err((status, message)) => return (status, Json(json!({ "error": "computer_unavailable", "message": message }))).into_response(),
    };
    let (target_label, sandboxed, unavailable) = match build_target(&state, &user, &computer, toolset, None).await {
        Ok(t) => {
            let down = match &t {
                Target::Browser { base, .. } if !gateway_healthy(base).await => Some("The browser driver (ACU gateway) isn't running for this computer."),
                _ => None,
            };
            (t.label(), t.sandboxed(), down)
        }
        Err((_, message)) => {
            return Json(json!({
                "toolset": toolset.as_str(), "contract": c.id, "anthropic_type": c.upstream.anthropic_type,
                "computer_id": computer.id, "target": null, "available": false, "message": message,
                "members": c.members.iter().map(|m| json!({ "name": m.name, "enabled": false, "reason": message })).collect::<Vec<_>>(),
            }))
            .into_response();
        }
    };
    // The structured v2 members answer through the Allternit Driver sidecar;
    // without it they would fail per call, so the schema says so up front.
    let structured_down = match (toolset, target_label) {
        (Toolset::Computer, "this_device") if crate::this_device_input::DriverEndpoint::resolve().is_none() => {
            Some("The Allternit Driver sidecar isn't running on this computer. Open Allternit Desktop (or update it); until then the structured members (read_ui, act, run_batch, verify) are unavailable.")
        }
        _ => None,
    };
    let members: Vec<Value> = c
        .members
        .iter()
        .map(|m| {
            let opted_in = q.enable.as_deref().is_some_and(|e| e.split(',').any(|n| n.trim() == m.name));
            let reason = if !m.default_enabled && !opted_in {
                Some("Off by default in the contract; request it with ?enable= and send it with \"enable\".")
            } else if crate::computer_v2::is_v2_member(&m.name) {
                unavailable.or(structured_down).or_else(|| unsupported_reason(target_label, toolset, &m.name))
            } else {
                unavailable.or_else(|| unsupported_reason(target_label, toolset, &m.name))
            };
            json!({
                "name": m.name,
                "enabled": reason.is_none(),
                "reason": reason,
                "default_enabled": m.default_enabled,
                "risk": m.risk,
                "needs_approval": needs_approval(m, sandboxed),
                "description": m.description,
                "input_schema": m.input_schema,
            })
        })
        .collect();
    let screen = cached_screen(&computer.id).map(|s| {
        let f = default_frame(s, &c.model_frame);
        json!({ "width": s.width, "height": s.height, "frame_width": f.width, "frame_height": f.height })
    });
    Json(json!({
        "toolset": toolset.as_str(),
        "contract": c.id,
        "anthropic_type": c.upstream.anthropic_type,
        "computer_id": computer.id,
        "target": target_label,
        "sandboxed": sandboxed,
        "available": unavailable.is_none(),
        "batch_halt_text": c.batch_halt_text,
        "model_frame": { "max_long_edge": c.model_frame.max_long_edge, "max_pixels": c.model_frame.max_pixels },
        "screen": screen,
        "members": members,
        "credential_backends": crate::computer_v2::credential_backends().into_iter().map(|(name, ok)| json!({ "name": name, "available": ok })).collect::<Vec<_>>(),
        "human_window": crate::computer_v2::human_gate_pending(&computer.id).map(|(reason, secs)| json!({ "open": true, "reason": reason, "for_secs": secs })),
    }))
    .into_response()
}

/// Server-sent `computer.action` events for one computer (any target,
/// including this device, whose events websocket has no guest collector).
async fn get_toolset_events(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    use axum::response::sse::{Event, KeepAlive, Sse};
    let computer = match resolve_computer(&state, &user, &id, &headers).await {
        Ok(c) => c,
        Err((status, message)) => return (status, Json(json!({ "error": "computer_unavailable", "message": message }))).into_response(),
    };
    let rx = ACTION_EVENTS.subscribe();
    let stream = futures::stream::unfold((rx, computer.id), |(mut rx, cid)| async move {
        loop {
            match rx.recv().await {
                Ok((id, event)) if id == cid => {
                    let ev = Event::default().event("computer.action").data(event.to_string());
                    return Some((Ok::<_, std::convert::Infallible>(ev), (rx, cid)));
                }
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule() -> ModelFrameRule {
        contract(Toolset::Computer).model_frame.clone()
    }

    #[test]
    fn contracts_load_with_anthropic_member_sets() {
        assert_eq!(contract(Toolset::Computer).id, "allternit.computer.v2");
        assert_eq!(contract(Toolset::Computer).members.len(), 27);
        assert_eq!(contract(Toolset::Browser).members.len(), 31);
        assert!(contract(Toolset::Computer).member("read_ui").is_some());
        assert!(contract(Toolset::Computer).member("use_credential").is_some());
        let sub = contract(Toolset::Computer).member("run_subtask").expect("run_subtask");
        assert!(needs_approval(sub, false) && !needs_approval(sub, true), "one approval per subtask on this device, none in a sandbox");
        let skill = contract(Toolset::Computer).member("run_skill").expect("run_skill");
        assert!(needs_approval(skill, false) && !needs_approval(skill, true), "a skill run is approved like a subtask");
        assert!(!needs_approval(contract(Toolset::Computer).member("skills").expect("skills"), false));
        assert_eq!(contract(Toolset::Computer).batch_halt_text, "Not executed: an earlier computer action in this turn failed.");
        assert_eq!(contract(Toolset::Browser).batch_halt_text, "Not executed: an earlier action in this turn failed.");
    }

    #[test]
    fn frame_fits_limits_and_scaling_round_trips() {
        let screen = Frame { width: 2560, height: 1440 };
        let f = default_frame(screen, &rule());
        assert!(f.width <= 1568 && (f.width as u64 * f.height as u64) <= 1_150_000, "{f:?}");
        let small = Frame { width: 1024, height: 768 };
        assert_eq!(default_frame(small, &rule()), small, "never upscales");

        let map = Mapping::new(screen, None, CoordinateSpace::Pixels, &rule());
        for (x, y) in [(0.0, 0.0), (100.0, 200.0), (f.width as f64 - 1.0, f.height as f64 - 1.0)] {
            let (sx, sy) = map.to_screen(x, y);
            let (mx, my) = map.to_model(sx as f64, sy as f64);
            assert!((mx - x as i64).abs() <= 1 && (my - y as i64).abs() <= 1, "({x},{y}) -> ({sx},{sy}) -> ({mx},{my})");
        }
        // Out-of-frame clicks clamp onto the screen.
        assert_eq!(map.to_screen(99_999.0, -5.0), (2559, 0));

        let grid = Mapping::new(screen, None, CoordinateSpace::Normalized1000, &rule());
        assert_eq!(grid.to_screen(500.0, 500.0), (1280, 720));

        let spec = contract(Toolset::Computer).member("left_click_drag").unwrap();
        let scaled = scale_input(spec, &json!({ "start_coordinate": [10, 10], "coordinate": [f.width / 2, f.height / 2] }), &map);
        // Half the frame lands on half the screen (within a pixel of rounding).
        let c = scaled["coordinate"].as_array().unwrap();
        assert!((c[0].as_i64().unwrap() - 1280).abs() <= 2 && (c[1].as_i64().unwrap() - 720).abs() <= 2, "{c:?}");
        let b = contract(Toolset::Browser).member("left_click").unwrap();
        let scaled = scale_input(b, &json!({ "target": { "type": "coordinate", "x": f.width / 2, "y": 0 } }), &map);
        assert!((scaled["target"]["x"].as_i64().unwrap() - 1280).abs() <= 2);
        let r = scale_input(b, &json!({ "target": { "type": "ref", "ref": "ref_1" } }), &map);
        assert_eq!(r["target"]["ref"], json!("ref_1"));
    }

    #[test]
    fn validation_follows_the_member_schema() {
        let c = contract(Toolset::Computer);
        let click = &c.member("left_click").unwrap().input_schema;
        assert!(validate(click, &json!({}), "").is_ok());
        assert!(validate(click, &json!({ "coordinate": [1, 2], "text": null }), "").is_ok());
        assert!(validate(click, &json!({ "coordinate": [1] }), "").is_err());
        assert!(validate(click, &json!({ "bogus": 1 }), "").is_err());
        let scroll = &c.member("scroll").unwrap().input_schema;
        assert!(validate(scroll, &json!({ "scroll_amount": 3, "scroll_direction": "sideways" }), "").is_err());
        let b = &contract(Toolset::Browser).member("left_click").unwrap().input_schema;
        assert!(validate(b, &json!({ "target": { "type": "ref", "ref": "ref_2" } }), "").is_ok());
        assert!(validate(b, &json!({ "target": { "type": "coordinate", "x": 1 } }), "").is_err());
    }

    #[test]
    fn batch_halts_after_failure_or_unresolved_approval() {
        let b = Batches::new();
        b.record("t", 0, true);
        assert!(!b.halted("t", 1));
        b.record("t", 1, false); // failed, or 409 waiting for approval
        assert!(b.halted("t", 2));
        assert!(!b.halted("t", 1), "the call itself may retry with a grant");
        b.record("t", 1, true); // the retry with a grant succeeded
        assert!(!b.halted("t", 2));
        assert!(!b.halted("other-turn", 5));
    }

    fn lease_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE computer_control_leases (computer_id TEXT PRIMARY KEY, holder_kind TEXT NOT NULL, holder_id TEXT NOT NULL,
               holder_label TEXT, device_id TEXT, acquired_at TEXT NOT NULL, expires_at TEXT NOT NULL);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn lease_gate_refuses_when_someone_else_controls() {
        let conn = lease_db();
        let now = chrono::Utc::now().timestamp();
        let agent = Holder { kind: HolderKind::Agent, id: "run-1".into(), label: None, device_id: None };
        let person = Holder { kind: HolderKind::User, id: "u1".into(), label: Some("Eoj".into()), device_id: None };
        // Cloud computer: free -> the agent takes it; a person takes over -> 423.
        assert_eq!(lease_gate(&conn, "c1", false, "u1", &agent, now), Ok(()));
        lease::take(&conn, "c1", &person, now).unwrap();
        assert_eq!(lease_gate(&conn, "c1", false, "u1", &agent, now), Err(LeaseRefusal::Held("Eoj".into())));
        // This device: nobody in control -> take control first; owner in control -> ok.
        assert_eq!(lease_gate(&conn, "me", true, "u1", &agent, now), Err(LeaseRefusal::TakeControlFirst));
        lease::take(&conn, "me", &person, now).unwrap();
        assert_eq!(lease_gate(&conn, "me", true, "u1", &agent, now), Ok(()));
        assert!(matches!(lease_gate(&conn, "me", true, "someone-else", &agent, now), Err(LeaseRefusal::Held(_))));
    }

    #[test]
    fn approval_409_then_grant_then_single_use() {
        use crate::aci_safety::{enforce_confirmation_with_mode, SafetyMode};
        let store = crate::permission_policy::ApprovalStore::default();
        let req = ToolsetRequest {
            toolset: Toolset::Computer,
            member: "type".into(),
            input: json!({ "text": "hello" }),
            run_id: None,
            turn_id: None,
            call_index: None,
            model_frame: None,
            coordinate_space: None,
            approval_grant: None,
            browser_session_id: None,
            enable: vec![],
            within_subtask: None,
            project_id: None,
        };
        let spec = contract(Toolset::Computer).member("type").unwrap();
        assert!(needs_approval(spec, false), "type needs approval on this device");
        assert!(!needs_approval(spec, true), "but not in a sandboxed cloud computer");
        let desc = action_descriptor("me", &req);
        let denial = enforce_confirmation_with_mode(SafetyMode::Enforce, &store, "u1", "computer.toolset", ConfirmationClass::Risky, &desc, None).unwrap_err();
        let id = denial.body["approval_id"].as_str().unwrap().to_string();
        // Not approved yet -> denied.
        assert!(enforce_confirmation_with_mode(SafetyMode::Enforce, &store, "u1", "computer.toolset", ConfirmationClass::Risky, &desc, Some(&id)).is_err());
        let fresh = enforce_confirmation_with_mode(SafetyMode::Enforce, &store, "u1", "computer.toolset", ConfirmationClass::Risky, &desc, None).unwrap_err();
        let id = fresh.body["approval_id"].as_str().unwrap().to_string();
        assert!(crate::aci_approvals::GRANTS.approve(&id));
        // A grant for another action is refused; the right one redeems once.
        let other = action_descriptor("me", &ToolsetRequest { input: json!({ "text": "rm -rf" }), ..req.clone() });
        assert!(enforce_confirmation_with_mode(SafetyMode::Enforce, &store, "u1", "computer.toolset", ConfirmationClass::Risky, &other, Some(&id)).is_err());
        assert!(enforce_confirmation_with_mode(SafetyMode::Enforce, &store, "u1", "computer.toolset", ConfirmationClass::Risky, &desc, Some(&id)).is_ok());
        assert!(enforce_confirmation_with_mode(SafetyMode::Enforce, &store, "u1", "computer.toolset", ConfirmationClass::Risky, &desc, Some(&id)).is_err(), "single use");
    }

    #[test]
    fn xdotool_scripts_quote_and_map_buttons() {
        assert_eq!(xdotool_script("left_click", &json!({ "coordinate": [5, 6] })).unwrap().unwrap(), "xdotool mousemove --sync 5 6 click 1");
        assert_eq!(xdotool_script("triple_click", &json!({ "coordinate": [1, 2], "text": "shift" })).unwrap().unwrap(), "xdotool mousemove --sync 1 2 keydown shift click --repeat 3 --delay 80 1 keyup shift");
        assert_eq!(xdotool_script("type", &json!({ "text": "it's" })).unwrap().unwrap(), "xdotool type --delay 12 -- 'it'\\''s'");
        assert!(xdotool_script("key", &json!({ "text": "ctrl+a; rm -rf /" })).is_err());
    }

    #[tokio::test]
    async fn browser_url_policy_and_windows_input() {
        // Schemes: only http(s) and about:blank, on every target.
        for bad in ["file:///etc/passwd", "javascript:alert(1)", "chrome://settings", "data:text/html,x"] {
            assert!(browser_url_refusal(bad, false).await.is_some(), "{bad}");
        }
        assert!(browser_url_refusal("about:blank", true).await.is_none());
        assert!(browser_url_refusal("http://localhost:3000", false).await.is_none());
        // Cloud targets: no loopback, private, mesh (CGNAT), metadata or v6 local hosts.
        for bad in [
            "http://127.0.0.1/", "http://localhost/", "http://10.1.2.3/", "http://192.168.1.1/", "http://100.64.0.3/",
            "http://169.254.169.254/latest/meta-data", "http://metadata.google.internal/", "http://[::1]/", "http://[fd00::1]/",
            "http://[::ffff:127.0.0.1]/", "http://0.0.0.0/",
        ] {
            assert!(browser_url_refusal(bad, true).await.is_some(), "{bad}");
        }
        assert!(browser_url_refusal("https://1.1.1.1/", true).await.is_none());
        // Windows: modifier-held click and chord mapping; bad keys refused.
        let click = windows_script("left_click", &json!({ "coordinate": [5, 6], "text": "ctrl+shift" })).unwrap().unwrap();
        let tail = click.split("'@").nth(1).unwrap();
        assert_eq!(
            tail.trim(),
            "[AltInput]::K(17,$false)\n[AltInput]::K(16,$false)\n[AltInput]::SetCursorPos(5,6) | Out-Null; Start-Sleep -Milliseconds 40\n[AltInput]::M(2,0); [AltInput]::M(4,0); Start-Sleep -Milliseconds 70\n[AltInput]::K(16,$true)\n[AltInput]::K(17,$true)"
        );
        assert!(windows_script("key", &json!({ "text": "ctrl+a; rm" })).is_err());
        assert_eq!(win_chord("cmd+F5").unwrap(), vec![0x5B, 0x74]);
        assert!(windows_script("type", &json!({ "text": "a'b" })).unwrap().unwrap().contains(&B64.encode("a'b")));
        assert_eq!(cua_modifiers("ctrl+Option+cmd").unwrap(), vec!["ctrl", "option", "cmd"]);
    }
}
