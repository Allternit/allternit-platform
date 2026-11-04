//! Where a connector sign-in returns to.
//!
//! A sign-in started from the iPhone app, ai.allternit.com or the PWA runs on the
//! user's own runtime (cloud-api relays `/api/v1/*` to it), but the browser that
//! finishes it is on another device and can't reach `http://127.0.0.1:8013`. So a
//! paired runtime sends every OAuth redirect to cloud-api instead:
//!
//! | flow | redirect URI | relayed to |
//! |---|---|---|
//! | owned connector OAuth | `{cloud}/connect/return/connectors` | `/api/v1/connectors/oauth/callback` |
//! | MCP connector OAuth (DCR, CIMD, configured client) | `{cloud}/connect/return/mcp` | `/mcp/oauth/callback` |
//! | open-connector sidecar | `{cloud}/connect/return/sidecar/oauth/callback` | `/oauth/callback` |
//!
//! The runtime is named in the OAuth `state`: `art.<runtime_id>.<random>`. cloud-api
//! reads the runtime id from it, relays the GET (query intact) over the runtime
//! socket and returns the runtime's page. The runtime still finds the pending
//! sign-in by the full state and still checks PKCE, so the cloud hop adds no
//! trust: it only carries the browser back. The CIMD document for the cloud
//! redirect is served by cloud-api at `{cloud}/connect/client.json`.
//!
//! Order: an explicit `ALLTERNIT_PUBLIC_BASE_URL` (a runtime with its own public
//! origin) wins; else a paired runtime uses the cloud; else loopback.

use crate::relay_auth::RelaySecret;

/// Prefix of a state that names its runtime.
pub const STATE_TAG: &str = "art";
const LOOPBACK: &str = "http://127.0.0.1:8013";

/// Which local callback a sign-in returns to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Connectors,
    Mcp,
    Sidecar,
}

impl Target {
    /// The runtime's own callback path.
    pub fn local_path(self) -> &'static str {
        match self {
            Target::Connectors => "/api/v1/connectors/oauth/callback",
            Target::Mcp => "/mcp/oauth/callback",
            Target::Sidecar => "/oauth/callback",
        }
    }
    /// The cloud return path.
    pub fn cloud_path(self) -> &'static str {
        match self {
            Target::Connectors => "/connect/return/connectors",
            Target::Mcp => "/connect/return/mcp",
            Target::Sidecar => "/connect/return/sidecar/oauth/callback",
        }
    }
}

/// How this runtime's sign-ins return.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReturnPlan {
    /// The origin redirects go to (cloud, explicit public base, or loopback).
    pub base: String,
    /// `Some` when redirects go through the cloud: the runtime id for the state.
    pub runtime_id: Option<String>,
}

fn trimmed(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().trim_end_matches('/').to_string()).filter(|s| !s.is_empty())
}

/// A runtime id cloud-api can route on: `rt_<hex>` today; letters, digits, `_`, `-`.
pub fn valid_runtime_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 80 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

impl ReturnPlan {
    /// `public_base_env`: `ALLTERNIT_PUBLIC_BASE_URL`. `runtime_id`: set only when
    /// the runtime is paired (identity with a live credential). `cloud`: cloud-api origin.
    pub fn from_parts(public_base_env: Option<String>, runtime_id: Option<String>, cloud: &str) -> Self {
        if let Some(base) = trimmed(public_base_env) {
            return Self { base, runtime_id: None };
        }
        match trimmed(runtime_id).filter(|id| valid_runtime_id(id)) {
            Some(id) => Self { base: cloud.trim().trim_end_matches('/').to_string(), runtime_id: Some(id) },
            None => Self { base: LOOPBACK.to_string(), runtime_id: None },
        }
    }

    /// Redirects to `base` itself (no cloud hop).
    pub fn at(base: &str) -> Self {
        Self { base: base.trim().trim_end_matches('/').to_string(), runtime_id: None }
    }

    /// This process's plan, from env + the paired identity.
    pub fn current() -> Self {
        let secret = crate::relay_auth::process_secret();
        Self::for_secret(secret.as_ref())
    }

    pub fn for_secret(secret: &dyn RelaySecret) -> Self {
        let paired = secret.credentials().is_some();
        let rid = if paired { secret.runtime_id() } else { None };
        Self::from_parts(std::env::var("ALLTERNIT_PUBLIC_BASE_URL").ok(), rid, &crate::phone_sync::cloud_base())
    }

    pub fn via_cloud(&self) -> bool {
        self.runtime_id.is_some()
    }

    pub fn redirect_uri(&self, t: Target) -> String {
        if self.via_cloud() {
            format!("{}{}", self.base, t.cloud_path())
        } else {
            format!("{}{}", self.base, t.local_path())
        }
    }

    /// The URL of the Client ID Metadata Document that lists [`Self::redirect_uri`] for MCP.
    pub fn cimd_url(&self) -> String {
        if self.via_cloud() {
            format!("{}/connect/client.json", self.base)
        } else {
            format!("{}/oauth/client.json", self.base)
        }
    }

    /// `inner` with this runtime's tag, when sign-ins return through the cloud.
    pub fn wrap_state(&self, inner: &str) -> String {
        match &self.runtime_id {
            Some(id) => format!("{STATE_TAG}.{id}.{inner}"),
            None => inner.to_string(),
        }
    }
}

/// `(runtime_id, inner)` of a tagged state; `None` for any other shape.
pub fn parse_state(state: &str) -> Option<(&str, &str)> {
    let rest = state.strip_prefix(STATE_TAG)?.strip_prefix('.')?;
    let (id, inner) = rest.split_once('.')?;
    (valid_runtime_id(id) && !inner.is_empty()).then_some((id, inner))
}

/// The state the sidecar issued: a tagged state loses its tag, anything else is unchanged.
pub fn unwrap_state(state: &str) -> &str {
    parse_state(state).map_or(state, |(_, inner)| inner)
}

/// Rewrite `state` in a query string with `f`, keeping every other pair as sent.
pub fn map_state_in_query(query: &str, f: impl Fn(&str) -> String) -> String {
    let pairs: Vec<(String, String)> = url::form_urlencoded::parse(query.as_bytes()).into_owned().collect();
    let mut out = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in pairs {
        if k == "state" {
            out.append_pair(&k, &f(&v));
        } else {
            out.append_pair(&k, &v);
        }
    }
    out.finish()
}

/// The sidecar's authorize URL with this runtime's tag on its `state`, when the
/// sidecar's redirect already goes to the cloud return. A loopback sidecar (started
/// before pairing, or unpaired) is left alone: its redirect can't reach the cloud.
pub fn tag_sidecar_authorize_url(authorize_url: &str, plan: &ReturnPlan) -> String {
    if !plan.via_cloud() {
        return authorize_url.to_string();
    }
    let Ok(mut url) = url::Url::parse(authorize_url) else { return authorize_url.to_string() };
    let redirect = url.query_pairs().find(|(k, _)| k == "redirect_uri").map(|(_, v)| v.into_owned());
    if redirect.as_deref() != Some(plan.redirect_uri(Target::Sidecar).as_str()) {
        return authorize_url.to_string();
    }
    let query = url.query().unwrap_or_default().to_string();
    url.set_query(Some(&map_state_in_query(&query, |s| plan.wrap_state(s))));
    url.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLOUD: &str = "https://api.allternit.com";

    #[test]
    fn paired_runtimes_return_through_the_cloud_and_others_do_not() {
        let paired = ReturnPlan::from_parts(None, Some("rt_abc123".into()), CLOUD);
        assert_eq!(paired.redirect_uri(Target::Connectors), "https://api.allternit.com/connect/return/connectors");
        assert_eq!(paired.redirect_uri(Target::Mcp), "https://api.allternit.com/connect/return/mcp");
        assert_eq!(paired.redirect_uri(Target::Sidecar), "https://api.allternit.com/connect/return/sidecar/oauth/callback");
        assert_eq!(paired.cimd_url(), "https://api.allternit.com/connect/client.json");
        assert_eq!(paired.wrap_state("s1"), "art.rt_abc123.s1");

        let local = ReturnPlan::from_parts(None, None, CLOUD);
        assert_eq!(local.redirect_uri(Target::Mcp), "http://127.0.0.1:8013/mcp/oauth/callback");
        assert_eq!(local.wrap_state("s1"), "s1");

        // An explicit public origin wins over pairing.
        let own = ReturnPlan::from_parts(Some("https://box.example.com/".into()), Some("rt_abc".into()), CLOUD);
        assert_eq!(own.redirect_uri(Target::Connectors), "https://box.example.com/api/v1/connectors/oauth/callback");
        assert_eq!(own.wrap_state("s1"), "s1");

        // A runtime id the cloud couldn't route on is never put in a state.
        let odd = ReturnPlan::from_parts(None, Some("rt.a/b".into()), CLOUD);
        assert!(!odd.via_cloud());
    }

    #[test]
    fn state_tag_round_trips_and_rejects_other_shapes() {
        assert_eq!(parse_state("art.rt_abc.inner-1_x"), Some(("rt_abc", "inner-1_x")));
        assert_eq!(parse_state("art.rt_abc.a.b"), Some(("rt_abc", "a.b")));
        for bad in ["", "art", "art.", "art.rt_abc", "art.rt_abc.", "art..x", "xart.rt.x", "art.r%2F.x", "550e8400-e29b"] {
            assert_eq!(parse_state(bad), None, "{bad}");
        }
        assert_eq!(unwrap_state("art.rt_abc.0f3c-uuid"), "0f3c-uuid");
        assert_eq!(unwrap_state("0f3c-uuid"), "0f3c-uuid");
    }

    #[test]
    fn sidecar_state_is_tagged_only_when_its_redirect_is_the_cloud_return() {
        let plan = ReturnPlan::from_parts(None, Some("rt_abc".into()), CLOUD);
        let cloud = "https://accounts.google.com/o/oauth2/auth?client_id=c&redirect_uri=https%3A%2F%2Fapi.allternit.com%2Fconnect%2Freturn%2Fsidecar%2Foauth%2Fcallback&state=u-1&scope=a+b";
        let tagged = url::Url::parse(&tag_sidecar_authorize_url(cloud, &plan)).unwrap();
        let q: std::collections::HashMap<_, _> = tagged.query_pairs().into_owned().collect();
        assert_eq!(q["state"], "art.rt_abc.u-1");
        assert_eq!(q["scope"], "a b");
        assert_eq!(q["client_id"], "c");

        let loopback = "https://accounts.google.com/o/oauth2/auth?redirect_uri=http%3A%2F%2F127.0.0.1%3A8013%2Foauth%2Fcallback&state=u-1";
        assert_eq!(tag_sidecar_authorize_url(loopback, &plan), loopback);

        // The callback hands the sidecar back its own state.
        let back = map_state_in_query("code=c%2F1&state=art.rt_abc.u-1", |s| unwrap_state(s).to_string());
        assert_eq!(back, "code=c%2F1&state=u-1");
    }
}
