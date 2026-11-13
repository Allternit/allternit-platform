//! In-process fan-out for meeting sessions: the user SSE stream
//! (`GET /meeting-sessions/:sid/events`, contract section 5) and the pod
//! control stream (`GET /meeting-sessions/:sid/control-stream`, section 6).
//!
//! cloud-api runs as one process, so a per-session broadcast channel plus a
//! ring of the last 500 frames covers `Last-Event-ID` replay. Event ids come
//! from one process-wide counter seeded with the start time in microseconds,
//! so they keep increasing across restarts (a client resuming after a
//! restart gets the current state frame and no stale replay). A session's
//! hub is dropped 10 minutes after it ends.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::Value;
use tokio::sync::broadcast;
use uuid::Uuid;

/// Replay window for `Last-Event-ID`.
pub const REPLAY_FRAMES: usize = 500;
const CHANNEL_CAPACITY: usize = 256;
const RETIRE_AFTER: Duration = Duration::from_secs(600);

#[derive(Debug, Clone)]
pub struct Frame {
    pub id: u64,
    pub event: &'static str,
    pub data: Value,
}

struct Hub {
    ring: VecDeque<Arc<Frame>>,
    events: broadcast::Sender<Arc<Frame>>,
    control: broadcast::Sender<Value>,
}

impl Hub {
    fn new() -> Self {
        Hub {
            ring: VecDeque::with_capacity(16),
            events: broadcast::channel(CHANNEL_CAPACITY).0,
            control: broadcast::channel(CHANNEL_CAPACITY).0,
        }
    }
}

fn hubs() -> &'static Mutex<HashMap<Uuid, Hub>> {
    static HUBS: OnceLock<Mutex<HashMap<Uuid, Hub>>> = OnceLock::new();
    HUBS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_id() -> u64 {
    static NEXT: OnceLock<AtomicU64> = OnceLock::new();
    NEXT.get_or_init(|| {
        let seed = chrono::Utc::now().timestamp_micros().max(1) as u64;
        AtomicU64::new(seed)
    })
    .fetch_add(1, Ordering::Relaxed)
}

fn with_hub<T>(sid: Uuid, f: impl FnOnce(&mut Hub) -> T) -> T {
    let mut map = hubs().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    f(map.entry(sid).or_insert_with(Hub::new))
}

/// Publish one event to the session's SSE subscribers and the replay ring.
/// `data` should already carry `type` and `session_id` (types.ts
/// `MeetingEvent`).
pub fn emit(sid: Uuid, event: &'static str, data: Value) -> u64 {
    with_hub(sid, |hub| {
        let frame = Arc::new(Frame { id: next_id(), event, data });
        if hub.ring.len() == REPLAY_FRAMES {
            hub.ring.pop_front();
        }
        hub.ring.push_back(frame.clone());
        // No subscribers is fine.
        let _ = hub.events.send(frame.clone());
        frame.id
    })
}

/// Subscribe, then return the frames after `last_event_id` from the ring.
/// The receiver is taken first so nothing emitted in between is lost; the
/// caller drops live frames with `id <= ` the last replayed id.
pub fn subscribe(sid: Uuid, last_event_id: Option<u64>) -> (Vec<Arc<Frame>>, broadcast::Receiver<Arc<Frame>>) {
    with_hub(sid, |hub| {
        let rx = hub.events.subscribe();
        let replay = match last_event_id {
            Some(after) => hub.ring.iter().filter(|f| f.id > after).cloned().collect(),
            None => Vec::new(),
        };
        (replay, rx)
    })
}

/// Send a control message to the session's pod (if one is listening).
pub fn push_control(sid: Uuid, message: Value) {
    with_hub(sid, |hub| {
        let _ = hub.control.send(message);
    });
}

pub fn subscribe_control(sid: Uuid) -> broadcast::Receiver<Value> {
    with_hub(sid, |hub| hub.control.subscribe())
}

/// Drop the hub some time after the session ends (open streams keep their
/// receivers until they close).
pub fn retire(sid: Uuid) {
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    tokio::spawn(async move {
        tokio::time::sleep(RETIRE_AFTER).await;
        hubs().lock().unwrap_or_else(|p| p.into_inner()).remove(&sid);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn replay_returns_only_frames_after_the_last_id_and_caps_at_500() {
        let sid = Uuid::new_v4();
        let first = emit(sid, "meeting.reply", json!({ "n": 0 }));
        for n in 1..(REPLAY_FRAMES + 20) {
            emit(sid, "meeting.reply", json!({ "n": n }));
        }
        let (all, _rx) = subscribe(sid, Some(0));
        assert_eq!(all.len(), REPLAY_FRAMES, "ring keeps the last 500");
        assert!(all.iter().all(|f| f.id > first), "oldest frames fell out");
        assert!(all.windows(2).all(|w| w[0].id < w[1].id), "ids increase");
        let after = all[REPLAY_FRAMES - 3].id;
        let (tail, _rx) = subscribe(sid, Some(after));
        assert_eq!(tail.len(), 2);
        let (none, _rx) = subscribe(sid, None);
        assert!(none.is_empty(), "no Last-Event-ID, no replay");
    }

    #[tokio::test]
    async fn subscribers_receive_live_events_and_control_messages() {
        let sid = Uuid::new_v4();
        let (_, mut rx) = subscribe(sid, None);
        let mut control = subscribe_control(sid);
        let id = emit(sid, "meeting.ended", json!({ "end_reason": "removed" }));
        push_control(sid, json!({ "type": "leave" }));
        let frame = rx.recv().await.unwrap();
        assert_eq!((frame.id, frame.event), (id, "meeting.ended"));
        assert_eq!(control.recv().await.unwrap(), json!({ "type": "leave" }));
    }
}
