//! ContextPack Module
//!
//! Implements deterministic context bundling for WIH rehydration.
//! SYSTEM_LAW.md LAW-AUT-002 (Deterministic Rehydration Rule)

pub mod store;
pub mod types;

pub use store::{ContextPackStore, ContextPackStoreOptions};
pub use types::{
    generate_pack_id, ContextPackInputs, ContextPackQuery, ContextPackSeal, ContractFile,
    DeltaFile, InputManifestEntry, SealContextPackRequest, SealContextPackResponse, WIH,
    PolicyBundleRef, sha256_with_prefix,
};
