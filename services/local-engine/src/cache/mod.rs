//! Local model cache.

pub mod model;
pub mod store;

pub use model::{CachedModel, FileVerification, ModelSource, ModelStatus};
pub use store::ModelStore;
