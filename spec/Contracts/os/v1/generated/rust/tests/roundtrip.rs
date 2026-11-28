//! Round-trips every valid os/v1 conformance example through the serde types.
//! Not compiled in the coding session (train guard blocks cargo); the build train runs `cargo test` here.
use allternit_os_contracts::*;
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
    rt::<ApprovalRedeemBody>("ApprovalRedeemBody");
    rt::<ApprovalRedeemResult>("ApprovalRedeemResult");
    rt::<ApprovalRequestBody>("ApprovalRequestBody");
    rt::<ApprovalRequestV1>("ApprovalRequestV1");
    rt::<ApprovalResolveBody>("ApprovalResolveBody");
    rt::<ApprovalTokenV1>("ApprovalTokenV1");
    rt::<AuditAppendBody>("AuditAppendBody");
    rt::<BudgetCommitBody>("BudgetCommitBody");
    rt::<BudgetReleaseBody>("BudgetReleaseBody");
    rt::<BudgetReservationV1>("BudgetReservationV1");
    rt::<BudgetReserveBody>("BudgetReserveBody");
    rt::<CheckExtensionV1>("CheckExtensionV1");
    rt::<DeployAdmissionV1>("DeployAdmissionV1");
    rt::<DeployAdmitBody>("DeployAdmitBody");
    rt::<KernelRequestV1>("KernelRequestV1");
    rt::<KernelResponseV1>("KernelResponseV1");
    rt::<LeaseAcquireBody>("LeaseAcquireBody");
    rt::<LeaseExtensionV1>("LeaseExtensionV1");
    rt::<LeaseHeartbeatBody>("LeaseHeartbeatBody");
    rt::<LeaseOsV1>("LeaseOsV1");
    rt::<LeaseReleaseBody>("LeaseReleaseBody");
    rt::<LeaseReleaseResult>("LeaseReleaseResult");
    rt::<PolicyCheckOsV1>("PolicyCheckOsV1");
    rt::<PolicyDecisionOsV1>("PolicyDecisionOsV1");
    rt::<SecretUseResult>("SecretUseResult");
    rt::<SecretUseV1>("SecretUseV1");
    rt::<InboxItemV1>("InboxItemV1");
    rt::<InboxListResult>("InboxListResult");
    rt::<InboxPrepareResult>("InboxPrepareResult");
    rt::<InboxSnoozeResult>("InboxSnoozeResult");
    rt::<InboxToolInputV1>("InboxToolInputV1");
    rt::<ShellCommandV1>("ShellCommandV1");
    rt::<ShellStateV1>("ShellStateV1");
    rt::<ShellToolResultV1>("ShellToolResultV1");
    rt::<ViewDescriptorV1>("ViewDescriptorV1");
    rt::<KernelRequestV1>("KernelRequestV1.approval_request_by_run");
    rt::<ShellToolResultV1>("ShellToolResultV1.fork_app");
    rt::<ShellCommandV1>("ShellCommandV1.fork_app");
}

#[test]
fn inbox_tool_rejects_approve() {
    let bad = serde_json::json!({"action": "approve", "id": "apr_01"});
    assert!(serde_json::from_value::<InboxToolInputV1>(bad).is_err());
}
