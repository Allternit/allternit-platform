//! Request-level retry/failover engine for the LLM gateway.
//!
//! This module owns the decision logic for when a failed or refused completion
//! should be retried, how long to back off, and which model to try next. The
//! actual orchestration loop lives in `proxy::chat_completions`; this module is
//! kept pure so the policy and selection rules can be unit-tested without a
//! live Gizzi runtime.
//!
//! Retry policy is read from `fallback_retry_policies` (managed via
//! `/api/v1/admin/fallback-retry-policy`).
//!
//! Health-based failover (LiteLLM-aligned): a process-local
//! [`CooldownTracker`] records the outcome of every gateway attempt keyed by
//! `(provider_id, model)`. HTTP 429 responses put a provider/model into an
//! immediate cooldown (default 5s); a sustained failure streak (>50% failure
//! rate over a rolling 60s window with at least 4 attempts — a success clears
//! the record, so any residual streak is effectively 100% failures) triggers a
//! longer cooldown (default 30s). Fallback selection skips cooling-down
//! candidates and fail-opens to the soonest-expiring one when the whole
//! remaining chain is cooling down, so a request is never hard-errored just
//! because every provider is briefly hot.
//!
//! Shared state (P2.9): the tracker stays the L1, and when
//! `GATEWAY_SHARED_STATE=sqlite` is set `main` installs a
//! [`super::shared_state::SqliteCooldownStore`] on the process-wide tracker
//! (see [`CooldownTracker::install_shared_store`]) so cooldowns are shared
//! across replicas via the node's SQLite database. Writes go through to the
//! store; reads consult it when the local record has no active cooldown. All
//! store errors fail open (log + absorbed) — a request is never steered or
//! errored because the cooldown database hiccuped. Unset env = pure
//! in-memory behavior, unchanged from before.

use once_cell::sync::Lazy;
use rusqlite::OptionalExtension;
use std::collections::{HashMap, HashSet};
use std::sync::RwLock;
use std::time::{Duration, Instant};
use tracing::warn;

use crate::db::DbHandle;

/// Cooldown applied after an HTTP 429 (rate limited) attempt outcome.
const DEFAULT_COOLDOWN_RATE_LIMITED_MS: i64 = 5_000;
/// Cooldown applied when the failure-rate threshold trips.
const DEFAULT_COOLDOWN_FAILURE_MS: i64 = 30_000;
/// Rolling window for the failure-rate evaluation.
const FAILURE_RATE_WINDOW: Duration = Duration::from_secs(60);
/// Minimum attempts inside the window before the failure rate can trip a
/// cooldown.
const FAILURE_RATE_MIN_ATTEMPTS: usize = 4;

/// Retry policy loaded from `fallback_retry_policies`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RetryPolicy {
    pub enabled: bool,
    pub max_retries: i64,
    pub retryable_statuses: HashSet<String>,
    pub retryable_errors: HashSet<String>,
    pub base_delay_ms: i64,
    pub max_delay_ms: i64,
    pub fallback_chain_enabled: bool,
    /// Cooldown after an HTTP 429 attempt outcome (milliseconds, default 5s).
    /// `serde(default)` keeps rows serialized before this field existed
    /// deserializable.
    #[serde(default = "default_cooldown_rate_limited_ms")]
    pub cooldown_rate_limited_ms: i64,
    /// Cooldown after the failure-rate threshold trips (milliseconds, default
    /// 30s).
    #[serde(default = "default_cooldown_failure_ms")]
    pub cooldown_failure_ms: i64,
}

fn default_cooldown_rate_limited_ms() -> i64 {
    DEFAULT_COOLDOWN_RATE_LIMITED_MS
}

fn default_cooldown_failure_ms() -> i64 {
    DEFAULT_COOLDOWN_FAILURE_MS
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            max_retries: 2,
            retryable_statuses: ["refusal", "error", "rate_limited", "timeout"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            retryable_errors: ["*"].iter().map(|s| s.to_string()).collect(),
            base_delay_ms: 500,
            max_delay_ms: 8000,
            fallback_chain_enabled: true,
            cooldown_rate_limited_ms: DEFAULT_COOLDOWN_RATE_LIMITED_MS,
            cooldown_failure_ms: DEFAULT_COOLDOWN_FAILURE_MS,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("invalid stored JSON: {0}")]
    Json(String),
}

fn parse_string_set(value: &str) -> Result<HashSet<String>, LoadError> {
    let array: Vec<String> = serde_json::from_str(value)
        .map_err(|e| LoadError::Json(format!("failed to parse string array: {e}")))?;
    Ok(array.into_iter().collect())
}

/// Load the retry policy for an organization. Returns the default policy if no
/// row exists.
pub fn load_policy(db: &DbHandle, org_id: &str) -> Result<RetryPolicy, LoadError> {
    let conn = db.connect()?;
    let row: Option<(i64, i64, String, String, i64, i64, i64)> = conn
        .query_row(
            "SELECT enabled, max_retries, retryable_statuses, retryable_errors,
                    base_delay_ms, max_delay_ms, fallback_chain_enabled
             FROM fallback_retry_policies
             WHERE org_id = ?1",
            [org_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .optional()?;

    match row {
        Some((enabled, max_retries, statuses_json, errors_json, base_delay, max_delay, fallback)) => {
            Ok(RetryPolicy {
                enabled: enabled == 1,
                max_retries,
                retryable_statuses: parse_string_set(statuses_json.as_str())?,
                retryable_errors: parse_string_set(errors_json.as_str())?,
                base_delay_ms: base_delay,
                max_delay_ms: max_delay,
                fallback_chain_enabled: fallback == 1,
                ..RetryPolicy::default()
            })
        }
        None => Ok(RetryPolicy::default()),
    }
}

/// Decide whether a request outcome warrants another attempt.
pub fn should_retry(
    status: &str,
    error_type: Option<&str>,
    attempt: u32,
    policy: &RetryPolicy,
) -> bool {
    if !policy.enabled || attempt == 0 || attempt > policy.max_retries as u32 {
        return false;
    }
    // The proxy records a refused completion with status "refused" (and
    // error_type "refusal"), while retry policies name it "refusal". Accept
    // either spelling so refusals actually fail over.
    let status_matches = policy.retryable_statuses.contains(status)
        || (status == "refused" && policy.retryable_statuses.contains("refusal"))
        || (status == "refusal" && policy.retryable_statuses.contains("refused"));
    if !status_matches {
        return false;
    }
    if policy.retryable_errors.contains("*") {
        return true;
    }
    match error_type {
        Some(error) => policy.retryable_errors.iter().any(|e| error.contains(e)),
        None => true,
    }
}

/// Compute the backoff delay for a given attempt using exponential backoff with
/// full jitter. `attempt` is 1-based.
pub fn next_backoff_ms(attempt: u32, base_delay_ms: i64, max_delay_ms: i64) -> u64 {
    let base = base_delay_ms.max(1) as u64;
    let max = (max_delay_ms as u64).max(base);
    let exponential = base.saturating_mul(2u64.saturating_pow(attempt.saturating_sub(1)));
    let capped = exponential.min(max);
    // Full jitter: sleep a random duration in [0, capped].
    rand::random::<u64>() % (capped + 1)
}

/// A candidate model reference.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ModelRef {
    pub provider_id: String,
    pub model_id: String,
}

impl ModelRef {
    pub fn full_id(&self) -> String {
        format!("{}/{}", self.provider_id, self.model_id)
    }
}

/// Select the model to use for a retry. `attempt` is 1-based: attempt 1 uses
/// the primary model, attempt 2 uses the first fallback, and so on.
pub fn select_fallback(
    attempt: u32,
    primary: &ModelRef,
    fallbacks: &[ModelRef],
    policy: &RetryPolicy,
) -> Option<ModelRef> {
    if !policy.fallback_chain_enabled {
        return Some(primary.clone());
    }
    if attempt == 1 {
        return Some(primary.clone());
    }
    let idx = attempt.saturating_sub(2) as usize;
    fallbacks.get(idx).cloned()
}

// ─── Health-based failover (cooldown tracker) ───────────────────────────────

/// The classified outcome of one gateway attempt, fed to the
/// [`CooldownTracker`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptOutcome {
    /// The attempt completed successfully.
    Success,
    /// The upstream answered HTTP 429 (rate limited).
    RateLimited,
    /// Any other failure (transport error, 5xx, refusal, ...).
    Failure,
}

/// Classify a gateway request outcome into an [`AttemptOutcome`].
pub fn classify_outcome(status: &str, error_type: Option<&str>) -> AttemptOutcome {
    if status == "ok" {
        return AttemptOutcome::Success;
    }
    if status == "rate_limited"
        || error_type
            .map(|e| e.contains("rate_limit"))
            .unwrap_or(false)
    {
        return AttemptOutcome::RateLimited;
    }
    AttemptOutcome::Failure
}

/// Per-provider/model health record kept by the [`CooldownTracker`].
#[derive(Debug, Default)]
struct ProviderHealthRecord {
    /// Timestamps of failed attempts (429s and other failures) inside the
    /// rolling [`FAILURE_RATE_WINDOW`]. Success removes the whole record, so
    /// every timestamp here is a failure — a residual streak's failure rate
    /// is effectively 100%, which is what makes the `>50%` rate test reduce
    /// to the streak-length threshold below.
    fail_timestamps: Vec<Instant>,
    /// While set and in the future, this provider/model is skipped during
    /// fallback selection.
    cooldown_until: Option<Instant>,
}

/// In-memory (provider_id, model) → health record map, optionally backed by a
/// shared SQLite cooldown store ([`super::shared_state::SqliteCooldownStore`])
/// so multiple replicas steer off the same cooldowns. The in-memory map is the
/// L1: records this process set are answered locally; the store is consulted
/// only on a local miss, and its own L1 keeps that read hot. Store errors
/// fail open (see `shared_state`).
#[derive(Debug, Default)]
pub struct CooldownTracker {
    records: RwLock<HashMap<(String, String), ProviderHealthRecord>>,
    shared: RwLock<Option<super::shared_state::SqliteCooldownStore>>,
}

static COOLDOWNS: Lazy<CooldownTracker> = Lazy::new(CooldownTracker::new);

/// The process-wide cooldown tracker consulted by the gateway retry loop.
pub fn cooldowns() -> &'static CooldownTracker {
    &COOLDOWNS
}

impl CooldownTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Install a shared cooldown store (called on the process-wide tracker
    /// from `main` when `GATEWAY_SHARED_STATE=sqlite`). After this, recorded
    /// cooldowns are written through to SQLite and local misses read through,
    /// so replicas steer off the same state.
    pub fn install_shared_store(&self, store: super::shared_state::SqliteCooldownStore) {
        *self.shared.write().expect("cooldown tracker poisoned") = Some(store);
    }

    fn shared_store(&self) -> Option<super::shared_state::SqliteCooldownStore> {
        self.shared.read().expect("cooldown tracker poisoned").clone()
    }

    /// Record one attempt outcome using the wall clock.
    pub fn record_outcome(
        &self,
        provider_id: &str,
        model_id: &str,
        outcome: AttemptOutcome,
        policy: &RetryPolicy,
    ) {
        self.record_outcome_at(provider_id, model_id, outcome, policy, Instant::now());
    }

    /// Record one attempt outcome at an explicit timestamp. Success clears the
    /// record; a 429 starts the short cooldown immediately; other failures
    /// feed the rolling failure-rate window, which trips the long cooldown
    /// after [`FAILURE_RATE_MIN_ATTEMPTS`] failures inside
    /// [`FAILURE_RATE_WINDOW`].
    pub fn record_outcome_at(
        &self,
        provider_id: &str,
        model_id: &str,
        outcome: AttemptOutcome,
        policy: &RetryPolicy,
        now: Instant,
    ) {
        let key = (provider_id.to_string(), model_id.to_string());
        let shared = self.shared_store();
        let mut records = self.records.write().expect("cooldown tracker poisoned");

        if outcome == AttemptOutcome::Success {
            records.remove(&key);
            drop(records);
            if let Some(store) = shared {
                store.clear(provider_id, model_id);
            }
            return;
        }

        let record = records.entry(key).or_default();
        record.fail_timestamps.push(now);

        let (cooldown, reason) = match outcome {
            AttemptOutcome::RateLimited => {
                let ms = policy.cooldown_rate_limited_ms.max(0) as u64;
                (Some(Duration::from_millis(ms)), "rate_limited")
            }
            AttemptOutcome::Failure => {
                record
                    .fail_timestamps
                    .retain(|ts| now.duration_since(*ts) <= FAILURE_RATE_WINDOW);
                // Success clears the record, so every retained timestamp is a
                // failure: the streak's failure rate is effectively 100%
                // (>50%), and the rate test reduces to the attempt threshold.
                if record.fail_timestamps.len() >= FAILURE_RATE_MIN_ATTEMPTS {
                    let ms = policy.cooldown_failure_ms.max(0) as u64;
                    (Some(Duration::from_millis(ms)), "failure_streak")
                } else {
                    (None, "")
                }
            }
            AttemptOutcome::Success => unreachable!("handled above"),
        };

        if let Some(duration) = cooldown {
            record.cooldown_until = Some(now + duration);
            crate::metrics::record_llm_failover_cooldown(provider_id, model_id);
        }
        drop(records);

        // Write-through to the shared store so other replicas see the
        // cooldown. Uses the store's wall clock: DB timestamps are epoch ms.
        if let (Some(store), Some(duration)) = (shared, cooldown) {
            let until_ms = store.now_ms().saturating_add(duration.as_millis() as i64);
            store.record_cooldown(provider_id, model_id, until_ms, reason);
        }
    }

    fn cooldown_expiry_at(&self, provider_id: &str, model_id: &str, now: Instant) -> Option<Instant> {
        {
            let records = self.records.read().expect("cooldown tracker poisoned");
            let expiry = records
                .get(&(provider_id.to_string(), model_id.to_string()))
                .and_then(|record| record.cooldown_until);
            if let Some(expiry) = expiry {
                if expiry > now {
                    return Some(expiry);
                }
            }
        }

        // Local miss: read through to the shared store so cooldowns recorded
        // by other replicas are honored. Fail-open inside the store maps DB
        // errors to None (healthy).
        let store = self.shared_store()?;
        let expiry_ms = store.cooling_until(provider_id, model_id)?;
        let remaining_ms = expiry_ms.saturating_sub(store.now_ms()).max(0) as u64;
        Some(now + Duration::from_millis(remaining_ms))
    }

    /// True when the provider/model is currently inside a cooldown.
    pub fn is_cooling_down(&self, provider_id: &str, model_id: &str) -> bool {
        self.is_cooling_down_at(provider_id, model_id, Instant::now())
    }

    /// Clock-injected variant of [`Self::is_cooling_down`] for tests.
    pub fn is_cooling_down_at(&self, provider_id: &str, model_id: &str, now: Instant) -> bool {
        self.cooldown_expiry_at(provider_id, model_id, now).is_some()
    }

    /// Remaining cooldown, if any.
    pub fn cooldown_remaining(&self, provider_id: &str, model_id: &str) -> Option<Duration> {
        let now = Instant::now();
        let expiry = self.cooldown_expiry_at(provider_id, model_id, now)?;
        Some(expiry.duration_since(now))
    }

    #[cfg(test)]
    fn fail_count(&self, provider_id: &str, model_id: &str) -> usize {
        let records = self.records.read().expect("cooldown tracker poisoned");
        records
            .get(&(provider_id.to_string(), model_id.to_string()))
            .map(|r| r.fail_timestamps.len())
            .unwrap_or(0)
    }
}

/// Record one attempt outcome against the process-wide tracker.
pub fn record_attempt_outcome(
    provider_id: &str,
    model_id: &str,
    outcome: AttemptOutcome,
    policy: &RetryPolicy,
) {
    cooldowns().record_outcome(provider_id, model_id, outcome, policy);
}

/// Select the model for a retry, skipping providers/models that are currently
/// cooling down. Falls back to [`select_fallback`] semantics for the
/// chain-disabled and first-attempt cases; when every remaining candidate is
/// cooling down, fail-open: pick the one whose cooldown expires soonest and
/// mark the attempt (log + metric) rather than erroring the request.
pub fn select_fallback_healthy(
    attempt: u32,
    primary: &ModelRef,
    fallbacks: &[ModelRef],
    policy: &RetryPolicy,
) -> Option<ModelRef> {
    select_fallback_healthy_at(attempt, primary, fallbacks, policy, Instant::now(), cooldowns())
}

/// Clock- and tracker-injected variant of [`select_fallback_healthy`] so tests
/// never sleep.
pub fn select_fallback_healthy_at(
    attempt: u32,
    primary: &ModelRef,
    fallbacks: &[ModelRef],
    policy: &RetryPolicy,
    now: Instant,
    tracker: &CooldownTracker,
) -> Option<ModelRef> {
    if !policy.fallback_chain_enabled {
        return Some(primary.clone());
    }
    if attempt == 1 {
        return Some(primary.clone());
    }
    let idx = attempt.saturating_sub(2) as usize;
    let candidates = fallbacks.get(idx..).unwrap_or(&[]);
    if candidates.is_empty() {
        return None;
    }

    let mut soonest: Option<(&ModelRef, Instant)> = None;
    for candidate in candidates {
        match tracker.cooldown_expiry_at(&candidate.provider_id, &candidate.model_id, now) {
            None => {
                // Healthy candidate — first one wins, exactly like the static
                // chain order.
                return Some(candidate.clone());
            }
            Some(expiry) => {
                crate::metrics::record_llm_failover_skipped(
                    &candidate.provider_id,
                    &candidate.model_id,
                );
                if soonest.map_or(true, |(_, best)| expiry < best) {
                    soonest = Some((candidate, expiry));
                }
            }
        }
    }

    // Fail-open: the whole remaining chain is cooling down. Attempt the
    // soonest-expiring candidate instead of erroring the request.
    let (picked, expiry) = soonest.expect("candidates non-empty");
    warn!(
        provider = %picked.provider_id,
        model = %picked.model_id,
        cooldown_remaining_ms = ?expiry.saturating_duration_since(now).as_millis(),
        "Failover fail-open: all fallback candidates cooling down; attempting soonest-expiring"
    );
    crate::metrics::record_llm_failover_cooldown(&picked.provider_id, &picked.model_id);
    Some(picked.clone())
}

// ─── Streaming failover: retry-hint events (P0.2) ───────────────────────────
//
// Owning-layer decision: the streaming path gets exactly one upstream attempt
// (transparent gateway-owned resume is deferred), so the *policy* question —
// is this failure worth re-driving, and against which model — lives here in
// `failover.rs` next to `should_retry`/`select_fallback_healthy`, while
// `proxy.rs::stream_completion` owns the SSE wire and simply serializes the
// hint. Gizzi (which owns session state) re-drives with full context.

/// The `allternit.retry_hint` SSE event payload, emitted on the stream when an
/// upstream streaming attempt fails. `retryable: false` marks a terminal
/// failure (policy would not retry it); `next_fallback` is the health-aware
/// pick for the next attempt (`null` when the chain is exhausted).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct RetryHint {
    pub retryable: bool,
    pub reason: String,
    pub next_fallback: Option<ModelRef>,
}

/// Build the retry hint for a failed streaming attempt using the process-wide
/// cooldown tracker.
pub fn stream_retry_hint(
    status: &str,
    error_type: Option<&str>,
    primary: &ModelRef,
    fallbacks: &[ModelRef],
    policy: &RetryPolicy,
) -> RetryHint {
    stream_retry_hint_at(
        status,
        error_type,
        primary,
        fallbacks,
        policy,
        Instant::now(),
        cooldowns(),
    )
}

/// Clock- and tracker-injected variant of [`stream_retry_hint`] for tests.
pub fn stream_retry_hint_at(
    status: &str,
    error_type: Option<&str>,
    primary: &ModelRef,
    fallbacks: &[ModelRef],
    policy: &RetryPolicy,
    now: Instant,
    tracker: &CooldownTracker,
) -> RetryHint {
    // The failed attempt was attempt 1; a re-drive would be attempt 2.
    let retryable = should_retry(status, error_type, 2, policy);
    let next_fallback = if retryable {
        select_fallback_healthy_at(2, primary, fallbacks, policy, now, tracker)
    } else {
        None
    };
    RetryHint {
        retryable,
        reason: error_type.unwrap_or(status).to_string(),
        next_fallback,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(provider: &str, id: &str) -> ModelRef {
        ModelRef {
            provider_id: provider.to_string(),
            model_id: id.to_string(),
        }
    }

    fn policy_with_statuses(statuses: &[&str]) -> RetryPolicy {
        RetryPolicy {
            retryable_statuses: statuses.iter().map(|s| s.to_string()).collect(),
            ..RetryPolicy::default()
        }
    }

    #[test]
    fn should_retry_respects_attempt_limit() {
        let policy = RetryPolicy {
            max_retries: 2,
            ..RetryPolicy::default()
        };
        assert!(should_retry("error", None, 1, &policy));
        assert!(should_retry("error", None, 2, &policy));
        assert!(!should_retry("error", None, 3, &policy));
    }

    #[test]
    fn should_retry_respects_status_set() {
        let policy = policy_with_statuses(&["rate_limited"]);
        assert!(should_retry("rate_limited", None, 1, &policy));
        assert!(!should_retry("error", None, 1, &policy));
    }

    #[test]
    fn should_retry_respects_error_type_filter() {
        let policy = RetryPolicy {
            retryable_statuses: ["error"].iter().map(|s| s.to_string()).collect(),
            retryable_errors: ["timeout", "connection_reset"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            ..RetryPolicy::default()
        };
        assert!(should_retry("error", Some("upstream timeout"), 1, &policy));
        assert!(should_retry("error", Some("connection_reset by peer"), 1, &policy));
        assert!(!should_retry("error", Some("bad_request"), 1, &policy));
    }

    #[test]
    fn should_retry_disabled_policy_is_noop() {
        let policy = RetryPolicy {
            enabled: false,
            ..RetryPolicy::default()
        };
        assert!(!should_retry("error", None, 1, &policy));
    }

    #[test]
    fn backoff_is_within_bounds() {
        for attempt in 1..=5 {
            let delay = next_backoff_ms(attempt, 100, 1000);
            assert!(delay <= 1000, "attempt {attempt} delay {delay} exceeds max");
        }
    }

    #[test]
    fn backoff_grows_with_attempt() {
        let delays: Vec<u64> = (1..=4)
            .map(|a| next_backoff_ms(a, 100, 100_000))
            .collect();
        // The cap is large, so median delay should increase. We compare means
        // to avoid jitter flakiness; with full jitter this is probabilistic,
        // so we just verify the max possible delay grows.
        let max_first = 100u64;
        let max_fourth = 800u64;
        assert!(delays[0] <= max_first);
        assert!(delays[3] <= max_fourth);
    }

    #[test]
    fn select_fallback_uses_primary_then_chain() {
        let primary = model("openai", "gpt-4o");
        let fallbacks = vec![model("anthropic", "claude-3-5-sonnet"), model("kimi", "k3")];
        let policy = RetryPolicy::default();
        assert_eq!(select_fallback(1, &primary, &fallbacks, &policy), Some(primary.clone()));
        assert_eq!(select_fallback(2, &primary, &fallbacks, &policy), Some(fallbacks[0].clone()));
        assert_eq!(select_fallback(3, &primary, &fallbacks, &policy), Some(fallbacks[1].clone()));
        assert_eq!(select_fallback(4, &primary, &fallbacks, &policy), None);
    }

    #[test]
    fn select_fallback_disabled_chain_reuses_primary() {
        let primary = model("openai", "gpt-4o");
        let policy = RetryPolicy {
            fallback_chain_enabled: false,
            ..RetryPolicy::default()
        };
        assert_eq!(select_fallback(3, &primary, &[], &policy), Some(primary));
    }

    // ── CooldownTracker ─────────────────────────────────────────────────────

    #[test]
    fn rate_limited_attempt_enters_short_cooldown() {
        let tracker = CooldownTracker::new();
        let policy = RetryPolicy::default();
        let t0 = Instant::now();

        tracker.record_outcome_at("openai", "gpt-4o", AttemptOutcome::RateLimited, &policy, t0);
        assert!(tracker.is_cooling_down_at("openai", "gpt-4o", t0));

        // Default 429 cooldown is 5s: still cooling at +4s, healthy at +6s.
        assert!(tracker.is_cooling_down_at("openai", "gpt-4o", t0 + Duration::from_secs(4)));
        assert!(!tracker.is_cooling_down_at("openai", "gpt-4o", t0 + Duration::from_secs(6)));

        // Cooldown duration is policy-configurable.
        let custom = RetryPolicy {
            cooldown_rate_limited_ms: 500,
            ..RetryPolicy::default()
        };
        let tracker = CooldownTracker::new();
        tracker.record_outcome_at("openai", "gpt-4o", AttemptOutcome::RateLimited, &custom, t0);
        assert!(tracker.is_cooling_down_at("openai", "gpt-4o", t0 + Duration::from_millis(400)));
        assert!(!tracker.is_cooling_down_at("openai", "gpt-4o", t0 + Duration::from_millis(600)));
    }

    #[test]
    fn failure_rate_trips_long_cooldown_after_four_failures() {
        let tracker = CooldownTracker::new();
        let policy = RetryPolicy::default();
        let t0 = Instant::now();

        // Three failures: not yet cooling.
        for i in 0..3 {
            tracker.record_outcome_at(
                "anthropic",
                "claude",
                AttemptOutcome::Failure,
                &policy,
                t0 + Duration::from_secs(i),
            );
        }
        assert!(!tracker.is_cooling_down_at("anthropic", "claude", t0 + Duration::from_secs(3)));

        // Fourth failure inside the window trips the 30s cooldown.
        tracker.record_outcome_at(
            "anthropic",
            "claude",
            AttemptOutcome::Failure,
            &policy,
            t0 + Duration::from_secs(3),
        );
        assert!(tracker.is_cooling_down_at("anthropic", "claude", t0 + Duration::from_secs(3)));
        // Cooldown runs 30s from the fourth failure (t0+33s).
        assert!(tracker.is_cooling_down_at("anthropic", "claude", t0 + Duration::from_secs(32)));
        assert!(!tracker.is_cooling_down_at("anthropic", "claude", t0 + Duration::from_secs(34)));
    }

    #[test]
    fn failures_outside_window_do_not_accumulate() {
        let tracker = CooldownTracker::new();
        let policy = RetryPolicy::default();
        let t0 = Instant::now();

        // Four failures spread 61s apart: only the last one is inside the
        // rolling 60s window, so the rate threshold never trips.
        for i in 0..4 {
            tracker.record_outcome_at(
                "anthropic",
                "claude",
                AttemptOutcome::Failure,
                &policy,
                t0 + Duration::from_secs(i * 61),
            );
        }
        assert_eq!(tracker.fail_count("anthropic", "claude"), 1);
        assert!(!tracker.is_cooling_down_at("anthropic", "claude", t0 + Duration::from_secs(183)));
    }

    #[test]
    fn success_clears_the_record() {
        let tracker = CooldownTracker::new();
        let policy = RetryPolicy::default();
        let t0 = Instant::now();

        for i in 0..3 {
            tracker.record_outcome_at(
                "kimi",
                "k3",
                AttemptOutcome::Failure,
                &policy,
                t0 + Duration::from_secs(i),
            );
        }
        assert_eq!(tracker.fail_count("kimi", "k3"), 3);

        tracker.record_outcome_at("kimi", "k3", AttemptOutcome::Success, &policy, t0 + Duration::from_secs(4));
        assert_eq!(tracker.fail_count("kimi", "k3"), 0);
        assert!(!tracker.is_cooling_down_at("kimi", "k3", t0 + Duration::from_secs(5)));

        // The streak starts over from scratch.
        for i in 5..8 {
            tracker.record_outcome_at(
                "kimi",
                "k3",
                AttemptOutcome::Failure,
                &policy,
                t0 + Duration::from_secs(i),
            );
        }
        assert!(!tracker.is_cooling_down_at("kimi", "k3", t0 + Duration::from_secs(7)));
        tracker.record_outcome_at(
            "kimi",
            "k3",
            AttemptOutcome::Failure,
            &policy,
            t0 + Duration::from_secs(7),
        );
        assert!(tracker.is_cooling_down_at("kimi", "k3", t0 + Duration::from_secs(7)));
    }

    #[test]
    fn success_clears_an_active_cooldown() {
        let tracker = CooldownTracker::new();
        let policy = RetryPolicy::default();
        let t0 = Instant::now();

        tracker.record_outcome_at("openai", "gpt-4o", AttemptOutcome::RateLimited, &policy, t0);
        assert!(tracker.is_cooling_down_at("openai", "gpt-4o", t0 + Duration::from_secs(1)));

        tracker.record_outcome_at(
            "openai",
            "gpt-4o",
            AttemptOutcome::Success,
            &policy,
            t0 + Duration::from_secs(2),
        );
        assert!(!tracker.is_cooling_down_at("openai", "gpt-4o", t0 + Duration::from_secs(3)));
    }

    #[test]
    fn healthy_selection_skips_cooling_candidates() {
        let tracker = CooldownTracker::new();
        let policy = RetryPolicy::default();
        let t0 = Instant::now();
        let primary = model("openai", "gpt-4o");
        let fallbacks = vec![model("anthropic", "claude"), model("kimi", "k3")];

        tracker.record_outcome_at("anthropic", "claude", AttemptOutcome::RateLimited, &policy, t0);
        let picked = select_fallback_healthy_at(2, &primary, &fallbacks, &policy, t0, &tracker);
        assert_eq!(picked, Some(model("kimi", "k3")));
    }

    #[test]
    fn healthy_selection_fail_opens_when_all_cooling() {
        let tracker = CooldownTracker::new();
        let policy = RetryPolicy::default();
        let t0 = Instant::now();
        let primary = model("openai", "gpt-4o");
        let fallbacks = vec![model("anthropic", "claude"), model("kimi", "k3")];

        // anthropic cools for the 30s failure cooldown, kimi for the 5s 429
        // cooldown — both relative to t0.
        for i in 0..4 {
            tracker.record_outcome_at(
                "anthropic",
                "claude",
                AttemptOutcome::Failure,
                &policy,
                t0 + Duration::from_secs(i),
            );
        }
        tracker.record_outcome_at("kimi", "k3", AttemptOutcome::RateLimited, &policy, t0);

        let picked = select_fallback_healthy_at(2, &primary, &fallbacks, &policy, t0, &tracker);
        assert_eq!(
            picked,
            Some(model("kimi", "k3")),
            "fail-open must pick the soonest-expiring cooldown (kimi 5s < anthropic 30s)"
        );
    }

    #[test]
    fn healthy_selection_respects_static_chain_semantics() {
        let tracker = CooldownTracker::new();
        let policy = RetryPolicy::default();
        let t0 = Instant::now();
        let primary = model("openai", "gpt-4o");
        let fallbacks = vec![model("anthropic", "claude"), model("kimi", "k3")];

        // Nobody cooling: identical to select_fallback.
        assert_eq!(
            select_fallback_healthy_at(1, &primary, &fallbacks, &policy, t0, &tracker),
            Some(primary.clone())
        );
        assert_eq!(
            select_fallback_healthy_at(2, &primary, &fallbacks, &policy, t0, &tracker),
            Some(fallbacks[0].clone())
        );
        assert_eq!(
            select_fallback_healthy_at(3, &primary, &fallbacks, &policy, t0, &tracker),
            Some(fallbacks[1].clone())
        );
        assert_eq!(
            select_fallback_healthy_at(4, &primary, &fallbacks, &policy, t0, &tracker),
            None
        );

        // Chain disabled reuses the primary.
        let disabled = RetryPolicy {
            fallback_chain_enabled: false,
            ..RetryPolicy::default()
        };
        assert_eq!(
            select_fallback_healthy_at(3, &primary, &fallbacks, &disabled, t0, &tracker),
            Some(primary)
        );
    }

    #[test]
    fn healthy_selection_ignores_expired_cooldowns() {
        let tracker = CooldownTracker::new();
        let policy = RetryPolicy::default();
        let t0 = Instant::now();
        let primary = model("openai", "gpt-4o");
        let fallbacks = vec![model("anthropic", "claude"), model("kimi", "k3")];

        tracker.record_outcome_at("anthropic", "claude", AttemptOutcome::RateLimited, &policy, t0);
        // After the 5s 429 cooldown expires, the chain is back to static order.
        let later = t0 + Duration::from_secs(6);
        let picked = select_fallback_healthy_at(2, &primary, &fallbacks, &policy, later, &tracker);
        assert_eq!(picked, Some(model("anthropic", "claude")));
    }

    #[test]
    fn refused_status_retries_under_refusal_policy() {
        let policy = RetryPolicy::default();
        assert!(should_retry("refused", Some("refusal"), 1, &policy));
        assert!(!should_retry("ok", None, 1, &policy));
    }

    #[test]
    fn classify_outcome_maps_statuses() {
        assert_eq!(classify_outcome("ok", None), AttemptOutcome::Success);
        assert_eq!(classify_outcome("ok", Some("refusal")), AttemptOutcome::Success);
        assert_eq!(
            classify_outcome("rate_limited", None),
            AttemptOutcome::RateLimited
        );
        assert_eq!(
            classify_outcome("error", Some("upstream rate_limited")),
            AttemptOutcome::RateLimited
        );
        assert_eq!(
            classify_outcome("error", Some("connection reset")),
            AttemptOutcome::Failure
        );
        assert_eq!(classify_outcome("refused", None), AttemptOutcome::Failure);
    }

    // ── RetryPolicy serde backwards compatibility ───────────────────────────

    #[test]
    fn policy_json_without_cooldown_fields_deserializes_with_defaults() {
        // A row serialized before the cooldown fields existed.
        let old = serde_json::json!({
            "enabled": true,
            "max_retries": 3,
            "retryable_statuses": ["error"],
            "retryable_errors": ["*"],
            "base_delay_ms": 250,
            "max_delay_ms": 4000,
            "fallback_chain_enabled": true
        });
        let policy: RetryPolicy = serde_json::from_value(old).expect("old row must deserialize");
        assert_eq!(policy.cooldown_rate_limited_ms, DEFAULT_COOLDOWN_RATE_LIMITED_MS);
        assert_eq!(policy.cooldown_failure_ms, DEFAULT_COOLDOWN_FAILURE_MS);
        assert_eq!(policy.max_retries, 3);
    }

    #[test]
    fn policy_json_with_cooldown_fields_round_trips() {
        let policy = RetryPolicy {
            cooldown_rate_limited_ms: 1_000,
            cooldown_failure_ms: 60_000,
            ..RetryPolicy::default()
        };
        let json = serde_json::to_value(&policy).unwrap();
        let decoded: RetryPolicy = serde_json::from_value(json).unwrap();
        assert_eq!(decoded.cooldown_rate_limited_ms, 1_000);
        assert_eq!(decoded.cooldown_failure_ms, 60_000);
        assert_eq!(policy, decoded);
    }

    #[test]
    fn default_policy_carries_default_cooldowns() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.cooldown_rate_limited_ms, 5_000);
        assert_eq!(policy.cooldown_failure_ms, 30_000);
    }

    // ── Retry hints (P0.2 streaming failover) ───────────────────────────────

    #[test]
    fn retry_hint_points_at_first_healthy_fallback() {
        let tracker = CooldownTracker::new();
        let policy = RetryPolicy::default();
        let t0 = Instant::now();
        let primary = model("openai", "gpt-4o");
        let fallbacks = vec![model("anthropic", "claude"), model("kimi", "k3")];

        let hint =
            stream_retry_hint_at("error", Some("upstream_error"), &primary, &fallbacks, &policy, t0, &tracker);
        assert!(hint.retryable);
        assert_eq!(hint.reason, "upstream_error");
        assert_eq!(hint.next_fallback, Some(model("anthropic", "claude")));
    }

    #[test]
    fn retry_hint_skips_cooling_fallback() {
        let tracker = CooldownTracker::new();
        let policy = RetryPolicy::default();
        let t0 = Instant::now();
        let primary = model("openai", "gpt-4o");
        let fallbacks = vec![model("anthropic", "claude"), model("kimi", "k3")];

        tracker.record_outcome_at("anthropic", "claude", AttemptOutcome::RateLimited, &policy, t0);
        let hint =
            stream_retry_hint_at("error", Some("upstream_error"), &primary, &fallbacks, &policy, t0, &tracker);
        assert!(hint.retryable);
        assert_eq!(hint.next_fallback, Some(model("kimi", "k3")));
    }

    #[test]
    fn retry_hint_marks_terminal_failure_not_retryable() {
        let tracker = CooldownTracker::new();
        let t0 = Instant::now();
        let primary = model("openai", "gpt-4o");
        let fallbacks = vec![model("anthropic", "claude")];

        // Policy disabled: nothing is retryable.
        let disabled = RetryPolicy {
            enabled: false,
            ..RetryPolicy::default()
        };
        let hint =
            stream_retry_hint_at("error", Some("upstream_error"), &primary, &fallbacks, &disabled, t0, &tracker);
        assert!(!hint.retryable);
        assert_eq!(hint.next_fallback, None);

        // Status outside the retryable set: terminal.
        let policy = RetryPolicy::default();
        let hint =
            stream_retry_hint_at("client_disconnected", None, &primary, &fallbacks, &policy, t0, &tracker);
        assert!(!hint.retryable);
        assert_eq!(hint.reason, "client_disconnected");
        assert_eq!(hint.next_fallback, None);
    }

    #[test]
    fn retry_hint_null_fallback_when_chain_exhausted() {
        let tracker = CooldownTracker::new();
        let policy = RetryPolicy::default();
        let t0 = Instant::now();
        let primary = model("openai", "gpt-4o");

        // Retryable error but no fallbacks configured.
        let hint = stream_retry_hint_at("error", Some("upstream_error"), &primary, &[], &policy, t0, &tracker);
        assert!(hint.retryable);
        assert_eq!(hint.next_fallback, None);
    }

    #[test]
    fn retry_hint_serializes_wire_shape() {
        let hint = RetryHint {
            retryable: true,
            reason: "rate_limit_error".to_string(),
            next_fallback: Some(model("anthropic", "claude")),
        };
        let json = serde_json::to_value(&hint).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "retryable": true,
                "reason": "rate_limit_error",
                "next_fallback": {"provider_id": "anthropic", "model_id": "claude"}
            })
        );
    }

    // ── Shared cooldown store integration (P2.9) ────────────────────────────

    use super::super::shared_state::SqliteCooldownStore;
    use crate::db::DbHandle;
    use std::sync::Arc;

    fn shared_store(db: &DbHandle, epoch_ms: i64) -> SqliteCooldownStore {
        SqliteCooldownStore::with_clock(db.clone(), Arc::new(move || epoch_ms))
    }

    #[test]
    fn cooldown_written_by_one_tracker_visible_to_another() {
        // Two trackers sharing one database simulate two replicas.
        let db = DbHandle::new_memory().unwrap();
        let policy = RetryPolicy::default();
        let t0 = Instant::now();

        let replica_a = CooldownTracker::new();
        replica_a.install_shared_store(shared_store(&db, 100_000));
        let replica_b = CooldownTracker::new();
        replica_b.install_shared_store(shared_store(&db, 101_000));

        replica_a.record_outcome_at("openai", "gpt-4o", AttemptOutcome::RateLimited, &policy, t0);
        assert!(
            replica_b.is_cooling_down_at("openai", "gpt-4o", t0),
            "replica B must see the cooldown replica A recorded"
        );

        // Fallback selection on B skips the provider A cooled down.
        let primary = model("openai", "gpt-4o");
        let fallbacks = vec![model("openai", "gpt-4o"), model("kimi", "k3")];
        let picked = select_fallback_healthy_at(2, &primary, &fallbacks, &policy, t0, &replica_b);
        assert_eq!(picked, Some(model("kimi", "k3")));
    }

    #[test]
    fn success_on_one_replica_clears_cooldown_for_another() {
        let db = DbHandle::new_memory().unwrap();
        let policy = RetryPolicy::default();
        let t0 = Instant::now();

        let replica_a = CooldownTracker::new();
        replica_a.install_shared_store(shared_store(&db, 200_000));
        let replica_b = CooldownTracker::new();
        replica_b.install_shared_store(shared_store(&db, 200_000));

        replica_a.record_outcome_at("openai", "gpt-4o", AttemptOutcome::RateLimited, &policy, t0);
        assert!(replica_b.is_cooling_down_at("openai", "gpt-4o", t0));

        replica_a.record_outcome_at("openai", "gpt-4o", AttemptOutcome::Success, &policy, t0);
        let replica_c = CooldownTracker::new();
        replica_c.install_shared_store(shared_store(&db, 200_000));
        assert!(!replica_c.is_cooling_down_at("openai", "gpt-4o", t0));
    }

    #[test]
    fn tracker_fail_opens_when_shared_store_db_is_broken() {
        let dir = tempfile::tempdir().unwrap();
        let db = DbHandle::new(dir.path().join("state.db")).unwrap();
        let store = shared_store(&db, 0);
        drop(dir); // every subsequent connect() fails

        let tracker = CooldownTracker::new();
        tracker.install_shared_store(store);
        let policy = RetryPolicy::default();
        let t0 = Instant::now();

        // Reads fail open to "healthy"; writes are absorbed without panic.
        assert!(!tracker.is_cooling_down_at("openai", "gpt-4o", t0));
        tracker.record_outcome_at("openai", "gpt-4o", AttemptOutcome::RateLimited, &policy, t0);
        // The local L1 still recorded the cooldown — the process keeps its
        // own steering even while the shared store is down.
        assert!(tracker.is_cooling_down_at("openai", "gpt-4o", t0));
        assert!(crate::llm_gateway::shared_state::db_error_count() >= 1);
    }

    #[test]
    fn tracker_without_shared_store_is_unchanged() {
        // Default-off behavior: no store installed, pure in-memory steering.
        let tracker = CooldownTracker::new();
        assert!(tracker.shared_store().is_none());
        let policy = RetryPolicy::default();
        let t0 = Instant::now();
        tracker.record_outcome_at("openai", "gpt-4o", AttemptOutcome::RateLimited, &policy, t0);
        assert!(tracker.is_cooling_down_at("openai", "gpt-4o", t0 + Duration::from_secs(4)));
        assert!(!tracker.is_cooling_down_at("openai", "gpt-4o", t0 + Duration::from_secs(6)));
    }
}
