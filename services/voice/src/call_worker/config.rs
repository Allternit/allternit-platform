//! Call worker configuration, read from the environment.

use std::time::Duration;

use anyhow::{Context, Result};

#[derive(Clone)]
pub struct WorkerConfig {
    /// `LIVEKIT_URL`, e.g. `http://100.83.199.24:7880` or `wss://livekit.allternit.com`.
    pub livekit_url: String,
    pub livekit_api_key: String,
    pub livekit_api_secret: String,
    /// `ALLTERNIT_CLOUD_API_URL`, base of cloud-api (no trailing slash).
    pub cloud_api_url: String,
    /// `ALLTERNIT_VOICE_WORKER_TOKEN`, bearer service token for cloud-api.
    pub worker_token: String,
    /// `VOICE_SESSION_URL`: the Voice Session WebSocket the worker drives
    /// (default: this binary's own service on 127.0.0.1:8001).
    pub voice_session_url: String,
    /// `VOICE_SESSION_TOKEN`: sidecar token for that socket, if it needs one.
    pub voice_session_token: Option<String>,
    /// `CALL_WORKER_MAX_CALLS`: concurrent calls before availability says no.
    pub max_calls: usize,
    /// `CALL_WORKER_START_TIMEOUT_MS`: how long call start may wait on
    /// cloud-api before the bot speaks the fallback disclosure anyway.
    pub start_timeout: Duration,
}

impl std::fmt::Debug for WorkerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print secrets.
        f.debug_struct("WorkerConfig")
            .field("livekit_url", &self.livekit_url)
            .field("cloud_api_url", &self.cloud_api_url)
            .field("voice_session_url", &self.voice_session_url)
            .field("max_calls", &self.max_calls)
            .field("start_timeout", &self.start_timeout)
            .finish_non_exhaustive()
    }
}

impl WorkerConfig {
    pub fn from_env() -> Result<Self> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let req = |k: &str| -> Result<String> {
            get(k)
                .filter(|v| !v.trim().is_empty())
                .with_context(|| format!("{k} is not set"))
        };
        Ok(Self {
            livekit_url: req("LIVEKIT_URL")?.trim_end_matches('/').to_string(),
            livekit_api_key: req("LIVEKIT_API_KEY")?,
            livekit_api_secret: req("LIVEKIT_API_SECRET")?,
            cloud_api_url: req("ALLTERNIT_CLOUD_API_URL")?
                .trim_end_matches('/')
                .to_string(),
            worker_token: req("ALLTERNIT_VOICE_WORKER_TOKEN")?,
            voice_session_url: get("VOICE_SESSION_URL")
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| "ws://127.0.0.1:8001/v1/voice/session".into()),
            voice_session_token: get("VOICE_SESSION_TOKEN").filter(|v| !v.trim().is_empty()),
            max_calls: get("CALL_WORKER_MAX_CALLS")
                .and_then(|v| v.parse().ok())
                .filter(|n| *n > 0)
                .unwrap_or(8),
            start_timeout: Duration::from_millis(
                get("CALL_WORKER_START_TIMEOUT_MS")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(900),
            ),
        })
    }
}

/// `http(s)://` → `ws(s)://` (LiveKit signalling and `/agent` are WebSockets).
pub fn ws_base(livekit_url: &str) -> String {
    let base = livekit_url.trim_end_matches('/');
    if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_string()
    }
}

/// `http(s)://host[:port]` → `ws(s)://host[:port]/agent?protocol=1`.
pub fn agent_ws_url(livekit_url: &str) -> String {
    format!(
        "{}/agent?protocol={}",
        ws_base(livekit_url),
        super::dispatch::WORKER_PROTOCOL
    )
}

/// `ws(s)://` → `http(s)://` for the LiveKit server API (Twirp).
pub fn http_base(livekit_url: &str) -> String {
    let base = livekit_url.trim_end_matches('/');
    if let Some(rest) = base.strip_prefix("wss://") {
        format!("https://{rest}")
    } else if let Some(rest) = base.strip_prefix("ws://") {
        format!("http://{rest}")
    } else {
        base.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| m.get(k).cloned()
    }

    const BASE: &[(&str, &str)] = &[
        ("LIVEKIT_URL", "http://100.83.199.24:7880/"),
        ("LIVEKIT_API_KEY", "k"),
        ("LIVEKIT_API_SECRET", "s"),
        ("ALLTERNIT_CLOUD_API_URL", "https://cloud.example/"),
        ("ALLTERNIT_VOICE_WORKER_TOKEN", "t"),
    ];

    #[test]
    fn reads_required_and_defaults() {
        let c = WorkerConfig::from_lookup(env(BASE)).unwrap();
        assert_eq!(c.livekit_url, "http://100.83.199.24:7880");
        assert_eq!(c.cloud_api_url, "https://cloud.example");
        assert_eq!(c.max_calls, 8);
        assert_eq!(c.voice_session_url, "ws://127.0.0.1:8001/v1/voice/session");
        assert!(!format!("{c:?}").contains("\"s\""));
    }

    #[test]
    fn missing_secret_is_an_error() {
        let pairs: Vec<_> = BASE
            .iter()
            .filter(|(k, _)| *k != "LIVEKIT_API_SECRET")
            .cloned()
            .collect();
        let err = WorkerConfig::from_lookup(env(&pairs)).unwrap_err();
        assert!(err.to_string().contains("LIVEKIT_API_SECRET"));
    }

    #[test]
    fn agent_url_schemes() {
        assert_eq!(
            agent_ws_url("http://h:7880"),
            "ws://h:7880/agent?protocol=1"
        );
        assert_eq!(agent_ws_url("https://lk.x/"), "wss://lk.x/agent?protocol=1");
        assert_eq!(agent_ws_url("wss://lk.x"), "wss://lk.x/agent?protocol=1");
        assert_eq!(http_base("wss://lk.x/"), "https://lk.x");
        assert_eq!(http_base("http://h:7880"), "http://h:7880");
    }
}
