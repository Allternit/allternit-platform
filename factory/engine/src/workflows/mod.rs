//! Factory part `workflows`: templates (+ the `kernel` template compiler), drive, wake,
//! wait-gates, leases (SPEC §5).

pub mod dependencies;
pub mod drive;
pub mod kernel;
pub mod leases;
pub mod merge_locks;
pub mod templates;
pub mod wait_gates;
pub mod wake;
