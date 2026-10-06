//! Fail-closed judge for the Factory (spec: `spec/JUDGE.md`).
//!
//! One module, two questions:
//! - **Node verdicts** at Gate 4 (`wih close`) when the node/plan policy has
//!   `verify: judge`: did the node accomplish its task?
//! - **Tool decisions** for Gate 2 (`judge tool`, and inside `pre_tool` when
//!   the policy has `tool_judge: true`): allow, ask, or deny a call that the
//!   hard rules and lease checks did not settle.
//!
//! Borrowed from Raven's `report_verdict` design (a separate judge call must
//! answer with a structured object; prose that says "accomplished" does not
//! count), with its failure mode inverted: a timeout, backend error, or
//! missing/invalid structured answer is **never** a pass. Node verdicts fail
//! to `needs_human`; tool decisions fail to `ask`.
//!
//! Self-attestation guard: node output and tool arguments are untrusted data.
//! They are fenced with a per-call random nonce, and the judge's answer must
//! echo that nonce, which the worker could not have known when it wrote the
//! output. A verdict forged inside the output therefore never parses.

pub mod backends;
pub mod completion;
pub mod config;
pub mod hard_rules;
pub mod heartbeat;
pub mod parse;
pub mod policy;
pub mod prompt;
pub mod state;
pub mod types;

pub use backends::{
    judge_node, judge_tool, CommandJudge, FailedJudge, Judge, StubJudge, StubReply,
    SystemOneFirstPass,
};
pub use config::{build_judge, load_config, JudgeConfig};
pub use policy::{
    effective_completion_policy, effective_policy, CloseBy, EffectivePolicy, JudgePolicy,
    PolicyOrigin, VerifyMode,
};
pub use state::{pending_judge_needs, project_node_judge, NodeJudgeState, PendingJudgeNeed};
pub use types::*;

/// Actor id used on events the judge emits.
pub const JUDGE_ACTOR_ID: &str = "judge";

/// Ledger event types owned by this module (see `spec/EVENT_TAXONOMY.md`).
pub mod events {
    pub const POLICY_SET: &str = "JudgePolicySet";
    pub const VERDICT_RECORDED: &str = "JudgeVerdictRecorded";
    pub const CONTINUATION_GRANTED: &str = "JudgeContinuationGranted";
    pub const HUMAN_RESOLVED: &str = "JudgeHumanResolved";
    pub const TOOL_DECISION: &str = "JudgeToolDecision";
    pub const COMPLETION_PROPOSED: &str = "CompletionProposed";
    pub const CLOSE_DENIED: &str = "WIHCloseDenied";
    pub const LEASE_HEARTBEAT: &str = "LeaseHolderHeartbeat";
    pub const LEASE_RECLAIMED: &str = "LeaseReclaimed";
    pub const WIH_RECLAIMED: &str = "WIHReclaimed";
}

/// Node statuses introduced by the judge.
pub mod status {
    /// Builder proposed completion; waiting on the verifier path.
    pub const VERIFYING: &str = "VERIFYING";
    /// Judge said not accomplished; a continuation can re-open the node.
    pub const EXCEPTION: &str = "EXCEPTION";
    /// Waiting on a person: judge failed, continuation cap hit, or the
    /// category needs a human (missing input / credential).
    pub const NEEDS_HUMAN: &str = "NEEDS_HUMAN";
}

/// Fresh random nonce for fencing one judge call.
pub fn new_nonce() -> String {
    format!(
        "{:016x}{:016x}",
        rand::random::<u64>(),
        rand::random::<u64>()
    )
}
