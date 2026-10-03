//! Call recording through LiveKit Egress (`livekit/egress`) into an
//! S3-compatible bucket (Cloudflare R2).
//!
//! Recording is off unless the bot's cloud config asks for it
//! (`bot.recording == true`, set only when the owner configured consent). Even
//! then a call is recorded only if egress and the bucket are really there. The
//! decision is made **before the opening is spoken**, because the fixed first
//! line states whether the call is recorded, and that statement must follow what
//! is actually happening, never just the config:
//!
//! | requested | bucket env | egress start | recorded | disclosure |
//! |---|---|---|---|---|
//! | no | any | not tried | no | "isn't recorded" |
//! | yes | incomplete | not tried | no (logged, `recording:false`) | "isn't recorded" |
//! | yes | complete | fails or times out | no (logged, `recording:false`) | "isn't recorded" |
//! | yes | complete | ok | yes | "may be recorded" |
//!
//! The LiveKit calls are behind [`Recorder`] so the decision logic is tested
//! with fakes; `room.rs` supplies the real one.

use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;

/// Longest the opening waits for egress to accept the job.
pub const START_TIMEOUT: Duration = Duration::from_millis(2500);

pub const ENV_ENDPOINT: &str = "ALLTERNIT_RECORDING_S3_ENDPOINT";
pub const ENV_BUCKET: &str = "ALLTERNIT_RECORDING_S3_BUCKET";
pub const ENV_ACCESS_KEY: &str = "ALLTERNIT_RECORDING_S3_ACCESS_KEY";
pub const ENV_SECRET: &str = "ALLTERNIT_RECORDING_S3_SECRET";
pub const ENV_REGION: &str = "ALLTERNIT_RECORDING_S3_REGION";

/// S3-compatible destination for call audio.
#[derive(Clone, PartialEq, Eq)]
pub struct RecordingConfig {
    pub endpoint: String,
    pub bucket: String,
    pub access_key: String,
    pub secret: String,
    /// R2 takes `auto`; real S3 needs the bucket's region.
    pub region: String,
}

impl std::fmt::Debug for RecordingConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecordingConfig")
            .field("endpoint", &self.endpoint)
            .field("bucket", &self.bucket)
            .field("region", &self.region)
            .finish_non_exhaustive()
    }
}

/// What the environment provides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordingEnv {
    Configured(RecordingConfig),
    /// Variables that are unset. All five when recording isn't set up at all.
    Missing(Vec<&'static str>),
}

impl RecordingEnv {
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let val = |k: &str| get(k).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let (endpoint, bucket, access_key, secret) =
            (val(ENV_ENDPOINT), val(ENV_BUCKET), val(ENV_ACCESS_KEY), val(ENV_SECRET));
        let region = val(ENV_REGION);
        match (endpoint, bucket, access_key, secret, region) {
            (Some(endpoint), Some(bucket), Some(access_key), Some(secret), Some(region)) => {
                RecordingEnv::Configured(RecordingConfig { endpoint, bucket, access_key, secret, region })
            }
            (e, b, a, s, r) => {
                let mut missing = Vec::new();
                for (v, name) in [
                    (e.is_none(), ENV_ENDPOINT),
                    (b.is_none(), ENV_BUCKET),
                    (a.is_none(), ENV_ACCESS_KEY),
                    (s.is_none(), ENV_SECRET),
                    (r.is_none(), ENV_REGION),
                ] {
                    if v {
                        missing.push(name);
                    }
                }
                RecordingEnv::Missing(missing)
            }
        }
    }

    pub fn from_env() -> Self {
        Self::from_lookup(|k| std::env::var(k).ok())
    }
}

/// What to do for a call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordingPlan {
    /// The bot isn't configured for recording.
    NotRequested,
    /// Try to start egress.
    Start,
    /// Requested, but it can't be done. The reason is logged.
    Unavailable(String),
}

pub fn plan(requested: bool, env: &RecordingEnv) -> RecordingPlan {
    if !requested {
        return RecordingPlan::NotRequested;
    }
    match env {
        RecordingEnv::Configured(_) => RecordingPlan::Start,
        RecordingEnv::Missing(vars) => RecordingPlan::Unavailable(format!(
            "recording is configured for this bot but the worker has no bucket settings (missing {})",
            vars.join(", ")
        )),
    }
}

/// Bucket key for a call's audio: `calls/<callId>.ogg`. The call id comes from
/// cloud-api, but is reduced to safe characters anyway since it ends up in a
/// storage path.
pub fn object_key(call_id: &str) -> String {
    let safe: String = call_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') { c } else { '_' })
        .collect();
    format!("calls/{safe}.ogg")
}

/// Starts and stops a recording. [`room`](super::room) supplies the LiveKit
/// Egress implementation.
pub trait Recorder: Send + Sync + 'static {
    /// Start an audio-only recording of `room` into `key`. Resolves with the
    /// egress id once the egress service has accepted the job.
    fn start(&self, room: &str, key: &str) -> BoxFuture<'_, Result<String, String>>;
    /// Stop the recording; the file finishes uploading shortly after.
    fn stop(&self, egress_id: &str) -> BoxFuture<'_, Result<(), String>>;
}

/// The recording of one call.
pub struct Recording {
    /// Whether this call is actually being recorded. Drives the disclosure.
    pub active: bool,
    /// Why recording was requested but isn't happening. Logged, and reported as
    /// `recording:false` in `call.state.changed`.
    pub unavailable: Option<String>,
    started: Option<(Arc<dyn Recorder>, String, String)>,
}

impl Recording {
    /// No recording and nothing to report.
    pub fn none() -> Self {
        Self { active: false, unavailable: None, started: None }
    }

    /// A recording state for call tests that don't exercise egress.
    #[cfg(test)]
    pub(crate) fn assumed(active: bool) -> Self {
        Self { active, unavailable: None, started: None }
    }

    /// Decide and, if asked and able, start. Never fails the call.
    pub async fn begin(
        recorder: Option<Arc<dyn Recorder>>,
        requested: bool,
        env: &RecordingEnv,
        room: &str,
        call_id: &str,
        timeout: Duration,
    ) -> Self {
        let unavailable = |why: String| {
            tracing::error!(call_id, "{why}; the call is NOT being recorded");
            Self { active: false, unavailable: Some(why), started: None }
        };
        match plan(requested, env) {
            RecordingPlan::NotRequested => Self::none(),
            RecordingPlan::Unavailable(why) => unavailable(why),
            RecordingPlan::Start => {
                let Some(recorder) = recorder else {
                    return unavailable("recording is configured but this worker build has no egress client".into());
                };
                let key = object_key(call_id);
                match tokio::time::timeout(timeout, recorder.start(room, &key)).await {
                    Ok(Ok(egress_id)) => {
                        tracing::info!(call_id, %egress_id, %key, "recording started");
                        Self { active: true, unavailable: None, started: Some((recorder, egress_id, key)) }
                    }
                    Ok(Err(e)) => unavailable(format!("egress refused the recording: {e}")),
                    Err(_) => unavailable(format!("egress did not answer within {} ms", timeout.as_millis())),
                }
            }
        }
    }

    /// Stop at call end. Returns the bucket key for `call.ended.recordingRef`,
    /// or `None` when nothing was recorded or stopping failed (which is logged:
    /// no reference is given for a file that may not exist).
    pub async fn finish(self, call_id: &str) -> Option<String> {
        let (recorder, egress_id, key) = self.started?;
        match recorder.stop(&egress_id).await {
            Ok(()) => Some(key),
            Err(e) => {
                tracing::error!(call_id, %egress_id, "stopping the recording failed, no recordingRef: {e}");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn env_with(pairs: &[(&str, &str)]) -> RecordingEnv {
        RecordingEnv::from_lookup(|k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string()))
    }

    fn full() -> RecordingEnv {
        env_with(&[
            (ENV_ENDPOINT, "https://acct.r2.cloudflarestorage.com"),
            (ENV_BUCKET, "allternit-calls"),
            (ENV_ACCESS_KEY, "AK"),
            (ENV_SECRET, "SK"),
            (ENV_REGION, "auto"),
        ])
    }

    #[derive(Default)]
    struct Fake {
        start: Mutex<Option<Result<String, String>>>,
        hang: bool,
        started: Mutex<Vec<(String, String)>>,
        stopped: Mutex<Vec<String>>,
        stop_err: bool,
    }

    impl Recorder for Fake {
        fn start(&self, room: &str, key: &str) -> BoxFuture<'_, Result<String, String>> {
            self.started.lock().unwrap().push((room.into(), key.into()));
            if self.hang {
                return Box::pin(std::future::pending());
            }
            let r = self.start.lock().unwrap().take().unwrap_or_else(|| Ok("EG_1".into()));
            Box::pin(async move { r })
        }
        fn stop(&self, id: &str) -> BoxFuture<'_, Result<(), String>> {
            self.stopped.lock().unwrap().push(id.into());
            let r = if self.stop_err { Err("boom".to_string()) } else { Ok(()) };
            Box::pin(async move { r })
        }
    }

    #[test]
    fn env_needs_all_five_and_names_the_gaps() {
        assert!(matches!(full(), RecordingEnv::Configured(c) if c.bucket == "allternit-calls" && c.region == "auto"));
        assert_eq!(
            env_with(&[(ENV_BUCKET, "b"), (ENV_SECRET, " ")]),
            RecordingEnv::Missing(vec![ENV_ENDPOINT, ENV_ACCESS_KEY, ENV_SECRET, ENV_REGION])
        );
        assert_eq!(env_with(&[]), RecordingEnv::Missing(vec![ENV_ENDPOINT, ENV_BUCKET, ENV_ACCESS_KEY, ENV_SECRET, ENV_REGION]));
    }

    #[test]
    fn config_debug_hides_secrets() {
        let RecordingEnv::Configured(c) = full() else { panic!() };
        let d = format!("{c:?}");
        assert!(d.contains("allternit-calls") && !d.contains("SK") && !d.contains("AK"), "{d}");
    }

    #[test]
    fn plan_follows_the_decision_table() {
        assert_eq!(plan(false, &full()), RecordingPlan::NotRequested);
        assert_eq!(plan(false, &env_with(&[])), RecordingPlan::NotRequested);
        assert_eq!(plan(true, &full()), RecordingPlan::Start);
        let RecordingPlan::Unavailable(why) = plan(true, &env_with(&[(ENV_BUCKET, "b")])) else { panic!() };
        assert!(why.contains(ENV_ENDPOINT) && why.contains("missing"), "{why}");
    }

    #[test]
    fn keys_are_path_safe() {
        assert_eq!(object_key("call_ab-12"), "calls/call_ab-12.ogg");
        assert_eq!(object_key("../../etc/passwd"), "calls/______etc_passwd.ogg");
    }

    #[tokio::test]
    async fn requested_and_available_records_and_returns_the_key() {
        let f = Arc::new(Fake::default());
        let rec = Recording::begin(Some(f.clone()), true, &full(), "call-xyz", "c1", Duration::from_secs(1)).await;
        assert!(rec.active && rec.unavailable.is_none());
        assert_eq!(*f.started.lock().unwrap(), [("call-xyz".to_string(), "calls/c1.ogg".to_string())]);
        assert_eq!(rec.finish("c1").await.as_deref(), Some("calls/c1.ogg"));
        assert_eq!(*f.stopped.lock().unwrap(), ["EG_1"]);
    }

    #[tokio::test]
    async fn not_requested_never_touches_egress() {
        let f = Arc::new(Fake::default());
        let rec = Recording::begin(Some(f.clone()), false, &full(), "r", "c1", Duration::from_secs(1)).await;
        assert!(!rec.active && rec.unavailable.is_none());
        assert!(f.started.lock().unwrap().is_empty());
        assert_eq!(rec.finish("c1").await, None);
        assert!(f.stopped.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn missing_bucket_config_means_not_recorded_and_says_why() {
        let f = Arc::new(Fake::default());
        let rec = Recording::begin(Some(f.clone()), true, &env_with(&[]), "r", "c1", Duration::from_secs(1)).await;
        assert!(!rec.active);
        assert!(rec.unavailable.as_deref().unwrap().contains("bucket"));
        assert!(f.started.lock().unwrap().is_empty(), "no egress job without a destination");
        assert_eq!(rec.finish("c1").await, None);
    }

    #[tokio::test]
    async fn egress_refusal_means_not_recorded() {
        let f = Arc::new(Fake { start: Mutex::new(Some(Err("no egress available".into()))), ..Default::default() });
        let rec = Recording::begin(Some(f.clone()), true, &full(), "r", "c1", Duration::from_secs(1)).await;
        assert!(!rec.active);
        assert!(rec.unavailable.as_deref().unwrap().contains("no egress available"));
        assert_eq!(rec.finish("c1").await, None);
        assert!(f.stopped.lock().unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn egress_that_never_answers_times_out_not_recorded() {
        let f = Arc::new(Fake { hang: true, ..Default::default() });
        let rec = Recording::begin(Some(f), true, &full(), "r", "c1", START_TIMEOUT).await;
        assert!(!rec.active);
        assert!(rec.unavailable.as_deref().unwrap().contains("2500 ms"));
    }

    #[tokio::test]
    async fn no_egress_client_means_not_recorded() {
        let rec = Recording::begin(None, true, &full(), "r", "c1", Duration::from_secs(1)).await;
        assert!(!rec.active && rec.unavailable.is_some());
    }

    #[tokio::test]
    async fn failed_stop_gives_no_reference() {
        let f = Arc::new(Fake { stop_err: true, ..Default::default() });
        let rec = Recording::begin(Some(f), true, &full(), "r", "c1", Duration::from_secs(1)).await;
        assert!(rec.active);
        assert_eq!(rec.finish("c1").await, None);
    }
}
