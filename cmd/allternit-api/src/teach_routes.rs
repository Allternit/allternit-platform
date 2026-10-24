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
use std::collections::HashSet;
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
}

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
    TeachDraft { title, intent, steps, inputs: vec![], source: "rules".into(), redactions }
}

// ---------- model ----------

const SYSTEM_PROMPT: &str = "You turn one recorded demonstration into a reusable procedure. \
Write a short title and a one-sentence intent (what the task achieves, so it can be found later). \
List the steps in order. Prefer steps of kind \"api\" when a listed endpoint does the job (set endpoint_id to its id); \
use \"ui\" for clicks and typing that have no endpoint, \"shell\" for command-line work, \"note\" for guidance. \
Mark every value that would change between runs (names, dates, amounts, search terms, ids) as an input: \
use {{input_name}} placeholders in step text (snake_case names) and list each input with a label and the example value seen. \
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
                    "description": { "type": "string" }
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
    let res = tokio::task::spawn_blocking(move || -> rusqlite::Result<TeachSkill> {
        let conn = db.connect()?;
        let existing: Option<String> = conn
            .query_row(
                "SELECT created_at FROM teach_skills WHERE user_id = ?1 AND id = ?2",
                rusqlite::params![uid, id],
                |r| r.get(0),
            )
            .ok();
        let mut skill = normalize_skill(body, &id, &now);
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
        Ok(skill)
    })
    .await;
    match res {
        Ok(Ok(skill)) => (StatusCode::OK, Json(skill)).into_response(),
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
}
