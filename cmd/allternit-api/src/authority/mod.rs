//! AllternitOS authority kernel, hosted in allternit-api (Stage 1, plan step 7).
//!
//! Implements the `allternit-authority` crate's approval calls over SQLite (migration V300):
//! `approval.request`, `approval.resolve` (human session only) and `approval.redeem`.
//! Spec: `handoffs/allternit-os-2026-11-27/CONTRACT.md` sections 2.4, 3.1, 3.2, 4.3, 5, 11, 12.
//!
//! | Module | What |
//! |---|---|
//! | [`tokens`] | `ApprovalTokenV1`: Ed25519 mint/verify, TTL 300 s default, 900 s cap, the kernel key |
//! | [`keystore`] | Where the seed lives: OS keystore via Desktop (stdin hand-off), 0600 file headless (CONTRACT.md 12.3) |
//! | [`presence`] | Desktop approve needs Electron main's user-presence proof (Touch ID / Windows Hello) |
//! | [`audit`] | The receipt log: per (principal, tenant) hash chain, Ed25519 signed, `append` + `verify_chain` |
//! | [`approvals`] | [`approvals::SqlAuthority`]: request, resolve, redeem; receipts written before acting |
//! | [`check`] | `check` at every tool-execution path: classify, apply autonomy, redeem a token for ask/human-only classes, receipt first; `ALLTERNIT_KERNEL_CHECK=off\|shadow\|enforce` |
//! | [`routes`] | `POST /api/v1/kernel/approvals/{request,resolve,redeem}`; `GET /api/v1/kernel/receipts[/verify]`; resolve and verify need a person's session |
//!
//! Not here yet: `secret.use` (10), moving the nine old
//! approval stores onto this one (11), budgets and leases (12).

pub mod approvals;
pub mod audit;
pub mod check;
pub mod keystore;
pub mod presence;
pub mod routes;
pub mod tokens;

/// `ReceiptChainV1.domain`: one fixed ABI value; the kernel kind rides in `payload.kind` (CONTRACT.md 11.2).
pub const RECEIPT_DOMAIN: &str = "allternit.receipt.v1";
/// `prev_hash` of the first receipt in a chain.
pub const GENESIS_HASH: &str = "sha256:0000000000000000000000000000000000000000000000000000000000000000";
