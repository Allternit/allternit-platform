//! Meeting worker configuration (environment) and per-job settings (LiveKit
//! dispatch metadata from cloud-api's meeting launcher).

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use super::ROOM_PREFIX;
use crate::stt::SttModel;

#[derive(Clone)]
pub struct MeetingWorkerConfig {
    /// `LIVEKIT_URL`.
    pub livekit_url: String,
    pub livekit_api_key: String,
    pub livekit_api_secret: String,
    /// `MEETING_WORKER_MAX_SESSIONS`: concurrent meetings before availability says no.
    pub max_sessions: usize,
    /// `MEETING_WORKER_STT_MODEL`: `moonshine` (default) or `parakeet`.
    pub stt_model: SttModel,
}

impl std::fmt::Debug for MeetingWorkerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print secrets.
        f.debug_struct("MeetingWorkerConfig")
            .field("livekit_url", &self.livekit_url)
            .field("max_sessions", &self.max_sessions)
            .field("stt_model", &self.stt_model)
            .finish_non_exhaustive()
    }
}

impl MeetingWorkerConfig {
    pub fn from_env() -> Result<Self> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let req = |k: &str| -> Result<String> {
            get(k).filter(|v| !v.trim().is_empty()).with_context(|| format!("{k} is not set"))
        };
        Ok(Self {
            livekit_url: req("LIVEKIT_URL")?.trim_end_matches('/').to_string(),
            livekit_api_key: req("LIVEKIT_API_KEY")?,
            livekit_api_secret: req("LIVEKIT_API_SECRET")?,
            max_sessions: get("MEETING_WORKER_MAX_SESSIONS")
                .and_then(|v| v.parse().ok())
                .filter(|n| *n > 0)
                .unwrap_or(4),
            stt_model: SttModel::parse(get("MEETING_WORKER_STT_MODEL").as_deref())
                .map_err(anyhow::Error::msg)?,
        })
    }
}

/// One meeting job: the session id from the room name `meeting-<sid>`, plus
/// optional dispatch metadata (JSON object) from the launcher:
/// `{"session_id"?: "<sid>", "voice"?: "af_heart"}`. Everything else the
/// worker needs comes from the pod in the room, so no token is passed.
#[derive(Debug, Clone)]
pub struct MeetingJob {
    pub session_id: String,
    /// Kokoro voice for spoken replies (default `af_heart`).
    pub voice: Option<String>,
}

#[derive(Deserialize)]
struct Metadata {
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    voice: Option<String>,
}

impl MeetingJob {
    pub fn parse(room_name: &str, metadata: &str) -> Result<Self> {
        let Some(sid) = room_name.strip_prefix(ROOM_PREFIX).filter(|s| !s.is_empty()) else {
            bail!("room {room_name} is not a meeting room");
        };
        let m: Metadata = if metadata.trim().is_empty() {
            Metadata { session_id: None, voice: None }
        } else {
            serde_json::from_str(metadata).context("meeting job metadata is not valid JSON")?
        };
        if let Some(meta_sid) = m.session_id.as_deref().filter(|s| *s != sid) {
            bail!("room {room_name} does not match session {meta_sid}");
        }
        Ok(Self { session_id: sid.to_string(), voice: m.voice.filter(|v| !v.trim().is_empty()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_comes_from_the_room_and_metadata_must_agree() {
        let j = MeetingJob::parse("meeting-7d0c", r#"{"session_id":"7d0c","voice":"bf_emma"}"#).unwrap();
        assert_eq!((j.session_id.as_str(), j.voice.as_deref()), ("7d0c", Some("bf_emma")));
        assert_eq!(MeetingJob::parse("meeting-7d0c", "").unwrap().session_id, "7d0c");
        assert!(MeetingJob::parse("meeting-other", r#"{"session_id":"7d0c"}"#).is_err());
        assert!(MeetingJob::parse("call-7d0c", "").is_err());
        assert!(MeetingJob::parse("meeting-", "").is_err());
        assert!(MeetingJob::parse("meeting-7d0c", "not json").is_err());
    }
}
