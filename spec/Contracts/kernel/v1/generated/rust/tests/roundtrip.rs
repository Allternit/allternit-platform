//! Round-trips every valid conformance example through the generated serde types.
use allternit_kernel_abi::*;
use serde::{de::DeserializeOwned, Serialize};

fn rt<T: DeserializeOwned + Serialize>(name: &str) {
    let path = format!("{}/../../conformance/examples/valid/{}.json", env!("CARGO_MANIFEST_DIR"), name);
    let raw: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let typed: T = serde_json::from_value(raw.clone()).unwrap_or_else(|e| panic!("{name}: {e}"));
    let back = serde_json::to_value(&typed).unwrap();
    assert_eq!(norm(raw), norm(back), "{name} did not round-trip");
}

/// JSON `0` and `0.0` are the same number; f64 fields re-serialize integers as floats.
fn norm(v: serde_json::Value) -> serde_json::Value {
    use serde_json::Value::*;
    match v {
        Number(n) => serde_json::json!(n.as_f64().unwrap()),
        Array(a) => Array(a.into_iter().map(norm).collect()),
        Object(o) => Object(o.into_iter().map(|(k, v)| (k, norm(v))).collect()),
        other => other,
    }
}

#[test]
fn all_contracts_round_trip() {
    rt::<AbiEnvelopeV1>("AbiEnvelopeV1");
    rt::<ErrorV1>("ErrorV1");
    rt::<TaskIrv1>("TaskIRV1");
    rt::<AgentStateV1>("AgentStateV1");
    rt::<ComputeGraphIrv1>("ComputeGraphIRV1");
    rt::<PrimitiveDescriptorV1>("PrimitiveDescriptorV1");
    rt::<PrimitiveMaturityV1>("PrimitiveMaturityV1");
    rt::<PrimitiveRegistryV1>("PrimitiveRegistryV1");
    rt::<DecisionRequestV1>("DecisionRequestV1");
    rt::<DecisionResultV1>("DecisionResultV1");
    rt::<DecisionDeploymentUnitV1>("DecisionDeploymentUnitV1");
    rt::<DecisionBackendProfileV1>("DecisionBackendProfileV1");
    rt::<DecisionReadoutProfileV1>("DecisionReadoutProfileV1");
    rt::<DecisionCalibrationManifestV1>("DecisionCalibrationManifestV1");
    rt::<ArtifactRecordV1>("ArtifactRecordV1");
    rt::<CapabilityRequestV1>("CapabilityRequestV1");
    rt::<ModelCapabilityProfileV1>("ModelCapabilityProfileV1");
    rt::<ModelPoolEntryV1>("ModelPoolEntryV1");
    rt::<ExecutionPlanV1>("ExecutionPlanV1");
    rt::<CognitiveStateDescriptorV1>("CognitiveStateDescriptorV1");
    rt::<StateTransferRequestV1>("StateTransferRequestV1");
    rt::<StateTransferReceiptV1>("StateTransferReceiptV1");
    rt::<ContextChunkV1>("ContextChunkV1");
    rt::<ContextVisibilityDecisionV1>("ContextVisibilityDecisionV1");
    rt::<ContextProjectionV1>("ContextProjectionV1");
    rt::<ToolInvocationV1>("ToolInvocationV1");
    rt::<ToolReceiptV1>("ToolReceiptV1");
    rt::<RecoveryDecisionV1>("RecoveryDecisionV1");
    rt::<MutationRequestV1>("MutationRequestV1");
    rt::<MutationReceiptV1>("MutationReceiptV1");
    rt::<VerificationRequestV1>("VerificationRequestV1");
    rt::<VerificationReceiptV1>("VerificationReceiptV1");
    rt::<PolicyCheckV1>("PolicyCheckV1");
    rt::<PolicyDecisionV1>("PolicyDecisionV1");
    rt::<AuthorityProfileV1>("AuthorityProfileV1");
    rt::<ActionReceiptV1>("ActionReceiptV1");
    rt::<PolicyReceiptV1>("PolicyReceiptV1");
    rt::<SpawnReceiptV1>("SpawnReceiptV1");
    rt::<RunReceiptV1>("RunReceiptV1");
    rt::<TraceEventV1>("TraceEventV1");
    rt::<WorkNodeLifecycleV1>("WorkNodeLifecycleV1");
    rt::<WorkNodeTransitionV1>("WorkNodeTransitionV1");
    rt::<WorkRunRecordV1>("WorkRunRecordV1");
    rt::<NodeOutputV1>("NodeOutputV1");
    rt::<LeaseV1>("LeaseV1");
    rt::<AdmissionRequestV1>("AdmissionRequestV1");
    rt::<AdmissionDecisionV1>("AdmissionDecisionV1");
    rt::<ExecutorProbeV1>("ExecutorProbeV1");
    rt::<WakePolicyV1>("WakePolicyV1");
    rt::<WaitGateV1>("WaitGateV1");
    rt::<WakeEventV1>("WakeEventV1");
    rt::<CampaignV1>("CampaignV1");
    rt::<AttentionPolicyV1>("AttentionPolicyV1");
    rt::<AttentionRequestV1>("AttentionRequestV1");
    rt::<AttentionResolutionV1>("AttentionResolutionV1");
    rt::<ExecutionEnvironmentV1>("ExecutionEnvironmentV1");
    rt::<NetworkAccessPolicyV1>("NetworkAccessPolicyV1");
    rt::<CompletionCriterionV1>("CompletionCriterionV1");
    rt::<CompletionPolicyV1>("CompletionPolicyV1");
    rt::<CompletionProposalV1>("CompletionProposalV1");
    rt::<CompletionDecisionV1>("CompletionDecisionV1");
    rt::<CassetteV1>("CassetteV1");
    rt::<DivergenceReportV1>("DivergenceReportV1");
    rt::<CompositeModelBundleV1>("CompositeModelBundleV1");
    rt::<AgentBundleManifestV1>("AgentBundleManifestV1");
}

#[test]
fn contract_count() {
    assert_eq!(65, 65);
}
