//! CommRails bridge: a scoped, default-off HTTP listener that lets a remote
//! agent (e.g. Chief on the shared box, over the mesh) create and read WIH
//! DAGs and send/read coordination mail — and nothing else.
//!
//! Threat model and enablement: `spec/BRIDGE.md`.

pub mod identity;
pub mod server;

pub use identity::{
    default_identities_path, is_forbidden_scope, parse_grant_scopes, Identity, IdentityRecord,
    IdentityStore, IssuedIdentity, Scope, GRANTABLE_SCOPES,
};
pub use server::{
    check_bind, classify_route, router, serve, serve_listener, BridgeConfig, BridgeState,
    RouteClass, DEFAULT_BRIDGE_BIND, DEFAULT_RATE_LIMIT_PER_MIN,
};
