//! Decision backends behind one interface. The router (`super::decide`) walks
//! them in chain order and escalates while confidence is under the kind's
//! threshold.
//!
//! - `local`: the owned scorer sidecar (`domains/decision-runtime`): one-pass
//!   option scoring on MLX (Apple silicon) or llama.cpp (everything else).
//! - `vendor` (OpenAI Decisions API), `typesafe` (TypeSafe Jev): vendor fast paths. Config-only stubs, off unless a
//!   key is set, and even then they answer "not available" until E6 verifies
//!   the vendor's docs and access.
//! - `oracle`: the planner model through gizzi (allternit-api never calls a
//!   provider directly). The escalation path and the label source.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::OnceLock;
use std::time::Duration;

/// One decision as the backends see it.
pub struct Query<'a> {
    pub kind: &'a str,
    pub context: &'a str,
    pub question: Option<&'a str>,
    pub image: Option<&'a str>,
    pub ids: Vec<&'a str>,
    pub texts: Vec<&'a str>,
    pub allow_abstain: bool,
}

/// A backend's answer: one probability per option, in option order.
pub struct Answer {
    pub probs: Vec<f64>,
    /// The backend itself declined to answer (the oracle may).
    pub abstain: bool,
    pub detail: Value,
}

#[async_trait]
pub trait DecisionBackend: Send + Sync {
    fn name(&self) -> &'static str;
    /// Whether it can see `Query::image`. Backends that cannot are skipped
    /// for decisions that carry an image: deciding about a screenshot without
    /// seeing it is not a decision.
    fn vision(&self) -> bool;
    /// Configured and allowed to run.
    fn enabled(&self) -> bool;
    async fn decide(&self, q: &Query<'_>) -> Result<Answer, String>;
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

// ── local scorer sidecar ────────────────────────────────────────────────────

pub struct LocalScorer;

impl LocalScorer {
    fn url() -> Option<String> {
        env("ALLTERNIT_DECISIONS_LOCAL_URL").map(|u| u.trim_end_matches('/').to_string())
    }
    fn client() -> &'static reqwest::Client {
        static C: OnceLock<reqwest::Client> = OnceLock::new();
        C.get_or_init(|| {
            reqwest::Client::builder()
                .no_proxy()
                .connect_timeout(Duration::from_millis(500))
                .pool_idle_timeout(Duration::from_secs(90))
                .tcp_nodelay(true)
                .build()
                .unwrap_or_default()
        })
    }
}

#[async_trait]
impl DecisionBackend for LocalScorer {
    fn name(&self) -> &'static str {
        "local"
    }
    fn vision(&self) -> bool {
        false
    }
    fn enabled(&self) -> bool {
        Self::url().is_some()
    }
    async fn decide(&self, q: &Query<'_>) -> Result<Answer, String> {
        let url = Self::url().ok_or("local scorer not configured")?;
        let mut rq = Self::client()
            .post(format!("{url}/v1/score"))
            .json(&json!({ "context": q.context, "options": q.texts, "kind": q.kind, "question": q.question }));
        if let Some(t) = env("ALLTERNIT_DECISIONS_TOKEN") {
            rq = rq.bearer_auth(t);
        }
        let resp = rq.send().await.map_err(|e| format!("local scorer unreachable: {e}"))?;
        let status = resp.status();
        let body: Value = resp.json().await.map_err(|e| format!("local scorer reply: {e}"))?;
        if status.as_u16() == 503 {
            // First use: the sidecar is installing, downloading or loading.
            let pct = (body["progress"].as_f64().unwrap_or(0.0) * 100.0).round();
            return Err(format!("local scorer {} ({pct}%)", body["status"].as_str().unwrap_or("preparing")));
        }
        if !status.is_success() {
            return Err(format!("local scorer {status}: {}", body["error"].as_str().unwrap_or("")));
        }
        let probs: Vec<f64> = body["probs"].as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
        if probs.len() != q.ids.len() {
            return Err("local scorer returned the wrong number of probabilities".into());
        }
        Ok(Answer {
            probs,
            abstain: false,
            detail: json!({ "engine": body["engine"], "model": body["model"], "timing": body["timing"] }),
        })
    }
}

// ── vendor stubs ────────────────────────────────────────────────────────────

/// A vendor decision API, wired for config only. Enabled when its key is set;
/// it still answers "not available" until E6 (vendor docs and access
/// verified, Eoj's OK on the paid account) replaces the stub.
pub struct VendorStub {
    pub name: &'static str,
    pub key_env: &'static str,
}

#[async_trait]
impl DecisionBackend for VendorStub {
    fn name(&self) -> &'static str {
        self.name
    }
    fn vision(&self) -> bool {
        true
    }
    fn enabled(&self) -> bool {
        env(self.key_env).is_some()
    }
    async fn decide(&self, _q: &Query<'_>) -> Result<Answer, String> {
        Err(format!("{} decisions adapter is a stub until E6", self.name))
    }
}

// ── oracle (planner model via gizzi) ────────────────────────────────────────

pub struct Oracle;

pub const ABSTAIN: &str = "__abstain__";

impl Oracle {
    fn model() -> Option<(String, String)> {
        let m = env("ALLTERNIT_DECISIONS_ORACLE_MODEL")?;
        let (p, id) = m.split_once('/')?;
        Some((p.to_string(), id.to_string()))
    }

    fn prompt(q: &Query<'_>) -> String {
        let mut s = format!("Context:\n{}\n\nOptions:\n", q.context.trim());
        for (id, text) in q.ids.iter().zip(&q.texts) {
            s.push_str(&format!("- {id}: {}\n", text.split_whitespace().collect::<Vec<_>>().join(" ")));
        }
        let question = q.question.map(str::to_string).unwrap_or_else(|| format!("Decision kind: {}. Choose the best option.", q.kind));
        s.push_str(&format!("\nQuestion: {question}\n"));
        if q.allow_abstain {
            s.push_str(&format!("If no option is clearly right, answer \"{ABSTAIN}\".\n"));
        }
        s.push_str("Answer with the id of exactly one option.");
        s
    }
}

#[async_trait]
impl DecisionBackend for Oracle {
    fn name(&self) -> &'static str {
        "oracle"
    }
    fn vision(&self) -> bool {
        true
    }
    fn enabled(&self) -> bool {
        env("ALLTERNIT_DECISIONS_ORACLE").as_deref() != Some("0")
    }
    async fn decide(&self, q: &Query<'_>) -> Result<Answer, String> {
        let mut ids: Vec<&str> = q.ids.clone();
        if q.allow_abstain {
            ids.push(ABSTAIN);
        }
        let schema = json!({
            "type": "object",
            "properties": { "choice": { "type": "string", "enum": ids } },
            "required": ["choice"],
            "additionalProperties": false,
        });
        let images: Vec<String> = q.image.map(|i| vec![i.to_string()]).unwrap_or_default();
        let system = "You are the oracle of a decision runtime. Pick exactly one of the given option ids. Never invent an option.";
        let model = Self::model();
        let (text, usage) = crate::gizzi_completion::complete_decision(&Self::prompt(q), &images, Some(system), model.as_ref(), &schema).await?;
        let choice = parse_choice(&text, &ids).ok_or_else(|| format!("oracle answered outside the options: {}", truncate(&text, 120)))?;
        let probs = q.ids.iter().map(|id| if *id == choice { 1.0 } else { 0.0 }).collect();
        Ok(Answer {
            probs,
            abstain: choice == ABSTAIN,
            detail: json!({ "model": model.map(|(p, m)| format!("{p}/{m}")), "tokens": usage.tokens, "cost_usd": usage.cost_usd }),
        })
    }
}

/// The option id in a structured (`{"choice": ...}`) or plain-text reply.
pub fn parse_choice<'a>(text: &str, ids: &[&'a str]) -> Option<&'a str> {
    let t = text.trim();
    let from_json = |v: &Value| v["choice"].as_str().map(str::to_string);
    let raw = serde_json::from_str::<Value>(t).ok().and_then(|v| from_json(&v)).or_else(|| {
        let (a, b) = (t.find('{')?, t.rfind('}')?);
        serde_json::from_str::<Value>(&t[a..=b]).ok().and_then(|v| from_json(&v))
    });
    let raw = raw.unwrap_or_else(|| t.trim_matches(|c: char| c == '"' || c == '`' || c.is_whitespace()).to_string());
    ids.iter().copied().find(|id| *id == raw)
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// The configured chain, in escalation order.
pub fn chain() -> Vec<Box<dyn DecisionBackend>> {
    let order = env("ALLTERNIT_DECISIONS_CHAIN").unwrap_or_else(|| "local,vendor,typesafe,oracle".into());
    order
        .split(',')
        .filter_map(|n| backend(n.trim()))
        .collect()
}

pub fn backend(name: &str) -> Option<Box<dyn DecisionBackend>> {
    Some(match name {
        "local" => Box::new(LocalScorer),
        // OpenAI's Decisions API (DevDay 2026 preview).
        "vendor" => Box::new(VendorStub { name: "vendor", key_env: "ALLTERNIT_DECISIONS_VENDOR_KEY" }),
        "typesafe" => Box::new(VendorStub { name: "typesafe", key_env: "ALLTERNIT_DECISIONS_TYPESAFE_KEY" }),
        "oracle" => Box::new(Oracle),
        _ => return None,
    })
}
