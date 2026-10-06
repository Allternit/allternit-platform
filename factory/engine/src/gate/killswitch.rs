//! Kill switch and SLO monitoring for the Rails CLI.
//!
//! The kill switch provides an emergency brake that can disable mutating
//! operations across a workspace. SLO monitoring records operation metrics
//! for observability.

use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::core::io::{read_json, write_json_atomic};

/// Path to the kill switch file, relative to workspace root.
pub const KILL_SWITCH_PATH: &str = ".allternit/rails/kill_switch.json";

/// Path to the SLO metrics file, relative to workspace root.
pub const SLO_METRICS_PATH: &str = ".allternit/rails/slo_metrics.json";

/// Kill switch state.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct KillSwitch {
    pub enabled: bool,
    pub reason: Option<String>,
    pub enabled_at: Option<DateTime<Utc>>,
    pub enabled_by: Option<String>,
}

impl KillSwitch {
    /// Load the kill switch from disk.
    pub fn load(root: impl AsRef<Path>) -> Result<Self> {
        let path = root.as_ref().join(KILL_SWITCH_PATH);
        Ok(read_json(&path)?.unwrap_or_default())
    }

    /// Persist the kill switch.
    pub fn save(&self, root: impl AsRef<Path>) -> Result<()> {
        let path = root.as_ref().join(KILL_SWITCH_PATH);
        write_json_atomic(&path, self)
            .with_context(|| format!("failed to write kill switch {path:?}"))
    }

    /// Enable the kill switch.
    pub fn enable(&mut self, reason: impl Into<String>, actor: impl Into<String>) {
        self.enabled = true;
        self.reason = Some(reason.into());
        self.enabled_at = Some(Utc::now());
        self.enabled_by = Some(actor.into());
    }

    /// Disable the kill switch.
    pub fn disable(&mut self) {
        self.enabled = false;
        self.reason = None;
        self.enabled_at = None;
        self.enabled_by = None;
    }

    /// Return an error if the kill switch is enabled.
    pub fn check(&self) -> Result<()> {
        if self.enabled {
            anyhow::bail!(
                "kill switch is active{}: mutating operations are disabled",
                self.reason
                    .as_ref()
                    .map(|r| format!(" ({r})"))
                    .unwrap_or_default()
            );
        }
        Ok(())
    }
}

/// A single SLO metric sample.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct OperationMetric {
    pub operation: String,
    pub success: bool,
    pub duration_ms: u64,
    pub recorded_at: DateTime<Utc>,
}

/// Aggregate SLO metrics.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SloMetrics {
    pub operations: Vec<OperationMetric>,
}

impl SloMetrics {
    /// Load metrics from disk.
    pub fn load(root: impl AsRef<Path>) -> Result<Self> {
        let path = root.as_ref().join(SLO_METRICS_PATH);
        Ok(read_json(&path)?.unwrap_or_default())
    }

    /// Persist metrics.
    pub fn save(&self, root: impl AsRef<Path>) -> Result<()> {
        let path = root.as_ref().join(SLO_METRICS_PATH);
        write_json_atomic(&path, self)
            .with_context(|| format!("failed to write SLO metrics {path:?}"))
    }

    /// Record a new operation metric.
    pub fn record(&mut self, operation: impl Into<String>, success: bool, duration_ms: u64) {
        self.operations.push(OperationMetric {
            operation: operation.into(),
            success,
            duration_ms,
            recorded_at: Utc::now(),
        });
        // Keep last 10,000 samples.
        if self.operations.len() > 10_000 {
            let drop = self.operations.len() - 10_000;
            self.operations.drain(0..drop);
        }
    }

    /// Return a summary of recent metrics.
    pub fn summary(&self, window_minutes: i64) -> SloSummary {
        let cutoff = Utc::now() - chrono::Duration::minutes(window_minutes);
        let recent: Vec<_> = self
            .operations
            .iter()
            .filter(|m| m.recorded_at >= cutoff)
            .collect();

        let total = recent.len();
        let successes = recent.iter().filter(|m| m.success).count();
        let failures = total - successes;
        let avg_duration_ms = if total > 0 {
            recent.iter().map(|m| m.duration_ms).sum::<u64>() / total as u64
        } else {
            0
        };

        SloSummary {
            window_minutes,
            total,
            successes,
            failures,
            avg_duration_ms,
        }
    }
}

/// Human-readable SLO summary.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SloSummary {
    pub window_minutes: i64,
    pub total: usize,
    pub successes: usize,
    pub failures: usize,
    pub avg_duration_ms: u64,
}

/// Wrap a mutating operation with kill-switch and SLO instrumentation.
pub fn guard<T>(
    root: &Path,
    operation: &str,
    f: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let ks = KillSwitch::load(root)?;
    ks.check()?;

    let start = std::time::Instant::now();
    let result = f();
    let duration_ms = start.elapsed().as_millis() as u64;

    let mut metrics = SloMetrics::load(root).unwrap_or_default();
    metrics.record(operation, result.is_ok(), duration_ms);
    let _ = metrics.save(root);

    result
}
