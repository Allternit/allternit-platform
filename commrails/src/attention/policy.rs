//! Attention policy: the pure decision behind every agent→human notification.
//!
//! `decide` is a pure function of (policy, candidate, history, now). It never
//! drops anything: an item is delivered now, deferred to a computed release
//! time (quiet hours, hourly cap), or coalesced into an identical item that is
//! already delivered inside the dedupe window or still queued.

use chrono::{DateTime, Duration, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

/// Local quiet-hours window. `start > end` spans midnight (22:30–06:00);
/// `start == end` means no quiet hours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuietHours {
    pub start: NaiveTime,
    pub end: NaiveTime,
}

impl QuietHours {
    pub fn parse(start: &str, end: &str) -> anyhow::Result<Self> {
        let p = |s: &str| {
            NaiveTime::parse_from_str(s, "%H:%M")
                .map_err(|e| anyhow::anyhow!("quiet-hours time {s:?} must be HH:MM ({e})"))
        };
        Ok(Self {
            start: p(start)?,
            end: p(end)?,
        })
    }

    fn contains(&self, t: NaiveTime) -> bool {
        if self.start == self.end {
            false
        } else if self.start < self.end {
            t >= self.start && t < self.end
        } else {
            t >= self.start || t < self.end
        }
    }
}

#[derive(Debug, Clone)]
pub struct AttentionPolicy {
    pub timezone: Tz,
    pub quiet_hours: Option<QuietHours>,
    /// Same key + same content hash inside this window is coalesced.
    pub dedupe_window: Duration,
    /// Max deliveries in any rolling 60 minutes. 0 = no cap.
    pub per_hour_cap: u32,
}

impl Default for AttentionPolicy {
    fn default() -> Self {
        Self {
            timezone: chrono_tz::UTC,
            quiet_hours: None,
            dedupe_window: Duration::hours(24),
            per_hour_cap: 6,
        }
    }
}

/// What the gate knows about an item it has already seen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub item_id: String,
    pub key: String,
    pub content_hash: String,
    pub state: HistoryState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryState {
    /// Delivered at this instant.
    Delivered(DateTime<Utc>),
    /// Queued, not yet delivered.
    Queued,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeferReason {
    QuietHours,
    HourlyCap,
}

impl std::fmt::Display for DeferReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeferReason::QuietHours => write!(f, "quiet_hours"),
            DeferReason::HourlyCap => write!(f, "hourly_cap"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Deliver,
    Defer {
        until: DateTime<Utc>,
        reason: DeferReason,
    },
    Coalesce {
        into: String,
    },
}

/// The item being decided on.
#[derive(Debug, Clone)]
pub struct Candidate<'a> {
    /// The item's own id when re-deciding a queued item (excluded from
    /// dedupe/history so it does not coalesce into itself).
    pub item_id: Option<&'a str>,
    pub key: &'a str,
    pub content_hash: &'a str,
    /// True when releasing an already-queued item: dedupe was decided at
    /// submit time and is not re-applied.
    pub releasing: bool,
}

/// Decide what to do with `candidate` at `now`. Order: dedupe, quiet hours,
/// hourly cap. A cap deferral that lands inside quiet hours is pushed to the
/// end of that quiet window.
pub fn decide(
    policy: &AttentionPolicy,
    candidate: &Candidate<'_>,
    history: &[HistoryEntry],
    now: DateTime<Utc>,
) -> Decision {
    let others = history
        .iter()
        .filter(|h| candidate.item_id != Some(h.item_id.as_str()));

    if !candidate.releasing {
        for h in others.clone() {
            if h.key != candidate.key || h.content_hash != candidate.content_hash {
                continue;
            }
            let live = match h.state {
                HistoryState::Queued => true,
                HistoryState::Delivered(at) => now - at < policy.dedupe_window,
            };
            if live {
                return Decision::Coalesce {
                    into: h.item_id.clone(),
                };
            }
        }
    }

    if let Some(until) = quiet_end_if_quiet(policy, now) {
        return Decision::Defer {
            until,
            reason: DeferReason::QuietHours,
        };
    }

    if policy.per_hour_cap > 0 {
        let window_start = now - Duration::hours(1);
        let mut recent: Vec<DateTime<Utc>> = others
            .filter_map(|h| match h.state {
                HistoryState::Delivered(at) if at > window_start && at <= now => Some(at),
                _ => None,
            })
            .collect();
        if recent.len() as u32 >= policy.per_hour_cap {
            recent.sort();
            // The slot frees when the oldest delivery counted against the cap
            // leaves the window: after (len - cap + 1) deliveries age out.
            let idx = recent.len() - policy.per_hour_cap as usize;
            let mut until = recent[idx] + Duration::hours(1);
            if let Some(q) = quiet_end_if_quiet(policy, until) {
                until = q;
            }
            return Decision::Defer {
                until,
                reason: DeferReason::HourlyCap,
            };
        }
    }

    Decision::Deliver
}

/// True when `now` falls inside the policy's quiet hours (local time).
pub fn is_quiet(policy: &AttentionPolicy, now: DateTime<Utc>) -> bool {
    policy
        .quiet_hours
        .is_some_and(|q| q.contains(now.with_timezone(&policy.timezone).time()))
}

/// When `now` is quiet, the UTC instant the quiet window ends.
pub fn quiet_end_if_quiet(policy: &AttentionPolicy, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let q = policy.quiet_hours?;
    let local = now.with_timezone(&policy.timezone);
    let t = local.time();
    if !q.contains(t) {
        return None;
    }
    let date = local.date_naive();
    let end_date = if q.start > q.end && t >= q.start {
        date.succ_opt()?
    } else {
        date
    };
    let end = resolve_local(policy.timezone, end_date, q.end);
    // A fall-back repeat can resolve the end to an instant not after now;
    // never hand back a release time in the past.
    Some(if end <= now { now } else { end })
}

/// Resolve a local wall-clock time to UTC. Ambiguous (fall-back) times take
/// the earlier instant; nonexistent (spring-forward gap) times move to the
/// first valid minute after the gap.
pub fn resolve_local(tz: Tz, date: NaiveDate, time: NaiveTime) -> DateTime<Utc> {
    let mut ndt = NaiveDateTime::new(date, time);
    for _ in 0..(24 * 60) {
        match tz.from_local_datetime(&ndt) {
            LocalResult::Single(dt) => return dt.with_timezone(&Utc),
            LocalResult::Ambiguous(a, b) => {
                return a.min(b).with_timezone(&Utc);
            }
            LocalResult::None => ndt += Duration::minutes(1),
        }
    }
    // Unreachable for real zones; fall back to treating the time as UTC.
    Utc.from_utc_datetime(&NaiveDateTime::new(date, time))
}

/// Stable content hash of an item's title + body (sha256, hex).
pub fn content_hash(title: &str, body: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(title.as_bytes());
    h.update([0u8]);
    h.update(body.as_bytes());
    hex::encode(h.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chicago(quiet: Option<(&str, &str)>) -> AttentionPolicy {
        AttentionPolicy {
            timezone: chrono_tz::America::Chicago,
            quiet_hours: quiet.map(|(s, e)| QuietHours::parse(s, e).unwrap()),
            dedupe_window: Duration::hours(24),
            per_hour_cap: 3,
        }
    }

    fn at(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    fn cand<'a>(key: &'a str, hash: &'a str) -> Candidate<'a> {
        Candidate {
            item_id: None,
            key,
            content_hash: hash,
            releasing: false,
        }
    }

    fn delivered(id: &str, key: &str, hash: &str, t: &str) -> HistoryEntry {
        HistoryEntry {
            item_id: id.into(),
            key: key.into(),
            content_hash: hash.into(),
            state: HistoryState::Delivered(at(t)),
        }
    }

    #[test]
    fn delivers_outside_quiet_hours() {
        let p = chicago(Some(("22:30", "06:00")));
        // 12:00 CDT
        assert_eq!(
            decide(&p, &cand("k", "h"), &[], at("2026-09-29T17:00:00Z")),
            Decision::Deliver
        );
    }

    #[test]
    fn quiet_hours_across_midnight_defer_to_next_morning() {
        let p = chicago(Some(("22:30", "06:00")));
        // 23:00 CDT Sep 29 -> 06:00 CDT Sep 30 = 11:00Z
        let d = decide(&p, &cand("k", "h"), &[], at("2026-09-30T04:00:00Z"));
        assert_eq!(
            d,
            Decision::Defer {
                until: at("2026-09-30T11:00:00Z"),
                reason: DeferReason::QuietHours
            }
        );
        // 03:00 CDT Sep 30 (after midnight) -> same morning 06:00 CDT
        let d = decide(&p, &cand("k", "h"), &[], at("2026-09-30T08:00:00Z"));
        assert_eq!(
            d,
            Decision::Defer {
                until: at("2026-09-30T11:00:00Z"),
                reason: DeferReason::QuietHours
            }
        );
        // 22:29 CDT is not quiet; 06:00 CDT exactly is not quiet.
        assert_eq!(
            decide(&p, &cand("k", "h"), &[], at("2026-09-30T03:29:00Z")),
            Decision::Deliver
        );
        assert_eq!(
            decide(&p, &cand("k", "h"), &[], at("2026-09-30T11:00:00Z")),
            Decision::Deliver
        );
    }

    #[test]
    fn quiet_hours_across_spring_forward() {
        let p = chicago(Some(("22:30", "06:00")));
        // 23:00 CST Mar 7 2026 (05:00Z Mar 8). Clocks jump 02:00->03:00 on
        // Mar 8, so 06:00 local is CDT (UTC-5) = 11:00Z, not 12:00Z.
        let d = decide(&p, &cand("k", "h"), &[], at("2026-03-08T05:00:00Z"));
        assert_eq!(
            d,
            Decision::Defer {
                until: at("2026-03-08T11:00:00Z"),
                reason: DeferReason::QuietHours
            }
        );
    }

    #[test]
    fn quiet_hours_across_fall_back() {
        let p = chicago(Some(("22:30", "06:00")));
        // 23:00 CDT Oct 31 2026 (04:00Z Nov 1). Clocks fall back 02:00->01:00
        // on Nov 1, so 06:00 local is CST (UTC-6) = 12:00Z.
        let d = decide(&p, &cand("k", "h"), &[], at("2026-11-01T04:00:00Z"));
        assert_eq!(
            d,
            Decision::Defer {
                until: at("2026-11-01T12:00:00Z"),
                reason: DeferReason::QuietHours
            }
        );
    }

    #[test]
    fn quiet_end_inside_dst_gap_moves_to_first_valid_minute() {
        // Quiet 01:00-02:30; 02:30 does not exist on Mar 8 2026 in Chicago.
        let p = chicago(Some(("01:00", "02:30")));
        // 01:30 CST = 07:30Z -> first valid minute after the gap is 03:00 CDT = 08:00Z
        let d = decide(&p, &cand("k", "h"), &[], at("2026-03-08T07:30:00Z"));
        assert_eq!(
            d,
            Decision::Defer {
                until: at("2026-03-08T08:00:00Z"),
                reason: DeferReason::QuietHours
            }
        );
    }

    #[test]
    fn dedupe_coalesces_same_key_and_hash_inside_window() {
        let p = chicago(None);
        let hist = vec![delivered("a1", "k", "h", "2026-09-29T00:00:00Z")];
        assert_eq!(
            decide(&p, &cand("k", "h"), &hist, at("2026-09-29T20:00:00Z")),
            Decision::Coalesce { into: "a1".into() }
        );
        // Outside the 24h window it delivers again.
        assert_eq!(
            decide(&p, &cand("k", "h"), &hist, at("2026-09-30T00:00:01Z")),
            Decision::Deliver
        );
        // Same key, different content delivers.
        assert_eq!(
            decide(&p, &cand("k", "other"), &hist, at("2026-09-29T20:00:00Z")),
            Decision::Deliver
        );
        // Different key, same content delivers.
        assert_eq!(
            decide(&p, &cand("k2", "h"), &hist, at("2026-09-29T20:00:00Z")),
            Decision::Deliver
        );
    }

    #[test]
    fn dedupe_coalesces_into_a_queued_item() {
        let p = chicago(None);
        let hist = vec![HistoryEntry {
            item_id: "q1".into(),
            key: "k".into(),
            content_hash: "h".into(),
            state: HistoryState::Queued,
        }];
        assert_eq!(
            decide(&p, &cand("k", "h"), &hist, at("2026-09-29T20:00:00Z")),
            Decision::Coalesce { into: "q1".into() }
        );
    }

    #[test]
    fn hourly_cap_defers_until_a_slot_frees() {
        let p = chicago(None); // cap 3
        let hist = vec![
            delivered("a", "k1", "h1", "2026-09-29T17:10:00Z"),
            delivered("b", "k2", "h2", "2026-09-29T17:20:00Z"),
            delivered("c", "k3", "h3", "2026-09-29T17:30:00Z"),
        ];
        let d = decide(&p, &cand("k4", "h4"), &hist, at("2026-09-29T17:40:00Z"));
        assert_eq!(
            d,
            Decision::Defer {
                until: at("2026-09-29T18:10:00Z"),
                reason: DeferReason::HourlyCap
            }
        );
        // After the first ages out, it delivers.
        assert_eq!(
            decide(&p, &cand("k4", "h4"), &hist, at("2026-09-29T18:10:00Z")),
            Decision::Deliver
        );
    }

    #[test]
    fn cap_deferral_landing_in_quiet_hours_moves_to_quiet_end() {
        let mut p = chicago(Some(("22:30", "06:00")));
        p.per_hour_cap = 1;
        // Delivered 22:00 CDT; next slot 23:00 CDT is quiet -> 06:00 CDT.
        let hist = vec![delivered("a", "k1", "h1", "2026-09-30T03:00:00Z")];
        let d = decide(&p, &cand("k2", "h2"), &hist, at("2026-09-30T03:20:00Z"));
        assert_eq!(
            d,
            Decision::Defer {
                until: at("2026-09-30T11:00:00Z"),
                reason: DeferReason::HourlyCap
            }
        );
    }

    #[test]
    fn releasing_skips_dedupe_and_self() {
        let p = chicago(None);
        let hist = vec![HistoryEntry {
            item_id: "q1".into(),
            key: "k".into(),
            content_hash: "h".into(),
            state: HistoryState::Queued,
        }];
        let c = Candidate {
            item_id: Some("q1"),
            key: "k",
            content_hash: "h",
            releasing: true,
        };
        assert_eq!(
            decide(&p, &c, &hist, at("2026-09-29T20:00:00Z")),
            Decision::Deliver
        );
    }
}
