//! Optional vendor decision backends (decision-runtime spec §2.1 tier 3,
//! phase E6). Both are **off by default** and gated twice:
//!
//! 1. the key is configured (`ALLTERNIT_DECISIONS_VENDOR_KEY`,
//!    `ALLTERNIT_DECISIONS_TYPESAFE_KEY`, injected like every other runtime
//!    secret; never logged, never echoed in errors), and
//! 2. the decision's `project` is enabled for that backend in
//!    `ALLTERNIT_DECISIONS_VENDOR_PROJECTS` (`{"vendor": ["proj_a"],
//!    "typesafe": ["*"]}`; `*` = every decision). No entry, no vendor call,
//!    even when a request names the backend in `backends`.
//!
//! Wire formats follow the vendors' published docs, checked 2026-10-09:
//!
//! - `vendor`: OpenAI Decisions API, public beta. `POST
//!   https://api.openai.com/v1/decisions`, `{model, input, questions:[{type:
//!   "choice", name, instructions, choices:[{value, description}]}]}` →
//!   `answers[].{choice, probabilities:[{value, probability}], confidence}` or
//!   `type: "refusal"`. Images as `input_image` parts with a base64 data URL.
//!   Default model `gpt-6-luna` (the only one the docs list).
//! - `typesafe`: TypeSafe Jev. `POST https://api.typesafe.ai/v1/systemone`,
//!   `{model, state, questions:{name:{type:"choice", instructions, criteria:
//!   {key: description}}}}` → `answers.name.{choice, probabilities:{key: p},
//!   confidence}`. Default model `jev-latest`. Its docs show no image input,
//!   so it is blind to images here.
//!
//! Neither was called live from here: a paid account needs Eoj's OK.

use super::backends::{Answer, DecisionBackend, Query};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

pub const VENDOR_BACKENDS: &[&str] = &["vendor", "typesafe"];

/// Whether `backend` may run for a decision in `project`. Non-vendor
/// backends always may.
pub fn allowed(backend: &str, project: Option<&str>) -> bool {
    allowed_in(env("ALLTERNIT_DECISIONS_VENDOR_PROJECTS").as_deref(), backend, project)
}

/// `allowed` against an explicit `ALLTERNIT_DECISIONS_VENDOR_PROJECTS` value.
pub fn allowed_in(config: Option<&str>, backend: &str, project: Option<&str>) -> bool {
    if !VENDOR_BACKENDS.contains(&backend) {
        return true;
    }
    let cfg: HashMap<String, Vec<String>> = config.and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default();
    cfg.get(backend).is_some_and(|ps| ps.iter().any(|p| p == "*" || Some(p.as_str()) == project))
}

fn client() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .pool_idle_timeout(Duration::from_secs(90))
            .tcp_nodelay(true)
            .build()
            .unwrap_or_default()
    })
}

fn instructions(q: &Query<'_>) -> String {
    q.question.map(str::to_string).unwrap_or_else(|| format!("Decision kind: {}. Choose the best option.", q.kind))
}

/// POST with the bearer key; the key never reaches an error string.
async fn post(url: &str, key: &str, body: &Value, who: &str) -> Result<Value, String> {
    let resp = client().post(url).bearer_auth(key).json(body).send().await.map_err(|e| format!("{who} unreachable: {}", e.without_url()))?;
    let status = resp.status();
    let v: Value = resp.json().await.map_err(|e| format!("{who} reply: {}", e.without_url()))?;
    if !status.is_success() {
        let msg = v["error"]["message"].as_str().or(v["error"].as_str()).or(v["message"].as_str()).unwrap_or("");
        return Err(format!("{who} {status}: {}", msg.chars().take(200).collect::<String>()));
    }
    Ok(v)
}

// ── OpenAI Decisions API ────────────────────────────────────────────────────

pub struct OpenAiDecisions;

impl OpenAiDecisions {
    pub const KEY_ENV: &'static str = "ALLTERNIT_DECISIONS_VENDOR_KEY";

    pub fn request(q: &Query<'_>, model: &str) -> Value {
        let input = match q.image {
            Some(img) => json!([{ "role": "user", "content": [
                { "type": "input_text", "text": q.context },
                { "type": "input_image", "image_url": img },
            ]}]),
            None => json!(q.context),
        };
        json!({
            "model": model,
            "input": input,
            "questions": [{
                "type": "choice",
                "name": "decision",
                "instructions": instructions(q),
                "choices": q.ids.iter().zip(&q.texts).map(|(id, t)| json!({ "value": id, "description": t })).collect::<Vec<_>>(),
            }],
        })
    }

    pub fn parse(v: &Value, ids: &[&str]) -> Result<Answer, String> {
        let a = v["answers"].as_array().and_then(|a| a.first()).ok_or("vendor reply has no answers")?;
        if a["type"] == "refusal" {
            return Ok(Answer { probs: vec![0.0; ids.len()], abstain: true, detail: json!({ "refusal": true, "model": v["model"] }) });
        }
        let mut probs = vec![0.0; ids.len()];
        for p in a["probabilities"].as_array().ok_or("vendor reply has no probabilities")? {
            if let (Some(val), Some(pr)) = (p["value"].as_str(), p["probability"].as_f64()) {
                if let Some(i) = ids.iter().position(|id| *id == val) {
                    probs[i] = pr;
                }
            }
        }
        if probs.iter().all(|p| *p == 0.0) {
            return Err("vendor answered outside the options".into());
        }
        Ok(Answer { probs, abstain: false, detail: json!({ "model": v["model"], "vendor_confidence": a["confidence"], "usage": v["usage"] }) })
    }
}

#[async_trait]
impl DecisionBackend for OpenAiDecisions {
    fn name(&self) -> &'static str {
        "vendor"
    }
    fn vision(&self) -> bool {
        true
    }
    fn enabled(&self) -> bool {
        env(Self::KEY_ENV).is_some()
    }
    async fn decide(&self, q: &Query<'_>) -> Result<Answer, String> {
        let key = env(Self::KEY_ENV).ok_or("vendor decisions key not configured")?;
        let url = env("ALLTERNIT_DECISIONS_VENDOR_URL").unwrap_or_else(|| "https://api.openai.com/v1/decisions".into());
        let model = env("ALLTERNIT_DECISIONS_VENDOR_MODEL").unwrap_or_else(|| "gpt-6-luna".into());
        let v = post(&url, &key, &Self::request(q, &model), "vendor decisions").await?;
        Self::parse(&v, &q.ids)
    }
}

// ── TypeSafe Jev ────────────────────────────────────────────────────────────

pub struct TypeSafeJev;

impl TypeSafeJev {
    pub const KEY_ENV: &'static str = "ALLTERNIT_DECISIONS_TYPESAFE_KEY";

    pub fn request(q: &Query<'_>, model: &str) -> Value {
        let criteria: serde_json::Map<String, Value> = q.ids.iter().zip(&q.texts).map(|(id, t)| (id.to_string(), json!(t))).collect();
        json!({
            "model": model,
            "state": q.context,
            "questions": { "decision": { "type": "choice", "instructions": instructions(q), "criteria": criteria } },
        })
    }

    pub fn parse(v: &Value, ids: &[&str]) -> Result<Answer, String> {
        let a = &v["answers"]["decision"];
        let p = a["probabilities"].as_object().ok_or("typesafe reply has no probabilities")?;
        let probs: Vec<f64> = ids.iter().map(|id| p.get(*id).and_then(Value::as_f64).unwrap_or(0.0)).collect();
        if probs.iter().all(|p| *p == 0.0) {
            return Err("typesafe answered outside the options".into());
        }
        Ok(Answer { probs, abstain: false, detail: json!({ "model": v["model"], "vendor_confidence": a["confidence"], "usage": v["usage"] }) })
    }
}

#[async_trait]
impl DecisionBackend for TypeSafeJev {
    fn name(&self) -> &'static str {
        "typesafe"
    }
    fn vision(&self) -> bool {
        false
    }
    fn enabled(&self) -> bool {
        env(Self::KEY_ENV).is_some()
    }
    async fn decide(&self, q: &Query<'_>) -> Result<Answer, String> {
        let key = env(Self::KEY_ENV).ok_or("typesafe key not configured")?;
        let url = env("ALLTERNIT_DECISIONS_TYPESAFE_URL").unwrap_or_else(|| "https://api.typesafe.ai/v1/systemone".into());
        let model = env("ALLTERNIT_DECISIONS_TYPESAFE_MODEL").unwrap_or_else(|| "jev-latest".into());
        let v = post(&url, &key, &Self::request(q, &model), "typesafe").await?;
        Self::parse(&v, &q.ids)
    }
}
