//! AllternitOS contracts os/v1 (DRAFT, gate G0): serde types.
//!
//! HAND-WRITTEN in the coding session that created this package: the train guard blocks
//! cargo there, so cargo-typify could not run. `generated.rs` matches `schemas/*.json` by hand.
//! The build train replaces it with `scripts/regenerate.sh` (cargo-typify 0.8.0, same as the
//! frozen ABI crate) and re-hashes MANIFEST.json. Conditional rules (if/then) and patterns
//! are enforced by schema validation, not by these types. Frozen ABI types are reused from
//! `allternit-kernel-abi`, never redefined.
#![allow(clippy::all, missing_docs)]

mod generated;
pub use generated::*;

/// os/v1 package version these types match.
pub const OS_CONTRACTS_VERSION: &str = "1.0.0";
/// Frozen Kernel ABI version this package references.
pub const REQUIRED_ABI_VERSION: &str = allternit_kernel_abi::ABI_VERSION;
