//! Computers console list (surfaces/docs/dev/computers-console-contract.mdx,
//! cloud route `GET /api/v1/computers/console`, step 2.1).
//!
//! One list of every computer the caller can use, as `ConsoleComputer` rows
//! (allternit-ai `src/lib/computers/console-types.ts`, camelCase JSON):
//!
//! - `cloud`: the caller's own cloud computers (`provisioned_instances`, not
//!   deleted). Owner-scoped, the same rule as `GET /api/v1/provisioned-instances`.
//! - `paired`: computers paired with a code (`paired_computers`), the
//!   caller's own and those shared through the caller's organization (the
//!   `paired_computers.rs` `list_paired` rule), plus runtime devices paired
//!   with `gizzi pair` / `ao fabric pair` (`runtime_type` `vps` or `ao`).
//! - `desktop`: the caller's Allternit Desktop runtimes (`runtime_type`
//!   `desktop`).
//!
//! A `paired_computers` row and the runtime device of the same machine are
//! one row (pairing doc D7): linked by `paired_computers.runtime_device_id`
//! (migration 153), or, until that is filled in, by the same owner and name.
//!
//! The route only reads the database and the relay hub's in-memory presence.
//! It never calls a provider and never wakes an asleep computer. `lastSample`
//! is the newest raw heartbeat sample of the machine (routes::computer_metrics,
//! kept 48 hours): of the cloud computer's runtime device or self-paired row,
//! of the paired row or its linked runtime device, or of the runtime device.
//! `runtimeId` (step 2.3) is the runtime device the relay is keyed by, null
//! when the machine has none; relay clients (terminal, host data) use it.
//! `servicesCount` is left out until host data exists; `lastBackupAt` until
//! Orgo daily backups (step 2.6).

use axum::{
    extract::State,
    http::HeaderMap,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use std::collections::HashSet;
use std::sync::Arc;

use crate::routes::paired_computers::{is_mesh_ip, signed_in_caller};
use crate::{ApiError, ApiState};

/// A heartbeat older than this reads offline (the runtime device list uses
/// the same window).
const ONLINE_WINDOW_MINUTES: i64 = 10;

pub const GROUP_DESK: &str = "On your desk";
pub const GROUP_CLOUD: &str = "Allternit Cloud";
pub const GROUP_SERVERS: &str = "Your servers";
pub const GROUP_SHARED: &str = "Shared with you";

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new().route("/api/v1/computers/console", get(list_console))
}

/// One console row. Field names follow `ConsoleComputer` in console-types.ts;
/// optional fields are left out when unknown.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsoleComputer {
    pub id: String,
    pub kind: &'static str,
    pub group: String,
    pub name: String,
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_logo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    pub os: String,
    pub os_family: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    pub reach: &'static str,
    pub status: String,
    /// The runtime device the relay is keyed by (`runtime_devices.id`): a
    /// cloud computer's `device_id`, a paired computer's `runtime_device_id`
    /// or name-matched device, a Desktop's own id. Always sent, null when
    /// there is none. Terminal and other relay clients address the machine
    /// by this, not by `id`.
    pub runtime_id: Option<String>,
    /// `HostSnapshot`: the newest heartbeat sample, when one is kept.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_sample: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub services_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_backup_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CloudRow {
    pub id: String,
    pub status: String,
    pub device_id: Option<String>,
    pub device_name: Option<String>,
    pub tailnet_ip: Option<String>,
    pub location: Option<String>,
    pub host_region: Option<String>,
    /// Newest raw sample (`HostSnapshot`) and when it was stored.
    #[sqlx(default)]
    pub last_sample: Option<serde_json::Value>,
    #[sqlx(default)]
    pub last_sample_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PairedRow {
    pub id: String,
    pub name: String,
    pub os: Option<String>,
    pub mesh_ip: Option<String>,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub runtime_device_id: Option<String>,
    /// Paired by someone else in the caller's organization.
    pub shared: bool,
    #[sqlx(default)]
    pub last_sample: Option<serde_json::Value>,
    #[sqlx(default)]
    pub last_sample_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DeviceRow {
    pub id: String,
    pub name: Option<String>,
    pub runtime_type: Option<String>,
    pub hostname: Option<String>,
    pub platform: Option<String>,
    pub tailnet_ip: Option<String>,
    pub status: Option<String>,
    pub last_seen_at: Option<DateTime<Utc>>,
    #[sqlx(default)]
    pub last_sample: Option<serde_json::Value>,
    #[sqlx(default)]
    pub last_sample_at: Option<DateTime<Utc>>,
}

async fn list_console(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Result<Response, ApiError> {
    let caller = signed_in_caller(&state, &headers).await?;
    let cloud: Vec<CloudRow> = sqlx::query_as(
        "SELECT pi.id, pi.status, pi.device_id, d.name AS device_name, d.tailnet_ip,
                pi.location, h.region AS host_region, ls.snapshot AS last_sample, ls.ts AS last_sample_at
         FROM provisioned_instances pi
         LEFT JOIN provisioned_hosts h ON h.id = pi.host_id
         LEFT JOIN runtime_devices d ON d.id = pi.device_id
         LEFT JOIN LATERAL (
             SELECT s.snapshot, s.ts FROM computer_metric_samples s
             WHERE s.resolution_secs = 60 AND s.snapshot IS NOT NULL
               AND ((s.computer_kind = 'device' AND s.computer_id = pi.device_id)
                 OR (s.computer_kind = 'paired' AND s.computer_id IN
                       (SELECT spc.id FROM paired_computers spc WHERE spc.provisioned_instance_id = pi.id)))
             ORDER BY s.ts DESC LIMIT 1
         ) ls ON true
         WHERE pi.user_id = $1 AND pi.status <> 'deleted'
         ORDER BY pi.created_at",
    )
    .bind(&caller.user_id)
    .fetch_all(&state.db)
    .await?;
    let paired: Vec<PairedRow> = sqlx::query_as(
        "SELECT pc.id, pc.name, pc.os, pc.mesh_ip, pc.last_seen_at, pc.runtime_device_id, pc.user_id <> $1 AS shared,
                ls.snapshot AS last_sample, ls.ts AS last_sample_at
         FROM paired_computers pc
         LEFT JOIN LATERAL (
             SELECT s.snapshot, s.ts FROM computer_metric_samples s
             WHERE s.computer_kind = 'paired' AND s.computer_id = pc.id
               AND s.resolution_secs = 60 AND s.snapshot IS NOT NULL
             ORDER BY s.ts DESC LIMIT 1
         ) ls ON true
         WHERE (pc.user_id = $1 OR (pc.organization_id IS NOT NULL AND pc.organization_id = $2))
           AND pc.provisioned_instance_id IS NULL
         ORDER BY pc.created_at",
    )
    .bind(&caller.user_id)
    .bind(&caller.organization_id)
    .fetch_all(&state.db)
    .await?;
    let devices: Vec<DeviceRow> = sqlx::query_as(
        "SELECT d.id, d.name, d.runtime_type, d.hostname, d.platform, d.tailnet_ip, d.status, d.last_seen_at,
                ls.snapshot AS last_sample, ls.ts AS last_sample_at
         FROM runtime_devices d
         LEFT JOIN LATERAL (
             SELECT s.snapshot, s.ts FROM computer_metric_samples s
             WHERE s.computer_kind = 'device' AND s.computer_id = d.id
               AND s.resolution_secs = 60 AND s.snapshot IS NOT NULL
             ORDER BY s.ts DESC LIMIT 1
         ) ls ON true
         WHERE d.user_id = $1 AND d.revoked_at IS NULL
           AND d.runtime_type IN ('desktop', 'vps', 'ao')
           AND NOT EXISTS (SELECT 1 FROM provisioned_instances pi WHERE pi.device_id = d.id)
         ORDER BY d.created_at",
    )
    .bind(&caller.user_id)
    .fetch_all(&state.db)
    .await?;
    let connected = crate::routes::runtime_relay::connected_runtime_ids().await;
    let computers = build_console(cloud, paired, devices, &connected, Utc::now());
    Ok(Json(serde_json::json!({ "computers": computers })).into_response())
}

/// Join the three sources into console rows: cloud first, then desks, then
/// servers, each in creation order.
pub fn build_console(
    cloud: Vec<CloudRow>,
    paired: Vec<PairedRow>,
    devices: Vec<DeviceRow>,
    connected: &HashSet<String>,
    now: DateTime<Utc>,
) -> Vec<ConsoleComputer> {
    let mut rows: Vec<ConsoleComputer> = cloud.into_iter().map(|c| cloud_computer(c, connected)).collect();
    let mut linked: HashSet<String> = HashSet::new();
    let mut desk = Vec::new();
    let mut servers = Vec::new();
    for pc in paired {
        let device = linked_device(&pc, &devices, &linked);
        if let Some(d) = device {
            linked.insert(d.id.clone());
        }
        let row = paired_computer(pc, device, connected, now);
        if row.kind == "desktop" { desk.push(row) } else { servers.push(row) }
    }
    for d in devices.iter().filter(|d| !linked.contains(&d.id)) {
        let row = device_computer(d, connected, now);
        if row.kind == "desktop" { desk.push(row) } else { servers.push(row) }
    }
    rows.extend(desk);
    rows.extend(servers);
    rows
}

/// The runtime device of the same machine: the stored link, else (own rows
/// only, link not stored yet) the first unlinked device with the same name.
fn linked_device<'a>(pc: &PairedRow, devices: &'a [DeviceRow], taken: &HashSet<String>) -> Option<&'a DeviceRow> {
    if let Some(id) = &pc.runtime_device_id {
        return devices.iter().find(|d| &d.id == id);
    }
    if pc.shared {
        return None;
    }
    devices.iter().find(|d| {
        !taken.contains(&d.id) && d.name.as_deref().is_some_and(|n| n.trim().eq_ignore_ascii_case(pc.name.trim()))
    })
}

fn cloud_computer(c: CloudRow, connected: &HashSet<String>) -> ConsoleComputer {
    let online = c.device_id.as_ref().is_some_and(|id| connected.contains(id));
    let runtime_id = c.device_id.clone();
    let address = c.tailnet_ip.filter(|ip| is_mesh_ip(ip));
    ConsoleComputer {
        name: c.device_name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "Cloud computer".to_string()),
        id: c.id,
        kind: "cloud",
        group: GROUP_CLOUD.to_string(),
        provider: "Allternit Cloud".to_string(),
        provider_logo: Some("allternit".to_string()),
        region: c.location.or(c.host_region).filter(|r| !r.trim().is_empty()),
        // Cloud images are Ubuntu only (gate G5).
        os: "Ubuntu".to_string(),
        os_family: "linux",
        reach: if address.is_some() { "mesh" } else { "relay" },
        address,
        status: cloud_status(&c.status, online).to_string(),
        runtime_id,
        last_sample: c.last_sample,
        services_count: None,
        last_backup_at: None,
    }
}

/// Console status for a cloud computer, from the instance state machine
/// (`provisioned_instances.status`) and whether its runtime holds a relay.
pub fn cloud_status(status: &str, runtime_online: bool) -> &'static str {
    match status {
        "running" if runtime_online => "online",
        "running" => "offline",
        "sleeping" => "asleep",
        "waking" | "provisioning" => "starting",
        "stopped" => "stopped",
        "suspended" => "suspended",
        "error" => "error",
        _ => "unknown",
    }
}

fn recent(seen: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    seen.is_some_and(|t| t > now - Duration::minutes(ONLINE_WINDOW_MINUTES))
}

fn device_online(d: &DeviceRow, connected: &HashSet<String>, now: DateTime<Utc>) -> bool {
    connected.contains(&d.id) || (d.status.as_deref() == Some("online") && recent(d.last_seen_at, now))
}

fn paired_computer(
    pc: PairedRow,
    device: Option<&DeviceRow>,
    connected: &HashSet<String>,
    now: DateTime<Utc>,
) -> ConsoleComputer {
    let online = recent(pc.last_seen_at, now) || device.is_some_and(|d| device_online(d, connected, now));
    let is_desktop = device.is_some_and(|d| d.runtime_type.as_deref() == Some("desktop"));
    let os_source = pc.os.clone().or_else(|| device.and_then(|d| d.platform.clone()));
    let (os, os_family) = os_label(os_source.as_deref());
    let address = pc
        .mesh_ip
        .clone()
        .filter(|ip| is_mesh_ip(ip))
        .or_else(|| device.and_then(|d| d.tailnet_ip.clone()).filter(|ip| is_mesh_ip(ip)));
    let last_sample = newest(
        (pc.last_sample.clone(), pc.last_sample_at),
        device.map(|d| (d.last_sample.clone(), d.last_sample_at)).unwrap_or_default(),
    );
    let group = if is_desktop {
        GROUP_DESK
    } else if pc.shared {
        GROUP_SHARED
    } else {
        GROUP_SERVERS
    };
    // A shared row's linked device is not in the caller's own device list,
    // but its stored link still names the runtime.
    let runtime_id = device.map(|d| d.id.clone()).or_else(|| pc.runtime_device_id.clone());
    ConsoleComputer {
        id: pc.id,
        kind: if is_desktop { "desktop" } else { "paired" },
        group: group.to_string(),
        name: pc.name,
        provider: (if is_desktop { "Allternit Desktop" } else { "Paired" }).to_string(),
        provider_logo: None,
        region: None,
        os,
        os_family,
        reach: if address.is_some() { "mesh" } else { "relay" },
        address,
        status: (if online { "online" } else { "offline" }).to_string(),
        runtime_id,
        last_sample,
        services_count: None,
        last_backup_at: None,
    }
}

type Sample = (Option<serde_json::Value>, Option<DateTime<Utc>>);

/// The newer of two `(sample, stored at)` pairs; a missing one loses.
fn newest(a: Sample, b: Sample) -> Option<serde_json::Value> {
    match (a, b) {
        ((Some(x), ta), (Some(y), tb)) => Some(if tb > ta { y } else { x }),
        ((x, _), (y, _)) => x.or(y),
    }
}

fn device_computer(d: &DeviceRow, connected: &HashSet<String>, now: DateTime<Utc>) -> ConsoleComputer {
    let is_desktop = d.runtime_type.as_deref() == Some("desktop");
    let (os, os_family) = os_label(d.platform.as_deref());
    let address = d.tailnet_ip.clone().filter(|ip| is_mesh_ip(ip));
    let name = d
        .name
        .clone()
        .or_else(|| d.hostname.clone())
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| (if is_desktop { "Desktop" } else { "Computer" }).to_string());
    ConsoleComputer {
        id: d.id.clone(),
        kind: if is_desktop { "desktop" } else { "paired" },
        group: (if is_desktop { GROUP_DESK } else { GROUP_SERVERS }).to_string(),
        name,
        provider: (if is_desktop { "Allternit Desktop" } else { "Paired" }).to_string(),
        provider_logo: None,
        region: None,
        os,
        os_family,
        reach: if address.is_some() { "mesh" } else { "relay" },
        address,
        status: (if device_online(d, connected, now) { "online" } else { "offline" }).to_string(),
        runtime_id: Some(d.id.clone()),
        last_sample: d.last_sample.clone(),
        services_count: None,
        last_backup_at: None,
    }
}

/// Display name and family for a free-form OS string (`paired_computers.os`,
/// or a runtime's `platform` such as `darwin`, `win32`, `linux`).
pub fn os_label(raw: Option<&str>) -> (String, &'static str) {
    let raw = raw.map(str::trim).filter(|s| !s.is_empty());
    let Some(raw) = raw else { return ("Unknown".to_string(), "linux") };
    let lower = raw.to_ascii_lowercase();
    if lower == "darwin" || lower == "macos" || lower == "mac" {
        ("macOS".to_string(), "macos")
    } else if lower == "win32" || lower == "windows" {
        ("Windows".to_string(), "windows")
    } else if lower == "linux" {
        ("Linux".to_string(), "linux")
    } else if lower.contains("mac") || lower.contains("darwin") {
        (raw.to_string(), "macos")
    } else if lower.contains("win") {
        (raw.to_string(), "windows")
    } else {
        (raw.to_string(), "linux")
    }
}

#[cfg(test)]
#[path = "computers_console_tests.rs"]
mod tests;
