//! Org Constraint Registry (seed, CL-128): organization-specific rules with an
//! explicit scope, severity, checker, evidence requirement and autofix policy.
//! Replaces ad-hoc hard-coded regex floors with data an org can own. The
//! registry only evaluates and reports; callers decide how a `Block` is
//! enforced. Checkers are deterministic (regex over a subject string).

use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warn,
    Block,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AutofixPolicy {
    #[default]
    Never,
    /// A fix may be proposed but a human approves it.
    Propose,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Constraint {
    pub id: String,
    /// Where it applies: `"*"`, or a scope prefix such as `"shell"`, `"fs:src/"`, `"net"`.
    pub scope: String,
    pub severity: Severity,
    /// Regex; a match means the constraint is violated.
    pub violates_if: String,
    pub message: String,
    /// What a reviewer must attach to waive it.
    #[serde(default)]
    pub evidence_required: Option<String>,
    #[serde(default)]
    pub autofix: AutofixPolicy,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Violation {
    pub constraint_id: String,
    pub severity: Severity,
    pub message: String,
}

#[derive(Debug, Default)]
pub struct Registry {
    items: Vec<(Constraint, Regex)>,
}

impl Registry {
    pub fn from_json(json: &str) -> Result<Self, String> {
        let list: Vec<Constraint> = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let mut r = Self::default();
        for c in list {
            r.add(c)?;
        }
        Ok(r)
    }

    pub fn add(&mut self, c: Constraint) -> Result<(), String> {
        if self.items.iter().any(|(x, _)| x.id == c.id) {
            return Err(format!("duplicate constraint id {}", c.id));
        }
        let re = Regex::new(&c.violates_if).map_err(|e| format!("{}: {e}", c.id))?;
        self.items.push((c, re));
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Violations for `subject` in `scope`, highest severity first.
    pub fn evaluate(&self, scope: &str, subject: &str) -> Vec<Violation> {
        let mut out: Vec<Violation> = self
            .items
            .iter()
            .filter(|(c, _)| c.scope == "*" || scope.starts_with(&c.scope))
            .filter(|(_, re)| re.is_match(subject))
            .map(|(c, _)| Violation { constraint_id: c.id.clone(), severity: c.severity, message: c.message.clone() })
            .collect();
        out.sort_by(|a, b| b.severity.cmp(&a.severity));
        out
    }

    pub fn blocks(&self, scope: &str, subject: &str) -> bool {
        self.evaluate(scope, subject).iter().any(|v| v.severity == Severity::Block)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEED: &str = r#"[
      {"id":"org.no-force-push","scope":"shell","severity":"block","violates_if":"git\\s+push\\s+.*--force","message":"force push is not allowed","evidence_required":"approval id"},
      {"id":"org.todo-marker","scope":"*","severity":"warn","violates_if":"TODO\\(unowned\\)","message":"unowned TODO","autofix":"propose"}
    ]"#;

    #[test]
    fn scoped_evaluation_orders_by_severity() {
        let r = Registry::from_json(SEED).unwrap();
        assert!(r.blocks("shell", "git push origin main --force"));
        assert!(!r.blocks("fs:src/a.rs", "git push origin main --force"));
        let v = r.evaluate("shell", "git push --force # TODO(unowned)");
        assert_eq!(v.iter().map(|x| x.severity).collect::<Vec<_>>(), [Severity::Block, Severity::Warn]);
        assert!(r.evaluate("shell", "ls").is_empty());
    }

    #[test]
    fn rejects_duplicates_and_bad_regex() {
        assert!(Registry::from_json(r#"[{"id":"a","scope":"*","severity":"info","violates_if":"(","message":"m"}]"#).is_err());
        let dup = r#"[{"id":"a","scope":"*","severity":"info","violates_if":"x","message":"m"},{"id":"a","scope":"*","severity":"info","violates_if":"y","message":"m"}]"#;
        assert!(Registry::from_json(dup).is_err());
    }
}
