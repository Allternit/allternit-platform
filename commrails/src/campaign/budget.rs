//! Campaign budgets.
//!
//! The unit is declared by the campaign and never interpreted: it is carried,
//! printed and compared. How spend accumulates is declared too:
//!
//! - **additive** — every spend entry pays for itself (core-minutes, runs).
//! - **shared** — entries that carry a span on the same resource overlap:
//!   the overlapping stretch costs what the most expensive single entry costs
//!   for that stretch (occupancy: a GPU busy twice at once was busy once).
//!   Entries without a span or resource are added as-is.
//!
//! A budget bounds what a campaign may *record* as spent. It does **not**
//! cap model-provider or cloud bills: nothing here meters a provider, and a
//! campaign whose executor never reports spend never exhausts its budget.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum BudgetMode {
    Shared,
    #[default]
    Additive,
}

impl std::fmt::Display for BudgetMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BudgetMode::Shared => write!(f, "shared"),
            BudgetMode::Additive => write!(f, "additive"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Budget {
    /// Declared unit (`sweep-run`, `gpu-minute`, `usd-estimate`…). Not interpreted.
    pub unit: String,
    pub limit: f64,
    #[serde(default)]
    pub mode: BudgetMode,
    /// Amount recorded automatically each time a campaign check wake fires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_wake: Option<f64>,
    /// Computed from spend entries by the projection.
    #[serde(default)]
    pub spent: f64,
}

impl Budget {
    pub fn exhausted(&self) -> bool {
        self.spent >= self.limit
    }
    pub fn remaining(&self) -> f64 {
        (self.limit - self.spent).max(0.0)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpendEntry {
    pub amount: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Total spend under `mode`.
pub fn total_spent(mode: BudgetMode, entries: &[SpendEntry]) -> f64 {
    match mode {
        BudgetMode::Additive => entries.iter().map(|e| e.amount).sum(),
        BudgetMode::Shared => {
            let mut flat = 0.0;
            // resource -> [(start_ms, end_ms, rate per ms)]
            let mut spans: BTreeMap<&str, Vec<(i64, i64, f64)>> = BTreeMap::new();
            for e in entries {
                match (e.start, e.end, e.resource.as_deref()) {
                    (Some(s), Some(en), Some(r)) if en > s => {
                        let (s, en) = (s.timestamp_millis(), en.timestamp_millis());
                        spans
                            .entry(r)
                            .or_default()
                            .push((s, en, e.amount / (en - s) as f64));
                    }
                    _ => flat += e.amount,
                }
            }
            let mut shared = 0.0;
            for list in spans.values() {
                let mut points: Vec<i64> = list.iter().flat_map(|(s, e, _)| [*s, *e]).collect();
                points.sort_unstable();
                points.dedup();
                for w in points.windows(2) {
                    let (a, b) = (w[0], w[1]);
                    let rate = list
                        .iter()
                        .filter(|(s, e, _)| *s <= a && *e >= b)
                        .map(|(_, _, r)| *r)
                        .fold(0.0_f64, f64::max);
                    shared += rate * (b - a) as f64;
                }
            }
            flat + shared
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(amount: f64, s: &str, e: &str, r: &str) -> SpendEntry {
        SpendEntry {
            amount,
            start: Some(s.parse().unwrap()),
            end: Some(e.parse().unwrap()),
            resource: Some(r.into()),
            note: None,
        }
    }

    #[test]
    fn additive_sums_every_entry() {
        let e = vec![
            span(60.0, "2026-09-29T10:00:00Z", "2026-09-29T11:00:00Z", "gpu0"),
            span(60.0, "2026-09-29T10:00:00Z", "2026-09-29T11:00:00Z", "gpu0"),
        ];
        assert_eq!(total_spent(BudgetMode::Additive, &e), 120.0);
    }

    #[test]
    fn shared_counts_overlap_on_one_resource_once() {
        let e = vec![
            span(60.0, "2026-09-29T10:00:00Z", "2026-09-29T11:00:00Z", "gpu0"),
            span(60.0, "2026-09-29T10:00:00Z", "2026-09-29T11:00:00Z", "gpu0"),
        ];
        assert!((total_spent(BudgetMode::Shared, &e) - 60.0).abs() < 1e-9);
        // Half overlap: 10:00-11:00 and 10:30-11:30 at 1/min = 90.
        let e = vec![
            span(60.0, "2026-09-29T10:00:00Z", "2026-09-29T11:00:00Z", "gpu0"),
            span(60.0, "2026-09-29T10:30:00Z", "2026-09-29T11:30:00Z", "gpu0"),
        ];
        assert!((total_spent(BudgetMode::Shared, &e) - 90.0).abs() < 1e-9);
        // Different resources do not share.
        let e = vec![
            span(60.0, "2026-09-29T10:00:00Z", "2026-09-29T11:00:00Z", "gpu0"),
            span(60.0, "2026-09-29T10:00:00Z", "2026-09-29T11:00:00Z", "gpu1"),
        ];
        assert!((total_spent(BudgetMode::Shared, &e) - 120.0).abs() < 1e-9);
    }

    #[test]
    fn shared_adds_spanless_entries_flat() {
        let e = vec![
            SpendEntry {
                amount: 2.0,
                start: None,
                end: None,
                resource: None,
                note: None,
            },
            span(60.0, "2026-09-29T10:00:00Z", "2026-09-29T11:00:00Z", "gpu0"),
        ];
        assert!((total_spent(BudgetMode::Shared, &e) - 62.0).abs() < 1e-9);
    }
}
