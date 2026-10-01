//! Ground-truth reporters for the S1 calibration pipeline (Q22). When a
//! deterministic verifier/parser settles a fact an S1 decision predicted, the
//! outcome is POSTed to the decision runtime's `/v1/decision/outcome` so the
//! shadow log can be joined into calibration data. Fire-and-forget: a failed or
//! unreachable POST never affects a run; it is logged at debug level and dropped.
//!
//! Join key: the runtime returns `x-decision_id` in `DecisionResultV1.extensions`.
//! The S1 path records it as an evidence ref `s1-verify:<id>` (completion
//! GATE/VERIFY decisions; truth "true"/"false") on the node's receipt.

use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::judge::types::{NodeOutcome, ToolDecision};

/// Shadow-ledger ids for the S1 permission GATE (shared with the TS client).
pub const PERMISSION_GATE_BANK: &str = "bank.permission_gate";
pub const PERMISSION_GATE_QUESTION: &str = "may_proceed";

/// Q26 tighten-only combinator: the result is never looser than the incumbent,
/// so an S1 `Allow` can never turn `Ask`/`Deny` into `Allow`.
pub fn tighten(incumbent: ToolDecision, s1: Option<ToolDecision>) -> ToolDecision {
    let rank = |d: ToolDecision| match d { ToolDecision::Allow => 0, ToolDecision::Ask => 1, ToolDecision::Deny => 2 };
    match s1 {
        Some(r) if rank(r) > rank(incumbent) => r,
        _ => incumbent,
    }
}

/// One shadow GATE question for `/v1/decision`.
#[derive(Debug, Clone)]
pub struct GateAsk<'a> {
    pub producer: &'a str,
    pub bank: &'a str,
    pub primitive_id: &'a str,
    pub question_id: &'a str,
    /// "GATE" or "CONFIDENCE_GATE".
    pub motif: &'a str,
    pub instructions: &'a str,
    pub subject_ref: Option<&'a str>,
    pub extensions: Map<String, Value>,
}

/// What came back from a GATE: the shadow-ledger id and P(true).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GateReadout {
    pub decision_id: Option<String>,
    pub p_true: Option<f64>,
}

pub const DEFAULT_RUNTIME_URL: &str = "http://127.0.0.1:7717";
pub const VERIFY_REF_PREFIX: &str = "s1-verify:";

/// `x-decision_id` from a decision result's extensions.
pub fn decision_id(ext: &Option<Map<String, Value>>) -> Option<String> {
    ext.as_ref()?.get("x-decision_id")?.as_str().map(str::to_owned)
}

/// Evidence ref that carries the decision id for a completion (GATE/VERIFY) decision.
pub fn verify_evidence_ref(ext: &Option<Map<String, Value>>) -> Option<String> {
    decision_id(ext).map(|id| format!("{VERIFY_REF_PREFIX}{id}"))
}

#[derive(Debug, Clone)]
pub struct OutcomeReporter {
    pub base_url: String,
    pub token: Option<String>,
    pub timeout: Duration,
    pub enabled: bool,
}

impl OutcomeReporter {
    /// URL from `ALLTERNIT_S1_URL` / `SYSTEM_ONE_URL` (default 127.0.0.1:7717);
    /// `ALLTERNIT_S1_OUTCOMES=0` disables reporting.
    pub fn from_env() -> Self {
        let url = std::env::var("ALLTERNIT_S1_URL")
            .or_else(|_| std::env::var("SYSTEM_ONE_URL"))
            .unwrap_or_else(|_| DEFAULT_RUNTIME_URL.to_string());
        Self {
            base_url: url.trim_end_matches('/').to_string(),
            token: std::env::var("SYSTEM_ONE_TOKEN").ok().filter(|t| !t.is_empty()),
            timeout: Duration::from_millis(1500),
            enabled: std::env::var("ALLTERNIT_S1_OUTCOMES").map(|v| v != "0").unwrap_or(true),
        }
    }

    /// POST one outcome. Returns whether the runtime accepted it; never errors.
    pub async fn report(&self, decision_id: &str, truth: &str, source: &str) -> bool {
        if !self.enabled || decision_id.is_empty() {
            return false;
        }
        let client = match reqwest::Client::builder().timeout(self.timeout).build() {
            Ok(c) => c,
            Err(_) => return false,
        };
        let mut rb = client
            .post(format!("{}/v1/decision/outcome", self.base_url))
            .header("content-type", "application/json")
            .body(json!({ "decision_id": decision_id, "truth": truth, "source": source }).to_string());
        if let Some(t) = &self.token {
            rb = rb.bearer_auth(t);
        }
        match rb.send().await {
            Ok(r) if r.status().is_success() => true,
            Ok(r) => {
                tracing::debug!(status = %r.status(), "s1 outcome rejected; ignored");
                false
            }
            Err(e) => {
                tracing::debug!(error = %e, "s1 runtime unreachable; outcome dropped");
                false
            }
        }
    }

    /// POST one GATE to `/v1/decision` (backend `ALLTERNIT_S1_BACKEND`, default `auto`).
    /// `None` on any failure; never errors.
    pub async fn gate(&self, ask: &GateAsk<'_>, state: &str) -> Option<GateReadout> {
        if !self.enabled {
            return None;
        }
        let client = reqwest::Client::builder().timeout(self.timeout).build().ok()?;
        let mut ext = ask.extensions.clone();
        ext.insert("x-motif".into(), json!(ask.motif));
        ext.insert("x-primitive_id".into(), json!(ask.primitive_id));
        if let Some(s) = ask.subject_ref {
            ext.insert("x-subject_ref".into(), json!(s));
        }
        let now = chrono::Utc::now().to_rfc3339();
        let request = json!({
            "envelope": {"abi_version": "1.0.0", "schema_id": "allternit.kernel.DecisionRequestV1", "schema_version": "1.0.0",
                "run_id": ask.producer, "session_id": ask.producer, "task_id": ask.producer, "state_version": 0,
                "created_at": now, "producer": ask.producer, "trace_id": ask.producer, "provenance": []},
            "operation": "GATE", "state_projection_ref": format!("state.{}", ask.producer),
            "decision_bank_id": ask.bank, "question_id": ask.question_id, "instructions": ask.instructions,
            "latency_class": "INTERACTIVE", "extensions": ext,
        });
        let backend = std::env::var("ALLTERNIT_S1_BACKEND").unwrap_or_else(|_| "auto".into());
        let mut rb = client
            .post(format!("{}/v1/decision", self.base_url))
            .header("content-type", "application/json")
            .body(json!({ "request": request, "state": state, "backend": backend }).to_string());
        if let Some(t) = &self.token {
            rb = rb.bearer_auth(t);
        }
        let r = rb.send().await.ok()?;
        if !r.status().is_success() {
            return None;
        }
        let v: Value = r.json().await.ok()?;
        Some(GateReadout {
            decision_id: v.get("extensions").and_then(|e| e.get("x-decision_id")).and_then(Value::as_str).map(str::to_owned),
            p_true: v.get("probabilities").and_then(|p| p.get("true")).and_then(Value::as_f64),
        })
    }

    /// Detached shadow GATE (never awaited, never changes the caller's decision).
    /// Off under `cfg(test)` so unit tests never write to a real local ledger.
    pub fn spawn_gate(&self, ask: GateAsk<'static>, state: String) {
        if !self.enabled || cfg!(test) {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else { return };
        let me = self.clone();
        handle.spawn(async move {
            me.gate(&ask, &state).await;
        });
    }

    /// Detached: returns immediately, the run never waits on or sees the result.
    pub fn spawn_report(&self, decision_id: String, truth: String, source: String) {
        if !self.enabled {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else { return };
        let me = self.clone();
        handle.spawn(async move {
            me.report(&decision_id, &truth, &source).await;
        });
    }
}

/// (a) A verifier/judge CompletionDecision for a node whose plan used S1 completion
/// decisions: report `true`/`false` for each recorded `s1-verify:<id>` evidence ref.
/// NeedsHuman is not ground truth and reports nothing.
pub fn report_completion(reporter: &OutcomeReporter, evidence_refs: &[String], outcome: NodeOutcome, source: &str) {
    let truth = match outcome {
        NodeOutcome::Accomplished => "true",
        NodeOutcome::NotAccomplished => "false",
        NodeOutcome::NeedsHuman => return,
    };
    for id in evidence_refs.iter().filter_map(|r| r.strip_prefix(VERIFY_REF_PREFIX)) {
        reporter.spawn_report(id.to_string(), truth.to_string(), source.to_string());
    }
}

/// (b) A deterministic S0 step (test run / parser) produced the error code an S1
/// CLASSIFY_ERROR decision predicted. Only a known class is ground truth; UNKNOWN is not.
pub fn report_classify_outcome(reporter: &OutcomeReporter, decision_id: &str, deterministic_code: &str, source: &str) {
    let class = super::bug_fix::error_class(deterministic_code);
    if class == super::bug_fix::error_ontology().unknown {
        return;
    }
    reporter.spawn_report(decision_id.to_string(), class, source.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn mock() -> (String, tokio::sync::mpsc::UnboundedReceiver<String>) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = l.accept().await else { return };
                let tx = tx.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let mut got = Vec::new();
                    loop {
                        let n = s.read(&mut buf).await.unwrap_or(0);
                        if n == 0 { break; }
                        got.extend_from_slice(&buf[..n]);
                        let txt = String::from_utf8_lossy(&got).to_string();
                        if let Some(i) = txt.find("\r\n\r\n") {
                            let len = txt.lines().find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
                            if got.len() >= i + 4 + len {
                                let _ = tx.send(format!("{}|{}", txt.lines().next().unwrap_or(""), &txt[i + 4..]));
                                break;
                            }
                        }
                    }
                    let _ = s.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\ncontent-type: application/json\r\n\r\n{}").await;
                });
            }
        });
        (url, rx)
    }

    fn reporter(url: &str) -> OutcomeReporter {
        OutcomeReporter { base_url: url.to_string(), token: None, timeout: Duration::from_millis(500), enabled: true }
    }

    #[tokio::test]
    async fn verifier_decision_posts_outcome_per_recorded_decision() {
        let (url, mut rx) = mock().await;
        let refs = vec!["receipt:1".to_string(), "s1-verify:dec-7".to_string()];
        report_completion(&reporter(&url), &refs, NodeOutcome::Accomplished, "verifier:judge");
        let got = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await.unwrap().unwrap();
        assert!(got.starts_with("POST /v1/decision/outcome"), "{got}");
        let body: Value = serde_json::from_str(got.split('|').nth(1).unwrap()).unwrap();
        assert_eq!(body, json!({"decision_id": "dec-7", "truth": "true", "source": "verifier:judge"}));

        report_completion(&reporter(&url), &refs, NodeOutcome::NotAccomplished, "verifier:judge");
        let got = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await.unwrap().unwrap();
        assert!(got.contains("\"truth\":\"false\""));

        // NeedsHuman is not ground truth; no refs means nothing to report.
        report_completion(&reporter(&url), &refs, NodeOutcome::NeedsHuman, "v");
        report_completion(&reporter(&url), &["x".into()], NodeOutcome::Accomplished, "v");
        assert!(tokio::time::timeout(Duration::from_millis(300), rx.recv()).await.is_err());
    }

    #[tokio::test]
    async fn classify_outcome_reports_known_class_only() {
        let (url, mut rx) = mock().await;
        report_classify_outcome(&reporter(&url), "dec-9", "ERR_INTERNAL", "parser:test");
        assert!(tokio::time::timeout(Duration::from_millis(300), rx.recv()).await.is_err());
        report_classify_outcome(&reporter(&url), "dec-9", "TEST_ASSERTION", "parser:test");
        let got = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await.unwrap().unwrap();
        assert!(got.contains("\"truth\":\"TEST_ASSERTION\"") && got.contains("dec-9"), "{got}");
    }

    #[test]
    fn tighten_never_loosens() {
        use ToolDecision::*;
        assert_eq!(tighten(Ask, Some(Allow)), Ask);
        assert_eq!(tighten(Deny, Some(Allow)), Deny);
        assert_eq!(tighten(Allow, Some(Ask)), Ask);
        assert_eq!(tighten(Deny, None), Deny);
        let rank = |d: ToolDecision| d as u8;
        for inc in [Allow, Ask, Deny] {
            for s1 in [None, Some(Allow), Some(Ask), Some(Deny)] {
                assert!(rank(tighten(inc, s1)) >= rank(inc));
            }
        }
    }

    #[tokio::test]
    async fn gate_posts_a_gate_request_with_bank_primitive_subject() {
        let (url, mut rx) = mock().await;
        let r = OutcomeReporter { base_url: url, token: None, timeout: Duration::from_secs(2), enabled: true };
        let ask = GateAsk {
            producer: "t", bank: PERMISSION_GATE_BANK, primitive_id: "permission.x", question_id: PERMISSION_GATE_QUESTION,
            motif: "GATE", instructions: "i", subject_ref: Some("s1"), extensions: Map::new(),
        };
        assert_eq!(r.gate(&ask, "state").await, Some(GateReadout::default()));
        let got = rx.recv().await.unwrap();
        assert!(got.starts_with("POST /v1/decision "), "{got}");
        let body: Value = serde_json::from_str(got.split_once('|').unwrap().1).unwrap();
        assert_eq!(body["request"]["operation"], "GATE");
        assert_eq!(body["request"]["decision_bank_id"], PERMISSION_GATE_BANK);
        assert_eq!(body["request"]["extensions"]["x-subject_ref"], "s1");
        assert_eq!(body["request"]["extensions"]["x-primitive_id"], "permission.x");
        assert_eq!(body["state"], "state");
    }

    #[tokio::test]
    async fn unreachable_or_disabled_runtime_is_a_noop() {
        let dead = reporter("http://127.0.0.1:1");
        assert!(!dead.report("dec-1", "true", "v").await);
        report_completion(&dead, &["s1-verify:dec-1".into()], NodeOutcome::Accomplished, "v"); // must not panic
        let mut off = reporter("http://127.0.0.1:1");
        off.enabled = false;
        assert!(!off.report("dec-1", "true", "v").await);
    }

    #[test]
    fn spawn_outside_a_runtime_is_a_noop() {
        report_completion(&reporter("http://127.0.0.1:1"), &["s1-verify:d".into()], NodeOutcome::Accomplished, "v");
    }

    #[test]
    fn decision_id_is_read_from_result_extensions() {
        let mut m = Map::new();
        m.insert("x-decision_id".into(), json!("abc"));
        assert_eq!(verify_evidence_ref(&Some(m)).as_deref(), Some("s1-verify:abc"));
        assert_eq!(decision_id(&None), None);
    }
}
