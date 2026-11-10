//! Phone run history: a bounded, redacted record of every phone tool call a
//! bot (or a replayed skill, or `phone.task`) made, with a downscaled frame
//! when the call returned a screenshot. Same shape as `computer_history`:
//! 500 steps / 30 days / 100 MB of frames per phone, files under the data
//! dir keyed by a hash of the serial (never the serial as a path).
//!
//! Allternit Desktop posts one step per call (fire-and-forget) from its phone
//! tool runner; nothing here talks to the phone.
//!
//! * `POST /api/v1/phones/:serial/history` — ingest one step.
//! * `GET  /api/v1/phones/:serial/history[?run_id=]` — steps + runs.
//! * `GET  /api/v1/phones/:serial/history/:step/snapshot` — the frame (PNG).
//! * `GET  /api/v1/phones/history` — every phone with history, newest first.
//!
//! Redaction: typed text, SMS bodies and numbers never reach disk (Desktop
//! strips them and this module strips them again), and screen text is kept
//! only as the element outline the bot already read (labels, ids, bounds).
use crate::{auth::AuthUser, AppState};
use axum::{
    extract::{DefaultBodyLimit, Extension, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{
    path::{Path as FsPath, PathBuf},
    sync::Arc,
};

pub const MAX_STEPS: usize = 500;
pub const RETENTION_DAYS: i64 = 30;
const MAX_FRAME_BYTES: u64 = 100 * 1024 * 1024;
/// Screen outline kept per step (the read_ui text form), in bytes.
const MAX_UI_BYTES: usize = 24 * 1024;
const MAX_ERROR: usize = 400;
/// Tool arguments whose values are never stored.
const REDACT_KEYS: [&str; 5] = ["text", "to", "number", "body", "message"];

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Step {
    pub id: String,
    pub run_id: String,
    pub serial: String,
    pub at: String,
    /// The phone tool (`phone.tap`, …).
    pub tool: String,
    pub label: String,
    /// Redacted arguments.
    #[serde(default)]
    pub args: Value,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub duration_ms: u64,
    /// `read_ui` text form of the screen the bot last read before (or, for
    /// `phone.read_ui`, in) this step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui: Option<String>,
    /// The element a tap landed on, from the last read: `{id, label, class,
    /// rect: [x,y,w,h], screen: [w,h]}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<Value>,
    /// Who ran it: `bot` | `skill` | `task`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,
    #[serde(default)]
    pub snapshot: bool,
}

/// What Desktop posts.
#[derive(Debug, Default, Deserialize)]
pub struct StepIn {
    #[serde(default)]
    pub run_id: Option<String>,
    pub tool: String,
    #[serde(default)]
    pub args: Value,
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub ui: Option<String>,
    #[serde(default)]
    pub target: Option<Value>,
    #[serde(default)]
    pub source: Option<String>,
    /// Base64 PNG or JPEG (no data: prefix needed; one is stripped).
    #[serde(default)]
    pub image_base64: Option<String>,
    #[serde(default)]
    pub at: Option<String>,
}

pub fn root(data_dir: &FsPath) -> PathBuf {
    data_dir.join("phone-history")
}

pub fn directory(data_dir: &FsPath, serial: &str) -> PathBuf {
    root(data_dir).join(hex::encode(Sha256::digest(serial.as_bytes())))
}

fn valid_step(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok()
}

fn clip(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let mut end = n;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// Strip every value that could be typed content or a contact: `text`, `to`,
/// `number`… become `{redacted, len}`. Placeholders (`{{name}}`) stay, so a
/// replayed skill's steps still say which input they typed.
pub fn redact_args(tool: &str, args: &Value) -> Value {
    let Some(obj) = args.as_object() else { return json!({}) };
    let mut out = Map::new();
    for (k, v) in obj {
        if k.starts_with('_') {
            continue;
        }
        let sensitive = REDACT_KEYS.contains(&k.as_str()) || (tool.contains("sms") && k != "serial");
        match v {
            Value::String(s) if sensitive && !(s.starts_with("{{") && s.ends_with("}}")) => {
                out.insert(k.clone(), json!({ "redacted": true, "len": s.chars().count() }));
            }
            Value::String(s) => {
                out.insert(k.clone(), Value::String(clip(s, 200)));
            }
            Value::Number(_) | Value::Bool(_) | Value::Null => {
                out.insert(k.clone(), v.clone());
            }
            // Nested filters ({label, type}) are short screen words, kept clipped.
            Value::Object(_) if k == "filter" => {
                out.insert(k.clone(), json!(clip(&v.to_string(), 200)));
            }
            _ if sensitive => {
                out.insert(k.clone(), json!({ "redacted": true }));
            }
            _ => {}
        }
    }
    Value::Object(out)
}

fn label_of(tool: &str, args: &Value) -> String {
    let base = tool.strip_prefix("phone.").unwrap_or(tool).replace(['_', '.'], " ");
    let detail = match tool {
        "phone.open_app" => args.get("app").and_then(Value::as_str).map(str::to_string),
        "phone.key" => args.get("combo").and_then(Value::as_str).map(str::to_string),
        "phone.swipe" => args.get("direction").and_then(Value::as_str).map(str::to_string),
        "phone.find_and_tap" => args.get("match").or_else(|| args.get("element_id")).and_then(Value::as_str).map(str::to_string),
        "phone.task" => args.get("goal").or_else(|| args.get("task")).and_then(Value::as_str).map(|s| clip(s, 80)),
        _ => None,
    };
    match detail {
        Some(d) if !d.is_empty() => format!("{base} {d}"),
        _ => base,
    }
}

fn read_steps(dir: &FsPath) -> Vec<Step> {
    let mut steps: Vec<Step> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
        .filter_map(|e| serde_json::from_slice::<Step>(&std::fs::read(e.path()).ok()?).ok())
        .collect();
    steps.retain(|step| valid_step(&step.id));
    steps.sort_by(|a, b| a.at.cmp(&b.at).then(a.id.cmp(&b.id)));
    steps
}

fn png_at_size(img: &[u8], edge: u32) -> Result<Vec<u8>, String> {
    let image = image::load_from_memory(img).map_err(|e| e.to_string())?.thumbnail(edge, edge);
    let mut out = std::io::Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

fn private_dir(dir: &FsPath) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn save(data_dir: &FsPath, mut step: Step, image: Option<Vec<u8>>) -> Result<Step, String> {
    let dir = directory(data_dir, &step.serial);
    private_dir(&dir)?;
    if let Some(image) = image {
        // A frame that can't be decoded still records the step.
        if let Ok(full) = png_at_size(&image, 1280) {
            if full.len() <= 3 * 1024 * 1024 {
                std::fs::write(dir.join(format!("{}.png", step.id)), full).map_err(|e| e.to_string())?;
                if let Ok(thumb) = png_at_size(&image, 192) {
                    step.thumbnail = Some(format!("data:image/png;base64,{}", B64.encode(thumb)));
                }
                step.snapshot = true;
            }
        }
    }
    std::fs::write(dir.join(format!("{}.json", step.id)), serde_json::to_vec(&step).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    prune(&dir)?;
    Ok(step)
}

pub fn prune(dir: &FsPath) -> Result<Vec<Step>, String> {
    let steps = read_steps(dir);
    let cutoff = (chrono::Utc::now() - chrono::Duration::days(RETENTION_DAYS)).to_rfc3339();
    let extra = steps.len().saturating_sub(MAX_STEPS);
    let mut kept = Vec::new();
    for (index, step) in steps.into_iter().enumerate() {
        if index < extra || step.at < cutoff {
            let _ = std::fs::remove_file(dir.join(format!("{}.json", step.id)));
            let _ = std::fs::remove_file(dir.join(format!("{}.png", step.id)));
        } else {
            kept.push(step);
        }
    }
    let mut total: u64 = kept
        .iter()
        .filter_map(|s| std::fs::metadata(dir.join(format!("{}.png", s.id))).ok())
        .map(|m| m.len())
        .sum();
    for step in &mut kept {
        if total <= MAX_FRAME_BYTES {
            break;
        }
        let path = dir.join(format!("{}.png", step.id));
        if let Ok(meta) = std::fs::metadata(&path) {
            std::fs::remove_file(path).map_err(|e| e.to_string())?;
            total = total.saturating_sub(meta.len());
            step.snapshot = false;
            std::fs::write(dir.join(format!("{}.json", step.id)), serde_json::to_vec(step).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(kept)
}

/// The steps of one phone (pruned), optionally one run.
pub fn steps(data_dir: &FsPath, serial: &str, run_id: Option<&str>) -> Vec<Step> {
    let mut steps = prune(&directory(data_dir, serial)).unwrap_or_default();
    if let Some(run) = run_id {
        steps.retain(|s| s.run_id == run);
    }
    steps
}

/// The PNG frame of a step, when kept.
pub fn frame(data_dir: &FsPath, serial: &str, step: &str) -> Option<Vec<u8>> {
    if !valid_step(step) {
        return None;
    }
    let dir = directory(data_dir, serial);
    let meta: Step = serde_json::from_slice(&std::fs::read(dir.join(format!("{step}.json"))).ok()?).ok()?;
    let cutoff = (chrono::Utc::now() - chrono::Duration::days(RETENTION_DAYS)).to_rfc3339();
    if meta.at < cutoff || !meta.snapshot {
        return None;
    }
    std::fs::read(dir.join(format!("{step}.png"))).ok()
}

/// Runs, newest first: `{run_id, serial, started_at, ended_at, steps, failed, source, thumbnail}`,
/// plus `status` (`running` | `waiting_approval`, with `approvalId` and `effect`
/// while a hold waits) for a task or replay still in progress.
pub fn runs(steps: &[Step]) -> Vec<Value> {
    let mut order: Vec<String> = Vec::new();
    let mut by: std::collections::HashMap<String, Vec<&Step>> = std::collections::HashMap::new();
    for s in steps {
        if !by.contains_key(&s.run_id) {
            order.push(s.run_id.clone());
        }
        by.entry(s.run_id.clone()).or_default().push(s);
    }
    let mut out: Vec<Value> = order
        .iter()
        .map(|id| {
            let list = &by[id];
            let mut run = json!({
                "run_id": id,
                "serial": list[0].serial,
                "started_at": list[0].at,
                "ended_at": list[list.len() - 1].at,
                "steps": list.len(),
                "failed": list.iter().filter(|s| !s.ok).count(),
                "source": list.iter().find_map(|s| s.source.clone()),
                "thumbnail": list.iter().rev().find_map(|s| s.thumbnail.clone()),
            });
            if let Some(live) = live_of(id) {
                for k in ["status", "approvalId", "effect"] {
                    if let Some(v) = live.get(k).filter(|v| !v.is_null()) {
                        run[k] = v.clone();
                    }
                }
            }
            run
        })
        .collect();
    out.reverse();
    out
}

/// Record one step (from Desktop, or in-process from a replay or task).
pub fn ingest(data_dir: &FsPath, serial: &str, body: StepIn) -> Result<Step, String> {
    let serial = serial.trim();
    if serial.is_empty() || serial.len() > 200 {
        return Err("serial is required".into());
    }
    let tool = clip(body.tool.trim(), 80);
    if !tool.starts_with("phone.") {
        return Err("tool must be a phone tool".into());
    }
    let args = redact_args(&tool, &body.args);
    let image = body.image_base64.as_deref().and_then(|b| {
        let raw = b.split_once("base64,").map(|(_, d)| d).unwrap_or(b);
        B64.decode(raw.trim()).ok()
    });
    let run_id = body
        .run_id
        .map(|r| clip(r.trim(), 120))
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| format!("run-{}", chrono::Utc::now().format("%Y%m%d%H%M")));
    let at = body
        .at
        .filter(|a| chrono::DateTime::parse_from_rfc3339(a).is_ok())
        .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
    let step = Step {
        id: uuid::Uuid::new_v4().to_string(),
        run_id,
        serial: serial.to_string(),
        at,
        label: label_of(&tool, &args),
        tool,
        args,
        ok: body.ok,
        error: body.error.map(|e| clip(&e, MAX_ERROR)),
        duration_ms: body.duration_ms,
        ui: body.ui.map(|u| clip(&u, MAX_UI_BYTES)),
        target: body.target.filter(Value::is_object),
        source: body.source.map(|s| clip(&s, 20)),
        thumbnail: None,
        snapshot: false,
    };
    save(data_dir, step, image)
}

/// Black out personal data (emails, cards, phone numbers, SSNs, secrets) in a
/// phone frame before it is kept, the same OCR pass computers use
/// (`computer_safety::pii_boxes`). Phone frames are the person's own screen,
/// so when OCR can't run the frame is dropped and only the step is kept.
async fn redact_frame(image_base64: Option<String>, data_dir: &FsPath) -> Option<String> {
    let b = image_base64?;
    let raw = b.split_once("base64,").map(|(_, d)| d).unwrap_or(&b);
    let png = B64.decode(raw.trim()).ok()?;
    if std::env::var("ALLTERNIT_PHONE_HISTORY_REDACT").ok().as_deref() == Some("off") {
        return Some(b);
    }
    let ocr = match crate::computer_ocr::ocr_png(&png, data_dir).await {
        Ok(ocr) => ocr,
        Err(e) => {
            tracing::debug!("phone frame not kept: redaction OCR unavailable ({e})");
            return None;
        }
    };
    let kinds = ["email", "card", "phone", "ssn", "secret"];
    let (boxes, _) = crate::computer_safety::pii_boxes(&ocr, &kinds, &[]);
    if boxes.is_empty() {
        return Some(b);
    }
    crate::computer_safety::black_out(&png, &boxes).ok().map(|out| B64.encode(out))
}

/// A run's live status from the phone guard (`waiting_approval` while a hold waits).
pub fn live_of(run_id: &str) -> Option<Value> {
    crate::phone_guard::live_status(run_id)
}

// ─── Events (phone.step, phone.run.completed) ─────────────────────────────

/// The owner ledger row of a recorded step: what `phone.step` webhooks carry.
/// Never the screen, the frame or redacted values (`args` stays local).
pub fn step_event(step: &Step) -> Value {
    json!({
        "step_id": step.id,
        "run_id": step.run_id,
        "serial": step.serial,
        "tool": step.tool,
        "label": step.label,
        "ok": step.ok,
        "error": step.error,
        "duration_ms": step.duration_ms,
        "source": step.source,
        "snapshot": step.snapshot,
    })
}

// ─── The phone_task card (a task started on a thread) ─────────────────────

/// Post (or update) the `phone_task` card on the task's thread. The card is a
/// ledger event on the thread's bot; each status change is a new event for the
/// same `runId`, and the newest one is the card's state. Nothing is written for
/// a task with no thread. Card shape:
/// `{"type":"phone_task","runId","serial","goal","status","step","inboxId?"}`.
pub fn post_task_card(db: &crate::db::DbHandle, ctx: &crate::phone_guard::TaskCtx, run_id: &str, serial: &str, goal: &str, status: &str, step: u32, inbox_id: Option<&str>) -> bool {
    let Some(thread) = ctx.thread_id.as_deref().map(str::trim).filter(|t| !t.is_empty()) else { return false };
    let Ok(conn) = db.connect() else { return false };
    let bot = ctx
        .bot_id
        .clone()
        .filter(|b| !b.trim().is_empty())
        .or_else(|| conn.query_row("SELECT bot_id FROM bot_threads WHERE id = ?1", rusqlite::params![thread], |r| r.get::<_, String>(0)).ok());
    drop(conn);
    let Some(bot) = bot else { return false };
    let mut card = json!({
        "type": "phone_task",
        "runId": run_id,
        "serial": serial,
        "goal": goal.chars().take(300).collect::<String>(),
        "status": status,
        "step": step,
    });
    if let Some(i) = inbox_id {
        card["inboxId"] = json!(i);
    }
    let key = format!("phone-task:{run_id}:{status}:{step}:{}", inbox_id.unwrap_or(""));
    crate::gateway_runner::led(db, &bot, thread, ctx.session_id.as_deref(), "phone_task", ("system", "phone"), card, Some(key))
}

// ─── The run's latest screen (the status card's thumbnail) ────────────────

/// True when `user_id` owns the run: a hold or a `phone_task` card for it sits on one of their
/// bots, or the caller is this runtime's paired owner. A run nobody can trace is nobody's.
fn owns_run(db: &crate::db::DbHandle, user_id: &str, run_id: &str) -> bool {
    if crate::relay_auth::process_secret().paired_owner().is_some_and(|o| o.to_string() == user_id) {
        return true;
    }
    let Ok(conn) = db.connect() else { return false };
    let held: i64 = conn
        .query_row("SELECT COUNT(*) FROM gateway_approvals WHERE owner = ?1 AND correlation_id LIKE ?2", rusqlite::params![user_id, format!("phone:{run_id}:%")], |r| r.get(0))
        .unwrap_or(0);
    if held > 0 {
        return true;
    }
    let carded: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM bot_events e JOIN agents a ON a.id = e.bot_id WHERE a.user_id = ?1 AND e.event_type = 'phone_task' AND json_extract(e.payload, '$.runId') = ?2",
            rusqlite::params![user_id, run_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    carded > 0
}

/// The newest kept frame of a run, downscaled to 360 px wide, as a JPEG. Frames are already
/// redacted (OCR) when kept; `None` when the run has none.
pub fn run_thumb(data_dir: &FsPath, run_id: &str) -> Option<Vec<u8>> {
    if run_id.is_empty() || !run_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return None;
    }
    let mut best: Option<(String, std::path::PathBuf)> = None;
    for entry in std::fs::read_dir(root(data_dir)).into_iter().flatten().filter_map(Result::ok) {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        for step in read_steps(&dir).into_iter().filter(|s| s.run_id == run_id && s.snapshot) {
            let png = dir.join(format!("{}.png", step.id));
            if png.is_file() && best.as_ref().map_or(true, |(at, _)| step.at >= *at) {
                best = Some((step.at.clone(), png));
            }
        }
    }
    let bytes = std::fs::read(best?.1).ok()?;
    let img = image::load_from_memory(&bytes).ok()?;
    let img = if img.width() > 360 { img.resize(360, u32::MAX, image::imageops::FilterType::Triangle) } else { img };
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img.to_rgb8()).write_to(&mut out, image::ImageFormat::Jpeg).ok()?;
    Some(out.into_inner())
}

/// `GET /phones/runs/:run_id/thumb`: owner session only; 404 when the run has no frame or isn't theirs.
async fn get_run_thumb(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(run_id): Path<String>) -> Response {
    if !owns_run(&state.db, &user.user_id, &run_id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || run_thumb(&dir, &run_id)).await {
        Ok(Some(jpg)) => ([("content-type", "image/jpeg"), ("cache-control", "private, no-store")], jpg).into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

/// The owner ledger row of a finished task / skill run, from its report.
pub fn run_event(report: &Value, source: &str) -> Value {
    let steps = report.get("steps").and_then(Value::as_array);
    json!({
        "run_id": report.get("run_id"),
        "serial": report.get("serial"),
        "ok": report.get("ok").and_then(Value::as_bool).unwrap_or(false),
        "status": report.get("status"),
        "skill": report.get("skill"),
        "source": source,
        "steps": steps.map(|s| s.len()).unwrap_or(0),
        "failed": steps.map(|s| s.iter().filter(|x| x.get("ok").and_then(Value::as_bool) == Some(false)).count()).unwrap_or(0),
        "model_calls": report.get("model_calls"),
        "duration_ms": report.get("duration_ms"),
        // Time spent waiting on the owner (left out of `duration_ms`) and the
        // holds the run met: `[{approvalId, effect, state}]`. Statuses add
        // `denied`, `approval_timeout` and `phone_locked`.
        "held_ms": report.get("held_ms").and_then(Value::as_u64).unwrap_or(0),
        "holds": report.get("holds").cloned().unwrap_or_else(|| json!([])),
        // The conversation the run belongs to, when it started on one.
        "thread_id": report.get("thread_id"),
        "session_id": report.get("session_id"),
        "reason": report.get("reason"),
    })
}

/// Write a phone event to the owner ledger, which `runtime_events` forwards
/// to the cloud (Platform API webhooks, MCP Events). Best effort and a no-op
/// on an unpaired runtime: phone runs never wait on, or fail for, the cloud.
/// Any type is kept: `phone.step`, `phone.run.completed`, and the guard's
/// `phone.approval.requested` / `phone.approval.resolved`.
pub fn record_event(db: &crate::db::DbHandle, event_type: &str, payload: Value, key: String) {
    let Some(owner) = crate::relay_auth::process_secret().paired_owner() else { return };
    let db = db.clone();
    let event_type = event_type.to_string();
    tokio::task::spawn_blocking(move || {
        let result = db
            .connect()
            .and_then(|conn| crate::runtime_events::record_user_event(&conn, &owner, &event_type, None, &payload, Some(&key)));
        if let Err(e) = result {
            tracing::debug!("{event_type} not recorded: {e}");
        }
    });
}

/// `phone.run.completed` for a task or skill report (one per run id).
/// `source`: `task` | `skill` | `routine`.
pub fn record_run_completed(db: &crate::db::DbHandle, report: &Value, source: &str) {
    let Some(run) = report.get("run_id").and_then(Value::as_str).filter(|r| !r.is_empty()) else { return };
    record_event(db, "phone.run.completed", run_event(report, source), format!("phone-run:{run}:completed"));
}

// ─── Routes ────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct HistoryQuery {
    run_id: Option<String>,
    /// At most this many newest steps (the whole history when absent).
    limit: Option<usize>,
}

/// The newest `limit` steps, newest first.
fn newest(mut shown: Vec<Step>, limit: Option<usize>) -> Vec<Step> {
    let Some(limit) = limit else { return shown };
    shown.sort_by(|a, b| b.at.cmp(&a.at));
    shown.truncate(limit);
    shown
}

fn bad(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({ "message": message.into() }))).into_response()
}

async fn post_history(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Path(serial): Path<String>,
    Json(mut body): Json<StepIn>,
) -> Response {
    let dir = state.data_dir.clone();
    body.image_base64 = redact_frame(body.image_base64.take(), &dir).await;
    match tokio::task::spawn_blocking(move || ingest(&dir, &serial, body)).await {
        Ok(Ok(step)) => {
            record_event(&state.db, "phone.step", step_event(&step), format!("phone-step:{}", step.id));
            Json(json!({ "ok": true, "id": step.id, "run_id": step.run_id, "snapshot": step.snapshot })).into_response()
        }
        Ok(Err(e)) => bad(StatusCode::BAD_REQUEST, e),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn get_history(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Path(serial): Path<String>,
    Query(q): Query<HistoryQuery>,
) -> Response {
    let dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || {
        let all = steps(&dir, &serial, None);
        let runs = runs(&all);
        let shown: Vec<Step> = match q.run_id.as_deref() {
            Some(r) => all.into_iter().filter(|s| s.run_id == r).collect(),
            None => all,
        };
        let shown = newest(shown, q.limit);
        json!({ "serial": serial, "steps": shown, "runs": runs, "retention_days": RETENTION_DAYS, "max_steps": MAX_STEPS })
    })
    .await
    {
        Ok(body) => Json(body).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn get_snapshot(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Path((serial, step)): Path<(String, String)>,
) -> Response {
    let dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || frame(&dir, &serial, &step)).await {
        Ok(Some(png)) => ([("content-type", "image/png"), ("cache-control", "private, no-store")], png).into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Every phone with history: `{ phones: [{serial, steps, last_at, runs}], runs: [...] }`.
pub fn all_phones(data_dir: &FsPath) -> Value {
    let mut phones = Vec::new();
    let mut all_runs = Vec::new();
    for entry in std::fs::read_dir(root(data_dir)).into_iter().flatten().filter_map(Result::ok) {
        if !entry.path().is_dir() {
            continue;
        }
        let steps = prune(&entry.path()).unwrap_or_default();
        let Some(last) = steps.last() else { continue };
        let runs = runs(&steps);
        phones.push(json!({ "serial": last.serial, "steps": steps.len(), "last_at": last.at, "runs": runs.len() }));
        all_runs.extend(runs);
    }
    phones.sort_by(|a, b| b["last_at"].as_str().cmp(&a["last_at"].as_str()));
    all_runs.sort_by(|a, b| b["ended_at"].as_str().cmp(&a["ended_at"].as_str()));
    all_runs.truncate(200);
    json!({ "phones": phones, "runs": all_runs, "retention_days": RETENTION_DAYS, "max_steps": MAX_STEPS })
}

async fn get_all(State(state): State<Arc<AppState>>, Extension(_user): Extension<AuthUser>) -> Response {
    let dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || all_phones(&dir)).await {
        Ok(body) => Json(body).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// Merged under `/api/v1` (Clerk / desktop-local auth, like the computers).
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/phones/history", get(get_all))
        .route(
            "/phones/:serial/history",
            get(get_history).post(post_history).layer(DefaultBodyLimit::max(24 * 1024 * 1024)),
        )
        .route("/phones/:serial/history/:step/snapshot", get(get_snapshot))
        .route("/phones/runs/:run_id/thumb", get(get_run_thumb))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_text_and_numbers_never_reach_disk() {
        let a = redact_args("phone.type", &json!({ "text": "hunter2", "serial": "link:x", "_run_id": "r" }));
        assert_eq!(a["text"]["redacted"], true);
        assert_eq!(a["serial"], "link:x");
        assert!(a.get("_run_id").is_none());
        let s = redact_args("phone.sms.send", &json!({ "to": "+15551234", "text": "hi" }));
        assert_eq!(s["to"]["redacted"], true);
        let p = redact_args("phone.type", &json!({ "text": "{{email}}" }));
        assert_eq!(p["text"], "{{email}}");
    }

    #[test]
    fn ingest_groups_runs_and_bounds_the_path() {
        let tmp = tempfile::tempdir().unwrap();
        for tool in ["phone.read_ui", "phone.tap"] {
            ingest(tmp.path(), "../../x", StepIn { run_id: Some("r1".into()), tool: tool.into(), ok: true, ..Default::default() }).unwrap();
        }
        let got = steps(tmp.path(), "../../x", None);
        assert_eq!(got.len(), 2);
        assert_eq!(runs(&got)[0]["steps"], 2);
        assert!(ingest(tmp.path(), "s", StepIn { tool: "rm".into(), ..Default::default() }).is_err());
        assert!(frame(tmp.path(), "s", "../x").is_none());
    }

    #[test]
    fn run_event_carries_holds_and_runs_show_a_waiting_hold() {
        let report = json!({
            "run_id": "skill-h1", "status": "approval_timeout", "held_ms": 300000,
            "holds": [{ "approvalId": "a1", "effect": "pay", "state": "expired" }], "steps": [],
        });
        let ev = run_event(&report, "routine");
        assert_eq!((ev["status"].as_str(), ev["held_ms"].as_u64()), (Some("approval_timeout"), Some(300000)));
        assert_eq!(ev["holds"][0]["state"], "expired");
        // An old report with no hold fields still makes a complete event.
        assert_eq!((run_event(&json!({ "run_id": "x" }), "task")["held_ms"].as_u64(), run_event(&json!({}), "task")["holds"].clone()), (Some(0), json!([])));

        let tmp = tempfile::tempdir().unwrap();
        let step = ingest(tmp.path(), "link:w", StepIn { run_id: Some("task-live-h1".into()), tool: "phone.read_ui".into(), args: json!({}), ok: true, ..Default::default() }).unwrap();
        crate::phone_guard::set_live("task-live-h1", Some(json!({ "status": "waiting_approval", "approvalId": "a2", "effect": "delete" })));
        let waiting = runs(std::slice::from_ref(&step));
        crate::phone_guard::set_live("task-live-h1", None);
        assert_eq!((waiting[0]["status"].as_str(), waiting[0]["approvalId"].as_str()), (Some("waiting_approval"), Some("a2")));
        assert!(runs(&[step])[0].get("status").is_none());
    }

    #[tokio::test]
    async fn the_run_thumb_is_the_newest_frame_small_and_only_the_owners() {
        let dir = std::env::temp_dir().join(format!("phone-thumb-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = crate::test_helpers::app_state(&dir).await;
        let c = st.db.connect().unwrap();
        c.execute("INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES ('bot-1','user-a','b','m','p',1,'{}')", []).unwrap();
        c.execute("INSERT INTO bot_threads (id, user_id, bot_id, title, status, last_activity_at, created_at, updated_at) VALUES ('th-1','user-a','bot-1','Plans','idle','t','t','t')", []).unwrap();
        // A real 1000x600 frame kept as a step.
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(1000, 600, image::Rgb([30, 120, 200]))).write_to(&mut png, image::ImageFormat::Png).unwrap();
        let step = Step { id: uuid::Uuid::new_v4().to_string(), run_id: "task-thumb".into(), serial: "link:1".into(), at: chrono::Utc::now().to_rfc3339(), tool: "phone.tap".into(), label: "tap".into(), ok: true, ..Default::default() };
        save(&st.data_dir, step, Some(png.into_inner())).unwrap();
        let ctx = crate::phone_guard::TaskCtx { owner: "user-a".into(), bot_id: Some("bot-1".into()), thread_id: Some("th-1".into()), source: "chat".into(), ..Default::default() };
        assert!(post_task_card(&st.db, &ctx, "task-thumb", "link:1", "g", "running", 0, None));
        let get = |user: &str, run: &str| {
            let (st, user, run) = (st.clone(), user.to_string(), run.to_string());
            async move { get_run_thumb(State(st), Extension(AuthUser { user_id: user, email: None, name: None, avatar_url: None, tenant_id: None, organization_id: None, organization_role: None, organization_slug: None }), Path(run)).await }
        };
        let ok = get("user-a", "task-thumb").await;
        assert_eq!(ok.status(), StatusCode::OK);
        assert_eq!(ok.headers()["content-type"], "image/jpeg");
        let body = axum::body::to_bytes(ok.into_body(), 10_000_000).await.unwrap();
        let img = image::load_from_memory(&body).unwrap();
        assert_eq!((img.width(), img.height()), (360, 216));
        assert_eq!(get("user-b", "task-thumb").await.status(), StatusCode::NOT_FOUND, "not their run");
        assert_eq!(get("user-a", "task-unknown").await.status(), StatusCode::NOT_FOUND);
        assert_eq!(get("user-a", "../etc").await.status(), StatusCode::NOT_FOUND);
    }
}
