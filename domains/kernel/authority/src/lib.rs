//! `allternit-authority`: the AllternitOS kernel interface (plan step 4).
//!
//! The six calls plus admit, as one synchronous [`Authority`] trait, and the os/v1 wire types they
//! carry. Types and the pure policy table only: no store, no signing, no network.
//!
//! The core is `no_std` + `alloc` (build with `--no-default-features`). The same messages run
//! in-process ([`Authority::call`]), over a Unix socket (length-prefixed JSON, [`framing`], std only)
//! or as kernel IPC. Spec: `handoffs/allternit-os-2026-11-27/CONTRACT.md` sections 2 to 5 and 11.
//! Wire shapes match `spec/Contracts/os/v1` field for field (the train checks they agree).
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod calls;
pub mod error;
#[cfg(all(feature = "std", feature = "serde"))]
pub mod framing;
pub mod policy;
pub mod types;

pub use calls::{Request, Response};
pub use error::AuthorityError;
pub use types::*;

/// os/v1 package version these types match.
pub const OS_CONTRACTS_VERSION: &str = "1.0.0";
/// Wire protocol version carried in every envelope (`"v": 1`).
pub const PROTOCOL_VERSION: u8 = 1;
/// Max frame body on the socket transport: 1 MiB.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// The kernel interface. Every call is synchronous and fails closed: an implementation that cannot
/// reach its store or write the receipt returns `ErrorCode::KernelUnavailable`, and the caller treats
/// that as DENY. A denial still writes a receipt, so `AuthorityError::receipt_id` is set for every
/// error except `bad_request`, `frame_too_large` and `unsupported_version`.
///
/// `caller` is set by the transport (session type, or `SO_PEERCRED`), never by the message body.
///
/// **`approval.resolve` is deliberately not in this trait.** Only a human session route (Clerk
/// session on web, phone and PWA; the local owner session on Desktop and the TUI) may resolve an
/// approval, and it reaches the store directly. Nothing that holds an `Authority` handle, and so
/// nothing a model or run can call, has a way to approve (CONTRACT.md 4.3).
pub trait Authority {
    /// `check`: policy decision for one action. The kernel computes `human_approval_state` itself
    /// from the token store; a model cannot claim `APPROVED`. A human-only class is never ALLOW
    /// without a redeemed token for the same `action_hash`.
    fn check(
        &mut self,
        caller: &Caller,
        body: PolicyCheck,
    ) -> Result<PolicyDecision, AuthorityError>;

    /// `lease.acquire`, `lease.heartbeat` and `lease.release`.
    fn lease(
        &mut self,
        caller: &Caller,
        req: LeaseRequest,
    ) -> Result<LeaseResponse, AuthorityError>;

    /// `secret.use`: bind a credential by handle. No secret value appears in the request or reply.
    fn secret_use(
        &mut self,
        caller: &Caller,
        body: SecretUseV1,
    ) -> Result<SecretUseResult, AuthorityError>;

    /// `approval.request`: create an `ApprovalRequestV1` and its open attention request.
    fn approval_request(
        &mut self,
        caller: &Caller,
        body: ApprovalRequestV1,
    ) -> Result<ApprovalRequestV1, AuthorityError>;

    /// `approval.redeem`: spend a token, atomically with the redemption receipt, before the action runs.
    fn approval_redeem(
        &mut self,
        caller: &Caller,
        body: ApprovalRedeemBody,
    ) -> Result<ApprovalRedeemResult, AuthorityError>;

    /// `audit.append`: append to the receipt chain. The caller never sets `prev_hash`, `hash` or `signature`.
    fn audit_append(
        &mut self,
        caller: &Caller,
        body: AuditAppendBody,
    ) -> Result<ReceiptChain, AuthorityError>;

    /// `budget.reserve`: reserve before spend.
    fn budget_reserve(
        &mut self,
        caller: &Caller,
        body: BudgetReservationV1,
    ) -> Result<BudgetReservationV1, AuthorityError>;

    /// `budget.commit`: commit actual spend and release the unused remainder.
    fn budget_commit(
        &mut self,
        caller: &Caller,
        body: BudgetCommitBody,
    ) -> Result<BudgetReservationV1, AuthorityError>;

    /// `budget.release`: give back a reservation without committing (part of `budget.*`, CONTRACT.md 2.6).
    fn budget_release(
        &mut self,
        caller: &Caller,
        body: BudgetReleaseBody,
    ) -> Result<BudgetReservationV1, AuthorityError>;

    /// `deploy.admit`: the only way a deploy starts. Always a human-only class.
    fn deploy_admit(
        &mut self,
        caller: &Caller,
        body: DeployAdmissionV1,
    ) -> Result<DeployAdmissionV1, AuthorityError>;

    /// In-process transport: dispatch one [`Request`] to the matching method.
    fn call(&mut self, caller: &Caller, req: Request) -> Result<Response, AuthorityError> {
        Ok(match req {
            Request::Check(b) => Response::Check(self.check(caller, b)?),
            Request::LeaseAcquire(b) => {
                lease_response(self.lease(caller, LeaseRequest::Acquire(b))?)
            }
            Request::LeaseHeartbeat(b) => {
                lease_response(self.lease(caller, LeaseRequest::Heartbeat(b))?)
            }
            Request::LeaseRelease(b) => {
                lease_response(self.lease(caller, LeaseRequest::Release(b))?)
            }
            Request::SecretUse(b) => Response::SecretUse(self.secret_use(caller, b)?),
            Request::ApprovalRequest(b) => {
                Response::ApprovalRequest(self.approval_request(caller, b)?)
            }
            Request::ApprovalRedeem(b) => {
                Response::ApprovalRedeem(self.approval_redeem(caller, b)?)
            }
            Request::AuditAppend(b) => Response::AuditAppend(self.audit_append(caller, b)?),
            Request::BudgetReserve(b) => Response::BudgetReserve(self.budget_reserve(caller, b)?),
            Request::BudgetCommit(b) => Response::BudgetCommit(self.budget_commit(caller, b)?),
            Request::BudgetRelease(b) => Response::BudgetRelease(self.budget_release(caller, b)?),
            Request::DeployAdmit(b) => Response::DeployAdmit(self.deploy_admit(caller, b)?),
        })
    }
}

fn lease_response(r: LeaseResponse) -> Response {
    match r {
        LeaseResponse::Lease(l) => Response::Lease(l),
        LeaseResponse::Released(r) => Response::LeaseReleased(r),
    }
}
