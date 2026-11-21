//! Computers console history (surfaces/docs/dev/computers-console-contract.mdx,
//! D4 and the cloud route `GET /api/v1/computers/:id/metrics`, step 2.2).
//!
//! * Ingest: a node adds `hostSample` (a `HostSnapshot`, the host-probe
//!   `GET /v1/host/snapshot` body) to its heartbeat about once a minute. Three
//!   heartbeats carry it, each optional, so old nodes that send none keep
//!   working unchanged:
//!   - `POST /api/v1/runtime-presence/heartbeat` (signed, every 60 s) and
//!     `POST /api/v1/runtime-devices/:id/heartbeat` (device token): stored
//!     under the runtime device (`computer_kind` `device`).
//!   - `POST /api/v1/computers/paired/:id/report` (computer secret): stored
//!     under the paired computer (`computer_kind` `paired`).
//!
//!   A sample is checked and re-encoded (unknown fields dropped), stored at
//!   the cloud's receive time cut to the minute, at most one per computer per
//!   minute. A bad or missing sample never fails the heartbeat.
//! * `GET /api/v1/computers/:id/metrics?window=1h|24h|7d|30d` (default 24h):
//!   `{ "series": MetricSeries[] }` for a console id (`ConsoleComputer.id`:
//!   a cloud computer, a paired computer or a runtime device). Access is the
//!   console list's rule (routes::computers_console): your own cloud
//!   computers and runtimes, your own paired computers and those of your
//!   organization. Anyone else's id answers 404, like a missing one.
//!   Metrics: `cpu` (percent), `memory` (percent used), `disk` (percent used of
//!   the fullest mount), `net_rx` / `net_tx` (bytes per second), averaged per
//!   bucket (1h: 1 min, 24h: 5 min, 7d: 30 min, 30d: 2 h), oldest first.
//! * Retention ([`sweep`], every 5 minutes from [`start_metrics_sweeper`]):
//!   raw samples are kept 48 hours, then rolled up into 5 minute rows
//!   (averages, weighted by `sample_count`), kept 30 days.
//!
//! Alert rules (`computer_alert_rules`, migration 154) are evaluated and
//! pushed in step 2.8, not here.

use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use std::sync::Arc;

use crate::routes::paired_computers::{may_use, signed_in_caller, Caller};
use crate::{ApiError, ApiState};

/// The heartbeat field that carries the sample.
pub const SAMPLE_FIELD: &str = "hostSample";
/// `computer_metric_samples.computer_kind` of a runtime device's samples.
pub const KIND_DEVICE: &str = "device";
/// `computer_metric_samples.computer_kind` of a paired computer's samples.
pub const KIND_PAIRED: &str = "paired";

const RAW_SECS: i64 = 60;
const ROLLUP_SECS: i64 = 300;
const RAW_KEEP_HOURS: i64 = 48;
const ROLLUP_KEEP_DAYS: i64 = 30;
const MAX_DISKS: usize = 32;
const MAX_TEXT: usize = 256;
const SWEEP_EVERY: std::time::Duration = std::time::Duration::from_secs(300);

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new().route("/api/v1/computers/:id/metrics", get(metrics))
}

// ------------------------------------------------------------- the sample ----

/// `HostSnapshot` (console-types.ts; host-probe `HostSnapshot`). The optional
/// extras host-probe sends (`primaryDisk`, `kernel`, `hostname`, `model`,
/// `fs`) are kept; anything else is dropped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostSample {
    pub sampled_at: String,
    pub uptime_sec: u64,
    pub os: SampleOs,
    pub cpu: SampleCpu,
    pub memory: SampleMemory,
    pub disk: Vec<SampleDisk>,
    pub net: SampleNet,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_disk: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleOs {
    pub name: String,
    pub version: String,
    pub family: String,
    pub arch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleCpu {
    pub percent: f64,
    pub cores: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load1: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleMemory {
    pub used_bytes: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleDisk {
    pub mount: String,
    pub used_bytes: u64,
    pub total_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fs: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleNet {
    pub rx_bytes_per_sec: f64,
    pub tx_bytes_per_sec: f64,
}

/// The stored columns of one raw sample.
#[derive(Debug, Clone, PartialEq)]
pub struct SampleRow {
    pub cpu_percent: f64,
    pub cpu_cores: i32,
    pub load1: Option<f64>,
    pub mem_used_bytes: i64,
    pub mem_total_bytes: i64,
    /// Percent 0-100, from the bytes.
    pub mem_percent: Option<f64>,
    pub disk_mount: Option<String>,
    pub disk_used_bytes: Option<i64>,
    pub disk_total_bytes: Option<i64>,
    /// Percent 0-100 of the fullest mount, from the bytes.
    pub disk_percent: Option<f64>,
    pub net_rx_bytes_per_sec: f64,
    pub net_tx_bytes_per_sec: f64,
    pub uptime_sec: i64,
    /// The checked sample, re-encoded: the console's `lastSample`.
    pub snapshot: Value,
}

fn short(s: &str) -> bool {
    s.len() <= MAX_TEXT
}

fn rate(v: f64) -> Option<f64> {
    (v.is_finite() && v >= 0.0).then_some(v)
}

fn bytes(v: u64) -> Option<i64> {
    i64::try_from(v).ok()
}

/// Check one `hostSample` and turn it into its stored row. `None` = not a
/// usable sample (wrong shape, a non-finite or negative number, oversized
/// text): the heartbeat goes on without it.
pub fn parse_sample(raw: &Value) -> Option<SampleRow> {
    let mut s: HostSample = serde_json::from_value(raw.clone()).ok()?;
    let texts = [&s.sampled_at, &s.os.name, &s.os.version, &s.os.family, &s.os.arch];
    if !texts.iter().all(|t| short(t)) || DateTime::parse_from_rfc3339(&s.sampled_at).is_err() {
        return None;
    }
    let extras = [&s.primary_disk, &s.os.kernel, &s.os.hostname, &s.cpu.model];
    if !extras.iter().all(|t| t.as_deref().is_none_or(short)) {
        return None;
    }
    s.disk.truncate(MAX_DISKS);
    if !s.disk.iter().all(|d| short(&d.mount) && d.fs.as_deref().is_none_or(short)) {
        return None;
    }
    let cpu = rate(s.cpu.percent)?.min(100.0);
    s.cpu.percent = cpu;
    let load1 = match s.cpu.load1 {
        Some(l) => Some(rate(l)?),
        None => None,
    };
    let fullest = s
        .disk
        .iter()
        .filter(|d| d.total_bytes > 0)
        .max_by(|a, b| fill(a).total_cmp(&fill(b)));
    let (disk_mount, disk_used_bytes, disk_total_bytes) = match fullest {
        Some(d) => (Some(d.mount.clone()), Some(bytes(d.used_bytes)?), Some(bytes(d.total_bytes)?)),
        None => (None, None, None),
    };
    for d in &s.disk {
        bytes(d.used_bytes)?;
        bytes(d.total_bytes)?;
    }
    let disk_percent = fullest.map(|d| percent(d.used_bytes, d.total_bytes));
    let mem_percent = (s.memory.total_bytes > 0).then(|| percent(s.memory.used_bytes, s.memory.total_bytes));
    Some(SampleRow {
        cpu_percent: cpu,
        cpu_cores: i32::try_from(s.cpu.cores).ok()?,
        load1,
        mem_used_bytes: bytes(s.memory.used_bytes)?,
        mem_total_bytes: bytes(s.memory.total_bytes)?,
        mem_percent,
        disk_mount,
        disk_used_bytes,
        disk_total_bytes,
        disk_percent,
        net_rx_bytes_per_sec: rate(s.net.rx_bytes_per_sec)?,
        net_tx_bytes_per_sec: rate(s.net.tx_bytes_per_sec)?,
        uptime_sec: bytes(s.uptime_sec)?,
        snapshot: serde_json::to_value(&s).ok()?,
    })
}

fn fill(d: &SampleDisk) -> f64 {
    d.used_bytes as f64 / d.total_bytes as f64
}

/// `used` of `total` as a percent, 0 to 100.
fn percent(used: u64, total: u64) -> f64 {
    (used as f64 / total as f64 * 100.0).clamp(0.0, 100.0)
}

/// `t` cut down to a multiple of `secs` since the epoch.
pub fn floor_to(t: DateTime<Utc>, secs: i64) -> DateTime<Utc> {
    let s = t.timestamp().div_euclid(secs) * secs;
    DateTime::from_timestamp(s, 0).unwrap_or(t)
}

/// Store the `hostSample` of a heartbeat body, if it has one. Best effort:
/// returns whether a row was written and never fails the caller.
pub async fn store_from_heartbeat(db: &PgPool, kind: &str, computer_id: &str, body: &Value, now: DateTime<Utc>) -> bool {
    match body.get(SAMPLE_FIELD).filter(|v| !v.is_null()) {
        Some(sample) => store_sample(db, kind, computer_id, sample, now).await,
        None => false,
    }
}

/// Store one sample for `(kind, computer_id)` in the minute of `now`. A
/// second sample in the same minute is ignored.
pub async fn store_sample(db: &PgPool, kind: &str, computer_id: &str, sample: &Value, now: DateTime<Utc>) -> bool {
    let Some(row) = parse_sample(sample) else {
        tracing::debug!(%computer_id, "computer metrics: heartbeat sample does not parse");
        return false;
    };
    let result = sqlx::query(
        "INSERT INTO computer_metric_samples
           (computer_kind, computer_id, ts, resolution_secs, sample_count, cpu_percent, cpu_cores, load1,
            mem_used_bytes, mem_total_bytes, mem_percent, disk_mount, disk_used_bytes, disk_total_bytes,
            disk_percent, net_rx_bytes_per_sec, net_tx_bytes_per_sec, uptime_sec, snapshot)
         VALUES ($1, $2, $3, $4, 1, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18)
         ON CONFLICT (computer_kind, computer_id, resolution_secs, ts) DO NOTHING",
    )
    .bind(kind)
    .bind(computer_id)
    .bind(floor_to(now, RAW_SECS))
    .bind(RAW_SECS as i32)
    .bind(row.cpu_percent)
    .bind(row.cpu_cores)
    .bind(row.load1)
    .bind(row.mem_used_bytes)
    .bind(row.mem_total_bytes)
    .bind(row.mem_percent)
    .bind(&row.disk_mount)
    .bind(row.disk_used_bytes)
    .bind(row.disk_total_bytes)
    .bind(row.disk_percent)
    .bind(row.net_rx_bytes_per_sec)
    .bind(row.net_tx_bytes_per_sec)
    .bind(row.uptime_sec)
    .bind(&row.snapshot)
    .execute(db)
    .await;
    match result {
        Ok(r) => r.rows_affected() > 0,
        Err(e) => {
            tracing::warn!(%computer_id, "computer metrics: storing a sample failed: {e}");
            false
        }
    }
}

// ------------------------------------------------------------- the route ----

/// A metrics window: how far back, and the bucket width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub span_secs: i64,
    pub step_secs: i64,
}

impl Window {
    pub fn parse(raw: &str) -> Option<Window> {
        let (span_secs, step_secs) = match raw {
            "1h" => (3_600, 60),
            "24h" => (86_400, 300),
            "7d" => (7 * 86_400, 1_800),
            "30d" => (30 * 86_400, 7_200),
            _ => return None,
        };
        Some(Window { span_secs, step_secs })
    }
}

/// `MetricSeries` (console-types.ts).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MetricSeries {
    pub metric: &'static str,
    /// `[ISO timestamp, value]`, oldest first.
    pub points: Vec<(String, f64)>,
}

/// Where a console id's samples live: runtime device ids and paired
/// computer ids.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sources {
    pub devices: Vec<String>,
    pub paired: Vec<String>,
}

#[derive(Deserialize)]
struct MetricsQuery {
    window: Option<String>,
}

async fn metrics(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<MetricsQuery>,
) -> Result<Response, ApiError> {
    let caller = signed_in_caller(&state, &headers).await?;
    let window = Window::parse(q.window.as_deref().unwrap_or("24h"))
        .ok_or_else(|| ApiError::BadRequest("window must be 1h, 24h, 7d or 30d".into()))?;
    let Some(sources) = sources_for(&state.db, &caller, &id).await? else {
        return Err(ApiError::NotFound("no such computer".into()));
    };
    let series = load_series(&state.db, &sources, window, Utc::now()).await?;
    Ok(Json(serde_json::json!({ "series": series })).into_response())
}

/// The sample sources of console id `id`, when `caller` may see it (the
/// console list's rule). `None` = missing or someone else's.
pub async fn sources_for(db: &PgPool, caller: &Caller, id: &str) -> Result<Option<Sources>, sqlx::Error> {
    // A cloud computer: its runtime device and its self-paired row.
    let cloud: Option<(Option<String>, Option<Vec<String>>)> = sqlx::query_as(
        "SELECT pi.device_id,
                (SELECT array_agg(pc.id) FROM paired_computers pc WHERE pc.provisioned_instance_id = pi.id)
         FROM provisioned_instances pi
         WHERE pi.id = $1 AND pi.user_id = $2 AND pi.status <> 'deleted'",
    )
    .bind(id)
    .bind(&caller.user_id)
    .fetch_optional(db)
    .await?;
    if let Some((device, paired)) = cloud {
        return Ok(Some(Sources { devices: device.into_iter().collect(), paired: paired.unwrap_or_default() }));
    }
    // A paired computer: the row, and the runtime device of the same machine.
    let paired: Option<(String, Option<String>, String, Option<String>)> = sqlx::query_as(
        "SELECT user_id, organization_id, name, runtime_device_id FROM paired_computers
         WHERE id = $1 AND provisioned_instance_id IS NULL",
    )
    .bind(id)
    .fetch_optional(db)
    .await?;
    if let Some((owner, org, name, runtime_device_id)) = paired {
        if !may_use(caller, &owner, org.as_deref()) {
            return Ok(None);
        }
        let device = match runtime_device_id {
            Some(d) => Some(d),
            // Not linked yet: the owner's device with the same name, as the list matches it.
            None if owner == caller.user_id => {
                sqlx::query_scalar(
                    "SELECT id FROM runtime_devices
                     WHERE user_id = $1 AND revoked_at IS NULL AND runtime_type IN ('desktop', 'vps', 'ao')
                       AND lower(trim(name)) = lower(trim($2))
                     ORDER BY created_at LIMIT 1",
                )
                .bind(&owner)
                .bind(&name)
                .fetch_optional(db)
                .await?
            }
            None => None,
        };
        return Ok(Some(Sources { devices: device.into_iter().collect(), paired: vec![id.to_string()] }));
    }
    // A Desktop or a server paired with `gizzi pair`.
    let device: Option<String> = sqlx::query_scalar(
        "SELECT d.id FROM runtime_devices d
         WHERE d.id = $1 AND d.user_id = $2 AND d.revoked_at IS NULL
           AND d.runtime_type IN ('desktop', 'vps', 'ao')
           AND NOT EXISTS (SELECT 1 FROM provisioned_instances pi WHERE pi.device_id = d.id)",
    )
    .bind(id)
    .bind(&caller.user_id)
    .fetch_optional(db)
    .await?;
    Ok(device.map(|d| Sources { devices: vec![d], paired: vec![] }))
}

type BucketRow = (f64, Option<f64>, Option<f64>, Option<f64>, Option<f64>, Option<f64>);

/// The five series of `sources` over `window` ending at `now`. Raw samples
/// and rollups are averaged together, each weighted by its sample count.
pub async fn load_series(
    db: &PgPool,
    sources: &Sources,
    window: Window,
    now: DateTime<Utc>,
) -> Result<Vec<MetricSeries>, sqlx::Error> {
    let rows: Vec<BucketRow> = if sources.devices.is_empty() && sources.paired.is_empty() {
        Vec::new()
    } else {
        sqlx::query_as(
            "SELECT floor(extract(epoch FROM ts)::float8 / $4) * $4 AS bucket,
                    sum(cpu_percent * sample_count) / NULLIF(sum(sample_count) FILTER (WHERE cpu_percent IS NOT NULL), 0),
                    sum(mem_percent * sample_count) / NULLIF(sum(sample_count) FILTER (WHERE mem_percent IS NOT NULL), 0),
                    sum(disk_percent * sample_count) / NULLIF(sum(sample_count) FILTER (WHERE disk_percent IS NOT NULL), 0),
                    sum(net_rx_bytes_per_sec * sample_count)
                      / NULLIF(sum(sample_count) FILTER (WHERE net_rx_bytes_per_sec IS NOT NULL), 0),
                    sum(net_tx_bytes_per_sec * sample_count)
                      / NULLIF(sum(sample_count) FILTER (WHERE net_tx_bytes_per_sec IS NOT NULL), 0)
             FROM computer_metric_samples
             WHERE ((computer_kind = 'device' AND computer_id = ANY($1))
                 OR (computer_kind = 'paired' AND computer_id = ANY($2)))
               AND ts >= $3
             GROUP BY 1
             ORDER BY 1",
        )
        .bind(&sources.devices)
        .bind(&sources.paired)
        .bind(now - Duration::seconds(window.span_secs))
        .bind(window.step_secs as f64)
        .fetch_all(db)
        .await?
    };
    Ok(series_from_buckets(&rows))
}

/// Split bucket rows into the five series; a bucket with no value for a
/// metric is left out of that series. Percents keep one decimal, rates whole
/// bytes.
pub fn series_from_buckets(rows: &[BucketRow]) -> Vec<MetricSeries> {
    let names = ["cpu", "memory", "disk", "net_rx", "net_tx"];
    names
        .iter()
        .enumerate()
        .map(|(i, metric)| {
            let points = rows
                .iter()
                .filter_map(|r| {
                    let v = [r.1, r.2, r.3, r.4, r.5][i]?;
                    let v = if i < 3 { (v * 10.0).round() / 10.0 } else { v.round() };
                    let t = DateTime::from_timestamp(r.0 as i64, 0)?;
                    Some((t.to_rfc3339_opts(SecondsFormat::Secs, true), v))
                })
                .collect();
            MetricSeries { metric, points }
        })
        .collect()
}

// ------------------------------------------------------------- retention ----

/// What one sweep did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SweepCounts {
    pub rolled_up: u64,
    pub raw_deleted: u64,
    pub rollups_deleted: u64,
}

/// Roll raw samples older than 48 hours into 5 minute rows, then drop them,
/// and drop rollups older than 30 days. The cut is on a 5 minute boundary, so
/// a bucket is always rolled up whole. One transaction; safe to run on
/// several replicas at once (a rollup row already there is kept).
pub async fn sweep(db: &PgPool, now: DateTime<Utc>) -> Result<SweepCounts, sqlx::Error> {
    let raw_cut = floor_to(now - Duration::hours(RAW_KEEP_HOURS), ROLLUP_SECS);
    let rollup_cut = now - Duration::days(ROLLUP_KEEP_DAYS);
    let mut tx = db.begin().await?;
    let rolled = sqlx::query(
        "INSERT INTO computer_metric_samples
           (computer_kind, computer_id, ts, resolution_secs, sample_count, cpu_percent, cpu_cores, load1,
            mem_used_bytes, mem_total_bytes, mem_percent, disk_mount, disk_used_bytes, disk_total_bytes,
            disk_percent, net_rx_bytes_per_sec, net_tx_bytes_per_sec, uptime_sec)
         SELECT computer_kind, computer_id,
                to_timestamp(floor(extract(epoch FROM ts)::float8 / $2) * $2), $3, sum(sample_count)::int,
                sum(cpu_percent * sample_count) / NULLIF(sum(sample_count) FILTER (WHERE cpu_percent IS NOT NULL), 0),
                max(cpu_cores),
                sum(load1 * sample_count) / NULLIF(sum(sample_count) FILTER (WHERE load1 IS NOT NULL), 0),
                round(sum(mem_used_bytes::float8 * sample_count) / NULLIF(sum(sample_count) FILTER (WHERE mem_used_bytes IS NOT NULL), 0))::bigint,
                max(mem_total_bytes),
                sum(mem_percent * sample_count) / NULLIF(sum(sample_count) FILTER (WHERE mem_percent IS NOT NULL), 0),
                max(disk_mount),
                round(sum(disk_used_bytes::float8 * sample_count) / NULLIF(sum(sample_count) FILTER (WHERE disk_used_bytes IS NOT NULL), 0))::bigint,
                max(disk_total_bytes),
                sum(disk_percent * sample_count) / NULLIF(sum(sample_count) FILTER (WHERE disk_percent IS NOT NULL), 0),
                sum(net_rx_bytes_per_sec * sample_count) / NULLIF(sum(sample_count) FILTER (WHERE net_rx_bytes_per_sec IS NOT NULL), 0),
                sum(net_tx_bytes_per_sec * sample_count) / NULLIF(sum(sample_count) FILTER (WHERE net_tx_bytes_per_sec IS NOT NULL), 0),
                max(uptime_sec)
         FROM computer_metric_samples
         WHERE resolution_secs = $4 AND ts < $1
         GROUP BY computer_kind, computer_id, 3
         ON CONFLICT (computer_kind, computer_id, resolution_secs, ts) DO NOTHING",
    )
    .bind(raw_cut)
    .bind(ROLLUP_SECS as f64)
    .bind(ROLLUP_SECS as i32)
    .bind(RAW_SECS as i32)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    let raw_deleted = sqlx::query("DELETE FROM computer_metric_samples WHERE resolution_secs = $1 AND ts < $2")
        .bind(RAW_SECS as i32)
        .bind(raw_cut)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    let rollups_deleted = sqlx::query("DELETE FROM computer_metric_samples WHERE resolution_secs = $1 AND ts < $2")
        .bind(ROLLUP_SECS as i32)
        .bind(rollup_cut)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    tx.commit().await?;
    Ok(SweepCounts { rolled_up: rolled, raw_deleted, rollups_deleted })
}

pub fn start_metrics_sweeper(state: Arc<ApiState>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(SWEEP_EVERY).await;
            if let Err(e) = sweep(&state.db, Utc::now()).await {
                tracing::warn!("computer metrics sweep: {e}");
            }
        }
    });
}

#[cfg(test)]
#[path = "computer_metrics_tests.rs"]
mod tests;
