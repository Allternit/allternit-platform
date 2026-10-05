//! ao Fabric Transport node (P3, spec: Allternit Brain/Research/specs/
//! ao-fabric-node.md; spike: Research/drafts/spike-p3-clerk-device-auth.md).
//!
//! Makes the `ao` binary a pairable Fabric node: `ao fabric pair|serve|status`.
//! Pairing is Allternit's own 3-leg Ed25519 protocol (the node never holds a
//! Clerk token); `serve` runs the loopback HTTP+WS shim (this binary, port
//! 8014 default) plus a line-faithful Rust port of cmd/agent-daemon's relay
//! client so the cloud can tunnel PWA session-pickup traffic to the engine
//! socket API (session `ao`).

pub(crate) mod cli;
pub(crate) mod cloud;
pub(crate) mod identity;
pub(crate) mod relay;
pub(crate) mod shim;
pub(crate) mod wire;

pub(crate) const DEFAULT_AO_PORT: u16 = 8014;

/// Default loopback gateway the relay client forwards tunneled traffic to
/// (the shim). Distinct from Desktop's 8013 so both can coexist (spike Q5.5).
pub(crate) fn local_gateway_url() -> String {
    std::env::var("ALLTERNIT_AO_GATEWAY_URL")
        .unwrap_or_else(|_| format!("http://127.0.0.1:{DEFAULT_AO_PORT}"))
        .trim_end_matches('/')
        .to_string()
}

/// Loopback gateway the relay forwards tunneled traffic to. `ALLTERNIT_AO_GATEWAY_URL`
/// wins when set (and non-empty); otherwise the gateway is the shim's own serve
/// port — the relay and the shim must agree, or tunneled requests land on whatever
/// unrelated service holds the default port (Desktop's connector sidecar holds
/// 8014 and answers foreign `/v1/*` traffic with a 401, 2026-09-10).
pub(crate) fn resolve_local_gateway(port: u16, env_override: Option<String>) -> String {
    env_override
        .map(|value| value.trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| format!("http://127.0.0.1:{port}"))
}

#[cfg(test)]
mod tests {
    use super::{resolve_local_gateway, DEFAULT_AO_PORT};

    #[test]
    fn gateway_env_override_wins_and_trims_trailing_slash() {
        assert_eq!(
            resolve_local_gateway(8015, Some("http://127.0.0.1:9999/".to_string())),
            "http://127.0.0.1:9999"
        );
    }

    #[test]
    fn gateway_empty_env_override_falls_back_to_serve_port() {
        assert_eq!(resolve_local_gateway(8015, Some(String::new())), "http://127.0.0.1:8015");
        assert_eq!(
            resolve_local_gateway(DEFAULT_AO_PORT, None),
            format!("http://127.0.0.1:{DEFAULT_AO_PORT}")
        );
    }
}

pub(crate) fn cloud_api_url() -> String {
    std::env::var("ALLTERNIT_CLOUD_API_URL")
        .unwrap_or_else(|_| "https://api.allternit.com".to_string())
        .trim_end_matches('/')
        .to_string()
}
