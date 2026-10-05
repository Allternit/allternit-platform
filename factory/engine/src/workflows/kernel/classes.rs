//! Capability classes (O1), output caps by call type (O5) and the S0-first law
//! (O13). One definition for the kernel router; gizzi mirrors the same table in
//! `runtime/model-pool/classes.ts` (a test on each side pins the values).
//!
//! Classes are logical: no vendor or model name appears here.

use serde_json::Value;

use super::router::{PoolEntry, Residency, Role};

pub const GEN_SMALL: &str = "gen.small";
pub const GEN_STANDARD: &str = "gen.standard";
pub const GEN_DEEP: &str = "gen.deep";
pub const GEN_CLASSES: &[&str] = &[GEN_SMALL, GEN_STANDARD, GEN_DEEP];

/// A remote entry whose blended cost (pool `cost`, USD per 1k tokens) is at or
/// below this is `gen.small`. gizzi: `SMALL_COST_MAX`.
pub const SMALL_COST_MAX: f64 = 0.0015;

/// Capability class of a pool entry: an explicit `x-gen_class` wins, else S3
/// (deep solver) entries are `gen.deep`, local or cheap entries `gen.small`,
/// everything else `gen.standard`.
pub fn gen_class(e: &PoolEntry) -> &'static str {
    if let Some(c) = e.extensions.as_ref().and_then(|x| x.get("x-gen_class")).and_then(Value::as_str) {
        if let Some(k) = GEN_CLASSES.iter().find(|k| **k == c) {
            return k;
        }
    }
    if e.cognitive_roles.contains(&Role::S3) {
        GEN_DEEP
    } else if e.residency != Residency::Remote || e.cost <= SMALL_COST_MAX {
        GEN_SMALL
    } else {
        GEN_STANDARD
    }
}

/// Default class for an internal call type. Easy generation (titles,
/// summaries, extraction, memory curation, lessons) runs on `gen.small`.
pub fn call_type_class(call_type: &str) -> &'static str {
    match call_type {
        "title" | "summary" | "compaction" | "extraction" | "memory_extraction" | "memory_curation" | "curation"
        | "lessons" | "classify" => GEN_SMALL,
        "plan" | "deep" | "solver" | "escalation" => GEN_DEEP,
        _ => GEN_STANDARD,
    }
}

/// O5 output cap by call type (versioned defaults, Q15). `None` = the
/// harness default. S1 readouts generate nothing (`0`). S0 has no model.
pub const OUTPUT_CAPS_VERSION: &str = "o5.v1";
pub fn default_max_output_tokens(role: Role, call_type: Option<&str>) -> Option<u64> {
    match role {
        Role::S0 => None,
        Role::S1 => Some(0),
        _ => match call_type? {
            "title" => Some(64),
            "summary" | "compaction" | "extraction" | "memory_extraction" | "memory_curation" | "curation" | "lessons" => {
                Some(1024)
            }
            "patch" | "edit" => Some(8192),
            _ => None,
        },
    }
}

/// Primitives with an S0 (deterministic) implementation. O13: the router never
/// sends these to a model, whatever role a graph asks for. Every id must exist
/// in the frozen registry (tested).
pub const S0_IMPLEMENTED: &[&str] = &[
    "obs.parse_task_request",
    "obs.fingerprint_environment",
    "obs.build_repo_index",
    "state.activate_primitive_packs",
    "state.persist_run_receipt",
    "ctx.retrieve_candidate_files",
    "ctx.compile_model_context",
    "policy.authorize_tool_call",
    "mut.apply_patch_transactionally",
    "ver.parse_validate",
    "ver.format_check",
    "ver.lint",
    "ver.typecheck_affected",
    "ver.run_target_tests",
    "ver.unit_test",
    "ver.integration_test",
    "ver.regression_check",
    "ver.diff_review",
    "ver.check_acceptance_evidence",
    "ver.check_artifact_existence",
    "ctl.rollback_mutation",
    "ctl.request_evidence",
];

pub fn has_s0_impl(primitive_id: &str) -> bool {
    S0_IMPLEMENTED.contains(&primitive_id)
}
