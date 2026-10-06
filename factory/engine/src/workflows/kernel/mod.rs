//! Kernel ABI 1.0.0 (frozen) — typed lifecycle, primitive registry,
//! ComputeGraphIR validator and the WIH DAG projection.
//!
//! Authority: `spec/Contracts/kernel/v1`. Decisions Q2 (Work Runtime ledger
//! owns lifecycle) and Q3 (ComputeGraphIR is authoritative; the WIH DAG is a
//! one-way projection).

pub mod bug_fix;
pub mod classes;
pub mod graph;
pub mod isa;
pub mod lifecycle;
pub mod projection;
pub mod registry;
pub mod router;
pub mod s1_outcome;

pub use lifecycle::{CloseOutcome, LifecycleError, NodeState};
