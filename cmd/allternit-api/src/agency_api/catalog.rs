//! Agency API catalog: agent bundle manifest (CL-147), authority profiles,
//! completion criteria/contracts. Versioned, static for the alpha. No model
//! or vendor identity anywhere (L5 / Q16).

use serde_json::{json, Value};

/// Version of the platform limit defaults applied by the compiler (Q15).
pub const DEFAULTS_VERSION: &str = "2026-09-29";

const BUNDLE_ALLTERNIT_CODE: &str = include_str!("bundles/allternit-code.v1.json");

/// The minimal `allternit-code` agent bundle manifest.
pub fn bundle_manifest() -> Value {
    serde_json::from_str(BUNDLE_ALLTERNIT_CODE).expect("bundled manifest is valid JSON")
}

/// Public `Agent` object for `GET /v1/agents`.
pub fn agent_object() -> Value {
    let m = bundle_manifest();
    json!({
        "id": m["agent_id"],
        "object": "agent",
        "versions": [m["version"]],
        "default_version": m["version"],
        "description": m["description"],
        "capabilities": m["capabilities"],
        "completion_criteria": completion_contract("completion.bug_fix").map(|c| c["require"].clone()).unwrap_or_default(),
        "authority_profiles": m["authority_profiles"],
        "templates": m["templates"],
    })
}

pub fn authority_profiles() -> Vec<Value> {
    vec![
        json!({
            "id": "code-safe", "object": "authority_profile", "version": 1,
            "description": "Read and write inside the run workspace under a lease; run tests; no network beyond the package allowlist; approval before any push or external effect.",
            "interaction_mode": "approval_gated",
            "grants_summary": {
                "capabilities": ["fs.read", "fs.write", "shell.exec", "vcs.commit", "tests.run"],
                "filesystem_scope": { "root": "workspace", "write": "leased" },
                "network": { "mode": "allowlist", "allow": ["package-registries"] },
                "approval_requirements": [{ "action": "vcs.push", "required": true }, { "action": "external_effect", "required": true }],
                "grant_count": 5
            }
        }),
        json!({
            "id": "read-only", "object": "authority_profile", "version": 1,
            "description": "Read the workspace and run tests; no writes, no network.",
            "interaction_mode": "recommend_only",
            "grants_summary": {
                "capabilities": ["fs.read", "tests.run"],
                "filesystem_scope": { "root": "workspace", "write": "none" },
                "network": { "mode": "deny_all" },
                "approval_requirements": [],
                "grant_count": 2
            }
        }),
    ]
}

pub fn authority_profile(id: &str) -> Option<Value> {
    authority_profiles().into_iter().find(|p| p["id"] == id)
}

pub fn criteria() -> Vec<Value> {
    [
        ("target_tests_pass", "The originally failing tests pass."),
        ("affected_tests_pass", "Tests touching the changed code pass."),
        ("no_new_regressions", "No previously passing test now fails."),
        ("diff_review_accept", "An independent verifier accepts the diff."),
        ("requirements_satisfied", "The stated goal is met, judged against the evidence."),
        ("tests_pass", "The workspace test suite passes."),
    ]
    .into_iter()
    .map(|(id, d)| json!({ "id": id, "object": "completion_criterion", "version": 1, "description": d, "evidence": "receipt" }))
    .collect()
}

pub fn criterion(id: &str) -> Option<Value> {
    criteria().into_iter().find(|c| c["id"] == id)
}

/// Completion contracts (mirrors `spec/Contracts/kernel/v1/data/completion_policy.bug_fix.v1.json`).
pub fn completion_contract(id: &str) -> Option<Value> {
    (id == "completion.bug_fix").then(|| {
        let require: Vec<Value> = ["target_tests_pass", "affected_tests_pass", "no_new_regressions", "diff_review_accept", "requirements_satisfied"]
            .iter()
            .map(|c| json!({ "id": c, "version": 1 }))
            .collect();
        json!({ "id": "completion.bug_fix", "version": 1, "task_type": "BUG_FIX", "allow_partial": false, "require": require })
    })
}

/// `GET /v1/capabilities`: capability ids only (no backend identity).
pub fn capabilities() -> Vec<Value> {
    bundle_manifest()["capabilities"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|c| json!({ "id": c, "object": "capability", "version": 1 }))
        .collect()
}
