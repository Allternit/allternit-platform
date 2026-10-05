//! Data-driven completion evidence policies (`spec/JUDGE.md`, origin rules).
//!
//! Policies are the frozen kernel data files under
//! `spec/Contracts/kernel/v1/data/`, embedded at build time. A policy lists
//! required criteria; DONE needs an evidence ref for each blocking one.
//! An evidence ref satisfies a criterion when it is `<criterion_id>:<ref>`
//! (for example `target_tests_pass:receipt:rcp_123`).

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Requirement {
    pub criterion_id: String,
    #[serde(default = "yes")]
    pub blocking: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
pub struct CompletionPolicy {
    pub policy_id: String,
    pub require: Vec<Requirement>,
    #[serde(default)]
    pub allow_partial: bool,
}

/// Embedded policy files; add a line here for a new template's policy.
const BUILTIN: &[&str] = &[include_str!(
    "../../../../../spec/Contracts/kernel/v1/data/completion_policy.bug_fix.v1.json"
)];

pub fn load_policy(policy_id: &str) -> Option<CompletionPolicy> {
    BUILTIN
        .iter()
        .filter_map(|raw| serde_json::from_str::<CompletionPolicy>(raw).ok())
        .find(|p| p.policy_id == policy_id)
}

/// Blocking criteria with no evidence ref. Empty means complete.
pub fn missing_evidence(policy: &CompletionPolicy, evidence_refs: &[String]) -> Vec<String> {
    policy
        .require
        .iter()
        .filter(|r| r.blocking || !policy.allow_partial)
        .filter(|r| {
            let prefix = format!("{}:", r.criterion_id);
            !evidence_refs
                .iter()
                .any(|e| e.strip_prefix(&prefix).is_some_and(|rest| !rest.trim().is_empty()))
        })
        .map(|r| r.criterion_id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bug_fix_policy_loads_and_lists_missing() {
        let p = load_policy("completion.bug_fix").unwrap();
        assert!(!p.allow_partial);
        assert_eq!(p.require.len(), 5);
        let refs = vec!["target_tests_pass:receipt:r1".to_string(), "diff_review_accept:".to_string()];
        let m = missing_evidence(&p, &refs);
        assert_eq!(m.len(), 4);
        assert!(!m.contains(&"target_tests_pass".to_string()));
        assert!(m.contains(&"diff_review_accept".to_string()));
        assert!(load_policy("nope").is_none());
    }
}
