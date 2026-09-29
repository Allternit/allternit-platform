//! Judge wire and outcome types.

use serde::{Deserialize, Serialize};

/// What the judge model reports for a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Accomplished,
    NotAccomplished,
}

/// Why a node was not accomplished (Raven's categories).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    MissingUserInput,
    MissingCredential,
    ToolFailure,
    DependencyOutputUnusable,
    OutputLimit,
    Other,
}

impl Category {
    pub const ALL: [Category; 6] = [
        Category::MissingUserInput,
        Category::MissingCredential,
        Category::ToolFailure,
        Category::DependencyOutputUnusable,
        Category::OutputLimit,
        Category::Other,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Category::MissingUserInput => "missing_user_input",
            Category::MissingCredential => "missing_credential",
            Category::ToolFailure => "tool_failure",
            Category::DependencyOutputUnusable => "dependency_output_unusable",
            Category::OutputLimit => "output_limit",
            Category::Other => "other",
        }
    }

    pub fn parse(s: &str) -> Option<Category> {
        Category::ALL.into_iter().find(|c| c.as_str() == s)
    }

    /// Categories a retry cannot fix: the node goes straight to a person.
    pub fn needs_human(&self) -> bool {
        matches!(
            self,
            Category::MissingUserInput | Category::MissingCredential
        )
    }
}

/// A valid structured verdict from a backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeVerdict {
    pub verdict: Verdict,
    /// Required for `not_accomplished`; dropped for `accomplished`.
    pub category: Option<Category>,
    pub reason: String,
    /// Which backend decided (`command`, `stub`, `system_one`, `human`).
    pub source: String,
}

/// Tool-call decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolDecision {
    Allow,
    Ask,
    Deny,
}

impl ToolDecision {
    pub fn as_str(&self) -> &'static str {
        match self {
            ToolDecision::Allow => "allow",
            ToolDecision::Ask => "ask",
            ToolDecision::Deny => "deny",
        }
    }
}

/// A valid structured tool decision from a backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolJudgeDecision {
    pub decision: ToolDecision,
    pub reason: String,
    pub source: String,
}

/// Why a judge call produced no usable answer. Every variant fails closed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum JudgeFailure {
    Timeout,
    Error(String),
    Invalid(String),
}

impl JudgeFailure {
    pub fn kind(&self) -> &'static str {
        match self {
            JudgeFailure::Timeout => "timeout",
            JudgeFailure::Error(_) => "error",
            JudgeFailure::Invalid(_) => "invalid",
        }
    }
}

impl std::fmt::Display for JudgeFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JudgeFailure::Timeout => write!(f, "judge timed out"),
            JudgeFailure::Error(e) => write!(f, "judge error: {e}"),
            JudgeFailure::Invalid(e) => write!(f, "judge answer invalid: {e}"),
        }
    }
}

/// A receipt the worker produced, listed as evidence for the judge.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReceiptSummary {
    pub receipt_id: String,
    pub tool: Option<String>,
}

/// Context for a node verdict. `output`, `evidence_refs` and `receipts` are
/// worker-produced and therefore untrusted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeJudgeRequest {
    pub dag_id: String,
    pub node_id: String,
    pub wih_id: String,
    pub title: String,
    /// Node description, with output placeholders resolved when the WIH has
    /// a resolved prompt.
    pub description: Option<String>,
    pub acceptance: Option<String>,
    pub output: Option<String>,
    pub evidence_refs: Vec<String>,
    pub receipts: Vec<ReceiptSummary>,
    pub nonce: String,
}

/// Context for a tool decision. Only the tool, command line and paths are
/// sent: write contents never reach the judge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolJudgeRequest {
    pub wih_id: String,
    pub dag_id: String,
    pub node_id: String,
    pub node_title: String,
    pub tool: String,
    pub command: Option<String>,
    pub paths: Vec<String>,
    pub nonce: String,
}

/// Fail-closed result of a node verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeOutcome {
    Accomplished,
    NotAccomplished,
    NeedsHuman,
}

impl NodeOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            NodeOutcome::Accomplished => "accomplished",
            NodeOutcome::NotAccomplished => "not_accomplished",
            NodeOutcome::NeedsHuman => "needs_human",
        }
    }
}

/// What `judge_node` returns: always an outcome, never an error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JudgedNode {
    pub outcome: NodeOutcome,
    pub category: Option<Category>,
    pub reason: String,
    pub backend: String,
    pub source: Option<String>,
    /// Set when the outcome is `needs_human` because the judge failed.
    pub failure: Option<JudgeFailure>,
}

/// What `judge_tool` returns: always a decision, never an error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JudgedTool {
    pub decision: ToolDecision,
    pub reason: String,
    pub backend: String,
    pub source: Option<String>,
    pub failure: Option<JudgeFailure>,
}

/// Where a Gate 2 tool decision came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolDecisionSource {
    /// Existing Gate 2 checks (open-signed, allowed tools, lease coverage).
    Gate2,
    /// The built-in hard floor (`judge::hard_rules`).
    HardRule,
    /// The judge answered.
    Judge,
    /// The judge failed; fail closed to `ask`.
    JudgeFailed,
}

/// Final Gate 2 answer for one call (`Gate::judge_tool_call`).
#[derive(Debug, Clone, Serialize)]
pub struct ToolCallVerdict {
    pub decision: ToolDecision,
    pub source: ToolDecisionSource,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
}
