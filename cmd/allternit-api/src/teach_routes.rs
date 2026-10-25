//! Teach v2: turn one recorded demonstration into an editable skill draft, and
//! keep the taught skills per user. Mounted by `har_api_router()` under
//! `/api/har-derived-api/teach/*`.
//!
//! Screenshots never reach this module: events only carry a snapshot id.
//! Everything text-like is redacted with the DLP patterns before a model sees it.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use tracing::warn;

use crate::auth::get_user;
use crate::llm_gateway::dlp_patterns::{redact_text, scan_text};
use crate::AppState;

const MAX_STEPS: usize = 40;
const MAX_INPUTS: usize = 12;
const MAX_EVENTS: usize = 600;
const MAX_FIELD: usize = 600;

pub fn teach_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/har-derived-api/teach/distill", post(distill))
        .route("/har-derived-api/teach/skills", get(list_skills))
        .route("/har-derived-api/teach/skills/:id", put(put_skill).delete(delete_skill))
        .route("/har-derived-api/teach/profile", get(get_profile).put(put_profile))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TeachStep {
    pub id: String,
    pub kind: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_refs: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TeachInput {
    pub name: String,
    pub label: String,
    pub example: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// text | number | date | datetime | email | url | choice | file | folder | list | secret
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub input_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choices: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_ref: Option<String>,
    /// UI-only: created automatically and not yet reviewed by a person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_review: Option<bool>,
}

const INPUT_TYPES: &[&str] =
    &["text", "number", "date", "datetime", "email", "url", "choice", "file", "folder", "list", "secret"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeachDraft {
    pub title: String,
    pub intent: String,
    pub steps: Vec<TeachStep>,
    pub inputs: Vec<TeachInput>,
    pub source: String,
    pub redactions: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeachSkill {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub intent: String,
    #[serde(default)]
    pub steps: Vec<TeachStep>,
    #[serde(default)]
    pub inputs: Vec<TeachInput>,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub redactions: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routine_id: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    /// Unknown fields (`workflow`, `version`, `auto_approve_unsafe`, ...) round-trip untouched.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Deserialize)]
struct DistillRequest {
    #[serde(default)]
    contract_id: Option<String>,
    #[serde(default)]
    events: Vec<Value>,
    #[serde(default)]
    narration: Option<String>,
    #[serde(default)]
    note: Option<String>,
}

/// One endpoint as the model and the fallback see it.
#[derive(Debug, Clone)]
struct EndpointLite {
    id: String,
    method: String,
    host: String,
    path_template: String,
    summary: Option<String>,
    body_template: Option<String>,
}

fn json_err(status: StatusCode, msg: &str) -> Response {
    (status, Json(json!({ "error": msg }))).into_response()
}

// ---------- redaction ----------

fn redact(text: &str, count: &mut usize) -> String {
    let matches = scan_text(text, &[]);
    if matches.is_empty() {
        return text.to_string();
    }
    *count += matches.len();
    redact_text(text, &matches)
}

fn clip(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

fn str_field(e: &Value, k: &str, count: &mut usize) -> Option<String> {
    e.get(k)
        .and_then(Value::as_str)
        .map(|s| clip(&redact(s, count), MAX_FIELD))
        .filter(|s| !s.trim().is_empty())
}

/// A redacted, compact event the prompt and the fallback both use.
#[derive(Debug, Clone, PartialEq)]
struct CleanEvent {
    t: i64,
    kind: String,
    text: String,
    url: Option<String>,
}

fn clean_events(events: &[Value], count: &mut usize) -> Vec<CleanEvent> {
    let mut out: Vec<CleanEvent> = Vec::new();
    for e in events.iter().take(MAX_EVENTS) {
        let kind = e.get("kind").and_then(Value::as_str).unwrap_or("");
        let t = e.get("t").and_then(Value::as_i64).unwrap_or(0);
        let url = str_field(e, "url", count);
        let text = match kind {
            "navigate" => str_field(e, "title", count)
                .map(|ti| format!("{} ({})", ti, url.clone().unwrap_or_default()))
                .or_else(|| url.clone())
                .unwrap_or_default(),
            "click" => str_field(e, "label", count).unwrap_or_default(),
            "input" => {
                let label = str_field(e, "label", count).unwrap_or_default();
                match str_field(e, "valuePreview", count) {
                    Some(v) => format!("{} = {}", label, v),
                    None => label,
                }
            }
            "app" => {
                let app = str_field(e, "app", count).unwrap_or_default();
                match str_field(e, "window", count) {
                    Some(w) => format!("{} - {}", app, w),
                    None => app,
                }
            }
            "narration" => str_field(e, "text", count).unwrap_or_default(),
            "key" => {
                let key = str_field(e, "key", count).unwrap_or_default();
                match str_field(e, "label", count) {
                    Some(l) => format!("{} in {}", key, l),
                    None => key,
                }
            }
            "select" => {
                let label = str_field(e, "label", count).unwrap_or_default();
                match str_field(e, "value", count) {
                    Some(v) => format!("{} = {}", label, v),
                    None => label,
                }
            }
            "upload" => {
                let label = str_field(e, "label", count).unwrap_or_default();
                match str_field(e, "fileName", count) {
                    Some(f) => format!("{} <- file {}", label, f),
                    None => label,
                }
            }
            "download" => str_field(e, "fileName", count).unwrap_or_default(),
            "copy" => str_field(e, "textPreview", count).unwrap_or_default(),
            "paste" => str_field(e, "label", count).unwrap_or_default(),
            // snapshots stay on the device; only the id exists
            _ => continue,
        };
        if text.trim().is_empty() {
            continue;
        }
        let ev = CleanEvent { t, kind: kind.to_string(), text, url };
        if out.last().map(|l| l.kind == ev.kind && l.text == ev.text).unwrap_or(false) {
            continue;
        }
        out.push(ev);
    }
    out
}

// ---------- rule-based fallback ----------

fn domain_of(endpoints: &[EndpointLite], events: &[CleanEvent]) -> Option<String> {
    if let Some(e) = endpoints.first() {
        if !e.host.is_empty() {
            return Some(e.host.clone());
        }
    }
    events.iter().filter_map(|e| e.url.as_deref()).find_map(|u| {
        let rest = u.split("://").nth(1)?;
        let host = rest.split(['/', '?', '#']).next()?;
        (!host.is_empty()).then(|| host.to_string())
    })
}

fn rules_draft(endpoints: &[EndpointLite], events: &[CleanEvent], note: &str, redactions: usize) -> TeachDraft {
    let domain = domain_of(endpoints, events);
    let mut steps: Vec<TeachStep> = Vec::new();
    let mut push = |kind: &str, text: String, endpoint_id: Option<String>, target: Option<String>| {
        if steps.len() < MAX_STEPS {
            let id = format!("s{}", steps.len() + 1);
            steps.push(TeachStep { id, kind: kind.into(), text, endpoint_id, target, input_refs: None });
        }
    };
    if !endpoints.is_empty() {
        for ep in endpoints {
            let text = format!("{} {}{}", ep.method.to_uppercase(), ep.host, ep.path_template);
            push("api", text, Some(ep.id.clone()), None);
        }
    } else {
        for ev in events {
            match ev.kind.as_str() {
                "navigate" => push("ui", format!("Open {}", ev.text), None, ev.url.clone()),
                "click" => push("ui", format!("Click {}", ev.text), None, ev.url.clone()),
                "input" => push("ui", format!("Fill in {}", ev.text), None, ev.url.clone()),
                "select" => push("ui", format!("Choose {}", ev.text), None, ev.url.clone()),
                "key" => push("ui", format!("Press {}", ev.text), None, ev.url.clone()),
                "upload" => push("ui", format!("Upload {}", ev.text), None, ev.url.clone()),
                "download" => push("ui", format!("Download {}", ev.text), None, ev.url.clone()),
                "copy" => push("ui", format!("Copy {}", ev.text), None, ev.url.clone()),
                "paste" => push("ui", format!("Paste into {}", ev.text), None, ev.url.clone()),
                "app" => push("ui", format!("Switch to {}", ev.text), None, None),
                _ => push("note", ev.text.clone(), None, None),
            }
        }
    }
    let intent = if !note.trim().is_empty() {
        clip(note.trim(), 200)
    } else if let Some(d) = &domain {
        format!("Repeat the task taught on {}", d)
    } else {
        "Repeat the task that was taught".to_string()
    };
    let title = match &domain {
        Some(d) => format!("Task on {}", d),
        None => "Taught task".to_string(),
    };
    let inputs = rules_inputs(events);
    TeachDraft { title, intent, steps, inputs, source: "rules".into(), redactions }
}

/// Typed inputs the rules fallback can guess from `label = value` events.
fn rules_inputs(events: &[CleanEvent]) -> Vec<TeachInput> {
    let mut out: Vec<TeachInput> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for ev in events {
        if out.len() >= MAX_INPUTS {
            break;
        }
        let (label, example) = match ev.kind.as_str() {
            "input" | "select" => match ev.text.split_once(" = ") {
                Some((l, v)) => (l.trim().to_string(), v.trim().to_string()),
                None => continue,
            },
            "upload" => match ev.text.split_once(" <- file ") {
                Some((l, v)) => (l.trim().to_string(), v.trim().to_string()),
                None => continue,
            },
            _ => continue,
        };
        let name = sanitize_name(&label);
        if name.is_empty() || example.is_empty() || !seen.insert(name.clone()) {
            continue;
        }
        let (input_type, format) = if ev.kind == "upload" {
            ("file".to_string(), None)
        } else {
            guess_type(&example)
        };
        out.push(TeachInput {
            name,
            label,
            example,
            description: None,
            input_type: Some(input_type),
            required: None,
            format,
            choices: None,
            root: None,
            secret_ref: None,
            needs_review: None,
        });
    }
    out
}

/// Guess an input type (and a date format) from an example value.
fn guess_type(v: &str) -> (String, Option<String>) {
    let v = v.trim();
    if let Some(f) = date_format(v) {
        return ("date".into(), Some(f.into()));
    }
    if v.starts_with("http://") || v.starts_with("https://") {
        return ("url".into(), None);
    }
    if let Some((l, d)) = v.split_once('@') {
        if !l.is_empty() && d.contains('.') && !d.starts_with('.') && !d.ends_with('.') && !v.contains(' ') {
            return ("email".into(), None);
        }
    }
    let digits = v.chars().filter(|c| c.is_ascii_digit()).count();
    if digits > 0 && v.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | '-' | '+' | ' ')) && {
        // 1,234.50 (en) or 1.234,50 (de): either grouping style is a number.
        let compact: String = v.chars().filter(|c| *c != ' ').collect();
        compact.replace(',', "").parse::<f64>().is_ok() || compact.replace('.', "").replace(',', ".").parse::<f64>().is_ok()
    } {
        return ("number".into(), None);
    }
    ("text".into(), None)
}

fn date_format(v: &str) -> Option<&'static str> {
    let b = v.as_bytes();
    let num = |r: std::ops::Range<usize>| v.get(r).map(|x| !x.is_empty() && x.chars().all(|c| c.is_ascii_digit())).unwrap_or(false);
    if v.len() == 10 && num(0..4) && b[4] == b'-' && num(5..7) && b[7] == b'-' && num(8..10) {
        return Some("YYYY-MM-DD");
    }
    if v.len() == 10 && num(0..2) && b[2] == b'/' && num(3..5) && b[5] == b'/' && num(6..10) {
        let a: u32 = v[0..2].parse().ok()?;
        let c: u32 = v[3..5].parse().ok()?;
        if a > 12 && c <= 12 {
            return Some("DD/MM/YYYY");
        }
        return Some("MM/DD/YYYY");
    }
    // D MMM YYYY
    let parts: Vec<&str> = v.split(' ').collect();
    if parts.len() == 3
        && parts[0].len() <= 2
        && !parts[0].is_empty()
        && parts[0].chars().all(|c| c.is_ascii_digit())
        && matches!(
            parts[1].to_lowercase().as_str(),
            "jan" | "feb" | "mar" | "apr" | "may" | "jun" | "jul" | "aug" | "sep" | "oct" | "nov" | "dec"
        )
        && parts[2].len() == 4
        && parts[2].chars().all(|c| c.is_ascii_digit())
    {
        return Some("D MMM YYYY");
    }
    None
}

// ---------- model ----------

const SYSTEM_PROMPT: &str = "You turn one recorded demonstration into a reusable procedure. \
Write a short title and a one-sentence intent (what the task achieves, so it can be found later). \
List the steps in order. Prefer steps of kind \"api\" when a listed endpoint does the job (set endpoint_id to its id); \
use \"ui\" for clicks and typing that have no endpoint, \"shell\" for command-line work, \"note\" for guidance. \
Mark every value that would change between runs (names, dates, amounts, search terms, ids) as an input: \
use {{input_name}} placeholders in step text (snake_case names) and list each input with a label and the example value seen. \
Give each input a type (text, number, date, datetime, email, url, choice, file, folder, list, secret) guessed from the example: \
dates get type date and the format seen (YYYY-MM-DD, MM/DD/YYYY, DD/MM/YYYY, D MMM YYYY), amounts and counts are number, \
addresses are email, links are url, uploaded files are file; never type a password or token as anything but secret and never include its value. \
The timeline may contain key presses, select choices, uploads, downloads, copies and pastes: a copy followed by a paste moves a value between fields. \
Ask for approval before any step that submits, pays, sends or deletes. \
Redacted values appear as placeholders; never invent secrets. Keep at most 40 steps and 12 inputs.";

fn reply_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["title", "intent", "steps", "inputs"],
        "properties": {
            "title": { "type": "string" },
            "intent": { "type": "string" },
            "steps": { "type": "array", "items": { "type": "object",
                "required": ["kind", "text"],
                "properties": {
                    "id": { "type": "string" },
                    "kind": { "type": "string", "enum": ["api", "ui", "shell", "note"] },
                    "text": { "type": "string" },
                    "endpoint_id": { "type": "string" },
                    "target": { "type": "string" },
                    "input_refs": { "type": "array", "items": { "type": "string" } }
                } } },
            "inputs": { "type": "array", "items": { "type": "object",
                "required": ["name", "label", "example"],
                "properties": {
                    "name": { "type": "string" },
                    "label": { "type": "string" },
                    "example": { "type": "string" },
                    "description": { "type": "string" },
                    "type": { "type": "string", "enum": ["text", "number", "date", "datetime", "email", "url", "choice", "file", "folder", "list", "secret"] },
                    "format": { "type": "string" },
                    "choices": { "type": "array", "items": { "type": "string" } }
                } } }
        }
    })
}

fn build_prompt(endpoints: &[EndpointLite], events: &[CleanEvent], narration: &str, note: &str) -> String {
    let mut p = String::new();
    if !note.trim().is_empty() {
        p.push_str(&format!("Note from the user:\n{}\n\n", note.trim()));
    }
    if !narration.trim().is_empty() {
        p.push_str(&format!("Narration (spoken while demonstrating):\n{}\n\n", clip(narration.trim(), 6000)));
    }
    if !endpoints.is_empty() {
        p.push_str("Endpoints seen (id | method host path):\n");
        for ep in endpoints.iter().take(80) {
            p.push_str(&format!("{} | {} {}{}", ep.id, ep.method.to_uppercase(), ep.host, ep.path_template));
            if let Some(s) = &ep.summary {
                p.push_str(&format!(" | {}", clip(s, 120)));
            }
            if let Some(b) = &ep.body_template {
                p.push_str(&format!(" | body {}", clip(b, 300)));
            }
            p.push('\n');
        }
        p.push('\n');
    }
    if !events.is_empty() {
        p.push_str("Timeline (seconds, kind, detail):\n");
        for ev in events {
            p.push_str(&format!("{:.1} {} {}\n", ev.t as f64 / 1000.0, ev.kind, ev.text));
        }
    }
    p
}

fn sanitize_name(raw: &str) -> String {
    let mut s: String = raw
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    while s.contains("__") {
        s = s.replace("__", "_");
    }
    s.trim_matches('_').chars().take(40).collect()
}

/// Parse the model reply defensively. Returns None when nothing usable is there.
fn parse_model_reply(reply: &str, endpoints: &[EndpointLite], redactions: usize) -> Option<TeachDraft> {
    let start = reply.find('{')?;
    let end = reply.rfind('}')?;
    if end <= start {
        return None;
    }
    let v: Value = serde_json::from_str(&reply[start..=end]).ok()?;
    let s = |x: &Value, k: &str| x.get(k).and_then(Value::as_str).map(|t| t.trim().to_string()).filter(|t| !t.is_empty());

    let mut inputs: Vec<TeachInput> = Vec::new();
    let mut seen_names: HashSet<String> = HashSet::new();
    for i in v.get("inputs").and_then(Value::as_array).into_iter().flatten() {
        if inputs.len() >= MAX_INPUTS {
            break;
        }
        let Some(name) = s(i, "name").map(|n| sanitize_name(&n)).filter(|n| !n.is_empty()) else { continue };
        if !seen_names.insert(name.clone()) {
            continue;
        }
        inputs.push(TeachInput {
            label: s(i, "label").unwrap_or_else(|| name.replace('_', " ")),
            example: s(i, "example").unwrap_or_default(),
            description: s(i, "description"),
            input_type: s(i, "type").map(|t| t.to_lowercase()).filter(|t| INPUT_TYPES.contains(&t.as_str())),
            required: None,
            format: s(i, "format").map(|f| clip(&f, 40)),
            choices: i.get("choices").and_then(Value::as_array).map(|a| {
                a.iter().filter_map(Value::as_str).map(|c| clip(c.trim(), 80)).filter(|c| !c.is_empty()).take(40).collect::<Vec<_>>()
            }).filter(|c| !c.is_empty()),
            root: None,
            secret_ref: None,
            needs_review: None,
            name,
        });
    }

    let known_eps: HashSet<&str> = endpoints.iter().map(|e| e.id.as_str()).collect();
    let mut steps: Vec<TeachStep> = Vec::new();
    let mut used_ids: HashSet<String> = HashSet::new();
    for st in v.get("steps").and_then(Value::as_array).into_iter().flatten() {
        if steps.len() >= MAX_STEPS {
            break;
        }
        let Some(text) = s(st, "text") else { continue };
        let kind = match s(st, "kind").as_deref() {
            Some(k @ ("api" | "ui" | "shell" | "note")) => k.to_string(),
            _ => "note".to_string(),
        };
        let mut id = s(st, "id").unwrap_or_default();
        if id.is_empty() || !used_ids.insert(id.clone()) {
            let mut n = steps.len() + 1;
            loop {
                id = format!("s{}", n);
                if used_ids.insert(id.clone()) {
                    break;
                }
                n += 1;
            }
        }
        let endpoint_id = s(st, "endpoint_id").filter(|e| known_eps.contains(e.as_str()));
        let refs: Vec<String> = st
            .get("input_refs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(sanitize_name)
            .filter(|n| seen_names.contains(n))
            .collect();
        steps.push(TeachStep {
            id,
            kind,
            text,
            endpoint_id,
            target: s(st, "target"),
            input_refs: (!refs.is_empty()).then_some(refs),
        });
    }
    if steps.is_empty() {
        return None;
    }
    Some(TeachDraft {
        title: s(&v, "title").map(|t| clip(&t, 120)).unwrap_or_else(|| "Taught task".into()),
        intent: s(&v, "intent").map(|t| clip(&t, 300)).unwrap_or_else(|| "Repeat the task that was taught".into()),
        steps,
        inputs,
        source: "model".into(),
        redactions,
    })
}

// ---------- routes ----------

async fn distill(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(req): Json<DistillRequest>) -> Response {
    let user = match get_user(&headers) {
        Some(u) => u,
        None => return json_err(StatusCode::UNAUTHORIZED, "Unauthorized"),
    };

    let mut redactions = 0usize;
    let events = clean_events(&req.events, &mut redactions);
    let narration = req.narration.as_deref().map(|s| clip(&redact(s, &mut redactions), 8000)).unwrap_or_default();
    let note = req.note.as_deref().map(|s| clip(&redact(s, &mut redactions), 2000)).unwrap_or_default();

    let mut endpoints: Vec<EndpointLite> = Vec::new();
    if let Some(cid) = req.contract_id.as_deref().filter(|c| !c.is_empty()) {
        match state.db.get_contract_with_endpoints(cid) {
            Ok(Some((contract, eps))) if contract.user_id == user.user_id => {
                for e in eps {
                    endpoints.push(EndpointLite {
                        id: e.id,
                        method: e.method,
                        host: e.host.unwrap_or_default(),
                        path_template: e.path_template.or(e.path).unwrap_or_default(),
                        summary: e.summary,
                        body_template: e.body_template.map(|b| redact(&b, &mut redactions)),
                    });
                }
            }
            Ok(_) => return json_err(StatusCode::NOT_FOUND, "Contract not found"),
            Err(err) => {
                warn!(error = %err, "teach distill: failed to load contract");
                return json_err(StatusCode::INTERNAL_SERVER_ERROR, "Failed to load contract");
            }
        }
    }

    if endpoints.is_empty() && events.is_empty() && narration.is_empty() && note.is_empty() {
        return json_err(StatusCode::BAD_REQUEST, "Nothing to teach from: send events, a note, narration or a contract_id");
    }

    let prompt = build_prompt(&endpoints, &events, &narration, &note);
    let model = crate::memory_extraction::extraction_model();
    let reply = crate::usage_ledger::scope(
        crate::usage_ledger::LedgerCtx::surface("teach").tenant(None, Some(&user.user_id)),
        crate::gizzi_completion::complete_ephemeral_structured(&prompt, Some(SYSTEM_PROMPT), Some(&model), &reply_schema()),
    )
    .await;

    let draft = reply
        .as_deref()
        .and_then(|r| parse_model_reply(r, &endpoints, redactions))
        .unwrap_or_else(|| rules_draft(&endpoints, &events, &note, redactions));
    (StatusCode::OK, Json(draft)).into_response()
}

// ---------- placeholder validation (spec section 1) ----------

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Problem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_id: Option<String>,
    pub message: String,
}

fn problem(step_id: Option<&str>, message: String) -> Problem {
    Problem { step_id: step_id.map(str::to_string), message }
}

/// Every `{{...}}` body in `text`, trimmed.
fn placeholders(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("{{") {
        let after = &rest[i + 2..];
        match after.find("}}") {
            Some(j) => {
                out.push(after[..j].trim().to_string());
                rest = &after[j + 2..];
            }
            None => break,
        }
    }
    out
}

struct Validator<'a> {
    inputs: HashSet<String>,
    used: HashSet<String>,
    profile_keys: &'a HashSet<String>,
    problems: Vec<Problem>,
    warnings: Vec<Problem>,
}

impl Validator<'_> {
    /// Check one text. `vars` = variables set so far; `scope` = for_each names in scope.
    fn check_text(&mut self, text: &str, step_id: Option<&str>, vars: &HashSet<String>, scope: &[String]) {
        for ph in placeholders(text) {
            if let Some(key) = ph.strip_prefix("profile.") {
                if key.is_empty() {
                    self.problems.push(problem(step_id, "Empty profile placeholder `{{profile.}}`".into()));
                } else if !self.profile_keys.contains(key) {
                    self.warnings.push(problem(step_id, format!("Profile value `{}` is not set yet (needed before a run)", key)));
                }
            } else if let Some(v) = ph.strip_prefix("var.") {
                if !vars.contains(v) {
                    self.problems.push(problem(step_id, format!("`{{{{var.{}}}}}` is used before any earlier step extracts `{}`", v, v)));
                }
            } else {
                let base = ph.split('.').next().unwrap_or("");
                if scope.iter().any(|s| s == base) {
                    continue;
                }
                if !ph.contains('.') && self.inputs.contains(&ph) {
                    self.used.insert(ph);
                } else if ph.is_empty() {
                    self.problems.push(problem(step_id, "Empty placeholder `{{}}`".into()));
                } else {
                    self.problems.push(problem(step_id, format!("`{{{{{}}}}}` is not a declared input, profile value, variable or loop item", ph)));
                }
            }
        }
    }

    /// A field that names an input / var / profile value without braces.
    fn check_bare(&mut self, name: &str, step_id: Option<&str>, vars: &HashSet<String>, scope: &[String]) {
        if name.contains("{{") {
            self.check_text(name, step_id, vars, scope);
        } else if !name.trim().is_empty() {
            self.check_text(&format!("{{{{{}}}}}", name.trim()), step_id, vars, scope);
        }
    }

    fn check_value(&mut self, v: &Value, step_id: Option<&str>, vars: &HashSet<String>, scope: &[String]) {
        match v {
            Value::String(s) => self.check_text(s, step_id, vars, scope),
            Value::Array(a) => a.iter().for_each(|x| self.check_value(x, step_id, vars, scope)),
            Value::Object(m) => m.values().for_each(|x| self.check_value(x, step_id, vars, scope)),
            _ => {}
        }
    }

    fn walk(&mut self, steps: &[Value], vars: &mut HashSet<String>, scope: &mut Vec<String>) {
        for st in steps {
            let Some(m) = st.as_object() else { continue };
            let sid = m.get("id").and_then(Value::as_str);
            let kind = m.get("kind").and_then(Value::as_str).unwrap_or("");
            for (k, v) in m {
                match k.as_str() {
                    "id" | "kind" | "note" | "var" | "as" | "on_error" | "attr" | "steps" | "unsafe_to_repeat" | "timeout_ms" => {}
                    "skip_if_empty" => {
                        if let Some(n) = v.as_str().filter(|n| !n.trim().is_empty()) {
                            self.check_bare(n, sid, vars, scope);
                        }
                    }
                    "list" if kind == "for_each" => {
                        if let Some(n) = v.as_str() {
                            self.check_bare(n, sid, vars, scope);
                        }
                    }
                    _ => self.check_value(v, sid, vars, scope),
                }
            }
            if kind == "extract" {
                if let Some(name) = m.get("var").and_then(Value::as_str).filter(|n| !n.is_empty()) {
                    vars.insert(name.to_string());
                }
            }
            if kind == "for_each" {
                let as_name = m.get("as").and_then(Value::as_str).unwrap_or("item").to_string();
                scope.push(as_name);
                if let Some(inner) = m.get("steps").and_then(Value::as_array) {
                    self.walk(inner, vars, scope);
                }
                scope.pop();
            }
        }
    }
}

fn collect_loop_names_and_vars(steps: &[Value], names: &mut Vec<String>, vars: &mut HashSet<String>) {
    for st in steps {
        let Some(m) = st.as_object() else { continue };
        match m.get("kind").and_then(Value::as_str) {
            Some("extract") => {
                if let Some(v) = m.get("var").and_then(Value::as_str) {
                    vars.insert(v.to_string());
                }
            }
            Some("for_each") => {
                names.push(m.get("as").and_then(Value::as_str).unwrap_or("item").to_string());
                if let Some(inner) = m.get("steps").and_then(Value::as_array) {
                    collect_loop_names_and_vars(inner, names, vars);
                }
            }
            _ => {}
        }
    }
}

/// Returns (errors, warnings). Errors block saving; warnings do not.
pub fn validate_skill(skill: &TeachSkill, profile_keys: &HashSet<String>) -> (Vec<Problem>, Vec<Problem>) {
    let mut v = Validator {
        inputs: skill.inputs.iter().map(|i| i.name.clone()).collect(),
        used: HashSet::new(),
        profile_keys,
        problems: Vec::new(),
        warnings: Vec::new(),
    };
    let wf_steps: Vec<Value> = skill
        .extra
        .get("workflow")
        .and_then(|w| w.get("steps"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    // Prose (intent, step text): variables and loop names count wherever they appear in the workflow.
    let mut loop_names = Vec::new();
    let mut all_vars = HashSet::new();
    collect_loop_names_and_vars(&wf_steps, &mut loop_names, &mut all_vars);
    v.check_text(&skill.intent, None, &all_vars, &loop_names);
    for st in &skill.steps {
        v.check_text(&st.text, Some(&st.id), &all_vars, &loop_names);
        for r in st.input_refs.iter().flatten() {
            if v.inputs.contains(r) {
                v.used.insert(r.clone());
            }
        }
    }
    // Workflow steps: strict order.
    if let Some(sa) = skill.extra.get("workflow").and_then(|w| w.get("start_url")).and_then(Value::as_str) {
        v.check_text(sa, None, &HashSet::new(), &[]);
    }
    let mut vars = HashSet::new();
    let mut scope = Vec::new();
    v.walk(&wf_steps, &mut vars, &mut scope);
    for inp in &skill.inputs {
        if let Some(root) = inp.root.as_deref() {
            v.check_text(root, None, &HashSet::new(), &[]);
        }
    }
    let unused: Vec<String> = skill.inputs.iter().filter(|i| !v.used.contains(&i.name)).map(|i| i.name.clone()).collect();
    for name in unused {
        v.problems.push(problem(None, format!("Input `{}` is declared but never used", name)));
    }
    (v.problems, v.warnings)
}

// ---------- business profile ----------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TeachProfileValue {
    #[serde(default)]
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub value_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TeachProfile {
    #[serde(default)]
    pub values: BTreeMap<String, TeachProfileValue>,
    #[serde(default)]
    pub updated_at: String,
}

fn valid_profile_key(k: &str) -> bool {
    !k.is_empty() && k.len() <= 64 && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

async fn get_profile(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let user = match get_user(&headers) {
        Some(u) => u,
        None => return json_err(StatusCode::UNAUTHORIZED, "Unauthorized"),
    };
    let db = state.db.clone();
    let uid = user.user_id.clone();
    let res = tokio::task::spawn_blocking(move || -> rusqlite::Result<Option<String>> {
        let conn = db.connect()?;
        match conn.query_row("SELECT body_json FROM teach_profiles WHERE user_id = ?1", rusqlite::params![uid], |r| r.get::<_, String>(0)) {
            Ok(j) => Ok(Some(j)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    })
    .await;
    match res {
        Ok(Ok(Some(j))) => {
            let p: TeachProfile = serde_json::from_str(&j).unwrap_or_default();
            (StatusCode::OK, Json(p)).into_response()
        }
        Ok(Ok(None)) => (StatusCode::OK, Json(TeachProfile::default())).into_response(),
        other => {
            warn!(error = ?other.err(), "teach: get profile failed");
            json_err(StatusCode::INTERNAL_SERVER_ERROR, "Failed to load profile")
        }
    }
}

async fn put_profile(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(mut body): Json<TeachProfile>) -> Response {
    let user = match get_user(&headers) {
        Some(u) => u,
        None => return json_err(StatusCode::UNAUTHORIZED, "Unauthorized"),
    };
    if body.values.len() > 100 {
        return json_err(StatusCode::BAD_REQUEST, "Too many profile values (max 100)");
    }
    if let Some(bad) = body.values.keys().find(|k| !valid_profile_key(k)) {
        return json_err(StatusCode::BAD_REQUEST, &format!("Invalid profile key: {}", clip(bad, 40)));
    }
    for v in body.values.values_mut() {
        v.value = clip(&v.value, 2000);
        v.label = v.label.take().map(|l| clip(l.trim(), 120)).filter(|l| !l.is_empty());
        v.value_type = v.value_type.take().map(|t| t.to_lowercase()).filter(|t| INPUT_TYPES.contains(&t.as_str()));
        // secrets live in the vault, never in the profile
        if v.value_type.as_deref() == Some("secret") {
            return json_err(StatusCode::BAD_REQUEST, "Secrets cannot be stored in the business profile");
        }
    }
    body.updated_at = chrono::Utc::now().to_rfc3339();
    let db = state.db.clone();
    let uid = user.user_id.clone();
    let res = tokio::task::spawn_blocking(move || -> rusqlite::Result<TeachProfile> {
        let conn = db.connect()?;
        let json = serde_json::to_string(&body).unwrap_or_else(|_| "{}".into());
        conn.execute(
            "INSERT INTO teach_profiles (user_id, body_json, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(user_id) DO UPDATE SET body_json = excluded.body_json, updated_at = excluded.updated_at",
            rusqlite::params![uid, json, body.updated_at],
        )?;
        Ok(body)
    })
    .await;
    match res {
        Ok(Ok(p)) => (StatusCode::OK, Json(p)).into_response(),
        other => {
            warn!(error = ?other.err(), "teach: save profile failed");
            json_err(StatusCode::INTERNAL_SERVER_ERROR, "Failed to save profile")
        }
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

fn normalize_skill(mut s: TeachSkill, id: &str, now: &str) -> TeachSkill {
    s.id = id.to_string();
    s.title = clip(s.title.trim(), 120);
    s.intent = clip(s.intent.trim(), 300);
    s.steps.truncate(MAX_STEPS);
    s.inputs.truncate(MAX_INPUTS);
    if !matches!(s.source.as_str(), "model" | "rules") {
        s.source = "rules".into();
    }
    s.updated_at = now.to_string();
    s
}

async fn list_skills(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let user = match get_user(&headers) {
        Some(u) => u,
        None => return json_err(StatusCode::UNAUTHORIZED, "Unauthorized"),
    };
    let db = state.db.clone();
    let uid = user.user_id.clone();
    let res = tokio::task::spawn_blocking(move || -> rusqlite::Result<Vec<TeachSkill>> {
        let conn = db.connect()?;
        let mut stmt = conn.prepare("SELECT body_json FROM teach_skills WHERE user_id = ?1 ORDER BY updated_at DESC")?;
        let rows = stmt.query_map(rusqlite::params![uid], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            if let Ok(skill) = serde_json::from_str::<TeachSkill>(&row?) {
                out.push(skill);
            }
        }
        Ok(out)
    })
    .await;
    match res {
        Ok(Ok(skills)) => (StatusCode::OK, Json(json!({ "skills": skills }))).into_response(),
        other => {
            warn!(error = ?other.err(), "teach: list skills failed");
            json_err(StatusCode::INTERNAL_SERVER_ERROR, "Failed to list skills")
        }
    }
}

async fn put_skill(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<TeachSkill>,
) -> Response {
    let user = match get_user(&headers) {
        Some(u) => u,
        None => return json_err(StatusCode::UNAUTHORIZED, "Unauthorized"),
    };
    if !valid_id(&id) {
        return json_err(StatusCode::BAD_REQUEST, "Invalid skill id");
    }
    let now = chrono::Utc::now().to_rfc3339();
    let db = state.db.clone();
    let uid = user.user_id.clone();
    let res = tokio::task::spawn_blocking(move || -> rusqlite::Result<Result<(TeachSkill, Vec<Problem>), Vec<Problem>>> {
        let conn = db.connect()?;
        let profile_keys: HashSet<String> = conn
            .query_row("SELECT body_json FROM teach_profiles WHERE user_id = ?1", rusqlite::params![uid], |r| r.get::<_, String>(0))
            .ok()
            .and_then(|j| serde_json::from_str::<TeachProfile>(&j).ok())
            .map(|p| p.values.keys().cloned().collect())
            .unwrap_or_default();
        let (problems, warnings) = validate_skill(&body, &profile_keys);
        if !problems.is_empty() {
            return Ok(Err(problems));
        }
        let existing: Option<String> = conn
            .query_row(
                "SELECT created_at FROM teach_skills WHERE user_id = ?1 AND id = ?2",
                rusqlite::params![uid, id],
                |r| r.get(0),
            )
            .ok();
        let mut skill = normalize_skill(body, &id, &now);
        let version = skill.extra.get("version").and_then(Value::as_u64).unwrap_or(0);
        skill.extra.insert("version".into(), json!(version + 1));
        skill.created_at = existing
            .or_else(|| Some(skill.created_at.clone()).filter(|c| !c.is_empty()))
            .unwrap_or_else(|| now.clone());
        let json = serde_json::to_string(&skill).unwrap_or_else(|_| "{}".into());
        conn.execute(
            "INSERT INTO teach_skills (id, user_id, contract_id, body_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(user_id, id) DO UPDATE SET contract_id = excluded.contract_id,
               body_json = excluded.body_json, updated_at = excluded.updated_at",
            rusqlite::params![id, uid, skill.contract_id, json, skill.created_at, skill.updated_at],
        )?;
        Ok(Ok((skill, warnings)))
    })
    .await;
    match res {
        Ok(Ok(Err(problems))) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "error": "invalid_skill", "problems": problems })),
        )
            .into_response(),
        Ok(Ok(Ok((skill, warnings)))) => {
            let mut resp = (StatusCode::OK, Json(skill)).into_response();
            if !warnings.is_empty() {
                if let Ok(v) = axum::http::HeaderValue::from_str(&warnings.iter().map(|w| w.message.replace(['\n', '\r'], " ")).collect::<Vec<_>>().join(" | ")) {
                    resp.headers_mut().insert("x-teach-warnings", v);
                }
            }
            resp
        }
        other => {
            warn!(error = ?other.err(), "teach: save skill failed");
            json_err(StatusCode::INTERNAL_SERVER_ERROR, "Failed to save skill")
        }
    }
}

async fn delete_skill(State(state): State<Arc<AppState>>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let user = match get_user(&headers) {
        Some(u) => u,
        None => return json_err(StatusCode::UNAUTHORIZED, "Unauthorized"),
    };
    let db = state.db.clone();
    let uid = user.user_id.clone();
    let res = tokio::task::spawn_blocking(move || -> rusqlite::Result<usize> {
        db.connect()?.execute("DELETE FROM teach_skills WHERE user_id = ?1 AND id = ?2", rusqlite::params![uid, id])
    })
    .await;
    match res {
        Ok(Ok(n)) if n > 0 => (StatusCode::NO_CONTENT, ()).into_response(),
        Ok(Ok(_)) => json_err(StatusCode::NOT_FOUND, "Skill not found"),
        other => {
            warn!(error = ?other.err(), "teach: delete skill failed");
            json_err(StatusCode::INTERNAL_SERVER_ERROR, "Failed to delete skill")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep(id: &str) -> EndpointLite {
        EndpointLite { id: id.into(), method: "post".into(), host: "api.shop.test".into(), path_template: "/orders/{id}".into(), summary: None, body_template: None }
    }

    #[test]
    fn rules_draft_prefers_endpoints() {
        let d = rules_draft(&[ep("e1"), ep("e2")], &[], "", 0);
        assert_eq!(d.source, "rules");
        assert_eq!(d.steps.len(), 2);
        assert_eq!(d.steps[0].kind, "api");
        assert_eq!(d.steps[0].text, "POST api.shop.test/orders/{id}");
        assert!(d.intent.contains("api.shop.test"));
    }

    #[test]
    fn rules_draft_from_events_and_clamps() {
        let events: Vec<CleanEvent> = (0..80)
            .map(|i| CleanEvent { t: i, kind: "click".into(), text: format!("Button {}", i), url: Some("https://x.test/a".into()) })
            .collect();
        let d = rules_draft(&[], &events, "", 0);
        assert_eq!(d.steps.len(), MAX_STEPS);
        assert_eq!(d.steps[0].text, "Click Button 0");
        assert_eq!(d.title, "Task on x.test");
    }

    #[test]
    fn parses_fenced_model_reply_and_fills_ids() {
        let reply = "```json\n{\"title\":\"Order\",\"intent\":\"Place an order\",\"steps\":[\
            {\"kind\":\"api\",\"text\":\"POST /orders {{item}}\",\"endpoint_id\":\"e1\",\"input_refs\":[\"Item\",\"ghost\"]},\
            {\"kind\":\"weird\",\"text\":\"Check\",\"endpoint_id\":\"nope\"}],\
            \"inputs\":[{\"name\":\"Item\",\"label\":\"Item\",\"example\":\"tea\"},{\"name\":\"item\",\"label\":\"dup\",\"example\":\"\"}]}\n```";
        let d = parse_model_reply(reply, &[ep("e1")], 3).unwrap();
        assert_eq!(d.source, "model");
        assert_eq!(d.redactions, 3);
        assert_eq!(d.steps[0].id, "s1");
        assert_eq!(d.steps[1].id, "s2");
        assert_eq!(d.steps[1].kind, "note");
        assert_eq!(d.steps[1].endpoint_id, None);
        assert_eq!(d.steps[0].input_refs, Some(vec!["item".to_string()]));
        assert_eq!(d.inputs.len(), 1);
        assert_eq!(d.inputs[0].name, "item");
    }

    #[test]
    fn rejects_bad_model_output() {
        assert!(parse_model_reply("sorry, no", &[], 0).is_none());
        assert!(parse_model_reply("{\"title\":\"x\",\"steps\":[]}", &[], 0).is_none());
    }

    fn skill(intent: &str, inputs: &[&str], steps: &[&str], wf: Option<Value>) -> TeachSkill {
        let mut extra = serde_json::Map::new();
        if let Some(w) = wf {
            extra.insert("workflow".into(), w);
        }
        TeachSkill {
            id: "t".into(),
            title: "t".into(),
            intent: intent.into(),
            steps: steps
                .iter()
                .enumerate()
                .map(|(i, t)| TeachStep { id: format!("s{}", i + 1), kind: "ui".into(), text: (*t).into(), endpoint_id: None, target: None, input_refs: None })
                .collect(),
            inputs: inputs
                .iter()
                .map(|n| TeachInput { name: (*n).into(), label: (*n).into(), example: "x".into(), description: None, input_type: None, required: None, format: None, choices: None, root: None, secret_ref: None, needs_review: None })
                .collect(),
            source: "model".into(),
            redactions: 0,
            contract_id: None,
            domain: None,
            skill_path: None,
            routine_id: None,
            created_at: String::new(),
            updated_at: String::new(),
            extra,
        }
    }

    fn msgs(p: &[Problem]) -> String {
        p.iter().map(|x| x.message.clone()).collect::<Vec<_>>().join(" | ")
    }

    #[test]
    fn valid_skill_passes() {
        let s = skill("Order {{item}}", &["item"], &["Type {{item}}"], None);
        let (e, w) = validate_skill(&s, &HashSet::new());
        assert!(e.is_empty() && w.is_empty(), "{:?}", e);
    }

    #[test]
    fn undeclared_placeholder_is_error_and_unused_input_is_error() {
        let s = skill("Do it", &["unused"], &["Type {{ghost}}"], None);
        let (e, _) = validate_skill(&s, &HashSet::new());
        assert!(msgs(&e).contains("ghost"));
        assert!(msgs(&e).contains("`unused` is declared but never used"));
        assert_eq!(e[0].step_id.as_deref(), Some("s1"));
    }

    #[test]
    fn profile_missing_is_warning_only() {
        let s = skill("Open {{profile.erp_url}}", &[], &[], None);
        let (e, w) = validate_skill(&s, &HashSet::new());
        assert!(e.is_empty());
        assert_eq!(w.len(), 1);
        let keys: HashSet<String> = ["erp_url".to_string()].into();
        assert!(validate_skill(&s, &keys).1.is_empty());
    }

    #[test]
    fn var_must_be_extracted_earlier() {
        let wf = json!({"version":1,"steps":[
            {"id":"a","kind":"fill","target":{"css":"#x"},"value":"{{var.total}}"},
            {"id":"b","kind":"extract","target":{"css":"#t"},"var":"total"},
            {"id":"c","kind":"fill","target":{"css":"#y"},"value":"{{var.total}}"}]});
        let s = skill("Copy total", &[], &[], Some(wf));
        let (e, _) = validate_skill(&s, &HashSet::new());
        assert_eq!(e.len(), 1, "{}", msgs(&e));
        assert_eq!(e[0].step_id.as_deref(), Some("a"));
    }

    #[test]
    fn for_each_scope_and_nested_steps() {
        let wf = json!({"version":1,"steps":[
            {"id":"l","kind":"for_each","list":"rows","as":"row","steps":[
                {"id":"f","kind":"fill","target":{"label":"{{row.name}}"},"value":"{{row}} {{region}}"}]},
            {"id":"g","kind":"fill","target":{"css":"#z"},"value":"{{row}}"}]});
        let s = skill("Loop", &["rows", "region"], &[], Some(wf));
        let (e, _) = validate_skill(&s, &HashSet::new());
        assert_eq!(e.len(), 1, "{}", msgs(&e));
        assert_eq!(e[0].step_id.as_deref(), Some("g"));
    }

    #[test]
    fn skip_if_empty_and_goto_count_as_use() {
        let wf = json!({"version":1,"steps":[
            {"id":"a","kind":"goto","url":"{{profile.erp_url}}/login"},
            {"id":"b","kind":"fill","target":{"css":"#n"},"value":"{{note}}","skip_if_empty":"note"}]});
        let s = skill("x", &["note"], &[], Some(wf));
        assert!(validate_skill(&s, &HashSet::new()).0.is_empty());
    }

    #[test]
    fn skill_round_trips_unknown_fields_and_typed_inputs() {
        let raw = json!({"id":"a","title":"t","workflow":{"version":1,"steps":[]},"version":3,"auto_approve_unsafe":true,
            "inputs":[{"name":"d","label":"D","example":"2026-01-02","type":"date","format":"YYYY-MM-DD"}]});
        let s: TeachSkill = serde_json::from_value(raw).unwrap();
        assert_eq!(s.inputs[0].input_type.as_deref(), Some("date"));
        let back = serde_json::to_value(&s).unwrap();
        assert_eq!(back["version"], 3);
        assert_eq!(back["auto_approve_unsafe"], true);
        assert!(back["workflow"]["steps"].is_array());
        assert_eq!(back["inputs"][0]["type"], "date");
        assert!(back["inputs"][0].get("choices").is_none());
    }

    #[test]
    fn clean_events_handles_new_kinds() {
        let events = vec![
            json!({"t":1,"kind":"key","key":"Enter","label":"Search"}),
            json!({"t":2,"kind":"select","label":"Region","value":"EU"}),
            json!({"t":3,"kind":"upload","label":"Invoice","fileName":"inv.pdf"}),
            json!({"t":4,"kind":"download","fileName":"report.xlsx"}),
            json!({"t":5,"kind":"copy","textPreview":"INV-1042"}),
            json!({"t":6,"kind":"paste","label":"Reference"}),
        ];
        let mut c = 0;
        let out = clean_events(&events, &mut c);
        assert_eq!(out.len(), 6);
        assert_eq!(out[0].text, "Enter in Search");
        assert_eq!(out[1].text, "Region = EU");
        assert_eq!(out[2].text, "Invoice <- file inv.pdf");
    }

    #[test]
    fn rules_infer_types() {
        assert_eq!(guess_type("2026-10-25"), ("date".into(), Some("YYYY-MM-DD".into())));
        assert_eq!(guess_type("25/10/2026").1.as_deref(), Some("DD/MM/YYYY"));
        assert_eq!(guess_type("3 Oct 2026").1.as_deref(), Some("D MMM YYYY"));
        assert_eq!(guess_type("a@b.co").0, "email");
        assert_eq!(guess_type("https://x.test").0, "url");
        assert_eq!(guess_type("1,234.50").0, "number");
        assert_eq!(guess_type("hello").0, "text");
        let evs = vec![CleanEvent { t: 0, kind: "input".into(), text: "Due date = 2026-10-25".into(), url: None }];
        let d = rules_draft(&[], &evs, "", 0);
        assert_eq!(d.inputs[0].input_type.as_deref(), Some("date"));
        assert_eq!(d.inputs[0].name, "due_date");
    }
}
