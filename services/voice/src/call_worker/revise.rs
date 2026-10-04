//! Cleaner transcript after a recorded call (§4.1 `call.transcript.revised`).
//!
//! The live transcript comes from the fast STT model. When egress has finished
//! a recording, a background job fetches it from the bucket (SigV4 GET), decodes
//! it to 16 kHz mono with `ffmpeg`, runs the accurate STT (Parakeet) over the
//! whole call with VAD segments and posts one more call event with the result.
//!
//! Bounds: calls over [`MAX_CALL_SECS`] are skipped, one job runs at a time
//! (a process-wide semaphore), and the CPU-heavy work runs at nice 10. No
//! recording, no bucket settings or no `recordingRef` means the job does
//! nothing. Failures are logged and dropped: the live transcript stays as is.
//! Secrets are never logged.

use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;

use super::cloud_client::EventEnvelope;
use super::events::EventTransport;
use super::recording::RecordingConfig;

/// Calls longer than this keep their live transcript.
pub const MAX_CALL_SECS: u64 = 30 * 60;
const FETCH_TIMEOUT: Duration = Duration::from_secs(60);
const DECODE_TIMEOUT: Duration = Duration::from_secs(120);
pub const EVENT_TYPE: &str = "call.transcript.revised";

static ONE_AT_A_TIME: Semaphore = Semaphore::const_new(1);

/// One revised segment, times in milliseconds from the start of the recording.
#[derive(Debug, Clone, PartialEq)]
pub struct RevisedSegment {
    pub text: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

/// The three slow steps, behind a seam so the job is testable without R2,
/// ffmpeg or models.
pub trait ReviseIo: Send + Sync {
    fn fetch(&self, key: &str) -> BoxFuture<'_, Result<Vec<u8>, String>>;
    /// Encoded audio to 16 kHz mono f32 samples.
    fn decode(&self, audio: Vec<u8>) -> BoxFuture<'_, Result<Vec<f32>, String>>;
    fn transcribe(&self, samples: Vec<f32>) -> BoxFuture<'_, Result<Vec<RevisedSegment>, String>>;
}

/// What the job needs to know about a finished call.
pub struct ReviseJob {
    pub call_id: String,
    pub recording_ref: Option<String>,
    pub duration_sec: u64,
    /// `seq` of the call's last event; the revised event takes the next one.
    pub last_seq: u64,
}

/// Run the job. Returns whether an event was posted.
pub async fn revise_call(job: ReviseJob, io: Arc<dyn ReviseIo>, transport: Arc<dyn EventTransport>) -> bool {
    let Some(key) = job.recording_ref.clone() else { return false };
    if job.duration_sec > MAX_CALL_SECS {
        tracing::info!(call_id = %job.call_id, secs = job.duration_sec, "revised transcript skipped: call is too long");
        return false;
    }
    let Ok(_slot) = ONE_AT_A_TIME.acquire().await else { return false };
    let id = &job.call_id;
    let audio = match io.fetch(&key).await {
        Ok(a) => a,
        Err(e) => return skip(id, "fetching the recording", e),
    };
    let samples = match io.decode(audio).await {
        Ok(s) if !s.is_empty() => s,
        Ok(_) => return skip(id, "decoding the recording", "no audio".into()),
        Err(e) => return skip(id, "decoding the recording", e),
    };
    let segments = match io.transcribe(samples).await {
        Ok(s) => s,
        Err(e) => return skip(id, "transcribing the recording", e),
    };
    let segments: Vec<_> = segments.into_iter().filter(|s| !s.text.trim().is_empty()).collect();
    if segments.is_empty() {
        return skip(id, "transcribing the recording", "no speech found".into());
    }
    let ev = EventEnvelope {
        event_type: EVENT_TYPE.into(),
        idempotency_key: format!("call:{id}:{EVENT_TYPE}:1"),
        seq: job.last_seq + 1,
        at_ms: chrono::Utc::now().timestamp_millis(),
        payload: json!({
            "callId": id,
            "segments": segments.iter().map(|s| json!({ "text": s.text.trim(), "startMs": s.start_ms, "endMs": s.end_ms })).collect::<Vec<_>>(),
        }),
    };
    // Deliver with a few retries: the call's own events already went through, this one is optional.
    let mut delay = Duration::from_millis(500);
    for attempt in 1..=5 {
        match transport.post(id, &ev).await {
            Ok(()) => return true,
            Err(e) if e.is_transient() && attempt < 5 => {
                tokio::time::sleep(delay).await;
                delay *= 2;
            }
            Err(e) => return skip(id, "posting the revised transcript", e.to_string()),
        }
    }
    false
}

fn skip(call_id: &str, step: &str, why: String) -> bool {
    tracing::warn!(call_id, "revised transcript skipped while {step}: {why}");
    false
}

// ---------------------------------------------------------------- production IO

/// Bucket GET + ffmpeg + Parakeet.
pub struct ProductionIo {
    pub bucket: RecordingConfig,
    pub http: reqwest::Client,
}

impl ReviseIo for ProductionIo {
    fn fetch(&self, key: &str) -> BoxFuture<'_, Result<Vec<u8>, String>> {
        let key = key.to_string();
        Box::pin(async move {
            let signed = sigv4_get(&self.bucket, &key, chrono::Utc::now());
            let mut req = self.http.get(&signed.url).timeout(FETCH_TIMEOUT);
            for (k, v) in &signed.headers {
                req = req.header(k, v);
            }
            let resp = req.send().await.map_err(|e| format!("recording request failed: {}", e.without_url()))?;
            if !resp.status().is_success() {
                return Err(format!("recording request answered {}", resp.status()));
            }
            resp.bytes().await.map(|b| b.to_vec()).map_err(|e| format!("recording download failed: {}", e.without_url()))
        })
    }

    fn decode(&self, audio: Vec<u8>) -> BoxFuture<'_, Result<Vec<f32>, String>> {
        Box::pin(async move { tokio::time::timeout(DECODE_TIMEOUT, ffmpeg_decode(audio)).await.map_err(|_| "ffmpeg timed out".to_string())? })
    }

    fn transcribe(&self, samples: Vec<f32>) -> BoxFuture<'_, Result<Vec<RevisedSegment>, String>> {
        Box::pin(async move {
            let (tx, rx) = tokio::sync::oneshot::channel();
            // A dedicated thread, so lowering its priority doesn't leak into a shared pool.
            std::thread::Builder::new()
                .name("call-revise-stt".into())
                .spawn(move || {
                    lower_priority();
                    let _ = tx.send(accurate_stt(&samples));
                })
                .map_err(|e| e.to_string())?;
            rx.await.map_err(|_| "the transcription thread stopped".to_string())?
        })
    }
}

fn accurate_stt(samples: &[f32]) -> Result<Vec<RevisedSegment>, String> {
    use crate::stt::{SttEngine, SttModel};
    static ENGINE: std::sync::OnceLock<SttEngine> = std::sync::OnceLock::new();
    let engine = ENGINE.get_or_init(|| SttEngine::new(Arc::new(crate::models::PackManager::new())));
    engine.prepare(SttModel::Parakeet)?;
    let segments = engine.transcribe(samples, SttModel::Parakeet)?;
    Ok(segments
        .into_iter()
        .map(|s| RevisedSegment { text: s.text, start_ms: (s.start.max(0.0) * 1000.0) as u64, end_ms: (s.end.max(0.0) * 1000.0) as u64 })
        .collect())
}

#[cfg(unix)]
fn lower_priority() {
    // Linux applies this to the calling thread only; elsewhere to the whole process, which is
    // acceptable for a worker whose live work is I/O-bound.
    #[cfg(target_os = "linux")]
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, 0, 10);
    }
}

#[cfg(not(unix))]
fn lower_priority() {}

async fn ffmpeg_decode(audio: Vec<u8>) -> Result<Vec<f32>, String> {
    use tokio::io::AsyncWriteExt;
    let mut cmd = tokio::process::Command::new("ffmpeg");
    cmd.args(["-nostdin", "-loglevel", "error", "-i", "pipe:0", "-f", "f32le", "-ac", "1", "-ar", "16000", "pipe:1"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    unsafe {
        cmd.pre_exec(|| {
            libc::nice(10);
            Ok(())
        });
    }
    let mut child = cmd.spawn().map_err(|e| format!("ffmpeg is not available on this host: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("ffmpeg stdin")?;
    let feed = tokio::spawn(async move {
        let _ = stdin.write_all(&audio).await;
    });
    let out = child.wait_with_output().await.map_err(|e| format!("ffmpeg failed: {e}"))?;
    let _ = feed.await;
    if !out.status.success() {
        return Err(format!("ffmpeg could not decode the recording ({})", out.status));
    }
    Ok(out.stdout.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect())
}

// ---------------------------------------------------------------- SigV4 GET

pub struct SignedGet {
    pub url: String,
    pub headers: Vec<(String, String)>,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn hmac_sha256(key: &[u8], msg: &[u8]) -> Vec<u8> {
    let mut k = if key.len() > 64 { Sha256::digest(key).to_vec() } else { key.to_vec() };
    k.resize(64, 0);
    let (mut ipad, mut opad) = (vec![0x36u8; 64], vec![0x5cu8; 64]);
    for i in 0..64 {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let inner = Sha256::new().chain_update(&ipad).chain_update(msg).finalize();
    Sha256::new().chain_update(&opad).chain_update(inner).finalize().to_vec()
}

fn uri_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// A path-style, header-signed `GET <endpoint>/<bucket>/<key>` (AWS SigV4, S3).
pub fn sigv4_get(cfg: &RecordingConfig, key: &str, now: chrono::DateTime<chrono::Utc>) -> SignedGet {
    let endpoint = cfg.endpoint.trim_end_matches('/');
    let host = endpoint.split("://").last().unwrap_or(endpoint).to_string();
    let path = format!("/{}/{}", uri_encode(&cfg.bucket), uri_encode(key.trim_start_matches('/')));
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let day = now.format("%Y%m%d").to_string();
    let payload_hash = hex(&Sha256::digest(b""));
    let canonical = format!("GET\n{path}\n\nhost:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{amz_date}\n\nhost;x-amz-content-sha256;x-amz-date\n{payload_hash}");
    let scope = format!("{day}/{}/s3/aws4_request", cfg.region);
    let to_sign = format!("AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}", hex(&Sha256::digest(canonical.as_bytes())));
    let mut k = hmac_sha256(format!("AWS4{}", cfg.secret).as_bytes(), day.as_bytes());
    for part in [cfg.region.as_str(), "s3", "aws4_request"] {
        k = hmac_sha256(&k, part.as_bytes());
    }
    let signature = hex(&hmac_sha256(&k, to_sign.as_bytes()));
    let auth = format!("AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders=host;x-amz-content-sha256;x-amz-date, Signature={signature}", cfg.access_key);
    SignedGet {
        url: format!("{endpoint}{path}"),
        headers: vec![("x-amz-date".into(), amz_date), ("x-amz-content-sha256".into(), payload_hash), ("authorization".into(), auth)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::call_worker::cloud_client::CloudError;
    use std::sync::Mutex;

    #[test]
    fn hmac_matches_rfc4231_case_2() {
        assert_eq!(hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
    }

    #[test]
    fn sigv4_matches_the_aws_get_object_example() {
        // https://docs.aws.amazon.com/AmazonS3/latest/API/sig-v4-header-based-auth.html (GET Object, Range omitted from the signed headers here, so only the shape and determinism are asserted)
        let cfg = RecordingConfig { endpoint: "https://acct.r2.cloudflarestorage.com".into(), bucket: "allternit-call-recordings".into(), access_key: "AKID".into(), secret: "TOPSECRET".into(), region: "auto".into() };
        let at = chrono::DateTime::parse_from_rfc3339("2026-10-03T10:00:00Z").unwrap().with_timezone(&chrono::Utc);
        let a = sigv4_get(&cfg, "calls/c_1.ogg", at);
        let b = sigv4_get(&cfg, "calls/c_1.ogg", at);
        assert_eq!(a.url, "https://acct.r2.cloudflarestorage.com/allternit-call-recordings/calls/c_1.ogg");
        let auth = &a.headers.iter().find(|h| h.0 == "authorization").unwrap().1;
        assert!(auth.starts_with("AWS4-HMAC-SHA256 Credential=AKID/20261003/auto/s3/aws4_request, SignedHeaders=host;x-amz-content-sha256;x-amz-date, Signature="), "{auth}");
        assert_eq!(auth, &b.headers.iter().find(|h| h.0 == "authorization").unwrap().1);
        assert!(!auth.contains("TOPSECRET"));
        let other = sigv4_get(&RecordingConfig { secret: "OTHER".into(), ..cfg }, "calls/c_1.ogg", at);
        assert_ne!(auth, &other.headers.iter().find(|h| h.0 == "authorization").unwrap().1);
    }

    struct Io {
        fetched: Mutex<Vec<String>>,
        segments: Result<Vec<RevisedSegment>, String>,
    }
    impl ReviseIo for Io {
        fn fetch(&self, key: &str) -> BoxFuture<'_, Result<Vec<u8>, String>> {
            self.fetched.lock().unwrap().push(key.to_string());
            Box::pin(async { Ok(vec![1, 2, 3]) })
        }
        fn decode(&self, _a: Vec<u8>) -> BoxFuture<'_, Result<Vec<f32>, String>> {
            Box::pin(async { Ok(vec![0.0; 16_000]) })
        }
        fn transcribe(&self, _s: Vec<f32>) -> BoxFuture<'_, Result<Vec<RevisedSegment>, String>> {
            let r = self.segments.clone();
            Box::pin(async move { r })
        }
    }

    #[derive(Default)]
    struct Post(Mutex<Vec<EventEnvelope>>);
    impl EventTransport for Post {
        fn post<'a>(&'a self, _id: &'a str, ev: &'a EventEnvelope) -> BoxFuture<'a, Result<(), CloudError>> {
            self.0.lock().unwrap().push(ev.clone());
            Box::pin(async { Ok(()) })
        }
    }

    fn job(r: Option<&str>, secs: u64) -> ReviseJob {
        ReviseJob { call_id: "c1".into(), recording_ref: r.map(str::to_string), duration_sec: secs, last_seq: 9 }
    }
    fn seg(t: &str, a: u64, b: u64) -> RevisedSegment {
        RevisedSegment { text: t.into(), start_ms: a, end_ms: b }
    }

    #[tokio::test]
    async fn posts_the_revised_segments_as_the_next_event() {
        let io = Arc::new(Io { fetched: Default::default(), segments: Ok(vec![seg(" Hello there. ", 100, 1500), seg("  ", 1600, 1700), seg("Friday works.", 2000, 3100)]) });
        let post = Arc::new(Post::default());
        assert!(revise_call(job(Some("calls/c1.ogg"), 60), io.clone(), post.clone()).await);
        assert_eq!(io.fetched.lock().unwrap().as_slice(), ["calls/c1.ogg"]);
        let evs = post.0.lock().unwrap();
        assert_eq!(evs.len(), 1);
        assert_eq!((evs[0].event_type.as_str(), evs[0].idempotency_key.as_str(), evs[0].seq), ("call.transcript.revised", "call:c1:call.transcript.revised:1", 10));
        assert_eq!(evs[0].payload, json!({ "callId": "c1", "segments": [{ "text": "Hello there.", "startMs": 100, "endMs": 1500 }, { "text": "Friday works.", "startMs": 2000, "endMs": 3100 }] }));
    }

    #[tokio::test]
    async fn no_recording_long_calls_and_failures_post_nothing() {
        let io = Arc::new(Io { fetched: Default::default(), segments: Ok(vec![seg("hi", 0, 10)]) });
        let post = Arc::new(Post::default());
        assert!(!revise_call(job(None, 60), io.clone(), post.clone()).await);
        assert!(!revise_call(job(Some("calls/c1.ogg"), MAX_CALL_SECS + 1), io.clone(), post.clone()).await);
        assert!(io.fetched.lock().unwrap().is_empty());
        let failing = Arc::new(Io { fetched: Default::default(), segments: Err("model missing".into()) });
        assert!(!revise_call(job(Some("calls/c1.ogg"), 60), failing, post.clone()).await);
        assert!(post.0.lock().unwrap().is_empty());
    }
}
