//! SSRF-guarded outbound fetch + domain-ownership challenge for the MCP App
//! directory.
//!
//! The browser cannot read another origin's `.well-known` (CORS), so the check
//! runs here. Rules: HTTPS only, default port only, no IP literals, the host
//! must resolve only to public addresses, the connection is pinned to the
//! address we validated (no DNS rebinding between check and connect), redirects
//! are never followed, 5 s total timeout, response body is size-capped.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use serde::Serialize;
use sha2::{Digest, Sha256};
use url::{Host, Url};

pub const DOMAIN_CHALLENGE_PATH: &str = "/.well-known/allternit-apps-challenge";
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(5);
const CHALLENGE_MAX_BYTES: usize = 4 * 1024;

// ─── Address / host classification ──────────────────────────────────────────

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_multicast()
        || a == 0
        || a >= 240 // reserved + broadcast
        || (a == 100 && (64..=127).contains(&b)) // CGNAT 100.64/10
        || (a == 192 && b == 0 && c == 0) // IETF protocol assignments
        || (a == 192 && b == 0 && c == 2) // TEST-NET-1
        || (a == 198 && (b == 18 || b == 19)) // benchmarking
        || (a == 198 && b == 51 && c == 100) // TEST-NET-2
        || (a == 203 && b == 0 && c == 113)) // TEST-NET-3
}

fn embedded_v4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    let s = ip.segments();
    // ::ffff:a.b.c.d (mapped) and ::a.b.c.d (compatible)
    if s[..5] == [0, 0, 0, 0, 0] && (s[5] == 0xffff || s[5] == 0) {
        return Some(Ipv4Addr::new((s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8));
    }
    // 64:ff9b::/96 NAT64
    if s[0] == 0x0064 && s[1] == 0xff9b && s[2..6] == [0, 0, 0, 0] {
        return Some(Ipv4Addr::new((s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8));
    }
    // 2002::/16 6to4
    if s[0] == 0x2002 {
        return Some(Ipv4Addr::new((s[1] >> 8) as u8, s[1] as u8, (s[2] >> 8) as u8, s[2] as u8));
    }
    None
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = embedded_v4(ip) {
        return is_public_v4(v4);
    }
    let s = ip.segments();
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_multicast()
        || (s[0] & 0xfe00) == 0xfc00 // unique local fc00::/7
        || (s[0] & 0xffc0) == 0xfe80 // link local fe80::/10
        || (s[0] & 0xffc0) == 0xfec0 // deprecated site local
        || (s[0] == 0x2001 && s[1] == 0x0db8) // documentation
        || (s[0] == 0x2001 && s[1] == 0) // Teredo
        || s[0] == 0x0100 && s[1..4] == [0, 0, 0]) // discard-only 100::/64
}

/// True when `ip` is a globally routable unicast address.
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => is_public_v6(v6),
    }
}

/// Name-level screen for a registrable-looking DNS name. The resolved
/// addresses are still checked separately; this rejects the obvious cases
/// without a lookup.
pub fn is_public_hostname(name: &str) -> bool {
    let n = name.trim_end_matches('.').to_ascii_lowercase();
    if n.is_empty() || n.len() > 253 || !n.contains('.') {
        return false;
    }
    if n == "localhost"
        || n.ends_with(".localhost")
        || n.ends_with(".local")
        || n.ends_with(".internal")
        || n.ends_with(".lan")
        || n.ends_with(".home.arpa")
    {
        return false;
    }
    n.split('.').all(|l| {
        !l.is_empty() && l.len() <= 63 && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') && !l.starts_with('-') && !l.ends_with('-')
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardedTarget {
    /// Lower-cased DNS name (no port).
    pub host: String,
    pub url: Url,
}

/// Parse a URL for a guarded fetch: https, default port, DNS name (not an IP
/// literal in any notation — the `url` crate normalises `2130706433`,
/// `0x7f.1`, `[::1]` into IP hosts, which we refuse), no credentials.
pub fn parse_guarded_url(input: &str) -> Result<GuardedTarget, String> {
    let url = Url::parse(input).map_err(|_| "Not a valid URL.".to_string())?;
    if url.scheme() != "https" {
        return Err("Only https:// URLs are allowed.".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("URLs with credentials are not allowed.".into());
    }
    if url.port().is_some() {
        return Err("Only the default HTTPS port is allowed.".into());
    }
    match url.host() {
        Some(Host::Domain(d)) => {
            if !is_public_hostname(d) {
                return Err("Host is not publicly routable.".into());
            }
            Ok(GuardedTarget { host: d.to_ascii_lowercase(), url: url.clone() })
        }
        Some(Host::Ipv4(_)) | Some(Host::Ipv6(_)) => Err("IP-address hosts are not allowed.".into()),
        None => Err("URL has no host.".into()),
    }
}

/// Host of a server URL or bare host (mirrors the client's `extractHost`, but
/// returns only the lower-cased DNS name — `Err` for anything unusable).
pub fn extract_public_host(input: &str) -> Result<String, String> {
    let t = input.trim();
    if t.is_empty() {
        return Err("Not a valid host or URL.".into());
    }
    let with_scheme = if t.contains("://") { t.to_string() } else { format!("https://{t}") };
    let url = Url::parse(&with_scheme).map_err(|_| "Not a valid host or URL.".to_string())?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err("Host must not include credentials.".into());
    }
    if url.port().is_some_and(|p| p != 443) {
        return Err("Only the default HTTPS port is allowed.".into());
    }
    match url.host() {
        Some(Host::Domain(d)) if is_public_hostname(d) => Ok(d.to_ascii_lowercase()),
        Some(Host::Domain(_)) => Err("Host is not publicly routable.".into()),
        Some(_) => Err("IP-address hosts are not allowed.".into()),
        None => Err("Not a valid host or URL.".into()),
    }
}

// ─── Transport ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetched {
    pub status: u16,
    pub content_type: Option<String>,
    pub body: String,
}

/// Network seam so the guard logic is testable without sockets.
pub trait Transport: Send + Sync {
    /// Resolve `host` to all of its addresses.
    fn resolve(&self, host: &str) -> impl std::future::Future<Output = Result<Vec<IpAddr>, String>> + Send;
    /// GET `url`, connecting to `addr` (the address the guard validated).
    /// Must not follow redirects and must cap the body at `max_bytes`.
    fn get(
        &self,
        url: &Url,
        addr: IpAddr,
        accept: &str,
        max_bytes: usize,
    ) -> impl std::future::Future<Output = Result<Fetched, String>> + Send;
}

/// Production transport: system DNS + reqwest pinned to the validated address.
pub struct ReqwestTransport;

impl Transport for ReqwestTransport {
    async fn resolve(&self, host: &str) -> Result<Vec<IpAddr>, String> {
        let addrs = tokio::net::lookup_host((host, 443))
            .await
            .map_err(|e| format!("DNS lookup failed: {e}"))?;
        Ok(addrs.map(|a| a.ip()).collect())
    }

    async fn get(&self, url: &Url, addr: IpAddr, accept: &str, max_bytes: usize) -> Result<Fetched, String> {
        let host = url.host_str().ok_or("URL has no host.")?.to_string();
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(FETCH_TIMEOUT)
            .resolve(&host, SocketAddr::new(addr, 443))
            .build()
            .map_err(|e| format!("HTTP client error: {e}"))?;
        let mut res = client
            .get(url.clone())
            .header("accept", accept)
            .send()
            .await
            .map_err(|e| format!("Fetch failed: {e}"))?;
        let status = res.status().as_u16();
        let content_type = res
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let mut buf: Vec<u8> = Vec::new();
        while let Some(chunk) = res.chunk().await.map_err(|e| format!("Fetch failed: {e}"))? {
            if buf.len() + chunk.len() > max_bytes {
                return Err(format!("Response larger than {max_bytes} bytes."));
            }
            buf.extend_from_slice(&chunk);
        }
        Ok(Fetched { status, content_type, body: String::from_utf8_lossy(&buf).into_owned() })
    }
}

/// SSRF-guarded GET. Every failure is a human-readable reason string.
pub async fn guarded_get<T: Transport>(
    transport: &T,
    target: &GuardedTarget,
    accept: &str,
    max_bytes: usize,
) -> Result<Fetched, String> {
    guarded_get_within(transport, target, accept, max_bytes, FETCH_TIMEOUT).await
}

async fn guarded_get_within<T: Transport>(
    transport: &T,
    target: &GuardedTarget,
    accept: &str,
    max_bytes: usize,
    timeout: Duration,
) -> Result<Fetched, String> {
    let work = async {
        let addrs = transport.resolve(&target.host).await?;
        if addrs.is_empty() {
            return Err("Host did not resolve.".to_string());
        }
        // Refuse if ANY answer is non-public: a mixed answer set is how
        // rebinding-style tricks smuggle in an internal address.
        if let Some(bad) = addrs.iter().find(|a| !is_public_ip(**a)) {
            return Err(format!("Host resolves to a non-public address ({bad})."));
        }
        let res = transport.get(&target.url, addrs[0], accept, max_bytes).await?;
        if (300..400).contains(&res.status) {
            return Err(format!("Redirects are not followed (HTTP {}).", res.status));
        }
        Ok(res)
    };
    match tokio::time::timeout(timeout, work).await {
        Ok(r) => r,
        Err(_) => Err("Timed out after 5 seconds.".into()),
    }
}

// ─── Domain challenge ────────────────────────────────────────────────────────

/// Server-issued challenge token; hex so it survives plain-text serving.
pub fn generate_domain_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 24];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    format!("allternit-verify-{}", hex::encode(bytes))
}

pub fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// Exact plain-text match against the stored hash (only one trailing newline is
/// tolerated). Constant-time compare on the hex digests.
pub fn challenge_matches(body: &str, expected_hash: &str) -> bool {
    let trimmed = body.strip_suffix("\r\n").or_else(|| body.strip_suffix('\n')).unwrap_or(body);
    if trimmed.is_empty() {
        return false;
    }
    let got = hash_token(trimmed);
    let (a, b) = (got.as_bytes(), expected_hash.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DomainCheckResult {
    pub ok: bool,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

fn fail(url: String, reason: impl Into<String>) -> DomainCheckResult {
    DomainCheckResult { ok: false, url, reason: Some(reason.into()) }
}

/// Fetch `https://<host>/.well-known/allternit-apps-challenge` and compare it
/// with the stored token hash. `host` must already be a validated DNS name.
pub async fn check_domain_challenge<T: Transport>(transport: &T, host: &str, expected_hash: &str) -> DomainCheckResult {
    let url = format!("https://{host}{DOMAIN_CHALLENGE_PATH}");
    let target = match parse_guarded_url(&url) {
        Ok(t) => t,
        Err(reason) => return fail(url, reason),
    };
    let res = match guarded_get(transport, &target, "text/plain", CHALLENGE_MAX_BYTES).await {
        Ok(r) => r,
        Err(reason) => return fail(url, format!("Could not fetch challenge: {reason}")),
    };
    if !(200..300).contains(&res.status) {
        return fail(url, format!("Challenge returned HTTP {}.", res.status));
    }
    if let Some(ct) = &res.content_type {
        if !ct.trim().to_ascii_lowercase().starts_with("text/plain") {
            return fail(url, format!("Content-Type must be text/plain (got {ct})."));
        }
    }
    if !challenge_matches(&res.body, expected_hash) {
        return fail(url, "Challenge body does not exactly match the token.");
    }
    DomainCheckResult { ok: true, url, reason: None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Fake {
        addrs: Vec<IpAddr>,
        response: Result<Fetched, String>,
        connected_to: Mutex<Option<IpAddr>>,
    }

    impl Fake {
        fn ok(body: &str) -> Self {
            Self {
                addrs: vec!["93.184.216.34".parse().unwrap()],
                response: Ok(Fetched { status: 200, content_type: Some("text/plain; charset=utf-8".into()), body: body.into() }),
                connected_to: Mutex::new(None),
            }
        }
    }

    impl Transport for Fake {
        async fn resolve(&self, _host: &str) -> Result<Vec<IpAddr>, String> {
            Ok(self.addrs.clone())
        }
        async fn get(&self, _u: &Url, addr: IpAddr, _a: &str, _m: usize) -> Result<Fetched, String> {
            *self.connected_to.lock().unwrap() = Some(addr);
            self.response.clone()
        }
    }

    #[test]
    fn private_and_special_ipv4_are_blocked() {
        for ip in [
            "127.0.0.1", "10.1.2.3", "172.16.0.1", "172.31.255.255", "192.168.1.1", "169.254.169.254", "0.0.0.0",
            "100.64.0.1", "100.127.255.255", "224.0.0.1", "255.255.255.255", "198.18.0.1", "192.0.2.1", "240.0.0.1",
        ] {
            assert!(!is_public_ip(ip.parse().unwrap()), "{ip} must be blocked");
        }
        for ip in ["93.184.216.34", "8.8.8.8", "172.32.0.1", "100.128.0.1", "1.1.1.1"] {
            assert!(is_public_ip(ip.parse().unwrap()), "{ip} must be allowed");
        }
    }

    #[test]
    fn private_and_mapped_ipv6_are_blocked() {
        for ip in [
            "::1", "::", "fe80::1", "fc00::1", "fd12:3456::1", "ff02::1", "::ffff:127.0.0.1", "::ffff:10.0.0.1",
            "::ffff:169.254.169.254", "64:ff9b::7f00:1", "2002:7f00:1::1", "2001:db8::1",
        ] {
            assert!(!is_public_ip(ip.parse().unwrap()), "{ip} must be blocked");
        }
        assert!(is_public_ip("2606:4700:4700::1111".parse().unwrap()));
        assert!(is_public_ip("::ffff:8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn hostnames_are_screened() {
        for h in ["localhost", "foo.localhost", "printer.local", "db.internal", "intranet", "a..b", "-a.com", "", "router.lan"] {
            assert!(!is_public_hostname(h), "{h:?} must be blocked");
        }
        assert!(is_public_hostname("mcp.example.com"));
        assert!(is_public_hostname("MCP.Example.com."));
    }

    #[test]
    fn url_parser_refuses_ip_literals_in_every_notation() {
        for u in [
            "https://127.0.0.1/x", "https://[::1]/x", "https://2130706433/x", "https://0x7f.0.0.1/x",
            "https://017700000001/x", "https://169.254.169.254/latest", "https://[::ffff:7f00:1]/x",
        ] {
            assert!(parse_guarded_url(u).is_err(), "{u} must be refused");
        }
    }

    #[test]
    fn url_parser_enforces_scheme_port_and_credentials() {
        assert!(parse_guarded_url("http://example.com/x").is_err());
        assert!(parse_guarded_url("https://example.com:8443/x").is_err());
        assert!(parse_guarded_url("https://user:pw@example.com/x").is_err());
        assert!(parse_guarded_url("https://localhost/x").is_err());
        assert!(parse_guarded_url("file:///etc/passwd").is_err());
        assert_eq!(parse_guarded_url("https://Example.COM/x").unwrap().host, "example.com");
    }

    #[test]
    fn extract_host_accepts_urls_and_bare_hosts() {
        assert_eq!(extract_public_host("https://mcp.example.com/mcp").unwrap(), "mcp.example.com");
        assert_eq!(extract_public_host("MCP.example.com").unwrap(), "mcp.example.com");
        assert_eq!(extract_public_host("mcp.example.com:443").unwrap(), "mcp.example.com");
        assert!(extract_public_host("mcp.example.com:8080").is_err());
        assert!(extract_public_host("10.0.0.1").is_err());
        assert!(extract_public_host("localhost").is_err());
        assert!(extract_public_host("").is_err());
        assert!(extract_public_host("https://u:p@example.com").is_err());
    }

    #[test]
    fn challenge_match_is_exact() {
        let token = generate_domain_token();
        let h = hash_token(&token);
        assert!(token.starts_with("allternit-verify-") && token.len() > 40);
        assert_ne!(token, generate_domain_token());
        assert!(challenge_matches(&token, &h));
        assert!(challenge_matches(&format!("{token}\n"), &h));
        assert!(challenge_matches(&format!("{token}\r\n"), &h));
        assert!(!challenge_matches(&format!("{token}\n\n"), &h));
        assert!(!challenge_matches(&format!(" {token}"), &h));
        assert!(!challenge_matches(&format!("{token}x"), &h));
        assert!(!challenge_matches("", &h));
        assert!(!challenge_matches(&token, &hash_token("other")));
    }

    #[tokio::test]
    async fn domain_check_passes_on_exact_token_and_pins_address() {
        let token = generate_domain_token();
        let t = Fake::ok(&format!("{token}\n"));
        let r = check_domain_challenge(&t, "mcp.example.com", &hash_token(&token)).await;
        assert!(r.ok, "{r:?}");
        assert_eq!(r.url, "https://mcp.example.com/.well-known/allternit-apps-challenge");
        assert_eq!(*t.connected_to.lock().unwrap(), Some("93.184.216.34".parse().unwrap()));
    }

    #[tokio::test]
    async fn domain_check_fails_on_wrong_body_status_type() {
        let token = generate_domain_token();
        let h = hash_token(&token);
        let r = check_domain_challenge(&Fake::ok("nope"), "mcp.example.com", &h).await;
        assert!(!r.ok && r.reason.unwrap().contains("exactly match"));

        let mut t = Fake::ok(&token);
        t.response = Ok(Fetched { status: 404, content_type: None, body: token.clone() });
        assert!(check_domain_challenge(&t, "mcp.example.com", &h).await.reason.unwrap().contains("HTTP 404"));

        let mut t = Fake::ok(&token);
        t.response = Ok(Fetched { status: 200, content_type: Some("text/html".into()), body: token.clone() });
        assert!(check_domain_challenge(&t, "mcp.example.com", &h).await.reason.unwrap().contains("text/plain"));
    }

    #[tokio::test]
    async fn domain_check_refuses_redirects() {
        let token = generate_domain_token();
        let mut t = Fake::ok(&token);
        t.response = Ok(Fetched { status: 302, content_type: None, body: String::new() });
        let r = check_domain_challenge(&t, "mcp.example.com", &hash_token(&token)).await;
        assert!(!r.ok && r.reason.unwrap().contains("Redirects are not followed"));
    }

    #[tokio::test]
    async fn domain_check_blocks_private_resolution_and_never_connects() {
        let token = generate_domain_token();
        for bad in ["127.0.0.1", "169.254.169.254", "10.0.0.5", "::1", "::ffff:192.168.0.1"] {
            let mut t = Fake::ok(&token);
            t.addrs = vec![bad.parse().unwrap()];
            let r = check_domain_challenge(&t, "evil.example.com", &hash_token(&token)).await;
            assert!(!r.ok && r.reason.as_deref().unwrap().contains("non-public"), "{bad}: {r:?}");
            assert!(t.connected_to.lock().unwrap().is_none(), "{bad}: must not connect");
        }
        // A mixed answer set (one public, one internal) is refused as a whole.
        let mut t = Fake::ok(&token);
        t.addrs = vec!["93.184.216.34".parse().unwrap(), "127.0.0.1".parse().unwrap()];
        assert!(!check_domain_challenge(&t, "evil.example.com", &hash_token(&token)).await.ok);
        assert!(t.connected_to.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn domain_check_rejects_unroutable_hosts_before_resolving() {
        let t = Fake::ok("x");
        for h in ["localhost", "127.0.0.1", "intranet"] {
            assert!(!check_domain_challenge(&t, h, "h").await.ok, "{h}");
        }
        assert!(t.connected_to.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn fetch_timeout_is_enforced() {
        struct Slow;
        impl Transport for Slow {
            async fn resolve(&self, _h: &str) -> Result<Vec<IpAddr>, String> {
                Ok(vec!["93.184.216.34".parse().unwrap()])
            }
            async fn get(&self, _u: &Url, _a: IpAddr, _c: &str, _m: usize) -> Result<Fetched, String> {
                tokio::time::sleep(Duration::from_secs(60)).await;
                unreachable!()
            }
        }
        let target = parse_guarded_url("https://mcp.example.com/x").unwrap();
        let r = guarded_get_within(&Slow, &target, "text/plain", 10, Duration::from_millis(50)).await;
        assert!(r.unwrap_err().contains("Timed out"));
        assert_eq!(FETCH_TIMEOUT, Duration::from_secs(5));
    }
}
