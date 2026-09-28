//! Reaching computers on the Allternit mesh (ACI P4 remote computers).
//!
//! The API can't route to tailnet addresses itself: the mesh runs in
//! userspace (`mesh-node`, tsnet). Desktop main owns the mesh and, on
//! request, starts `mesh-node --reverse <target>`, which listens on a
//! loopback port and dials the tailnet target per connection. This module
//! asks main for that loopback address over a local endpoint
//! (`ALLTERNIT_MESH_BRIDGE_URL`), authenticated with the API's spawn-time
//! desktop secret, which both processes share.

use serde::Deserialize;

/// Computers on remote machines paired over Fabric Transport.
pub const FABRIC_PROVIDER: &str = "fabric";
/// Where a node exposes its VNC server on the tailnet.
pub const NODE_VNC_PORT: u16 = 5900;

#[derive(Deserialize)]
struct BridgeReply {
    address: String,
}

/// `host:port` must be a tailnet address (100.64.0.0/10) — the bridge only
/// dials the mesh, never arbitrary hosts.
pub fn is_mesh_target(target: &str) -> bool {
    let Some((host, port)) = target.rsplit_once(':') else { return false };
    if port.parse::<u16>().is_err() {
        return false;
    }
    match host.parse::<std::net::Ipv4Addr>() {
        Ok(ip) => {
            let [a, b, ..] = ip.octets();
            a == 100 && (64..=127).contains(&b)
        }
        Err(_) => false,
    }
}

/// A loopback `127.0.0.1:port` that reaches `target` on the mesh.
pub async fn loopback_for(target: &str) -> Result<String, String> {
    if !is_mesh_target(target) {
        return Err(format!("{target} is not a mesh address"));
    }
    let base = std::env::var("ALLTERNIT_MESH_BRIDGE_URL")
        .ok()
        .filter(|s| !s.is_empty())
        .ok_or("the mesh isn't available here (no mesh bridge)")?;
    let secret = std::env::var("ALLTERNIT_DESKTOP_ACCESS_TOKEN").unwrap_or_default();
    let reply = reqwest::Client::new()
        .post(format!("{}/mesh/tcp", base.trim_end_matches('/')))
        .header("x-allternit-desktop-access-token", secret)
        .json(&serde_json::json!({ "target": target }))
        .timeout(std::time::Duration::from_secs(90))
        .send()
        .await
        .map_err(|e| format!("mesh bridge unreachable: {e}"))?;
    if !reply.status().is_success() {
        let status = reply.status();
        let body = reply.text().await.unwrap_or_default();
        return Err(format!("mesh bridge returned {status}: {}", body.chars().take(200).collect::<String>()));
    }
    let BridgeReply { address } = reply.json().await.map_err(|e| format!("bad mesh bridge reply: {e}"))?;
    Ok(address)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_tailnet_targets() {
        assert!(is_mesh_target("100.64.0.7:5900"));
        assert!(is_mesh_target("100.127.255.1:5900"));
        assert!(!is_mesh_target("100.128.0.1:5900"));
        assert!(!is_mesh_target("10.0.0.5:5900"));
        assert!(!is_mesh_target("example.com:5900"));
        assert!(!is_mesh_target("100.64.0.7"));
    }
}
