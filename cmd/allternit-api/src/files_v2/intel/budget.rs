//! The CPU budget for background indexing (SPEC decision D12).
//!
//! A 2 CPU / 4 GB cloud computer also runs the user's agents, so the indexer
//! is held to a duty cycle on one low-priority thread:
//!
//! - **Default cap: 25% of one core** (`ALLTERNIT_FILES_INTEL_CPU_PERCENT`,
//!   1 to 100). After every file it rests for `busy * (100 - cap) / cap`, so
//!   at 25% a file that took 100 ms is followed by 300 ms of rest.
//! - **Batch: 8 files per pass** (`ALLTERNIT_FILES_INTEL_BATCH`), at most
//!   2 MiB of text read per file.
//! - **Back-off:** when a pass finds nothing to do, or fails, the wait before
//!   the next pass doubles from 30 s up to 5 min, and resets as soon as a pass
//!   finds work.

use std::time::Duration;

pub const DEFAULT_CPU_PERCENT: u32 = 25;
pub const DEFAULT_BATCH: usize = 8;
pub const MAX_TEXT_BYTES: u64 = 2 * 1024 * 1024;
pub const IDLE_PASS: Duration = Duration::from_secs(30);
pub const MAX_BACKOFF: Duration = Duration::from_secs(300);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    /// Share of one core the worker may use, 1 to 100.
    pub cpu_percent: u32,
    /// Files indexed per pass.
    pub batch_files: usize,
}

impl Default for Budget {
    fn default() -> Self {
        Self { cpu_percent: DEFAULT_CPU_PERCENT, batch_files: DEFAULT_BATCH }
    }
}

impl Budget {
    pub fn from_env() -> Self {
        let num = |name: &str| std::env::var(name).ok().and_then(|v| v.trim().parse::<u64>().ok());
        Self {
            cpu_percent: num("ALLTERNIT_FILES_INTEL_CPU_PERCENT").map_or(DEFAULT_CPU_PERCENT, |n| n.clamp(1, 100) as u32),
            batch_files: num("ALLTERNIT_FILES_INTEL_BATCH").map_or(DEFAULT_BATCH, |n| n.clamp(1, 200) as usize),
        }
    }

    /// How long to rest after `busy` of work so the duty cycle stays at or
    /// under `cpu_percent`.
    pub fn rest_after(&self, busy: Duration) -> Duration {
        let cap = self.cpu_percent.clamp(1, 100) as u128;
        let rest_nanos = busy.as_nanos() * (100 - cap) / cap;
        Duration::from_nanos(rest_nanos.min(u64::MAX as u128) as u64)
    }
}

/// Doubling wait between passes that found nothing.
#[derive(Debug)]
pub struct Backoff {
    current: Duration,
}

impl Backoff {
    pub fn new() -> Self {
        Self { current: IDLE_PASS }
    }

    /// The wait for an empty or failed pass; the next one is twice as long.
    pub fn next_wait(&mut self) -> Duration {
        let wait = self.current;
        self.current = (self.current * 2).min(MAX_BACKOFF);
        wait
    }

    pub fn reset(&mut self) {
        self.current = IDLE_PASS;
    }
}
