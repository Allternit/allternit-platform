// Factory part `gate`: the only writer (SPEC §5).
pub mod constraints;
pub mod egress;
pub mod fence;
pub mod hook;
pub mod killswitch;
pub mod policy;
pub mod errors;
pub mod gate;
#[cfg(test)]
pub mod tests;

pub use errors::GateError;
pub use gate::{
    AutolandImpact, AutolandResult, DagMutation, Gate, GateOptions, GateResult, MutationProvenance,
    PromptOrigin,
    WihPickup, WihPickupOptions, CONTEXT_PACK_OUTPUT_INLINE_CAP,
};

// Re-export visual verification types for convenience
pub use crate::verification::types::{Evidence, ProviderError, VerificationProvider, VisualConfig};
