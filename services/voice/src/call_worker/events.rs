//! `call.*` events (§4.1) and the per-call ordered delivery queue.
//!
//! Each event gets idempotency key `call:<callId>:<type>:<n>` (`n` counts that
//! type within the call, from 1) and a call-wide `seq`. One task per call posts
//! them strictly in order: a transient failure (network, 408/429/5xx) blocks the
//! queue and retries with capped exponential backoff forever, so nothing is lost
//! or reordered while cloud-api is down. A permanent rejection (other 4xx) is
//! logged and skipped, since retrying a malformed event can't succeed and would
//! wedge every later event of the call.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::cloud_client::{CloudClient, CloudError, EventEnvelope};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Speaker {
    Caller,
    Bot,
    Human,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TransferMode {
    Cold,
    Warm,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CallEvent {
    Started { direction: String, from: String, to: String, number_id: String },
    TranscriptDelta { speaker: Speaker, text: String, is_final: bool, segment_id: String },
    Dtmf { digits: String, from: String },
    StateChanged { held: bool, muted_bot: bool, muted_caller: bool, speaker: Option<Speaker>, recording: bool },
    Transferred { to: String, mode: TransferMode, ok: bool, reason: Option<String> },
    Takeover { by: String, active: bool },
    VoicemailDetected { action: String },
    Ended { duration_sec: u64, reason: String, recording_ref: Option<String> },
}

impl CallEvent {
    pub fn type_name(&self) -> &'static str {
        match self {
            CallEvent::Started { .. } => "call.started",
            CallEvent::TranscriptDelta { .. } => "call.transcript.delta",
            CallEvent::Dtmf { .. } => "call.dtmf",
            CallEvent::StateChanged { .. } => "call.state.changed",
            CallEvent::Transferred { .. } => "call.transferred",
            CallEvent::Takeover { .. } => "call.takeover",
            CallEvent::VoicemailDetected { .. } => "call.voicemail.detected",
            CallEvent::Ended { .. } => "call.ended",
        }
    }

    /// camelCase payload; always includes `callId`.
    pub fn payload(&self, call_id: &str) -> Value {
        let mut v = match self {
            CallEvent::Started { direction, from, to, number_id } => {
                json!({"direction": direction, "from": from, "to": to, "numberId": number_id})
            }
            CallEvent::TranscriptDelta { speaker, text, is_final, segment_id } => {
                json!({"speaker": speaker, "text": text, "final": is_final, "segmentId": segment_id})
            }
            CallEvent::Dtmf { digits, from } => json!({"digits": digits, "from": from}),
            CallEvent::StateChanged { held, muted_bot, muted_caller, speaker, recording } => json!({
                "held": held, "mutedBot": muted_bot, "mutedCaller": muted_caller, "speaker": speaker,
                "recording": recording
            }),
            CallEvent::Transferred { to, mode, ok, reason } => {
                let mut v = json!({"to": to, "mode": mode, "ok": ok});
                if let Some(r) = reason {
                    v["reason"] = json!(r);
                }
                v
            }
            CallEvent::Takeover { by, active } => json!({"by": by, "active": active}),
            CallEvent::VoicemailDetected { action } => json!({"action": action}),
            CallEvent::Ended { duration_sec, reason, recording_ref } => {
                let mut v = json!({"durationSec": duration_sec, "reason": reason});
                if let Some(r) = recording_ref {
                    v["recordingRef"] = json!(r);
                }
                v
            }
        };
        v["callId"] = json!(call_id);
        v
    }
}

/// Assigns `seq` and idempotency keys.
#[derive(Debug)]
pub struct Sequencer {
    call_id: String,
    seq: u64,
    per_type: HashMap<&'static str, u64>,
}

impl Sequencer {
    pub fn new(call_id: &str) -> Self {
        Self { call_id: call_id.to_string(), seq: 0, per_type: HashMap::new() }
    }

    pub fn envelope(&mut self, ev: &CallEvent) -> EventEnvelope {
        let ty = ev.type_name();
        let n = self.per_type.entry(ty).or_insert(0);
        *n += 1;
        self.seq += 1;
        EventEnvelope {
            event_type: ty.to_string(),
            idempotency_key: format!("call:{}:{}:{}", self.call_id, ty, n),
            seq: self.seq,
            at_ms: chrono::Utc::now().timestamp_millis(),
            payload: ev.payload(&self.call_id),
        }
    }
}

/// Where events go. [`CloudClient`] in production; fakes in tests.
pub trait EventTransport: Send + Sync + 'static {
    fn post<'a>(
        &'a self,
        call_id: &'a str,
        ev: &'a EventEnvelope,
    ) -> BoxFuture<'a, Result<(), CloudError>>;
}

impl EventTransport for CloudClient {
    fn post<'a>(
        &'a self,
        call_id: &'a str,
        ev: &'a EventEnvelope,
    ) -> BoxFuture<'a, Result<(), CloudError>> {
        Box::pin(self.post_event(call_id, ev))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Backoff {
    pub initial: Duration,
    pub max: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self { initial: Duration::from_millis(200), max: Duration::from_secs(10) }
    }
}

/// Ordered, retrying event queue for one call.
pub struct EventQueue {
    seq: Sequencer,
    tx: Option<mpsc::UnboundedSender<EventEnvelope>>,
    task: Option<JoinHandle<()>>,
}

impl EventQueue {
    pub fn start(call_id: &str, transport: Arc<dyn EventTransport>, backoff: Backoff) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<EventEnvelope>();
        let id = call_id.to_string();
        let task = tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                let mut delay = backoff.initial;
                let mut attempt = 0u32;
                loop {
                    attempt += 1;
                    match transport.post(&id, &ev).await {
                        Ok(()) => break,
                        Err(CloudError::Transient(e)) => {
                            tracing::warn!(call_id = %id, key = %ev.idempotency_key, attempt, "event post failed, retrying: {e}");
                            tokio::time::sleep(delay).await;
                            delay = (delay * 2).min(backoff.max);
                        }
                        Err(CloudError::Permanent(e)) => {
                            tracing::error!(call_id = %id, key = %ev.idempotency_key, "event rejected by cloud-api, skipping: {e}");
                            break;
                        }
                    }
                }
            }
        });
        Self { seq: Sequencer::new(call_id), tx: Some(tx), task: Some(task) }
    }

    pub fn emit(&mut self, ev: CallEvent) {
        let env = self.seq.envelope(&ev);
        if let Some(tx) = &self.tx {
            // The receiver only stops after close(); a send error can't happen earlier.
            let _ = tx.send(env);
        }
    }

    /// Stop accepting events and return a handle that resolves once every
    /// queued event is delivered (or permanently rejected).
    pub fn close(mut self) -> JoinHandle<()> {
        self.tx.take();
        self.task.take().expect("queue task")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Recorder {
        got: Mutex<Vec<EventEnvelope>>,
        /// Fail this many posts transiently before succeeding.
        transient_failures: Mutex<u32>,
        reject_type: Option<&'static str>,
    }

    impl EventTransport for Recorder {
        fn post<'a>(&'a self, _id: &'a str, ev: &'a EventEnvelope) -> BoxFuture<'a, Result<(), CloudError>> {
            Box::pin(async move {
                {
                    let mut f = self.transient_failures.lock().unwrap();
                    if *f > 0 {
                        *f -= 1;
                        return Err(CloudError::Transient("down".into()));
                    }
                }
                if self.reject_type == Some(ev.event_type.as_str()) {
                    return Err(CloudError::Permanent("bad".into()));
                }
                self.got.lock().unwrap().push(ev.clone());
                Ok(())
            })
        }
    }

    fn fast() -> Backoff {
        Backoff { initial: Duration::from_millis(1), max: Duration::from_millis(4) }
    }

    #[test]
    fn idempotency_keys_count_per_type() {
        let mut s = Sequencer::new("c1");
        let t = |text: &str| CallEvent::TranscriptDelta {
            speaker: Speaker::Caller,
            text: text.into(),
            is_final: true,
            segment_id: "s".into(),
        };
        let a = s.envelope(&CallEvent::Started {
            direction: "inbound".into(),
            from: "+1".into(),
            to: "+2".into(),
            number_id: "n".into(),
        });
        let b = s.envelope(&t("hi"));
        let c = s.envelope(&t("there"));
        let d = s.envelope(&CallEvent::Dtmf { digits: "1".into(), from: "+1".into() });
        assert_eq!(a.idempotency_key, "call:c1:call.started:1");
        assert_eq!(b.idempotency_key, "call:c1:call.transcript.delta:1");
        assert_eq!(c.idempotency_key, "call:c1:call.transcript.delta:2");
        assert_eq!(d.idempotency_key, "call:c1:call.dtmf:1");
        assert_eq!([a.seq, b.seq, c.seq, d.seq], [1, 2, 3, 4]);
        assert_eq!(c.payload["callId"], "c1");
        assert_eq!(c.payload["final"], true);
        assert_eq!(c.payload["segmentId"], "s");
    }

    #[test]
    fn payload_shapes() {
        let p = CallEvent::StateChanged { held: true, muted_bot: false, muted_caller: true, speaker: None, recording: false }
            .payload("c");
        assert_eq!(
            p,
            json!({"held":true,"mutedBot":false,"mutedCaller":true,"speaker":null,"recording":false,"callId":"c"})
        );
        let p = CallEvent::Ended { duration_sec: 42, reason: "caller_hangup".into(), recording_ref: None }
            .payload("c");
        assert_eq!(p, json!({"durationSec":42,"reason":"caller_hangup","callId":"c"}));
        let p = CallEvent::Transferred { to: "+1".into(), mode: TransferMode::Cold, ok: false, reason: Some("x".into()) }
            .payload("c");
        assert_eq!(p["mode"], "cold");
        assert_eq!(p["reason"], "x");
    }

    #[tokio::test]
    async fn delivers_in_order_through_transient_failures() {
        let rec = Arc::new(Recorder { transient_failures: Mutex::new(3), ..Default::default() });
        let mut q = EventQueue::start("c1", rec.clone(), fast());
        for i in 0..5 {
            q.emit(CallEvent::Dtmf { digits: i.to_string(), from: "+1".into() });
        }
        q.close().await.unwrap();
        let got = rec.got.lock().unwrap();
        let digits: Vec<_> = got.iter().map(|e| e.payload["digits"].as_str().unwrap().to_string()).collect();
        assert_eq!(digits, ["0", "1", "2", "3", "4"]);
        let seqs: Vec<_> = got.iter().map(|e| e.seq).collect();
        assert_eq!(seqs, [1, 2, 3, 4, 5]);
    }

    #[tokio::test]
    async fn permanent_rejection_skips_without_wedging() {
        let rec = Arc::new(Recorder { reject_type: Some("call.dtmf"), ..Default::default() });
        let mut q = EventQueue::start("c1", rec.clone(), fast());
        q.emit(CallEvent::Dtmf { digits: "1".into(), from: "+1".into() });
        q.emit(CallEvent::Takeover { by: "u".into(), active: true });
        q.close().await.unwrap();
        let got = rec.got.lock().unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].event_type, "call.takeover");
    }
}
