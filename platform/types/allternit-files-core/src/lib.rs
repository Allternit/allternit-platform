//! Files drive wire types (protocol `files/2`).
//!
//! Everything in this crate is generated from `contracts/files-drive/schema/*.json`
//! by `node contracts/files-drive/generate.mjs`. Change the schema, not this crate.
//! The spec is `contracts/files-drive/SPEC.md`.

mod generated;
pub use generated::*;

/// Protocol name the runtime answers on `GET /api/v1/files/v2/ping`.
pub const PROTOCOL: &str = "files/2";
