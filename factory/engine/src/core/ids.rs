use chrono::Utc;
use rand::RngCore;
use std::sync::atomic::{AtomicU64, Ordering};

static LAST_TS_MS: AtomicU64 = AtomicU64::new(0);
static SEQ: AtomicU64 = AtomicU64::new(0);

pub fn create_event_id() -> String {
    let now_ms = Utc::now().timestamp_millis().max(0) as u64;
    loop {
        let last = LAST_TS_MS.load(Ordering::Relaxed);
        if now_ms == last {
            let seq = SEQ.fetch_add(1, Ordering::Relaxed);
            return format!("evt_{:013}_{:06}", now_ms, seq);
        }
        if LAST_TS_MS
            .compare_exchange(last, now_ms, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            SEQ.store(0, Ordering::Relaxed);
            return format!("evt_{:013}_{:06}", now_ms, 0);
        }
    }
}

pub fn create_receipt_id() -> String {
    let mut buf = [0u8; 4];
    rand::thread_rng().fill_bytes(&mut buf);
    format!("rcpt_{}", hex::encode(buf))
}

pub fn create_lease_id() -> String {
    let mut buf = [0u8; 4];
    rand::thread_rng().fill_bytes(&mut buf);
    format!("lease_{}", hex::encode(buf))
}

pub fn create_blob_id() -> String {
    let mut buf = [0u8; 6];
    rand::thread_rng().fill_bytes(&mut buf);
    format!("blob_{}", hex::encode(buf))
}

pub fn create_peer_id() -> String {
    let mut buf = [0u8; 6];
    rand::thread_rng().fill_bytes(&mut buf);
    format!("peer_{}", hex::encode(buf))
}
