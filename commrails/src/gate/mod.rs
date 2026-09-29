pub mod errors;
pub mod gate;
#[cfg(test)]
pub mod tests;

pub use errors::GateError;
pub use gate::{
    AutolandImpact, AutolandResult, DagMutation, Gate, GateOptions, GateResult, MutationProvenance,
    WihPickup, WihPickupOptions, CONTEXT_PACK_OUTPUT_INLINE_CAP,
};

// Re-export visual verification types for convenience
pub use crate::verification::types::{Evidence, ProviderError, VerificationProvider, VisualConfig};
