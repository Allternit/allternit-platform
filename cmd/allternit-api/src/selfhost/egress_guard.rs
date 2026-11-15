//! The one outbound HTTP client of the Agents Server.
//!
//! Every outbound request the server makes goes through [`EgressClient`],
//! which refuses any host that is not on an allowlist built from the
//! settings file:
//!
//! - the inference endpoint host (by provider),
//! - the OIDC issuer host and `jwks_uri` host,
//! - `mcp.allowed_hostnames` and `git.allowed_hostnames`,
//! - the OTLP endpoint host (`OTEL_EXPORTER_OTLP_ENDPOINT`),
//! - the Vault address host.
//!
//! Matching is exact: an entry `host` allows that host on any port, an entry
//! `host:port` allows only that port. No subdomain matching. Redirects are
//! checked against the same list. `allternit.com` and its subdomains are
//! refused unless one is listed by name, so the server cannot phone home by
//! accident. Each host gets allowed/refused counters for metrics.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use serde::Serialize;

use super::settings::{Provider, Settings};

#[derive(Debug, thiserror::Error)]
pub enum EgressError {
    #[error("host_not_allowed: {host} is not on the egress allowlist")]
    NotAllowed { host: String },
    #[error("host_not_allowed: {host} is an allternit.com host and is not listed in the settings")]
    AllternitDomain { host: String },
    #[error("bad url {0}")]
    BadUrl(String),
    #[error("http client: {0}")]
    Client(String),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct HostStats {
    pub allowed: u64,
    pub refused: u64,
}

#[derive(Debug, Default)]
pub struct EgressGuard {
    /// Lowercase `host` or `host:port`.
    allowed: BTreeSet<String>,
    stats: Mutex<BTreeMap<String, HostStats>>,
}

fn normalize(entry: &str) -> String {
    entry.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// `host` (no explicit port) or `host:port`, from a URL string.
fn entry_from_url(raw: &str) -> Option<String> {
    let u = url::Url::parse(raw).ok()?;
    let host = u.host_str()?.to_ascii_lowercase();
    Some(match u.port() {
        Some(p) => format!("{host}:{p}"),
        None => host,
    })
}

fn is_allternit(host: &str) -> bool {
    host == "allternit.com" || host.ends_with(".allternit.com")
}

impl EgressGuard {
    pub fn new<I, S>(hosts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self {
            allowed: hosts
                .into_iter()
                .map(|h| normalize(h.as_ref()))
                .filter(|h| !h.is_empty())
                .collect(),
            stats: Mutex::new(BTreeMap::new()),
        }
    }

    /// The allowlist the settings imply.
    pub fn from_settings(s: &Settings) -> Self {
        Self::new(Self::hosts_for(s))
    }

    pub fn hosts_for(s: &Settings) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let inf = &s.inference;
        let creds = inf.credentials.clone().unwrap_or_default();
        match inf.provider {
            Provider::Anthropic => out.push("api.anthropic.com".into()),
            Provider::Bedrock => {
                if let Some(r) = creds.aws_region.as_deref().filter(|r| !r.is_empty()) {
                    out.push(format!("bedrock-runtime.{r}.amazonaws.com"));
                }
            }
            Provider::Vertex => {
                match creds.gcp_region.as_deref().filter(|r| !r.is_empty()) {
                    Some("global") | None => out.push("aiplatform.googleapis.com".into()),
                    Some(r) => out.push(format!("{r}-aiplatform.googleapis.com")),
                }
                // Service-account JSON is exchanged for an access token here.
                out.push("oauth2.googleapis.com".into());
            }
            Provider::Foundry => {
                if let Some(e) = creds.foundry_endpoint.as_deref().and_then(entry_from_url) {
                    out.push(e);
                }
            }
            Provider::Gateway => {}
        }
        if let Some(gw) = &inf.gateway {
            if let Some(e) = entry_from_url(&gw.base_url) {
                out.push(e);
            }
        }
        if let Some(o) = &s.auth.oidc {
            out.extend(entry_from_url(&o.issuer));
            if let Some(j) = &o.jwks_uri {
                out.extend(entry_from_url(j));
            }
        }
        out.extend(s.mcp.allowed_hostnames.iter().cloned());
        out.extend(s.git.allowed_hostnames.iter().cloned());
        if let Ok(otlp) = std::env::var(crate::otel::ENV_OTLP_ENDPOINT) {
            out.extend(entry_from_url(otlp.trim()));
        }
        if let Some(v) = &s.secrets.vault {
            out.extend(entry_from_url(&v.address));
        }
        out
    }

    /// Allowed without counting. `port` is the URL's effective port.
    pub fn is_allowed(&self, host: &str, port: Option<u16>) -> bool {
        let host = normalize(host);
        if self.allowed.contains(&host) {
            return true;
        }
        match port {
            Some(p) => self.allowed.contains(&format!("{host}:{p}")),
            None => false,
        }
    }

    /// Check a URL and count the result.
    pub fn check_url(&self, url: &url::Url) -> Result<(), EgressError> {
        let Some(host) = url.host_str() else {
            return Err(EgressError::BadUrl(url.to_string()));
        };
        let host = normalize(host);
        let ok = self.is_allowed(&host, url.port_or_known_default());
        self.count(&host, ok);
        if ok {
            return Ok(());
        }
        if is_allternit(&host) {
            tracing::warn!(host = %host, "refused outbound request to an allternit.com host");
            return Err(EgressError::AllternitDomain { host });
        }
        tracing::warn!(host = %host, "refused outbound request to a host not on the allowlist");
        Err(EgressError::NotAllowed { host })
    }

    fn count(&self, host: &str, ok: bool) {
        if let Ok(mut m) = self.stats.lock() {
            let e = m.entry(host.to_string()).or_default();
            if ok {
                e.allowed += 1;
            } else {
                e.refused += 1;
            }
        }
    }

    pub fn allowed_hosts(&self) -> Vec<String> {
        self.allowed.iter().cloned().collect()
    }

    /// Per-host counters, for the metrics listener.
    pub fn stats(&self) -> BTreeMap<String, HostStats> {
        self.stats.lock().map(|m| m.clone()).unwrap_or_default()
    }

    /// The guarded client. Redirects are checked against this guard too.
    pub fn client(self: &Arc<Self>) -> Result<EgressClient, EgressError> {
        let guard = self.clone();
        let policy = reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= 5 {
                return attempt.error("too many redirects");
            }
            match guard.check_url(attempt.url()) {
                Ok(()) => attempt.follow(),
                Err(e) => attempt.error(e),
            }
        });
        let http = reqwest::Client::builder()
            .redirect(policy)
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(60))
            .user_agent(concat!("allternit-agents-server/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| EgressError::Client(e.to_string()))?;
        Ok(EgressClient {
            http,
            guard: self.clone(),
        })
    }
}

/// A `reqwest::Client` that checks every URL against the guard first.
#[derive(Clone, Debug)]
pub struct EgressClient {
    http: reqwest::Client,
    guard: Arc<EgressGuard>,
}

impl EgressClient {
    pub fn request(
        &self,
        method: reqwest::Method,
        url: &str,
    ) -> Result<reqwest::RequestBuilder, EgressError> {
        let parsed = url::Url::parse(url).map_err(|_| EgressError::BadUrl(url.to_string()))?;
        self.guard.check_url(&parsed)?;
        Ok(self.http.request(method, parsed))
    }

    pub fn get(&self, url: &str) -> Result<reqwest::RequestBuilder, EgressError> {
        self.request(reqwest::Method::GET, url)
    }

    pub fn post(&self, url: &str) -> Result<reqwest::RequestBuilder, EgressError> {
        self.request(reqwest::Method::POST, url)
    }

    pub fn guard(&self) -> &Arc<EgressGuard> {
        &self.guard
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> url::Url {
        url::Url::parse(s).unwrap()
    }

    #[test]
    fn exact_match_only() {
        let g = EgressGuard::new(["mcp.example.com", "git.example.com:8443"]);
        assert!(g.check_url(&url("https://mcp.example.com/x")).is_ok());
        assert!(g.check_url(&url("https://evil.mcp.example.com/")).is_err());
        assert!(g.check_url(&url("https://git.example.com:8443/repo")).is_ok());
        assert!(g.check_url(&url("https://git.example.com/repo")).is_err());
        let s = g.stats();
        assert_eq!(s["mcp.example.com"], HostStats { allowed: 1, refused: 0 });
        assert_eq!(s["evil.mcp.example.com"].refused, 1);
    }

    #[test]
    fn allternit_refused_unless_listed() {
        let g = EgressGuard::new(["api.anthropic.com"]);
        assert!(matches!(
            g.check_url(&url("https://api.allternit.com/v1")),
            Err(EgressError::AllternitDomain { .. })
        ));
        let listed = EgressGuard::new(["releases.allternit.com"]);
        assert!(listed.check_url(&url("https://releases.allternit.com/x")).is_ok());
        assert!(listed.check_url(&url("https://api.allternit.com/x")).is_err());
    }

    #[test]
    fn hosts_come_from_settings() {
        let s = Settings::from_str(
            "inference:\n  provider: gateway\n  gateway: {format: openai-chat, base_url: 'https://llm.corp:9443/v1'}\n  model_map: {m: m}\nmcp: {allowed_hostnames: [mcp.corp]}\nauth:\n  oidc: {issuer: 'https://idp.corp', audiences: [agents]}\n",
        )
        .unwrap();
        let g = EgressGuard::from_settings(&s);
        assert!(g.is_allowed("llm.corp", Some(9443)));
        assert!(!g.is_allowed("llm.corp", Some(443)));
        assert!(g.is_allowed("idp.corp", Some(443)));
        assert!(g.is_allowed("mcp.corp", Some(443)));
        assert!(!g.is_allowed("api.anthropic.com", Some(443)));
    }
}
