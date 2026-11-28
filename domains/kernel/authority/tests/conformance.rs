//! Decodes the os/v1 conformance examples (PR #3279) with this crate's types. Valid examples must
//! decode; the invalid examples that serde can see (missing or extra fields, bad enum values)
//! must not. Pattern and conditional rules stay with the schema suite.
//! A valid `approval.resolve` request is deliberately not a `Request`: resolve is human-session only.
#![cfg(all(feature = "std", feature = "serde"))]
use allternit_authority::calls::{KernelRequest, KernelResponse};
use allternit_authority::*;
use serde::de::DeserializeOwned;

fn decodes<T: DeserializeOwned>(raw: &str) -> bool {
    serde_json::from_str::<T>(raw).is_ok()
}

macro_rules! ex {
    ($dir:literal, $name:literal) => {
        include_str!(concat!(
            "../../../../spec/Contracts/os/v1/conformance/examples/",
            $dir,
            "/",
            $name,
            ".json"
        ))
    };
}

#[test]
fn valid_examples_decode() {
    assert!(
        decodes::<ApprovalRedeemBody>(ex!("valid", "ApprovalRedeemBody")),
        "valid/ApprovalRedeemBody"
    );
    assert!(
        decodes::<ApprovalRedeemResult>(ex!("valid", "ApprovalRedeemResult")),
        "valid/ApprovalRedeemResult"
    );
    assert!(
        decodes::<ApprovalRequestV1>(ex!("valid", "ApprovalRequestBody")),
        "valid/ApprovalRequestBody"
    );
    assert!(
        decodes::<ApprovalRequestV1>(ex!("valid", "ApprovalRequestV1")),
        "valid/ApprovalRequestV1"
    );
    assert!(
        decodes::<ApprovalTokenV1>(ex!("valid", "ApprovalTokenV1")),
        "valid/ApprovalTokenV1"
    );
    assert!(
        decodes::<AuditAppendBody>(ex!("valid", "AuditAppendBody")),
        "valid/AuditAppendBody"
    );
    assert!(
        decodes::<BudgetCommitBody>(ex!("valid", "BudgetCommitBody")),
        "valid/BudgetCommitBody"
    );
    assert!(
        decodes::<BudgetReleaseBody>(ex!("valid", "BudgetReleaseBody")),
        "valid/BudgetReleaseBody"
    );
    assert!(
        decodes::<BudgetReservationV1>(ex!("valid", "BudgetReservationV1")),
        "valid/BudgetReservationV1"
    );
    assert!(
        decodes::<BudgetReservationV1>(ex!("valid", "BudgetReserveBody")),
        "valid/BudgetReserveBody"
    );
    assert!(
        decodes::<CheckExtensionV1>(ex!("valid", "CheckExtensionV1")),
        "valid/CheckExtensionV1"
    );
    assert!(
        decodes::<DeployAdmissionV1>(ex!("valid", "DeployAdmissionV1")),
        "valid/DeployAdmissionV1"
    );
    assert!(
        decodes::<DeployAdmissionV1>(ex!("valid", "DeployAdmitBody")),
        "valid/DeployAdmitBody"
    );
    assert!(
        decodes::<LeaseAcquireBody>(ex!("valid", "LeaseAcquireBody")),
        "valid/LeaseAcquireBody"
    );
    assert!(
        decodes::<LeaseExtensionV1>(ex!("valid", "LeaseExtensionV1")),
        "valid/LeaseExtensionV1"
    );
    assert!(
        decodes::<LeaseHeartbeatBody>(ex!("valid", "LeaseHeartbeatBody")),
        "valid/LeaseHeartbeatBody"
    );
    assert!(
        decodes::<LeaseReleaseBody>(ex!("valid", "LeaseReleaseBody")),
        "valid/LeaseReleaseBody"
    );
    assert!(
        decodes::<LeaseReleaseResult>(ex!("valid", "LeaseReleaseResult")),
        "valid/LeaseReleaseResult"
    );
    assert!(
        decodes::<SecretUseResult>(ex!("valid", "SecretUseResult")),
        "valid/SecretUseResult"
    );
    assert!(
        decodes::<SecretUseV1>(ex!("valid", "SecretUseV1")),
        "valid/SecretUseV1"
    );
    assert!(
        decodes::<KernelRequest>(ex!("valid", "KernelRequestV1.approval_request_by_run")),
        "valid/KernelRequestV1.approval_request_by_run"
    );
    assert!(
        decodes::<KernelResponse>(ex!("valid", "KernelResponseV1")),
        "valid/KernelResponseV1"
    );
}

#[test]
fn invalid_examples_are_rejected() {
    assert!(
        !decodes::<ApprovalRedeemBody>(ex!("invalid", "ApprovalRedeemBody.no_payload_hash")),
        "invalid/ApprovalRedeemBody.no_payload_hash"
    );
    assert!(
        !decodes::<BudgetReleaseBody>(ex!("invalid", "BudgetReleaseBody.extra_field")),
        "invalid/BudgetReleaseBody.extra_field"
    );
    assert!(
        !decodes::<BudgetCommitBody>(ex!("invalid", "BudgetCommitBody.missing_idempotency_key")),
        "invalid/BudgetCommitBody.missing_idempotency_key"
    );
    assert!(
        !decodes::<BudgetReservationV1>(ex!("invalid", "BudgetReserveBody.missing_scope")),
        "invalid/BudgetReserveBody.missing_scope"
    );
    assert!(
        !decodes::<DeployAdmissionV1>(ex!("invalid", "DeployAdmitBody.missing_command_preview")),
        "invalid/DeployAdmitBody.missing_command_preview"
    );
    assert!(
        !decodes::<LeaseAcquireBody>(ex!("invalid", "LeaseAcquireBody.bad_reclaim_policy")),
        "invalid/LeaseAcquireBody.bad_reclaim_policy"
    );
    assert!(
        !decodes::<LeaseReleaseBody>(ex!("invalid", "LeaseReleaseBody.missing_lease_id")),
        "invalid/LeaseReleaseBody.missing_lease_id"
    );
    assert!(
        !decodes::<CheckExtensionV1>(ex!("invalid", "CheckExtensionV1.unknown_autonomy")),
        "invalid/CheckExtensionV1.unknown_autonomy"
    );
    assert!(
        !decodes::<SecretUseV1>(ex!("invalid", "SecretUseV1.carries_secret_value")),
        "invalid/SecretUseV1.carries_secret_value"
    );
    assert!(
        !decodes::<AuditAppendBody>(ex!("invalid", "AuditAppendBody.caller_sets_prev_hash")),
        "invalid/AuditAppendBody.caller_sets_prev_hash"
    );
    assert!(
        !decodes::<ApprovalRequestV1>(ex!("invalid", "ApprovalRequestBody.missing_action_payload")),
        "invalid/ApprovalRequestBody.missing_action_payload"
    );
    assert!(
        !decodes::<ApprovalTokenV1>(ex!("invalid", "ApprovalTokenV1.no_payload_hash")),
        "invalid/ApprovalTokenV1.no_payload_hash"
    );
    assert!(
        !decodes::<KernelRequest>(ex!("invalid", "KernelRequestV1.wrong_body_for_call")),
        "invalid/KernelRequestV1.wrong_body_for_call"
    );
    assert!(
        !decodes::<KernelRequest>(ex!("invalid", "KernelRequestV1.resolve_by_agent")),
        "invalid/KernelRequestV1.resolve_by_agent"
    );
    assert!(
        !decodes::<KernelRequest>(ex!("invalid", "KernelRequestV1.resolve_by_service")),
        "invalid/KernelRequestV1.resolve_by_service"
    );
}

#[test]
fn resolve_has_no_request_variant() {
    // Valid on the wire (a human session sends it), but never an `Authority` request.
    assert!(!decodes::<KernelRequest>(ex!("valid", "KernelRequestV1")));
}
