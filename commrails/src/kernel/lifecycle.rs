//! WorkNodeLifecycleV1: typed `NodeState`, transition table, and the
//! mapping to the legacy free-string node statuses stored in the ledger.
//!
//! Lifecycle (schema `work.schema.json`):
//! DECLARE -> ADMIT -> READY -> LEASE -> SPAWN -> RUN(+HEARTBEAT) -> OUTPUT
//! -> VERIFY -> {COMMIT | CONTINUE | REPLAN | NEEDS_HUMAN} -> CLOSE.
//! No executor goes RUNNING -> CLOSED(COMMITTED) directly.
//!
//! The on-disk format stays strings (`DagNodeStatusChanged.to`), so no data
//! migration: [`NodeState::from_legacy`] / [`legacy_status`] convert.

use std::collections::HashSet;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NodeState {
    Declared,
    Admitted,
    Ready,
    Leased,
    Spawned,
    Running,
    OutputReady,
    Verifying,
    Committed,
    Continue,
    Replan,
    NeedsHuman,
    Waiting,
    Cancelling,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CloseOutcome {
    Committed,
    Partial,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LifecycleError {
    #[error("unknown node status '{0}' (fail closed)")]
    UnknownStatus(String),
    #[error("illegal node transition {from} -> {to}")]
    IllegalTransition { from: NodeState, to: NodeState },
    #[error("illegal close: {from} -> CLOSED({outcome:?})")]
    IllegalClose { from: NodeState, outcome: CloseOutcome },
}

impl NodeState {
    pub const ALL: [NodeState; 15] = [
        NodeState::Declared,
        NodeState::Admitted,
        NodeState::Ready,
        NodeState::Leased,
        NodeState::Spawned,
        NodeState::Running,
        NodeState::OutputReady,
        NodeState::Verifying,
        NodeState::Committed,
        NodeState::Continue,
        NodeState::Replan,
        NodeState::NeedsHuman,
        NodeState::Waiting,
        NodeState::Cancelling,
        NodeState::Closed,
    ];

    /// Schema enum spelling (`LifecycleState`).
    pub fn as_str(self) -> &'static str {
        match self {
            NodeState::Declared => "DECLARED",
            NodeState::Admitted => "ADMITTED",
            NodeState::Ready => "READY",
            NodeState::Leased => "LEASED",
            NodeState::Spawned => "SPAWNED",
            NodeState::Running => "RUNNING",
            NodeState::OutputReady => "OUTPUT_READY",
            NodeState::Verifying => "VERIFYING",
            NodeState::Committed => "COMMITTED",
            NodeState::Continue => "CONTINUE",
            NodeState::Replan => "REPLAN",
            NodeState::NeedsHuman => "NEEDS_HUMAN",
            NodeState::Waiting => "WAITING",
            NodeState::Cancelling => "CANCELLING",
            NodeState::Closed => "CLOSED",
        }
    }

    pub fn parse(s: &str) -> Option<NodeState> {
        NodeState::ALL.iter().copied().find(|n| n.as_str() == s)
    }

    /// States directly reachable in one step (the transition table).
    pub fn successors(self) -> &'static [NodeState] {
        use NodeState::*;
        match self {
            Declared => &[Admitted, Cancelling],
            Admitted => &[Ready, Waiting, Cancelling],
            Ready => &[Leased, Waiting, NeedsHuman, Cancelling],
            Leased => &[Spawned, Ready, Cancelling],
            Spawned => &[Running, Ready, Cancelling],
            // Running -> Running is the heartbeat. Running -> Closed is legal
            // only for non-COMMITTED outcomes (see `try_close`).
            Running => &[Running, OutputReady, Waiting, NeedsHuman, Ready, Cancelling, Closed],
            OutputReady => &[Verifying, Cancelling],
            Verifying => &[Committed, Continue, Replan, NeedsHuman, Closed],
            Committed => &[Closed],
            Continue => &[Ready, Cancelling],
            Replan => &[Admitted, Closed],
            NeedsHuman => &[Ready, Closed, Cancelling],
            Waiting => &[Ready, Cancelling],
            Cancelling => &[Closed],
            Closed => &[],
        }
    }

    pub fn is_terminal(self) -> bool {
        self == NodeState::Closed
    }

    /// Reachable in one or more steps via the table.
    pub fn can_reach(self, to: NodeState) -> bool {
        let mut seen = HashSet::new();
        let mut stack = vec![self];
        while let Some(s) = stack.pop() {
            for &n in s.successors() {
                if n == to {
                    return true;
                }
                if seen.insert(n) {
                    stack.push(n);
                }
            }
        }
        false
    }

    /// Map a legacy free-string status to a typed state (+ close outcome for
    /// terminal strings). Unknown strings are `None` (callers fail closed).
    pub fn from_legacy(status: &str) -> Option<(NodeState, Option<CloseOutcome>)> {
        Some(match status {
            "NEW" => (NodeState::Admitted, None),
            "READY" => (NodeState::Ready, None),
            "IN_PROGRESS" => (NodeState::Running, None),
            // Judge said not accomplished; a continuation re-opens the node.
            "EXCEPTION" => (NodeState::Continue, None),
            "NEEDS_HUMAN" => (NodeState::NeedsHuman, None),
            "DONE" | "PASS" | "COMPLETED" => (NodeState::Closed, Some(CloseOutcome::Committed)),
            "FAILED" | "FAIL" => (NodeState::Closed, Some(CloseOutcome::Failed)),
            "CANCELLED" => (NodeState::Closed, Some(CloseOutcome::Cancelled)),
            other => return NodeState::parse(other).map(|s| (s, None)),
        })
    }
}

impl fmt::Display for NodeState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Legacy on-disk spelling for a typed state (inverse of `from_legacy` for
/// the strings the ledger has always used).
pub fn legacy_status(state: NodeState, outcome: Option<CloseOutcome>) -> &'static str {
    match (state, outcome) {
        (NodeState::Admitted, _) | (NodeState::Declared, _) => "NEW",
        (NodeState::Ready, _) => "READY",
        (NodeState::Running, _) => "IN_PROGRESS",
        (NodeState::Continue, _) => "EXCEPTION",
        (NodeState::NeedsHuman, _) => "NEEDS_HUMAN",
        (NodeState::Closed, Some(CloseOutcome::Failed)) => "FAILED",
        (NodeState::Closed, Some(CloseOutcome::Cancelled)) => "CANCELLED",
        (NodeState::Closed, _) => "DONE",
        (s, _) => s.as_str(),
    }
}

/// Strict single-step transition per the table.
pub fn try_transition(from: NodeState, to: NodeState) -> Result<NodeState, LifecycleError> {
    if from.successors().contains(&to) {
        Ok(to)
    } else {
        Err(LifecycleError::IllegalTransition { from, to })
    }
}

/// Close a node. `Committed` outcome requires the node to be `Committed`
/// (system transition backed by completion evidence); nothing closes from
/// `Closed`; a running executor may only close with a non-committed outcome.
pub fn try_close(from: NodeState, outcome: CloseOutcome) -> Result<NodeState, LifecycleError> {
    let ok = match outcome {
        CloseOutcome::Committed => from == NodeState::Committed,
        _ => from != NodeState::Committed && from.successors().contains(&NodeState::Closed)
            || (outcome == CloseOutcome::Cancelled && from == NodeState::Cancelling),
    };
    if ok {
        Ok(NodeState::Closed)
    } else {
        Err(LifecycleError::IllegalClose { from, outcome })
    }
}

/// Validate a change between two legacy status strings, as written by the
/// CLI/gate/judge. Legacy writers skip intermediate lifecycle states
/// (e.g. NEW -> IN_PROGRESS), so the rule is: both strings must parse
/// (fail closed) and the target must be reachable through the typed table.
/// Same-status is a no-op (heartbeat-like). The one legacy-only edge is
/// reopen: a closed node goes back to NEW as a new attempt.
pub fn check_legacy_change(from: &str, to: &str) -> Result<(), LifecycleError> {
    let (f, fo) = NodeState::from_legacy(from)
        .ok_or_else(|| LifecycleError::UnknownStatus(from.to_string()))?;
    let (t, to_outcome) = NodeState::from_legacy(to)
        .ok_or_else(|| LifecycleError::UnknownStatus(to.to_string()))?;
    if f == t && fo == to_outcome {
        return Ok(());
    }
    if f == NodeState::Closed {
        // Reopen (new attempt) is the only exit from a closed node.
        return if t == NodeState::Admitted {
            Ok(())
        } else {
            Err(LifecycleError::IllegalTransition { from: f, to: t })
        };
    }
    if t == NodeState::Closed {
        if let Some(o) = to_outcome {
            if o == CloseOutcome::Committed || f.can_reach(NodeState::Closed) {
                return if f.can_reach(NodeState::Committed) || f == NodeState::Committed {
                    Ok(())
                } else if o != CloseOutcome::Committed {
                    Ok(())
                } else {
                    Err(LifecycleError::IllegalClose { from: f, outcome: o })
                };
            }
        }
        return Err(LifecycleError::IllegalTransition { from: f, to: t });
    }
    if f.can_reach(t) {
        Ok(())
    } else {
        Err(LifecycleError::IllegalTransition { from: f, to: t })
    }
}

/// Fail-closed check for writers that only know the target status.
pub fn check_legacy_target(to: &str) -> Result<(), LifecycleError> {
    NodeState::from_legacy(to)
        .map(|_| ())
        .ok_or_else(|| LifecycleError::UnknownStatus(to.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_table_edge_is_legal() {
        let mut n = 0;
        for &f in &NodeState::ALL {
            for &t in f.successors() {
                assert_eq!(try_transition(f, t), Ok(t));
                n += 1;
            }
        }
        assert!(n >= 30);
    }

    #[test]
    fn every_non_edge_is_rejected() {
        for &f in &NodeState::ALL {
            for &t in &NodeState::ALL {
                if !f.successors().contains(&t) {
                    assert!(try_transition(f, t).is_err(), "{f}->{t}");
                }
            }
        }
    }

    #[test]
    fn sample_illegal() {
        assert!(try_transition(NodeState::Running, NodeState::Committed).is_err());
        assert!(try_transition(NodeState::Declared, NodeState::Running).is_err());
        assert!(try_transition(NodeState::Closed, NodeState::Ready).is_err());
        assert!(try_transition(NodeState::OutputReady, NodeState::Committed).is_err());
    }

    #[test]
    fn happy_path_reaches_closed() {
        use NodeState::*;
        let mut s = Declared;
        for n in [Admitted, Ready, Leased, Spawned, Running, OutputReady, Verifying, Committed] {
            s = try_transition(s, n).unwrap();
        }
        assert_eq!(try_close(s, CloseOutcome::Committed), Ok(Closed));
    }

    #[test]
    fn executor_cannot_close_committed() {
        assert!(try_close(NodeState::Running, CloseOutcome::Committed).is_err());
        assert!(try_close(NodeState::Verifying, CloseOutcome::Committed).is_err());
        assert!(try_close(NodeState::Running, CloseOutcome::Failed).is_ok());
        assert!(try_close(NodeState::Cancelling, CloseOutcome::Cancelled).is_ok());
        assert!(try_close(NodeState::Closed, CloseOutcome::Failed).is_err());
    }

    #[test]
    fn schema_spelling_round_trips() {
        for &s in &NodeState::ALL {
            assert_eq!(NodeState::parse(s.as_str()), Some(s));
        }
        assert_eq!(NodeState::parse("nope"), None);
    }

    #[test]
    fn legacy_mapping() {
        assert_eq!(NodeState::from_legacy("NEW"), Some((NodeState::Admitted, None)));
        assert_eq!(NodeState::from_legacy("IN_PROGRESS").unwrap().0, NodeState::Running);
        assert_eq!(
            NodeState::from_legacy("DONE"),
            Some((NodeState::Closed, Some(CloseOutcome::Committed)))
        );
        assert_eq!(
            NodeState::from_legacy("FAILED"),
            Some((NodeState::Closed, Some(CloseOutcome::Failed)))
        );
        assert_eq!(NodeState::from_legacy("BOGUS"), None);
        for s in ["NEW", "READY", "IN_PROGRESS", "DONE", "FAILED", "NEEDS_HUMAN", "EXCEPTION"] {
            let (st, o) = NodeState::from_legacy(s).unwrap();
            assert_eq!(legacy_status(st, o), s);
        }
    }

    #[test]
    fn legacy_changes_used_by_cli_and_gate() {
        assert!(check_legacy_change("NEW", "IN_PROGRESS").is_ok());
        assert!(check_legacy_change("READY", "IN_PROGRESS").is_ok());
        assert!(check_legacy_change("IN_PROGRESS", "DONE").is_ok());
        assert!(check_legacy_change("IN_PROGRESS", "NEEDS_HUMAN").is_ok());
        assert!(check_legacy_change("IN_PROGRESS", "EXCEPTION").is_ok());
        assert!(check_legacy_change("IN_PROGRESS", "FAILED").is_ok());
        assert!(check_legacy_change("EXCEPTION", "READY").is_ok());
        assert!(check_legacy_change("NEEDS_HUMAN", "DONE").is_ok());
        assert!(check_legacy_change("DONE", "NEW").is_ok());
        assert!(check_legacy_change("DONE", "DONE").is_ok());
    }

    #[test]
    fn legacy_illegal_and_unknown() {
        assert!(check_legacy_change("DONE", "IN_PROGRESS").is_err());
        assert!(check_legacy_change("FAILED", "DONE").is_err());
        assert!(check_legacy_change("IN_PROGRESS", "NEW").is_err());
        assert_eq!(
            check_legacy_change("NEW", "WEIRD"),
            Err(LifecycleError::UnknownStatus("WEIRD".into()))
        );
        assert!(check_legacy_target("WEIRD").is_err());
        assert!(check_legacy_target("DONE").is_ok());
    }
}
