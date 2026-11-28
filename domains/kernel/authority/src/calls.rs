//! Request and response enums for the calls (CONTRACT.md 2.0 to 2.7), plus the wire envelopes.
//! `approval.resolve` has no variant: it is human-session only and never goes through this path.
use crate::error::AuthorityError;
use crate::types::*;
use alloc::string::String;

/// One request. On the wire it is the envelope's `call` + `body` pair (adjacent tagging).
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "call", content = "body"))]
pub enum Request {
    #[cfg_attr(feature = "serde", serde(rename = "check"))]
    Check(PolicyCheck),
    #[cfg_attr(feature = "serde", serde(rename = "lease.acquire"))]
    LeaseAcquire(LeaseAcquireBody),
    #[cfg_attr(feature = "serde", serde(rename = "lease.heartbeat"))]
    LeaseHeartbeat(LeaseHeartbeatBody),
    #[cfg_attr(feature = "serde", serde(rename = "lease.release"))]
    LeaseRelease(LeaseReleaseBody),
    #[cfg_attr(feature = "serde", serde(rename = "secret.use"))]
    SecretUse(SecretUseV1),
    #[cfg_attr(feature = "serde", serde(rename = "approval.request"))]
    ApprovalRequest(ApprovalRequestV1),
    #[cfg_attr(feature = "serde", serde(rename = "approval.redeem"))]
    ApprovalRedeem(ApprovalRedeemBody),
    #[cfg_attr(feature = "serde", serde(rename = "audit.append"))]
    AuditAppend(AuditAppendBody),
    #[cfg_attr(feature = "serde", serde(rename = "budget.reserve"))]
    BudgetReserve(BudgetReservationV1),
    #[cfg_attr(feature = "serde", serde(rename = "budget.commit"))]
    BudgetCommit(BudgetCommitBody),
    #[cfg_attr(feature = "serde", serde(rename = "budget.release"))]
    BudgetRelease(BudgetReleaseBody),
    #[cfg_attr(feature = "serde", serde(rename = "deploy.admit"))]
    DeployAdmit(DeployAdmissionV1),
}

/// One success response. On the wire it is the envelope's `body` (untagged: the `id` echo pairs it
/// with its request).
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(untagged))]
pub enum Response {
    Check(PolicyDecision),
    /// `lease.acquire` and `lease.heartbeat`.
    Lease(Lease),
    LeaseReleased(LeaseReleaseResult),
    SecretUse(SecretUseResult),
    ApprovalRequest(ApprovalRequestV1),
    ApprovalRedeem(ApprovalRedeemResult),
    AuditAppend(ReceiptChain),
    BudgetReserve(BudgetReservationV1),
    BudgetCommit(BudgetReservationV1),
    BudgetRelease(BudgetReservationV1),
    DeployAdmit(DeployAdmissionV1),
}

/// Request envelope: `{"v":1,"id":..,"call":..,"caller":..,"body":..}`.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct KernelRequest {
    pub v: u8,
    pub id: String,
    pub caller: Caller,
    #[cfg_attr(feature = "serde", serde(flatten))]
    pub request: Request,
}

/// Response envelope: success carries `body`, failure carries `error`.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct KernelResponse {
    pub v: u8,
    pub id: String,
    pub ok: bool,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub body: Option<JsonObject>,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub error: Option<AuthorityError>,
}

#[cfg(feature = "serde")]
impl KernelResponse {
    /// Wrap a call result for the wire, echoing the request `id`.
    pub fn from_result(id: String, result: Result<Response, AuthorityError>) -> Self {
        match result {
            Ok(resp) => match serde_json::to_value(&resp) {
                Ok(serde_json::Value::Object(map)) => Self {
                    v: crate::PROTOCOL_VERSION,
                    id,
                    ok: true,
                    body: Some(map),
                    error: None,
                },
                _ => Self::failed(
                    id,
                    AuthorityError::kernel_unavailable("response did not serialize to an object"),
                ),
            },
            Err(e) => Self::failed(id, e),
        }
    }

    fn failed(id: String, error: AuthorityError) -> Self {
        Self {
            v: crate::PROTOCOL_VERSION,
            id,
            ok: false,
            body: None,
            error: Some(error),
        }
    }
}
