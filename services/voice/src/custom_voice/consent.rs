//! Consent gate for custom voices.
//!
//! A custom voice is a reference clip of a real person. Nothing here will
//! turn a clip into speech unless allternit-cloud-api says, *now*, that a
//! consent record for this owner and this voice is on file and not revoked.
//! The service never stores clips or embeddings on disk, and a revoked voice
//! fails the next check (sessions re-check every [`RECHECK_AFTER`]).

use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;

use crate::session::ws::CloudApi;

/// How long a positive consent answer is trusted inside a running session.
pub const RECHECK_AFTER: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsentError {
    /// No consent record, revoked, or owned by someone else. Deliberately one
    /// answer: callers must not learn which.
    NotOnFile,
    /// Cloud could not be asked. Fails closed.
    Unavailable(String),
}

impl std::fmt::Display for ConsentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConsentError::NotOnFile => f.write_str("no consent on file for this custom voice (it may have been revoked)"),
            ConsentError::Unavailable(m) => write!(f, "cannot verify consent for this custom voice: {m}"),
        }
    }
}

/// A live consent record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    pub clip_sha256: String,
    pub name: String,
}

/// Asks the consent system. Blocking: called from engine worker threads.
pub trait ConsentChecker: Send + Sync + 'static {
    fn grant(&self, owner: &str, voice_id: &str) -> Result<Grant, ConsentError>;
    /// The reference clip (a WAV) of a voice that has a live grant.
    fn clip(&self, owner: &str, voice_id: &str) -> Result<Vec<u8>, ConsentError>;
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GrantBody {
    clip_sha256: String,
    name: String,
}

/// Checks consent against cloud-api with the worker token (the same
/// credential tickets are redeemed with).
pub struct CloudConsentChecker {
    api: CloudApi,
    rt: tokio::runtime::Handle,
}

impl CloudConsentChecker {
    /// `None` outside a tokio runtime (it needs a handle to block on).
    pub fn new(api: CloudApi) -> Option<Arc<Self>> {
        let rt = tokio::runtime::Handle::try_current().ok()?;
        Some(Arc::new(Self { api, rt }))
    }

    fn get(&self, owner: &str, voice_id: &str, leaf: &str) -> Result<reqwest::Response, ConsentError> {
        let path = format!("/api/v1/voice/custom-voices/{voice_id}/{leaf}");
        let req = self.api.get(&path).query(&[("owner", owner)]);
        let resp = self
            .rt
            .block_on(req.send())
            .map_err(|e| ConsentError::Unavailable(if e.is_timeout() { "timed out".into() } else { "unreachable".into() }))?;
        match resp.status().as_u16() {
            200 => Ok(resp),
            404 | 403 | 410 => Err(ConsentError::NotOnFile),
            s => Err(ConsentError::Unavailable(format!("cloud answered {s}"))),
        }
    }
}

impl ConsentChecker for CloudConsentChecker {
    fn grant(&self, owner: &str, voice_id: &str) -> Result<Grant, ConsentError> {
        let resp = self.get(owner, voice_id, "consent")?;
        let body = self
            .rt
            .block_on(resp.json::<GrantBody>())
            .map_err(|_| ConsentError::Unavailable("unreadable answer".into()))?;
        Ok(Grant { clip_sha256: body.clip_sha256, name: body.name })
    }

    fn clip(&self, owner: &str, voice_id: &str) -> Result<Vec<u8>, ConsentError> {
        let resp = self.get(owner, voice_id, "clip")?;
        self.rt
            .block_on(resp.bytes())
            .map(|b| b.to_vec())
            .map_err(|_| ConsentError::Unavailable("clip download failed".into()))
    }
}
