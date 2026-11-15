//! Cloud product lane status.
//!
//! Each struct describes where a product line stands relative to the canonical
//! AllternitOS execution model. The `missing` list is honest: it flags the
//! pieces that are not yet implemented and that the AllternitOS integrator
//! must provide or consume.

use serde::{Deserialize, Serialize};

/// Lifecycle status for a product lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaneStatus {
    /// No implementation exists yet.
    NotStarted,
    /// Partial implementation; gaps are listed in `missing`.
    Partial,
    /// End-to-end works in happy path; hardening remains.
    Beta,
    /// Production ready.
    Stable,
}

/// A. Managed model inference.
///
/// Status: **Partial**. The Model Gateway, credits ledger, model catalog, and
/// deterministic stub responses are built. Real provider proxying, OS workload
/// conversion, and streaming/cache/logging are not yet implemented.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedInferenceLane {
    pub status: LaneStatus,
    pub supported_providers: Vec<String>,
    pub supported_model_profiles: Vec<String>,
    pub supports_auto_model: bool,
    pub supports_streaming: bool,
    pub supports_credits_charge: bool,
    pub supports_credits_reconcile: bool,
    pub missing: Vec<String>,
}

impl ManagedInferenceLane {
    pub fn current() -> Self {
        Self {
            status: LaneStatus::Partial,
            supported_providers: vec![
                "openai".to_string(),
                "together".to_string(),
                "fireworks".to_string(),
            ],
            supported_model_profiles: vec![
                "openai/gpt-4o-mini".to_string(),
                "openai/gpt-4o".to_string(),
                "openai/gpt-4.1-mini".to_string(),
                "openai/gpt-4.1".to_string(),
                "openai/o3-mini".to_string(),
                "together/meta-llama/Llama-3.3-70B-Instruct-Turbo".to_string(),
                "together/meta-llama/Meta-Llama-3.1-8B-Instruct-Turbo".to_string(),
                "together/deepseek-ai/DeepSeek-R1-Distill-Llama-70B".to_string(),
                "together/Qwen/Qwen2.5-72B-Instruct-Turbo".to_string(),
                "fireworks/accounts/fireworks/models/llama-v3p1-8b-instruct".to_string(),
                "fireworks/accounts/fireworks/models/llama-v3p1-70b-instruct".to_string(),
                "fireworks/accounts/fireworks/models/deepseek-r1".to_string(),
                "fireworks/accounts/fireworks/models/qwen2p5-72b-instruct".to_string(),
            ],
            supports_auto_model: true,
            supports_streaming: false,
            supports_credits_charge: true,
            supports_credits_reconcile: false,
            missing: vec![
                "real provider proxy/streaming".to_string(),
                "OS workload + model intent conversion".to_string(),
                "OS resource scheduler integration".to_string(),
                "private/local Fabric model workers".to_string(),
                "response cache".to_string(),
                "request/response logging".to_string(),
                "rate limiting".to_string(),
            ],
        }
    }
}

/// B. Managed harness execution.
///
/// Status: **Partial**. VM provisioning for harness runtimes exists via Agent
/// Cloud, but the harnesses are not yet wrapped as canonical AllternitOS
/// Harness/Workers. OpenCode runs through gizzi's `opencode run` CLI adapter
/// (cmd/gizzi-code/src/runtime/drivers/local-cli-driver.ts).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedHarnessLane {
    pub status: LaneStatus,
    pub supported_harnesses: Vec<String>,
    pub supports_gizzi: bool,
    pub supports_opencode: bool,
    pub supports_isolated_workspace: bool,
    pub supports_model_gateway_request: bool,
    pub supports_capability_lease: bool,
    pub supports_artifact_receipt: bool,
    pub missing: Vec<String>,
}

impl ManagedHarnessLane {
    pub fn current() -> Self {
        Self {
            status: LaneStatus::Partial,
            supported_harnesses: vec!["gizzi".to_string(), "opencode".to_string()],
            supports_gizzi: true,
            supports_opencode: true,
            supports_isolated_workspace: true,
            supports_model_gateway_request: false,
            supports_capability_lease: true,
            supports_artifact_receipt: false,
            missing: vec![
                "OS Harness/Worker contract wrapping".to_string(),
                "artifact + receipt emission".to_string(),
                "unified workload budget".to_string(),
            ],
        }
    }
}

/// C. Cloud Computer Use.
///
/// Status: **Partial**. The VM substrates (Incus, Tart, bare VM) and
/// provisioning paths exist, but the product does not yet expose canonical
/// computer/browser/shell/file capabilities or meter VM + AI usage as one
/// workload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudComputerUseLane {
    pub status: LaneStatus,
    pub supported_substrates: Vec<String>,
    pub supports_passthrough_gpu: bool,
    pub supports_mig_vgpu: bool,
    pub supports_time_sliced_gpu: bool,
    pub supports_desktop_observe: bool,
    pub supports_desktop_act: bool,
    pub supports_browser_capability: bool,
    pub supports_shell_capability: bool,
    pub supports_file_capability: bool,
    pub supports_app_capability: bool,
    pub supports_unified_metering: bool,
    pub missing: Vec<String>,
}

impl CloudComputerUseLane {
    pub fn current() -> Self {
        Self {
            status: LaneStatus::Partial,
            supported_substrates: vec![
                "incus".to_string(),
                "tart".to_string(),
                "bare-vm".to_string(),
            ],
            supports_passthrough_gpu: true,
            supports_mig_vgpu: false,
            supports_time_sliced_gpu: false,
            supports_desktop_observe: false,
            supports_desktop_act: false,
            supports_browser_capability: false,
            supports_shell_capability: false,
            supports_file_capability: false,
            supports_app_capability: false,
            supports_unified_metering: false,
            missing: vec![
                "canonical Workload/Step mapping".to_string(),
                "node capability advertisement".to_string(),
                "desktop.observe / desktop.act functions".to_string(),
                "browser.* capability surface".to_string(),
                "shell.exec capability surface".to_string(),
                "file.* capability surface".to_string(),
                "app.* adapter surface".to_string(),
                "lease-gated capability invocation".to_string(),
                "VM + AI + storage unified metering".to_string(),
                "MIG/vGPU/SR-IOV GPU partitioning".to_string(),
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_inference_is_partial() {
        let lane = ManagedInferenceLane::current();
        assert_eq!(lane.status, LaneStatus::Partial);
        assert!(lane.supports_auto_model);
        assert!(!lane.supports_streaming);
        assert!(lane.missing.contains(&"real provider proxy/streaming".to_string()));
    }

    #[test]
    fn managed_harness_is_partial_and_runs_opencode() {
        let lane = ManagedHarnessLane::current();
        assert_eq!(lane.status, LaneStatus::Partial);
        assert!(lane.supports_gizzi);
        assert!(lane.supports_opencode);
        assert!(lane.supported_harnesses.contains(&"opencode".to_string()));
        assert!(!lane.missing.iter().any(|m| m.contains("OpenCode")));
    }

    #[test]
    fn cloud_computer_use_is_substrate_only() {
        let lane = CloudComputerUseLane::current();
        assert_eq!(lane.status, LaneStatus::Partial);
        assert!(lane.supported_substrates.contains(&"incus".to_string()));
        assert!(!lane.supports_browser_capability);
        assert!(lane.missing.contains(&"canonical Workload/Step mapping".to_string()));
    }
}
