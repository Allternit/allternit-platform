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
//! | [`audit`] | The receipt log: per (principal, tenant) hash chain, Ed25519 signed, `append` + `verify_chain`; the tenant is always [`kernel_scope`] |
//! | [`approvals`] | [`approvals::SqlAuthority`]: request, resolve, redeem; receipts written before acting |
//! | [`check`] | `check` at every tool-execution path: classify, apply autonomy, redeem a token for ask/human-only classes, receipt first; `ALLTERNIT_KERNEL_CHECK=off\|shadow\|enforce` |
//! | [`adapters`] | Plan step 11a: the old stores (`factory_approvals`, `ApprovalStore` + ACI grants, `gateway_approvals`, `cowork_approvals`, `task_grants`) create, resolve and redeem through this one |
//! | [`routes`] | `POST /api/v1/kernel/approvals/{request,resolve,redeem}`; `GET /api/v1/kernel/receipts[/verify]`; resolve and verify need a person's session |
//!
//! Not here yet: `secret.use` (10), cloud-api `platform_approvals` and sign requests (11b),
//! budgets and leases (12).

pub mod adapters;
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

/// The one chain-scope rule (CONTRACT.md 11.2: a chain is per principal and tenant). The tenant
/// value is the caller's organization when there is one, else `AuthUser::tenant_id`, else
/// `default`. Every kernel path uses it: `check` (native, server tool, MCP, agent interface,
/// built-ins, computer_v2), the approval routes and the receipt routes. So an org user's check
/// receipts sit in the same chain `GET /api/v1/kernel/receipts` lists.
///
/// Organization first because the tool paths that carry no full `AuthUser` (MCP `tools/call`,
/// `execute_tool_internal`, `tool_auth_user`) only ever had the organization-or-tenant id.
pub fn kernel_scope(user: &crate::auth::AuthUser) -> String {
    scope_of(org_or_tenant(user))
}

/// The caller's organization id, else its tenant id (empty strings count as none). This is the
/// value `execute_tool_internal` and the MCP server already receive as their org id.
pub fn org_or_tenant(user: &crate::auth::AuthUser) -> Option<&str> {
    let nonempty = |v: Option<&str>| v.map(str::trim).filter(|s| !s.is_empty());
    nonempty(user.organization_id.as_deref()).or_else(|| nonempty(user.tenant_id.as_deref()))
}

/// [`kernel_scope`] for a path that only has the organization-or-tenant id (from
/// [`org_or_tenant`]); `None` or empty is `default`.
pub fn scope_of(org_or_tenant: Option<&str>) -> String {
    org_or_tenant
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| audit::DEFAULT_TENANT.to_string())
}
