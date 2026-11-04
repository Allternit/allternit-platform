//! Tiny in-process circuit breaker per MCP connector id: 3 consecutive failures open it for 60 s
//! (calls then fail fast with `connector_unavailable`); one success resets it.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const OPEN_AFTER: u32 = 3;
const OPEN_FOR: Duration = Duration::from_secs(60);

#[derive(Default, Debug)]
pub struct Breaker {
    failures: u32,
    open_until: Option<Instant>,
}

impl Breaker {
    /// True while the breaker is open at `now`.
    pub fn is_open(&self, now: Instant) -> bool {
        self.open_until.is_some_and(|t| now < t)
    }
    pub fn record_failure(&mut self, now: Instant) {
        self.failures += 1;
        if self.failures >= OPEN_AFTER {
            self.open_until = Some(now + OPEN_FOR);
            self.failures = 0;
        }
    }
    pub fn record_success(&mut self) {
        self.failures = 0;
        self.open_until = None;
    }
}

fn registry() -> &'static Mutex<HashMap<String, Breaker>> {
    static R: OnceLock<Mutex<HashMap<String, Breaker>>> = OnceLock::new();
    R.get_or_init(Default::default)
}

pub fn is_open(connector_id: &str) -> bool {
    let Ok(map) = registry().lock() else { return false };
    map.get(connector_id).is_some_and(|b| b.is_open(Instant::now()))
}

pub fn record(connector_id: &str, ok: bool) {
    let Ok(mut map) = registry().lock() else { return };
    let b = map.entry(connector_id.to_string()).or_default();
    if ok {
        b.record_success();
    } else {
        b.record_failure(Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_after_three_failures_then_expires() {
        let mut b = Breaker::default();
        let t0 = Instant::now();
        b.record_failure(t0);
        b.record_failure(t0);
        assert!(!b.is_open(t0));
        b.record_failure(t0);
        assert!(b.is_open(t0));
        assert!(!b.is_open(t0 + Duration::from_secs(61)));
    }

    #[test]
    fn success_resets_the_streak() {
        let mut b = Breaker::default();
        let t0 = Instant::now();
        b.record_failure(t0);
        b.record_failure(t0);
        b.record_success();
        b.record_failure(t0);
        assert!(!b.is_open(t0));
    }
}
