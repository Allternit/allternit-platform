//! Shared egress guard: the one place that decides whether Allternit code may
//! open a connection to a destination. Used today by the web proxy and the
//! design connector import. Connectors, MCP fetch, Decision Runtime clients
//! and spawned-harness egress are meant to route through it too, but are not
//! wired yet (spawned harnesses are only recorded in ExecutionEnvironmentV1).
//!
//! Policy: only publicly routable unicast addresses are allowed. Loopback,
//! private, link-local (cloud metadata 169.254.169.254), CGNAT (the Fabric
//! mesh), documentation, multicast and reserved ranges are refused, as are
//! IPv6 forms embedding a non-public IPv4. Domain names are resolved exactly
//! once by [`PublicOnlyResolver`] and the connector dials those same
//! addresses, so DNS rebinding cannot swap in a private address between check
//! and connect. Callers that cannot plug a resolver into their client use
//! [`resolve_public`] and dial the returned addresses themselves.

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use std::{
    error::Error as StdError,
    fmt,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
};

/// Literal-host check for URLs: true when `host` is an IP literal (or a
/// well-known local name) that must never be dialed.
pub fn host_is_forbidden_literal(host: &str) -> bool {
    let h = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = h.parse::<IpAddr>() {
        return !is_public_ip(ip);
    }
    let lower = h.trim_end_matches('.').to_ascii_lowercase();
    lower == "localhost" || lower.ends_with(".localhost") || lower.ends_with(".local")
}

/// Resolve `host` once and fail unless every address is public. Returns the
/// addresses that were vetted; dial exactly these.
pub async fn resolve_public(host: &str) -> Result<Vec<SocketAddr>, Box<dyn StdError + Send + Sync>> {
    PublicOnlyResolver::resolve_public(host).await
}

/// Whether `ip` is a publicly routable unicast address the proxy may connect
/// to. Everything special-purpose (loopback, private, link-local, CGNAT,
/// benchmarking, documentation, multicast, reserved, …) is rejected, and IPv6
/// forms that embed an IPv4 address are judged by the embedded address.
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_ipv4(v4),
        IpAddr::V6(v6) => is_public_ipv6(v6),
    }
}

pub fn is_public_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    let blocked = a == 0 // 0.0.0.0/8 "this network" (incl. unspecified)
        || a == 10 // 10/8 private
        || (a == 100 && (64..=127).contains(&b)) // 100.64/10 CGNAT (Fabric mesh)
        || a == 127 // loopback
        || (a == 169 && b == 254) // link-local (incl. cloud metadata)
        || (a == 172 && (16..=31).contains(&b)) // 172.16/12 private
        || (a == 192 && b == 0 && c == 0) // 192.0.0/24 IETF protocol assignments
        || (a == 192 && b == 0 && c == 2) // 192.0.2/24 TEST-NET-1
        || (a == 192 && b == 88 && c == 99) // 192.88.99/24 6to4 relay anycast
        || (a == 192 && b == 168) // 192.168/16 private
        || (a == 198 && (b == 18 || b == 19)) // 198.18/15 benchmarking
        || (a == 198 && b == 51 && c == 100) // TEST-NET-2
        || (a == 203 && b == 0 && c == 113) // TEST-NET-3
        || a >= 224; // multicast 224/4, reserved 240/4, broadcast
    !blocked
}

pub fn is_public_ipv6(ip: Ipv6Addr) -> bool {
    let seg = ip.segments();
    // IPv4-mapped ::ffff:a.b.c.d
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_public_ipv4(v4);
    }
    // IPv4-compatible ::a.b.c.d (deprecated) — also covers :: and ::1, whose
    // embedded 0.0.0.0 / 0.0.0.1 are blocked by the v4 rules.
    if seg[..6] == [0; 6] {
        return is_public_ipv4(Ipv4Addr::new(
            (seg[6] >> 8) as u8,
            seg[6] as u8,
            (seg[7] >> 8) as u8,
            seg[7] as u8,
        ));
    }
    // NAT64 well-known prefix 64:ff9b::/96 — judge the embedded v4.
    if seg[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        return is_public_ipv4(Ipv4Addr::new(
            (seg[6] >> 8) as u8,
            seg[6] as u8,
            (seg[7] >> 8) as u8,
            seg[7] as u8,
        ));
    }
    // 6to4 2002::/16 — the v4 lives in bits 16..48.
    if seg[0] == 0x2002 {
        return is_public_ipv4(Ipv4Addr::new(
            (seg[1] >> 8) as u8,
            seg[1] as u8,
            (seg[2] >> 8) as u8,
            seg[2] as u8,
        ));
    }
    // Only global unicast 2000::/3 is eligible. This excludes ULA fc00::/7,
    // link-local fe80::/10, site-local fec0::/10, multicast ff00::/8, discard
    // 100::/64, NAT64 local-use 64:ff9b:1::/48 and everything unassigned.
    if seg[0] & 0xe000 != 0x2000 {
        return false;
    }
    match seg[0] {
        // 2001::/23 IETF special (Teredo, ORCHID, …) and 2001:db8::/32 docs
        0x2001 => seg[1] >= 0x0200 && seg[1] != 0x0db8,
        // 3fff::/20 documentation
        0x3fff => seg[1] >= 0x1000,
        _ => true,
    }
}

/// Error raised when a destination fails [`is_public_ip`]. Detected in the
/// reqwest error chain to answer 403 instead of 502.
#[derive(Debug)]
pub struct BlockedDestination(pub String);

impl fmt::Display for BlockedDestination {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "blocked non-public destination: {}", self.0)
    }
}

impl StdError for BlockedDestination {}

pub fn is_blocked_destination(err: &reqwest::Error) -> bool {
    let mut source: Option<&(dyn StdError + 'static)> = Some(err);
    while let Some(current) = source {
        if current.is::<BlockedDestination>() {
            return true;
        }
        source = current.source();
    }
    false
}

/// DNS resolver that refuses any name resolving to a non-public address. The
/// connector dials exactly the addresses returned here, so the check and the
/// connection cannot diverge (no rebinding TOCTOU).
#[derive(Debug, Default, Clone, Copy)]
pub struct PublicOnlyResolver;

impl PublicOnlyResolver {
    /// Resolve `host` and fail unless every address is public.
    pub async fn resolve_public(
        host: &str,
    ) -> Result<Vec<SocketAddr>, Box<dyn StdError + Send + Sync>> {
        let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, 0)).await?.collect();
        if addrs.is_empty() {
            return Err(format!("{host} did not resolve").into());
        }
        if let Some(bad) = addrs.iter().find(|addr| !is_public_ip(addr.ip())) {
            return Err(Box::new(BlockedDestination(format!(
                "{host} -> {}",
                bad.ip()
            ))));
        }
        Ok(addrs)
    }
}

impl Resolve for PublicOnlyResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addrs = Self::resolve_public(&host).await?;
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn v4(s: &str) -> IpAddr {
        IpAddr::V4(s.parse().unwrap())
    }

    fn v6(s: &str) -> IpAddr {
        IpAddr::V6(s.parse().unwrap())
    }

    #[test]
    fn blocks_every_special_ipv4_range() {
        for ip in [
            "0.0.0.0",
            "0.1.2.3",
            "10.0.0.1",
            "10.255.255.255",
            "100.64.0.1",
            "100.100.100.100",
            "100.127.255.255",
            "127.0.0.1",
            "127.255.255.254",
            "169.254.169.254",
            "172.16.0.1",
            "172.31.255.255",
            "192.0.0.1",
            "192.0.0.170",
            "192.0.2.1",
            "192.88.99.1",
            "192.168.1.1",
            "198.18.0.1",
            "198.19.255.255",
            "198.51.100.7",
            "203.0.113.9",
            "224.0.0.1",
            "239.255.255.250",
            "240.0.0.1",
            "255.255.255.255",
        ] {
            assert!(!is_public_ip(v4(ip)), "{ip} must be blocked");
        }
    }

    #[test]
    fn allows_public_ipv4_including_range_edges() {
        for ip in [
            "1.1.1.1",
            "8.8.8.8",
            "93.184.216.34",
            "100.63.255.255",
            "100.128.0.0",
            "172.15.255.255",
            "172.32.0.0",
            "192.0.1.1",
            "198.17.255.255",
            "198.20.0.0",
            "223.255.255.254",
        ] {
            assert!(is_public_ip(v4(ip)), "{ip} must be allowed");
        }
    }

    #[test]
    fn blocks_special_ipv6_and_embedded_private_ipv4() {
        for ip in [
            "::",
            "::1",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
            "::ffff:10.0.0.1",
            "::ffff:100.64.0.1",
            "::127.0.0.1",
            "::10.0.0.1",
            "64:ff9b::127.0.0.1",
            "64:ff9b::a9fe:a9fe",
            "64:ff9b:1::1",
            "2002:7f00:0001::1",
            "2002:c0a8:0101::1",
            "2001::1",
            "2001:db8::1",
            "3fff::1",
            "100::1",
            "fc00::1",
            "fd12:3456::1",
            "fe80::1",
            "fec0::1",
            "ff02::1",
        ] {
            assert!(!is_public_ip(v6(ip)), "{ip} must be blocked");
        }
    }

    #[test]
    fn allows_public_ipv6_and_embedded_public_ipv4() {
        for ip in [
            "2606:4700:4700::1111",
            "2001:4860:4860::8888",
            "::ffff:8.8.8.8",
            "64:ff9b::808:808",
            "2002:0808:0808::1",
        ] {
            assert!(is_public_ip(v6(ip)), "{ip} must be allowed");
        }
    }

    #[tokio::test]
    async fn resolver_rejects_hostname_resolving_to_loopback() {
        let err = PublicOnlyResolver::resolve_public("localhost")
            .await
            .expect_err("localhost resolves to loopback and must be rejected");
        assert!(err.is::<BlockedDestination>(), "unexpected error: {err}");
    }

    #[test]
    fn literal_hosts_are_classified() {
        for h in ["127.0.0.1", "[::1]", "::1", "0.0.0.0", "10.0.0.5", "169.254.169.254", "100.64.0.1", "::ffff:192.168.0.1", "localhost", "localhost.", "a.localhost", "printer.local"] {
            assert!(host_is_forbidden_literal(h), "{h}");
        }
        for h in ["1.1.1.1", "example.com", "2606:4700::1111"] {
            assert!(!host_is_forbidden_literal(h), "{h}");
        }
    }

    #[tokio::test]
    async fn resolve_public_refuses_metadata_and_private_names() {
        // Literal IPs resolve to themselves via the system resolver.
        for host in ["169.254.169.254", "127.0.0.1", "10.1.2.3", "::1"] {
            let e = resolve_public(host).await.expect_err(host);
            assert!(e.is::<BlockedDestination>(), "{host}: {e}");
        }
    }
}
