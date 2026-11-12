//! `POST /v1/decisions`, `GET /v1/decisions/{id}`, `PATCH /v1/decisions/{id}`
//! (scope `decisions`, meter `decision_questions`). Lane 1 of the Platform API
//! parity program.
//!
//! A request carries typed questions about one input, in OpenAI's shape
//! (`questions[]` with `predicate | choice | score`) or Google's (`questions`
//! object keyed by name, `noul` for predicate, `criteria` for choices/levels).
//! [`normalize`] turns either into [`DecisionRequest`]. Each question then
//! becomes one call to the agency node's `POST /v1/decisions` (the Agency
//! decision runtime: `context`, `options`, `kind`, `question`), run here with
//! bounded concurrency ([`CONCURRENCY`]) under one `latency_budget_ms`:
//!
//! - predicate → options `yes`/`no`, kind `verify`; `probability` = P(yes)
//! - choice → options from `choices` (id = value, text = description or value)
//! - score → options from `levels` (id = level index, text = `label: description`);
//!   `score` = probability-weighted index scaled to 0..1
//!
//! The node `context` is the shared `instructions` then the input, and the node
//! `question` is only that question's own instructions. The scorer renders
//! context first, so every question of one request shares the same prompt
//! prefix and reuses its cache.
//!
//! The node is called as a trusted peer asserting `x-allternit-user-id:
//! platform:<project_id>` (the forwarder's node URL and secret,
//! `routes::agency_forward`); the caller's `backends` never pass through and
//! `project` is always the platform project. A question that misses the
//! budget, fails on the node, or abstains comes back as a `refusal` answer.
//!
//! The single-question Agency body (`context`, `options`, `kind`, `question`)
//! is accepted too and answered in the Agency shape (the node's response with
//! our `dec_` id), still stored and metered.
//!
//! Rows live in `platform_decisions` (migration 123). Each stored answer keeps
//! the node's decision id (`node_decision_id`, never in the public JSON) so
//! `PATCH` can forward outcomes to the node's outcome log. Billing: one
//! `decision_questions` per answered (non-refusal) question, live projects only
//! (sandbox usage isn't charged, as for every other fixed-price meter).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Extension, Json,
};
use chrono::{DateTime, Utc};
use futures::stream::{self, StreamExt};
use serde_json::{json, Map, Value};
use sqlx::{FromRow, PgPool};

use super::{new_id, record_usage_priced, ApiJson, PlatformCaller, PlatformError, ProjectEnv, RouteTable, UsageEvent};
use crate::routes::agency_forward::{AgencyForward, FORWARD_HOST};
use crate::ApiState;

pub const SCOPE: &str = "decisions";
pub const METER: &str = "decision_questions";

// Limits below are sized to the CPU scorer (llama.cpp, Qwen3.5-4B, 4 threads,
// one request at a time, n_ctx 8192, no vision); raise with faster hardware.
/// Questions per request. Sized to the CPU scorer; raise with faster hardware.
const MAX_QUESTIONS: usize = 16;
const MAX_CHOICES: usize = 255;
const MAX_LEVELS: usize = 10;
/// Input text bytes (the scorer's n_ctx is 8192 tokens). Sized to the CPU
/// scorer; raise with faster hardware.
const MAX_TEXT_BYTES: usize = 4 * 1024;
/// Default budget when the caller sends none: `BASE + PER_BYTE * input bytes
/// + PER_QUESTION * questions`, clamped to `MIN_DEFAULT_BUDGET_MS..=MAX_BUDGET_MS`.
/// Sized to the CPU scorer (cold prefill about 5 s per 100 tokens, 1-3 s per
/// question once the prefix is cached); raise with faster hardware.
const BUDGET_BASE_MS: u64 = 5_000;
const BUDGET_PER_BYTE_MS: u64 = 15;
const BUDGET_PER_QUESTION_MS: u64 = 3_000;
const MIN_DEFAULT_BUDGET_MS: u64 = 10_000;
const MAX_BUDGET_MS: u64 = 120_000;
/// Node calls in flight at once for one request. The scorer serialises
/// requests and a node timeout does not cancel its job, so more in flight only
/// queues work that may be thrown away. Sized to the CPU scorer; raise with
/// faster hardware.
pub const CONCURRENCY: usize = 2;
const MAX_NAME: usize = 100;
/// Node limits (`agency_api/decisions/mod.rs validate`): option id 1-128, text 1-2000 bytes.
const MAX_OPTION_ID: usize = 128;
const MAX_OPTION_TEXT: usize = 2000;
/// The node's reserved abstain id; never a valid option.
const NODE_ABSTAIN: &str = "__abstain__";
const OUTCOME_STATUSES: [&str; 4] = ["success", "failure", "error", "skipped"];
/// Name of the one question an Agency-shaped body is stored as.
const AGENCY_QUESTION: &str = "decision";
const OUTCOME_FORWARD_TIMEOUT: Duration = Duration::from_secs(10);

pub fn register(table: RouteTable) -> RouteTable {
    table
        .add("/v1/decisions", &["POST"], post(create))
        .add("/v1/decisions/:id", &["GET", "PATCH"], get(fetch).patch(patch))
}

// ── request model ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QType {
    Predicate,
    Choice,
    Score,
}

impl QType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Predicate => "predicate",
            Self::Choice => "choice",
            Self::Score => "score",
        }
    }
}

/// A choice (`value`, `description`) or a level (`label`, `description`).
#[derive(Debug, Clone, PartialEq)]
pub struct Opt {
    pub value: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Question {
    pub name: String,
    pub qtype: QType,
    pub instructions: String,
    /// Choices (choice) or levels low to high (score); empty for predicate.
    pub options: Vec<Opt>,
}

impl Question {
    /// The spec's `DecisionQuestion` form (what is stored).
    pub fn to_json(&self) -> Value {
        let mut q = json!({ "name": self.name, "type": self.qtype.as_str(), "instructions": self.instructions });
        let (key, field) = match self.qtype {
            QType::Predicate => return q,
            QType::Choice => ("choices", "value"),
            QType::Score => ("levels", "label"),
        };
        q[key] = Value::Array(
            self.options
                .iter()
                .map(|o| {
                    let mut v = json!({ field: o.value });
                    if let Some(d) = &o.description {
                        v["description"] = json!(d);
                    }
                    v
                })
                .collect(),
        );
        q
    }

    fn from_stored(v: &Value) -> Option<Question> {
        parse_question(None, v, "questions").ok()
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Input {
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct DecisionRequest {
    pub input: Input,
    pub instructions: String,
    pub questions: Vec<Question>,
    pub budget_ms: u64,
    pub allow_abstain: bool,
    pub metadata: Value,
}

fn bad(code: &str, message: impl Into<String>, param: &str) -> PlatformError {
    PlatformError::invalid_request(code, message).with_param(param)
}

/// The single-question Agency body: `options` (or its alias `candidates`) and no `questions`.
pub fn is_agency_body(body: &Value) -> bool {
    body.get("questions").is_none() && (body.get("options").is_some() || body.get("candidates").is_some())
}

/// OpenAI- or Google-shaped body → [`DecisionRequest`], with every limit checked.
pub fn normalize(body: &Value) -> Result<DecisionRequest, PlatformError> {
    let obj = body.as_object().ok_or_else(|| bad("invalid_request_body", "The body must be a JSON object.", "body"))?;
    let input = parse_input(obj.get("input"))?;
    let instructions = match obj.get("instructions") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(_) => return Err(bad("invalid_instructions", "instructions must be a string.", "instructions")),
    };
    if node_context(&instructions, &input).len() > MAX_TEXT_BYTES {
        return Err(bad(
            "input_too_large",
            format!("instructions plus input text are over {MAX_TEXT_BYTES} bytes."),
            "instructions",
        ));
    }
    let questions = parse_questions(obj.get("questions"))?;
    // The question tail (its instructions and option texts) must fit the scorer's
    // context next to the shared prefix, so it gets the same cap.
    for (i, q) in questions.iter().enumerate() {
        let tail = q.instructions.len()
            + q.options.iter().map(|o| o.value.len() + o.description.as_deref().map_or(0, str::len)).sum::<usize>();
        if tail > MAX_TEXT_BYTES {
            return Err(bad(
                "question_too_large",
                format!("a question's instructions plus its choices or levels are over {MAX_TEXT_BYTES} bytes."),
                &format!("questions[{i}]"),
            ));
        }
    }
    let budget_ms = match obj.get("latency_budget_ms") {
        None | Some(Value::Null) => default_budget_ms(node_context(&instructions, &input).len(), questions.len()),
        Some(v) => v.as_u64().filter(|n| (1..=MAX_BUDGET_MS).contains(n)).ok_or_else(|| {
            bad("invalid_latency_budget", format!("latency_budget_ms must be 1 to {MAX_BUDGET_MS}."), "latency_budget_ms")
        })?,
    };
    let allow_abstain = match obj.get("allow_abstain") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(bad("invalid_allow_abstain", "allow_abstain must be a boolean.", "allow_abstain")),
    };
    let metadata = match obj.get("metadata") {
        None | Some(Value::Null) => json!({}),
        Some(Value::Object(m)) => Value::Object(m.clone()),
        Some(_) => return Err(bad("invalid_metadata", "metadata must be an object.", "metadata")),
    };
    Ok(DecisionRequest { input, instructions, questions, budget_ms, allow_abstain, metadata })
}

/// The budget when the caller sends none, from the bytes the scorer reads
/// (shared instructions + input) and the question count.
pub fn default_budget_ms(input_bytes: usize, questions: usize) -> u64 {
    (BUDGET_BASE_MS + BUDGET_PER_BYTE_MS * input_bytes as u64 + BUDGET_PER_QUESTION_MS * questions as u64)
        .clamp(MIN_DEFAULT_BUDGET_MS, MAX_BUDGET_MS)
}

fn image_not_supported(param: &str) -> PlatformError {
    bad("image_not_supported", "Image inputs are not supported yet.", param)
}

fn input_too_large() -> PlatformError {
    bad("input_too_large", format!("input text is over {MAX_TEXT_BYTES} bytes."), "input")
}

/// `input`: a string, message items (`{role, content: string | parts}`), or bare
/// parts (Google style). Parts: `input_text`, `input_json`; an image part
/// (`input_image`, `inline_data`) is refused with `image_not_supported` (the
/// scorer has no vision). Text joins with blank lines.
pub fn parse_input(v: Option<&Value>) -> Result<Input, PlatformError> {
    let mut texts: Vec<String> = Vec::new();
    match v {
        None | Some(Value::Null) => {}
        Some(Value::String(s)) => texts.push(s.clone()),
        Some(Value::Array(items)) => {
            for (i, item) in items.iter().enumerate() {
                match item.get("content") {
                    Some(Value::String(s)) => texts.push(s.clone()),
                    Some(Value::Array(parts)) => {
                        for (j, p) in parts.iter().enumerate() {
                            push_part(p, &format!("input[{i}].content[{j}]"), &mut texts)?;
                        }
                    }
                    Some(_) => {
                        return Err(bad("invalid_input", "content must be a string or an array of parts.", &format!("input[{i}].content")))
                    }
                    None => push_part(item, &format!("input[{i}]"), &mut texts)?,
                }
            }
        }
        Some(_) => return Err(bad("invalid_input", "input must be a string or an array of items.", "input")),
    }
    let text = texts.join("\n\n");
    if text.len() > MAX_TEXT_BYTES {
        return Err(input_too_large());
    }
    Ok(Input { text })
}

fn push_part(p: &Value, param: &str, texts: &mut Vec<String>) -> Result<(), PlatformError> {
    let ty = p.get("type").and_then(Value::as_str).unwrap_or("");
    let is = |names: &[&str]| names.contains(&ty);
    if is(&["input_text", "text", "output_text"]) || (ty.is_empty() && p.get("text").is_some()) {
        let s = p
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| bad("invalid_input", "A text part needs a string 'text'.", param))?;
        texts.push(s.to_string());
    } else if is(&["input_json", "json"]) || (ty.is_empty() && p.get("json").is_some()) {
        let j = ["json", "data", "value"]
            .iter()
            .find_map(|k| p.get(*k))
            .ok_or_else(|| bad("invalid_input", "A JSON part needs 'json'.", param))?;
        texts.push(match j {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        });
    } else if is(&["input_image", "image"])
        || (ty.is_empty() && (p.get("inline_data").is_some() || p.get("image_url").is_some()))
    {
        return Err(image_not_supported(param));
    } else {
        return Err(bad(
            "unsupported_input_part",
            format!("Input part type '{ty}' is not supported (input_text, input_json)."),
            &format!("{param}.type"),
        ));
    }
    Ok(())
}

/// `questions`: an array (OpenAI) or an object keyed by name (Google).
pub fn parse_questions(v: Option<&Value>) -> Result<Vec<Question>, PlatformError> {
    let list: Vec<(Option<String>, &Value, String)> = match v {
        Some(Value::Array(a)) => a.iter().enumerate().map(|(i, q)| (None, q, format!("questions[{i}]"))).collect(),
        Some(Value::Object(m)) => m.iter().map(|(k, q)| (Some(k.clone()), q, format!("questions.{k}"))).collect(),
        None | Some(Value::Null) => {
            return Err(bad("questions_required", format!("Send 1 to {MAX_QUESTIONS} questions."), "questions"))
        }
        Some(_) => {
            return Err(bad("invalid_questions", "questions must be an array or an object keyed by name.", "questions"))
        }
    };
    if list.is_empty() || list.len() > MAX_QUESTIONS {
        return Err(bad("invalid_question_count", format!("Send 1 to {MAX_QUESTIONS} questions."), "questions"));
    }
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(list.len());
    for (key, q, param) in list {
        let q = parse_question(key, q, &param)?;
        if !seen.insert(q.name.clone()) {
            return Err(bad("duplicate_question_name", format!("Question name '{}' is used twice.", q.name), &format!("{param}.name")));
        }
        out.push(q);
    }
    Ok(out)
}

fn parse_question(key: Option<String>, q: &Value, param: &str) -> Result<Question, PlatformError> {
    let o = q.as_object().ok_or_else(|| bad("invalid_question", "Each question must be an object.", param))?;
    let name = match key {
        Some(k) => k,
        None => o
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| bad("invalid_question_name", "Each question needs a name.", &format!("{param}.name")))?,
    };
    if name.trim().is_empty() || name.len() > MAX_NAME {
        return Err(bad("invalid_question_name", format!("Question names are 1 to {MAX_NAME} characters."), &format!("{param}.name")));
    }
    let qtype = match o.get("type").and_then(Value::as_str) {
        Some("predicate") | Some("noul") => QType::Predicate,
        Some("choice") => QType::Choice,
        Some("score") => QType::Score,
        _ => {
            return Err(bad("invalid_question_type", "type must be predicate, choice or score.", &format!("{param}.type")))
        }
    };
    let instructions = match o.get("instructions") {
        Some(Value::String(s)) if !s.trim().is_empty() => s.clone(),
        _ => {
            return Err(bad("invalid_question_instructions", "Each question needs instructions.", &format!("{param}.instructions")))
        }
    };
    let options = match qtype {
        QType::Predicate => Vec::new(),
        QType::Choice => {
            let p = format!("{param}.choices");
            let opts = parse_options(o.get("choices").or_else(|| o.get("criteria")), "value", false, &p)?;
            if !(2..=MAX_CHOICES).contains(&opts.len()) {
                return Err(bad("invalid_choices", format!("A choice question needs 2 to {MAX_CHOICES} choices."), &p));
            }
            let mut seen = HashSet::new();
            for o in &opts {
                if o.value.trim().is_empty() || o.value.len() > MAX_OPTION_ID || o.value == NODE_ABSTAIN {
                    return Err(bad("invalid_choice_value", format!("Choice values are 1 to {MAX_OPTION_ID} characters."), &p));
                }
                if choice_text(o).len() > MAX_OPTION_TEXT {
                    return Err(bad("choice_too_long", format!("Choice '{}' is over {MAX_OPTION_TEXT} bytes.", o.value), &p));
                }
                if !seen.insert(o.value.as_str()) {
                    return Err(bad("duplicate_choice_value", format!("Choice '{}' is listed twice.", o.value), &p));
                }
            }
            opts
        }
        QType::Score => {
            let p = format!("{param}.levels");
            let opts = parse_options(o.get("levels").or_else(|| o.get("criteria")), "label", true, &p)?;
            if !(2..=MAX_LEVELS).contains(&opts.len()) {
                return Err(bad("invalid_levels", format!("A score question needs 2 to {MAX_LEVELS} levels."), &p));
            }
            for o in &opts {
                if o.value.trim().is_empty() {
                    return Err(bad("invalid_level_label", "Each level needs a label.", &p));
                }
                if level_text(o).len() > MAX_OPTION_TEXT {
                    return Err(bad("level_too_long", format!("Level '{}' is over {MAX_OPTION_TEXT} bytes.", o.value), &p));
                }
            }
            opts
        }
    };
    Ok(Question { name, qtype, instructions, options })
}

/// Choices/levels/criteria: an array of objects (`{value|label, description}`)
/// or strings, or (Google `criteria`) an object of name → description.
/// serde_json here does not keep object member order, so an object of levels
/// (`ordered`) must be keyed by level number; otherwise send an array.
fn parse_options(raw: Option<&Value>, key: &str, ordered: bool, param: &str) -> Result<Vec<Opt>, PlatformError> {
    let desc = |d: &Value| match d {
        Value::String(s) => Some(s.clone()),
        other => other.get("description").and_then(Value::as_str).map(str::to_string),
    };
    match raw {
        Some(Value::Array(a)) => a
            .iter()
            .enumerate()
            .map(|(i, e)| match e {
                Value::String(s) => Ok(Opt { value: s.clone(), description: None }),
                Value::Object(m) => {
                    let value = [key, "value", "label"]
                        .iter()
                        .find_map(|k| m.get(*k).and_then(Value::as_str))
                        .ok_or_else(|| bad("invalid_option", format!("Each entry needs '{key}'."), &format!("{param}[{i}]")))?;
                    Ok(Opt { value: value.to_string(), description: m.get("description").and_then(Value::as_str).map(str::to_string) })
                }
                _ => Err(bad("invalid_option", "Each entry must be an object or a string.", &format!("{param}[{i}]"))),
            })
            .collect(),
        Some(Value::Object(m)) => {
            let mut opts: Vec<Opt> = m.iter().map(|(k, d)| Opt { value: k.clone(), description: desc(d) }).collect();
            if ordered {
                let nums: Option<Vec<f64>> = opts.iter().map(|o| o.value.trim().parse::<f64>().ok()).collect();
                let Some(nums) = nums else {
                    return Err(bad(
                        "unordered_levels",
                        "Send levels as an array (low to high), or as an object keyed by level number.",
                        param,
                    ));
                };
                let mut idx: Vec<usize> = (0..opts.len()).collect();
                idx.sort_by(|a, b| nums[*a].partial_cmp(&nums[*b]).unwrap_or(std::cmp::Ordering::Equal));
                opts = idx.into_iter().map(|i| opts[i].clone()).collect();
            }
            Ok(opts)
        }
        _ => Err(bad("options_required", format!("This question type needs '{}'.", param.rsplit('.').next().unwrap_or(param)), param)),
    }
}

// ── mapping to and from the node ────────────────────────────────────────────

fn choice_text(o: &Opt) -> String {
    o.description.clone().filter(|d| !d.trim().is_empty()).unwrap_or_else(|| o.value.clone())
}

fn level_text(o: &Opt) -> String {
    match o.description.as_deref().filter(|d| !d.trim().is_empty()) {
        Some(d) => format!("{}: {d}", o.value),
        None => o.value.clone(),
    }
}

/// The node `kind` and `options` for one question.
pub fn node_options(q: &Question) -> (&'static str, Value) {
    match q.qtype {
        QType::Predicate => ("verify", json!([{ "id": "yes", "text": "yes" }, { "id": "no", "text": "no" }])),
        QType::Choice => (
            "choice",
            Value::Array(q.options.iter().map(|o| json!({ "id": o.value, "text": choice_text(o) })).collect()),
        ),
        QType::Score => (
            "score",
            Value::Array(q.options.iter().enumerate().map(|(i, o)| json!({ "id": i.to_string(), "text": level_text(o) })).collect()),
        ),
    }
}

/// Node `question`: only this question's own instructions. The shared
/// instructions live in [`node_context`] so the prompt prefix is the same for
/// every question of a request.
pub fn node_question(q: &Question) -> String {
    q.instructions.trim().to_string()
}

/// Node `context`: the shared instructions, a blank line, then the input text
/// (just the input when there are no shared instructions).
pub fn node_context(shared: &str, input: &Input) -> String {
    match shared.trim() {
        "" => input.text.clone(),
        shared => format!("{shared}\n\n{}", input.text),
    }
}

/// One node `POST /v1/decisions` body. `project` is always the platform
/// project; `backends` is never set (the node's configured chain runs).
pub fn node_body(req: &DecisionRequest, q: &Question, project_id: &str, budget_ms: u64) -> Value {
    let (kind, options) = node_options(q);
    json!({
        "context": node_context(&req.instructions, &req.input),
        "options": options,
        "kind": kind,
        "question": node_question(q),
        "allow_abstain": req.allow_abstain,
        "latency_budget_ms": budget_ms.clamp(1, MAX_BUDGET_MS),
        "project": project_id,
    })
}

fn round4(x: f64) -> f64 {
    (x * 10_000.0).round() / 10_000.0
}

fn argmax(ps: &[f64]) -> usize {
    ps.iter().enumerate().fold(0, |best, (i, p)| if *p > ps[best] { i } else { best })
}

pub fn refusal(name: &str, reason: &str) -> Value {
    json!({ "name": name, "type": "refusal", "reason": reason })
}

/// A node answer (`{choice, probs: {id: p}, confidence, abstained, backend}`)
/// → the public answer for `q`.
pub fn map_answer(q: &Question, node: &Value) -> Value {
    let backend = node.get("backend").and_then(Value::as_str).unwrap_or("none");
    if node.get("abstained").and_then(Value::as_bool).unwrap_or(false) {
        let mut r = refusal(&q.name, "abstained");
        r["backend"] = json!(backend);
        return r;
    }
    let probs = node.get("probs");
    let p = |id: &str| probs.and_then(|m| m.get(id)).and_then(Value::as_f64).unwrap_or(0.0).clamp(0.0, 1.0);
    match q.qtype {
        QType::Predicate => {
            let (yes, no) = (p("yes"), p("no"));
            json!({ "name": q.name, "type": "predicate", "probability": round4(yes), "confidence": round4(yes.max(no)), "backend": backend })
        }
        QType::Choice => {
            let ps: Vec<f64> = q.options.iter().map(|o| p(&o.value)).collect();
            let choice = node
                .get("choice")
                .and_then(Value::as_str)
                .filter(|c| q.options.iter().any(|o| o.value == *c))
                .map(str::to_string)
                .unwrap_or_else(|| q.options[argmax(&ps)].value.clone());
            let confidence = ps.iter().copied().fold(0.0, f64::max);
            json!({
                "name": q.name, "type": "choice", "choice": choice,
                "probabilities": q.options.iter().zip(&ps).map(|(o, p)| json!({ "value": o.value, "probability": round4(*p) })).collect::<Vec<_>>(),
                "confidence": round4(confidence), "backend": backend,
            })
        }
        QType::Score => {
            let n = q.options.len();
            let ps: Vec<f64> = (0..n).map(|i| p(&i.to_string())).collect();
            let total: f64 = ps.iter().sum();
            let score = if total > 0.0 && n > 1 {
                ps.iter().enumerate().map(|(i, p)| p * i as f64 / (n - 1) as f64).sum::<f64>() / total
            } else {
                0.0
            };
            let confidence = ps.iter().copied().fold(0.0, f64::max);
            json!({
                "name": q.name, "type": "score", "score": round4(score.clamp(0.0, 1.0)),
                "probabilities": q.options.iter().zip(&ps).enumerate()
                    .map(|(i, (o, p))| json!({ "value": i, "label": o.value, "probability": round4(*p) })).collect::<Vec<_>>(),
                "confidence": round4(confidence), "backend": backend,
            })
        }
    }
}

/// An outcome `label` in public terms → the node's option id.
pub fn node_label(q: &Question, label: &str) -> String {
    match q.qtype {
        QType::Predicate => match label.to_ascii_lowercase().as_str() {
            "yes" | "true" => "yes".into(),
            "no" | "false" => "no".into(),
            _ => label.to_string(),
        },
        QType::Choice => label.to_string(),
        QType::Score => q
            .options
            .iter()
            .position(|o| o.value == label)
            .map(|i| i.to_string())
            .unwrap_or_else(|| label.to_string()),
    }
}

/// Top-level `backend` (the one that answered the most; first seen wins a tie,
/// `none` when nothing answered) and the answered count.
pub fn summarize(answers: &[Value]) -> (String, usize) {
    let mut counts: Vec<(String, usize)> = Vec::new();
    let mut answered = 0;
    for a in answers.iter().filter(|a| a["type"] != "refusal") {
        answered += 1;
        let b = a["backend"].as_str().unwrap_or("none");
        match counts.iter_mut().find(|(n, _)| n == b) {
            Some((_, c)) => *c += 1,
            None => counts.push((b.to_string(), 1)),
        }
    }
    let backend = counts.iter().fold(None::<&(String, usize)>, |best, c| match best {
        Some(b) if b.1 >= c.1 => Some(b),
        _ => Some(c),
    });
    (backend.map(|b| b.0.clone()).unwrap_or_else(|| "none".into()), answered)
}

/// The node reports no token counts: characters / 4 over everything the model reads.
pub fn estimate_input_tokens(req: &DecisionRequest) -> u64 {
    let mut chars = req.input.text.chars().count() + req.instructions.chars().count();
    for q in &req.questions {
        chars += q.instructions.chars().count();
        chars += q.options.iter().map(|o| o.value.chars().count() + o.description.as_deref().map_or(0, |d| d.chars().count())).sum::<usize>();
    }
    (chars as u64).div_ceil(4)
}

// ── node calls ──────────────────────────────────────────────────────────────

/// What a decision needs from the forwarder: node URL, trusted-peer secret, client.
#[derive(Clone)]
struct Node {
    url: String,
    token: String,
    client: reqwest::Client,
}

impl Node {
    fn from_forward(fwd: &AgencyForward) -> Result<Node, PlatformError> {
        let token = fwd.access_token.clone().filter(|t| !t.is_empty()).ok_or_else(|| {
            PlatformError::service_unavailable("agency_not_configured", "Decisions are not configured on this server.")
        })?;
        Ok(Node { url: fwd.node_url.clone(), token, client: fwd.client.clone() })
    }

    /// Trusted-peer identity: the platform project as the node's user, `Host`
    /// pinned to the public name (as `agency_forward::forward` does).
    fn headers(&self, project_id: &str) -> reqwest::header::HeaderMap {
        use reqwest::header::{HeaderMap, HeaderValue, HOST};
        let mut h = HeaderMap::new();
        h.insert(HOST, HeaderValue::from_static(FORWARD_HOST));
        if let Ok(v) = HeaderValue::from_str(&format!("platform:{project_id}")) {
            h.insert("x-allternit-user-id", v);
        }
        if let Ok(v) = HeaderValue::from_str(&self.token) {
            h.insert("x-allternit-desktop-access-token", v);
        }
        h
    }

    async fn send(&self, method: reqwest::Method, path: &str, project_id: &str, body: &Value, timeout: Duration) -> Result<(StatusCode, Value), NodeErr> {
        let resp = self
            .client
            .request(method, format!("{}{path}", self.url))
            .headers(self.headers(project_id))
            .timeout(timeout)
            .json(body)
            .send()
            .await
            .map_err(NodeErr::from)?;
        let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
        let v = resp.json::<Value>().await.unwrap_or(Value::Null);
        Ok((status, v))
    }
}

enum NodeErr {
    Timeout,
    Other(String),
}

impl From<reqwest::Error> for NodeErr {
    fn from(e: reqwest::Error) -> Self {
        if e.is_timeout() {
            NodeErr::Timeout
        } else {
            NodeErr::Other(e.to_string())
        }
    }
}

fn node_message(v: &Value) -> String {
    v.pointer("/error/message")
        .or_else(|| v.get("message"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| "The decision runtime could not answer.".into())
}

/// The forwarder built from env once (the same values `lib.rs` gives the Agency forward).
fn default_forward() -> Arc<AgencyForward> {
    static FWD: OnceLock<Arc<AgencyForward>> = OnceLock::new();
    FWD.get_or_init(|| Arc::new(AgencyForward::from_env())).clone()
}

/// One node call per question, [`CONCURRENCY`] at a time, all inside one
/// budget. Returns `(answer, node_decision_id)` per question, in order.
async fn fan_out(node: &Node, project_id: &str, req: &DecisionRequest) -> Vec<(Value, Option<String>)> {
    let started = Instant::now();
    let budget = Duration::from_millis(req.budget_ms);
    let mut results: Vec<Option<(Value, Option<String>)>> = vec![None; req.questions.len()];
    // Collected first: a lazy `Map` over borrowed questions inside
    // `buffer_unordered` trips the "FnOnce is not general enough" HRTB error
    // once the handler future must be `Send` for axum.
    let futs: Vec<_> = req.questions.iter().enumerate().map(|(i, q)| async move {
        let left = budget.saturating_sub(started.elapsed());
        if left.is_zero() {
            return (i, (refusal(&q.name, "budget_exhausted"), None));
        }
        let body = node_body(req, q, project_id, left.as_millis() as u64);
        let res = node.send(reqwest::Method::POST, "/v1/decisions", project_id, &body, left).await;
        (i, answer_from(q, res))
    }).collect();
    let calls = stream::iter(futs).buffer_unordered(CONCURRENCY);
    let mut calls = Box::pin(calls);
    let _ = tokio::time::timeout(budget, async {
        while let Some((i, r)) = calls.next().await {
            results[i] = Some(r);
        }
    })
    .await;
    results
        .into_iter()
        .zip(&req.questions)
        .map(|(r, q)| r.unwrap_or_else(|| (refusal(&q.name, "budget_exhausted"), None)))
        .collect()
}

fn answer_from(q: &Question, res: Result<(StatusCode, Value), NodeErr>) -> (Value, Option<String>) {
    match res {
        Err(NodeErr::Timeout) => (refusal(&q.name, "budget_exhausted"), None),
        Err(NodeErr::Other(e)) => {
            tracing::warn!(error = %e, question = %q.name, "platform decisions: node unreachable");
            (refusal(&q.name, "unavailable"), None)
        }
        Ok((status, v)) if !status.is_success() => {
            tracing::warn!(%status, question = %q.name, message = %node_message(&v), "platform decisions: node refused");
            (refusal(&q.name, "unavailable"), None)
        }
        Ok((_, v)) => (map_answer(q, &v), v.get("id").and_then(Value::as_str).map(str::to_string)),
    }
}

// ── storage ─────────────────────────────────────────────────────────────────

const COLUMNS: &str = "id, questions, answers, backend, latency_ms, usage, metadata, outcomes, created_at";

#[derive(Debug, FromRow)]
struct Row {
    id: String,
    questions: Value,
    answers: Value,
    backend: String,
    latency_ms: i32,
    usage: Value,
    metadata: Value,
    outcomes: Value,
    created_at: DateTime<Utc>,
}

/// The public `Decision` (node decision ids stripped).
fn decision_json(row: &Row) -> Value {
    let answers: Vec<Value> = row
        .answers
        .as_array()
        .map(|a| {
            a.iter()
                .map(|x| {
                    let mut x = x.clone();
                    if let Some(o) = x.as_object_mut() {
                        o.remove("node_decision_id");
                    }
                    x
                })
                .collect()
        })
        .unwrap_or_default();
    json!({
        "id": row.id,
        "object": "decision",
        "created_at": row.created_at,
        "answers": answers,
        "backend": row.backend,
        "latency_ms": row.latency_ms,
        "usage": row.usage,
        "outcomes": row.outcomes,
        "metadata": row.metadata,
    })
}

#[allow(clippy::too_many_arguments)]
async fn insert(
    db: &PgPool,
    id: &str,
    caller: &PlatformCaller,
    questions: Value,
    answers: Value,
    backend: &str,
    latency_ms: i32,
    usage: Value,
    metadata: Value,
) -> Result<Row, PlatformError> {
    Ok(sqlx::query_as::<_, Row>(&format!(
        "INSERT INTO platform_decisions (id, project_id, account_id, questions, answers, backend, latency_ms, usage, metadata) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING {COLUMNS}"
    ))
    .bind(id)
    .bind(&caller.project_id)
    .bind(&caller.account_id)
    .bind(questions)
    .bind(answers)
    .bind(backend)
    .bind(latency_ms)
    .bind(usage)
    .bind(metadata)
    .fetch_one(db)
    .await?)
}

fn not_found() -> PlatformError {
    PlatformError::not_found("decision_not_found", "No such decision.")
}

async fn load(db: &PgPool, caller: &PlatformCaller, id: &str) -> Result<Row, PlatformError> {
    let account = caller.account_filter(None)?;
    sqlx::query_as::<_, Row>(&format!(
        "SELECT {COLUMNS} FROM platform_decisions WHERE id = $1 AND project_id = $2 AND ($3::text IS NULL OR account_id = $3)"
    ))
    .bind(id)
    .bind(&caller.project_id)
    .bind(&account)
    .fetch_optional(db)
    .await?
    .ok_or_else(not_found)
}

/// One `decision_questions` per answered question; live projects only.
async fn meter(db: &PgPool, caller: &PlatformCaller, id: &str, answered: usize) {
    if caller.project_env != ProjectEnv::Live || answered == 0 {
        return;
    }
    let event = UsageEvent {
        project_id: caller.project_id.clone(),
        account_id: caller.account_id.clone(),
        key_id: Some(caller.key_id.clone()),
        meter: METER.to_string(),
        quantity: answered as f64,
        unit: Some("question".to_string()),
        ref_id: Some(id.to_string()),
        idempotency: Some(format!("decision:{id}")),
    };
    // `record_usage_priced` runs `billing::after_usage` (spend thresholds) on a new row.
    if let Err(e) = record_usage_priced(db, event, None, None).await {
        tracing::warn!(project = %caller.project_id, decision = %id, "platform decisions: usage not recorded: {}", e.message);
    }
}

// ── handlers ────────────────────────────────────────────────────────────────

async fn create(
    State(state): State<Arc<ApiState>>,
    fwd: Option<Extension<Arc<AgencyForward>>>,
    caller: PlatformCaller,
    ApiJson(body): ApiJson<Value>,
) -> Result<(StatusCode, Json<Value>), PlatformError> {
    let fwd = fwd.map(|Extension(f)| f).unwrap_or_else(default_forward);
    decide(&state, &fwd, &caller, body).await.map(|v| (StatusCode::CREATED, Json(v)))
}

/// `POST /v1/decisions` for the MCP edge tool `decide`: the same path (scope,
/// validation, spend check, fan-out, store, meter), returning the decision JSON.
pub async fn decide_for_mcp(state: &ApiState, caller: &PlatformCaller, body: Value) -> Result<Value, PlatformError> {
    decide(state, &default_forward(), caller, body).await
}

async fn decide(state: &ApiState, fwd: &AgencyForward, caller: &PlatformCaller, body: Value) -> Result<Value, PlatformError> {
    caller.require(SCOPE)?;
    let req = if is_agency_body(&body) { None } else { Some(normalize(&body)?) };
    let node = Node::from_forward(fwd)?;
    super::spend_allowed(&state.db, &caller.project_id).await?;
    match req {
        Some(req) => decide_questions(&state.db, &node, caller, req).await,
        None => decide_agency(&state.db, &node, caller, body).await,
    }
}

async fn decide_questions(db: &PgPool, node: &Node, caller: &PlatformCaller, req: DecisionRequest) -> Result<Value, PlatformError> {
    let started = Instant::now();
    let results = fan_out(node, &caller.project_id, &req).await;
    let latency_ms = started.elapsed().as_millis().min(i32::MAX as u128) as i32;
    let stored: Vec<Value> = results
        .iter()
        .map(|(a, nid)| {
            let mut a = a.clone();
            if let Some(nid) = nid {
                a["node_decision_id"] = json!(nid);
            }
            a
        })
        .collect();
    let public: Vec<Value> = results.into_iter().map(|(a, _)| a).collect();
    let (backend, answered) = summarize(&public);
    let usage = json!({ "questions": answered, "input_tokens": estimate_input_tokens(&req) });
    let questions = Value::Array(req.questions.iter().map(Question::to_json).collect());
    let id = new_id("dec_");
    let row = insert(db, &id, caller, questions, Value::Array(stored), &backend, latency_ms, usage, req.metadata.clone()).await?;
    meter(db, caller, &id, answered).await;
    Ok(decision_json(&row))
}

/// The one question an Agency body asks, as a choice over its options.
fn agency_question(body: &Value) -> Question {
    let options = body
        .get("options")
        .or_else(|| body.get("candidates"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|o| {
                    let id = o.get("id")?.as_str()?.to_string();
                    let text = o.get("text").or_else(|| o.get("label")).and_then(Value::as_str).map(str::to_string);
                    Some(Opt { value: id, description: text })
                })
                .collect()
        })
        .unwrap_or_default();
    let instructions = body
        .get("question")
        .and_then(Value::as_str)
        .or_else(|| body.get("kind").and_then(Value::as_str))
        .unwrap_or("choice")
        .to_string();
    Question { name: AGENCY_QUESTION.into(), qtype: QType::Choice, instructions, options }
}

/// The Agency body passes to the node (minus `backends`, with `project` set)
/// and its answer comes back in the Agency shape under our `dec_` id.
async fn decide_agency(db: &PgPool, node: &Node, caller: &PlatformCaller, mut body: Value) -> Result<Value, PlatformError> {
    let started = Instant::now();
    let obj = body
        .as_object_mut()
        .ok_or_else(|| bad("invalid_request_body", "The body must be a JSON object.", "body"))?;
    obj.remove("backends");
    obj.remove("project_id");
    obj.insert("project".into(), json!(caller.project_id));
    // Same input limits as the typed form: text only, at most MAX_TEXT_BYTES.
    let context_bytes = match obj.get("context") {
        Some(Value::String(s)) => s.len(),
        Some(Value::Object(c)) => {
            if c.get("image").is_some_and(|v| !v.is_null()) {
                return Err(image_not_supported("context.image"));
            }
            c.get("text").and_then(Value::as_str).map_or(0, str::len)
        }
        _ => 0,
    };
    if context_bytes > MAX_TEXT_BYTES {
        return Err(bad("input_too_large", format!("context text is over {MAX_TEXT_BYTES} bytes."), "context"));
    }
    let budget = obj
        .get("latency_budget_ms")
        .or_else(|| obj.get("max_latency_ms"))
        .and_then(Value::as_u64)
        .unwrap_or_else(|| default_budget_ms(context_bytes, 1))
        .clamp(1, MAX_BUDGET_MS);
    // The node keeps its own budget; ours adds room for the round trip.
    let res = node
        .send(reqwest::Method::POST, "/v1/decisions", &caller.project_id, &body, Duration::from_millis(budget) + Duration::from_secs(5))
        .await;
    let mut answer = match res {
        Ok((status, v)) if status.is_success() => v,
        Ok((status, v)) if status.is_client_error() => {
            return Err(PlatformError::invalid_request("invalid_decision", node_message(&v)))
        }
        Ok((status, v)) => {
            tracing::warn!(%status, message = %node_message(&v), "platform decisions: node refused agency body");
            return Err(PlatformError::service_unavailable("decision_unavailable", node_message(&v)));
        }
        Err(NodeErr::Timeout) => {
            return Err(PlatformError::service_unavailable("decision_unavailable", "No decision backend answered within the latency budget."))
        }
        Err(NodeErr::Other(e)) => {
            tracing::warn!(error = %e, "platform decisions: node unreachable");
            return Err(PlatformError::service_unavailable("decision_unavailable", "The decision runtime is unreachable."));
        }
    };
    let q = agency_question(&body);
    let public = map_answer(&q, &answer);
    let mut stored = public.clone();
    if let Some(nid) = answer.get("id").and_then(Value::as_str) {
        stored["node_decision_id"] = json!(nid);
    }
    let (backend, answered) = summarize(std::slice::from_ref(&public));
    let latency_ms = started.elapsed().as_millis().min(i32::MAX as u128) as i32;
    let input_chars = body.get("context").map_or(0, |c| match c {
        Value::String(s) => s.chars().count(),
        other => other.get("text").and_then(Value::as_str).map_or(0, |s| s.chars().count()),
    });
    let usage = json!({ "questions": answered, "input_tokens": (input_chars as u64).div_ceil(4) });
    let mut question = q.to_json();
    question["kind"] = body.get("kind").cloned().unwrap_or_else(|| json!("choice"));
    let id = new_id("dec_");
    insert(db, &id, caller, json!([question]), json!([stored]), &backend, latency_ms, usage, json!({})).await?;
    meter(db, caller, &id, answered).await;
    answer["id"] = json!(id);
    Ok(answer)
}

async fn fetch(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    Path(id): Path<String>,
) -> Result<Json<Value>, PlatformError> {
    caller.require(SCOPE)?;
    Ok(Json(decision_json(&load(&state.db, &caller, &id).await?)))
}

/// `{outcomes: {<name>: {status, label?, detail?}}}` checked against the
/// decision's question names; returns the cleaned outcomes.
pub fn parse_outcomes(body: &Value, questions: &[Question]) -> Result<Map<String, Value>, PlatformError> {
    let obj = body.as_object().ok_or_else(|| bad("invalid_request_body", "The body must be a JSON object.", "body"))?;
    if let Some(k) = obj.keys().find(|k| *k != "outcomes") {
        return Err(bad("unknown_parameter", format!("Unknown parameter '{k}'."), k));
    }
    let outcomes = obj
        .get("outcomes")
        .and_then(Value::as_object)
        .filter(|m| !m.is_empty())
        .ok_or_else(|| bad("outcomes_required", "Send outcomes keyed by question name.", "outcomes"))?;
    let mut out = Map::new();
    for (name, o) in outcomes {
        let param = format!("outcomes.{name}");
        if !questions.iter().any(|q| &q.name == name) {
            return Err(bad("unknown_question", format!("This decision has no question '{name}'."), &param));
        }
        let status = o.get("status").and_then(Value::as_str).filter(|s| OUTCOME_STATUSES.contains(s)).ok_or_else(|| {
            bad("invalid_outcome_status", "status must be success, failure, error or skipped.", &format!("{param}.status"))
        })?;
        let mut clean = json!({ "status": status });
        for (field, max) in [("label", 128usize), ("detail", 4000usize)] {
            match o.get(field) {
                None | Some(Value::Null) => {}
                Some(Value::String(s)) if s.len() <= max => clean[field] = json!(s),
                Some(_) => {
                    return Err(bad("invalid_outcome", format!("{field} must be a string of at most {max} bytes."), &format!("{param}.{field}")))
                }
            }
        }
        out.insert(name.clone(), clean);
    }
    Ok(out)
}

async fn patch(
    State(state): State<Arc<ApiState>>,
    fwd: Option<Extension<Arc<AgencyForward>>>,
    caller: PlatformCaller,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<Value>,
) -> Result<Json<Value>, PlatformError> {
    caller.require(SCOPE)?;
    let row = load(&state.db, &caller, &id).await?;
    let questions: Vec<Question> = row.questions.as_array().map(|a| a.iter().filter_map(Question::from_stored).collect()).unwrap_or_default();
    let outcomes = parse_outcomes(&body, &questions)?;
    let account = caller.account_filter(None)?;
    let row = sqlx::query_as::<_, Row>(&format!(
        "UPDATE platform_decisions SET outcomes = outcomes || $4::jsonb \
         WHERE id = $1 AND project_id = $2 AND ($3::text IS NULL OR account_id = $3) RETURNING {COLUMNS}"
    ))
    .bind(&id)
    .bind(&caller.project_id)
    .bind(&account)
    .bind(Value::Object(outcomes.clone()))
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(not_found)?;
    let fwd = fwd.map(|Extension(f)| f).unwrap_or_else(default_forward);
    forward_outcomes(&fwd, &caller.project_id, &row, &questions, &outcomes);
    Ok(Json(decision_json(&row)))
}

/// Best effort: each outcome goes to the node's `PATCH /v1/decisions/{node id}`
/// in the background; failures are logged, never returned.
fn forward_outcomes(fwd: &AgencyForward, project_id: &str, row: &Row, questions: &[Question], outcomes: &Map<String, Value>) {
    let Ok(node) = Node::from_forward(fwd) else {
        tracing::warn!(decision = %row.id, "platform decisions: outcomes not forwarded (agency node not configured)");
        return;
    };
    let node_ids: HashMap<&str, &str> = row
        .answers
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| Some((x.get("name")?.as_str()?, x.get("node_decision_id")?.as_str()?)))
                .collect()
        })
        .unwrap_or_default();
    for (name, o) in outcomes {
        let (Some(nid), Some(q)) = (node_ids.get(name.as_str()), questions.iter().find(|q| &q.name == name)) else {
            continue;
        };
        let mut body = o.clone();
        if let Some(label) = o.get("label").and_then(Value::as_str) {
            body["label"] = json!(node_label(q, label));
        }
        let (node, project_id, nid, decision) = (node.clone(), project_id.to_string(), nid.to_string(), row.id.clone());
        tokio::spawn(async move {
            let path = format!("/v1/decisions/{nid}");
            match node.send(reqwest::Method::PATCH, &path, &project_id, &body, OUTCOME_FORWARD_TIMEOUT).await {
                Ok((status, _)) if status.is_success() => {}
                Ok((status, v)) => tracing::warn!(%decision, %status, message = %node_message(&v), "platform decisions: outcome not recorded on node"),
                Err(NodeErr::Timeout) => tracing::warn!(%decision, "platform decisions: outcome forward timed out"),
                Err(NodeErr::Other(e)) => tracing::warn!(%decision, error = %e, "platform decisions: outcome forward failed"),
            }
        });
    }
}

#[cfg(test)]
#[path = "decisions_tests.rs"]
mod tests;
