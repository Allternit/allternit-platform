//! Structured gate denials.
//!
//! Gate checks that refuse a transition return a [`GateError`] wrapped in
//! `anyhow::Error`, so existing callers keep working with the Display text
//! while callers that care (CLI, HTTP) can `downcast_ref::<GateError>()` and
//! surface the gate id, code, and details as data.

use serde::Serialize;
use serde_json::Value;

/// Gate ids used in [`GateError::gate`].
pub mod gate_ids {
    pub const PLAN: &str = "gate0.plan";
    pub const PICKUP: &str = "gate1.pickup";
    pub const CLOSE: &str = "gate4.close";
    pub const WAIT_GATE: &str = "wait_gate";
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct GateError {
    /// Which gate refused (see [`gate_ids`]).
    pub gate: String,
    /// Stable machine-readable reason code.
    pub code: String,
    /// Human-readable reason.
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dag_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    /// Code-specific data (offending refs, unmet blockers, gate ids, the
    /// prompt/delta provenance a rejected mutation carried, ...).
    #[serde(skip_serializing_if = "Value::is_null")]
    pub details: Value,
}

impl GateError {
    pub fn new(gate: &str, code: &str, reason: impl Into<String>) -> Self {
        Self {
            gate: gate.to_string(),
            code: code.to_string(),
            reason: reason.into(),
            dag_id: None,
            node_id: None,
            details: Value::Null,
        }
    }

    pub fn at(mut self, dag_id: &str, node_id: Option<&str>) -> Self {
        self.dag_id = Some(dag_id.to_string());
        self.node_id = node_id.map(|s| s.to_string());
        self
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = details;
        self
    }

    /// The `GateError` inside an `anyhow::Error`, if that is what it is.
    pub fn from_anyhow(err: &anyhow::Error) -> Option<&GateError> {
        err.downcast_ref::<GateError>()
    }
}

impl std::fmt::Display for GateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} denied [{}]: {}", self.gate, self.code, self.reason)
    }
}

impl std::error::Error for GateError {}
