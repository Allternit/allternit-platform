//! Artifacts v2: one account-level store of typed artifacts (contract:
//! docs/design/artifacts-v2.md). Routes live in `routes::artifacts_v2`;
//! this module holds the pieces they share and that unit tests pin down:
//! the kind list, ids, access computation and the sharing rules.

pub mod access;
pub mod error;
pub mod ids;
pub mod kinds;
pub mod sharing;

/// Largest version body (contract §2: `body` ≤ 16 MiB).
pub const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;
/// Request-size ceiling for routes that carry a body: JSON escaping can grow
/// a 16 MiB body, so the transport limit sits above it and the handler
/// enforces [`MAX_BODY_BYTES`] on the decoded body (413 `body_too_large`).
pub const MAX_REQUEST_BYTES: usize = 40 * 1024 * 1024;
