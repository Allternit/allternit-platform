//! Computers console node routes on this runtime: `/v1/host/*`, served by
//! the shared `host_probe::http` router (the ao shim mounts the same one).
//!
//! Credentials come from [`crate::relay_auth::process_secret`]: the relay key
//! (`sha256_hex(device_token)`) checks both the cloud's relay signature on
//! relayed calls and the cloud's `ap1.*` approval ids, and the runtime id
//! binds an approval to this computer. An unpaired runtime refuses every
//! action (`DenyApprovals`) and every relayed call (503).
//!
//! Scope rule: a relayed call must carry the cloud's relay signature, which
//! the cloud only adds after its `runtime:execute` check on the pairing
//! (`runtime_relay::required_capability`). A local call (the Desktop UI on
//! loopback) passes the same `auth_middleware` as every other protected
//! route; it still cannot run an action without a cloud-signed approval.

use std::sync::{Arc, OnceLock};

use axum::Router;
use host_probe::http::{HostApi, NodeCredentials};

use crate::AppState;

/// This runtime's credentials, read fresh each time (the identity file can
/// rotate).
fn credentials() -> Option<NodeCredentials> {
    let secret = crate::relay_auth::process_secret();
    let (relay_key, owner) = secret.credentials()?;
    Some(NodeCredentials {
        relay_key,
        owner,
        runtime_id: secret.runtime_id().unwrap_or_default(),
    })
}

/// One `HostApi` per process: one snapshot sampler (real network rates) and
/// one findings cache, shared by the routes and the presence heartbeat.
pub fn host_api() -> Arc<HostApi> {
    static API: OnceLock<Arc<HostApi>> = OnceLock::new();
    API.get_or_init(|| HostApi::new(Arc::new(credentials)))
        .clone()
}

/// `/v1/host/*`, mounted on the protected (auth_middleware) router.
pub fn router() -> Router<Arc<AppState>> {
    host_probe::http::router(host_api())
}
