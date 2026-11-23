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
//! One row per machine (step 2.10, surfaces/docs/dev/computer-pairing.mdx):
//! the paired rows and runtime devices of one computer (Desktop, `gizzi
//! pair`, `ao fabric pair`, `allternit computer pair`) are grouped by machine
//! key when the list is read (`build_console`), and the other ids of the
//! machine are sent as `aliases`. Nothing is merged in the database.
//! A name the owner gave the machine (`computer_machine_names`, migration
//! 159) replaces the reported one.
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
    /// Every other id of the same machine (step 2.10): its other runtime
    /// devices and paired rows, a cloud computer's runtime device. Always
    /// sent. Any console route given one of these answers for this row.
    pub aliases: Vec<String>,
    /// The code-paired row of the machine (`paired_computers.id`), for the
    /// screen-share and Factory peer routes. Left out when there is none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paired_id: Option<String>,
    /// Whole-disk file roots (step 2.11, `computer_file_roots`): filled by
    /// `load_console`, left out only by the pure `build_console`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub whole_disk: Option<crate::routes::computer_file_roots::WholeDisk>,
    /// `mid:<machine id>`, the hostname key, `cloud:<id>`, `paired:<id>`
    /// (shared) or `id:<id>`: what names and unpair are keyed by.
    #[serde(skip)]
    pub machine_key: String,
    #[serde(skip)]
    pub member_devices: Vec<String>,
    #[serde(skip)]
    pub member_paired: Vec<String>,
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
    /// What the machine sent at redeem time (migration 159), NULL before.
    #[sqlx(default)]
    pub machine_id: Option<String>,
    #[sqlx(default)]
    pub hostname: Option<String>,
    #[sqlx(default)]
    pub platform: Option<String>,
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
    /// ~/.allternit/machine-id as sent at pairing (migration 119).
    #[sqlx(default)]
    pub machine_id: Option<String>,
}

async fn list_console(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Result<Response, ApiError> {
    let caller = signed_in_caller(&state, &headers).await?;
    let computers = load_console(&state.db, &caller.user_id, caller.organization_id.as_deref()).await?;
    Ok(Json(serde_json::json!({ "computers": computers })).into_response())
}

/// Every console row `user_id` can see (the route above; also the alert
/// evaluator, routes::computer_alerts). `organization_id` adds the paired
/// computers shared through the person's organization.
pub async fn load_console(db: &sqlx::PgPool, user_id: &str, organization_id: Option<&str>) -> Result<Vec<ConsoleComputer>, sqlx::Error> {
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
    .bind(user_id)
    .fetch_all(db)
    .await?;
    let paired: Vec<PairedRow> = sqlx::query_as(
        "SELECT pc.id, pc.name, pc.os, pc.mesh_ip, pc.last_seen_at, pc.runtime_device_id, pc.user_id <> $1 AS shared,
                pc.machine_id, pc.hostname, pc.platform, ls.snapshot AS last_sample, ls.ts AS last_sample_at
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
    .bind(user_id)
    .bind(organization_id)
    .fetch_all(db)
    .await?;
    let devices: Vec<DeviceRow> = sqlx::query_as(
        "SELECT d.id, d.name, d.runtime_type, d.hostname, d.platform, d.tailnet_ip, d.status, d.last_seen_at,
                d.machine_id, ls.snapshot AS last_sample, ls.ts AS last_sample_at
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
    .bind(user_id)
    .fetch_all(db)
    .await?;
    let names: Vec<(String, String)> =
        sqlx::query_as("SELECT machine_key, name FROM computer_machine_names WHERE user_id = $1")
            .bind(user_id)
            .fetch_all(db)
            .await?;
    let connected = crate::routes::runtime_relay::connected_runtime_ids().await;
    let mut rows = build_console(cloud, paired, devices, &connected, Utc::now());
    apply_names(&mut rows, &names);
    crate::routes::computer_file_roots::apply_whole_disk(db, user_id, &mut rows).await;
    Ok(rows)
}

/// The owner's own names for machines (migration 159), by machine key.
/// Shared rows keep the name their owner gave them.
pub fn apply_names(rows: &mut [ConsoleComputer], names: &[(String, String)]) {
    for row in rows.iter_mut().filter(|r| r.group != GROUP_SHARED) {
        if let Some((_, name)) = names.iter().find(|(key, _)| *key == row.machine_key) {
            row.name = name.clone();
        }
    }
}

/// Hostnames too common to say, without a machine id, that two runtimes are
/// one machine (default VPS and image names). Rows on them group only by
/// machine id or a stored link.
const GENERIC_HOSTNAMES: &[&str] = &[
    "localhost", "ubuntu", "debian", "raspberrypi", "vps", "server", "linux", "fedora", "centos", "computer",
    "host", "box", "node", "machine", "default",
];

/// `mid:<machine id>` (step 2.10): the id in ~/.allternit/machine-id that
/// Desktop, `gizzi pair`, `ao fabric pair` and `allternit computer pair` send.
pub fn machine_id_key(machine_id: Option<&str>) -> Option<String> {
    machine_id.map(str::trim).filter(|m| !m.is_empty()).map(|m| format!("mid:{m}"))
}

/// The hostname key the active-machine cap uses for rows without a machine
/// id (`quota_service::machine_key`: lower case, `.local` dropped, then
/// `|platform`), or None for no hostname or a generic one.
pub fn host_key(hostname: Option<&str>, platform: Option<&str>) -> Option<String> {
    let key = crate::services::quota_service::machine_key(hostname, platform)?;
    let host = key.split('|').next().unwrap_or_default();
    (!GENERIC_HOSTNAMES.contains(&host)).then_some(key)
}

/// One machine: the paired rows and runtime devices that are the same
/// computer (indexes into the inputs of `build_console`).
#[derive(Default)]
struct Machine {
    paired: Vec<usize>,
    devices: Vec<usize>,
    mid: Option<String>,
    hosts: Vec<String>,
}

impl Machine {
    fn add_host(&mut self, host: Option<String>) {
        if let Some(h) = host {
            if !self.hosts.contains(&h) {
                self.hosts.push(h);
            }
        }
    }
}

/// Join the three sources into console rows, one row per machine (step
/// 2.10): cloud first, then desks, then servers, each in creation order.
///
/// Grouping, in this order:
/// 1. runtime devices with the same machine id are one machine;
/// 2. a device without one joins the first machine on the same (non-generic)
///    hostname + platform, else machines without ids group by that key;
/// 3. a paired row (own rows only) joins the machine of its stored
///    `runtime_device_id`, else the one with its machine id, else its
///    hostname key, else (link not stored yet) the first machine with a
///    device of the same name and no paired row. A shared row is always its
///    own machine.
///
/// The row id is the paired row's id when the machine has one (as before
/// 2.10), else its oldest device's id. Every other member id is in
/// `aliases`, and every console route resolves any of them to this row.
pub fn build_console(
    cloud: Vec<CloudRow>,
    paired: Vec<PairedRow>,
    devices: Vec<DeviceRow>,
    connected: &HashSet<String>,
    now: DateTime<Utc>,
) -> Vec<ConsoleComputer> {
    let mut rows: Vec<ConsoleComputer> = cloud.into_iter().map(|c| cloud_computer(c, connected)).collect();
    let mut machines: Vec<Machine> = Vec::new();
    let mut of_device: Vec<usize> = vec![usize::MAX; devices.len()];

    for (i, d) in devices.iter().enumerate() {
        let Some(mid) = machine_id_key(d.machine_id.as_deref()) else { continue };
        let m = match machines.iter().position(|m| m.mid.as_deref() == Some(mid.as_str())) {
            Some(m) => m,
            None => {
                machines.push(Machine { mid: Some(mid), ..Default::default() });
                machines.len() - 1
            }
        };
        machines[m].devices.push(i);
        machines[m].add_host(host_key(d.hostname.as_deref(), d.platform.as_deref()));
        of_device[i] = m;
    }
    for (i, d) in devices.iter().enumerate() {
        if of_device[i] != usize::MAX {
            continue;
        }
        let host = host_key(d.hostname.as_deref(), d.platform.as_deref());
        let m = match host.as_ref().and_then(|h| machines.iter().position(|m| m.hosts.contains(h))) {
            Some(m) => m,
            None => {
                machines.push(Machine::default());
                machines.len() - 1
            }
        };
        machines[m].devices.push(i);
        machines[m].add_host(host);
        of_device[i] = m;
    }
    for (i, pc) in paired.iter().enumerate() {
        let mid = machine_id_key(pc.machine_id.as_deref());
        let host = host_key(pc.hostname.as_deref(), pc.platform.as_deref());
        let found = if pc.shared {
            None
        } else {
            pc.runtime_device_id
                .as_ref()
                .and_then(|id| devices.iter().position(|d| &d.id == id))
                .map(|d| of_device[d])
                .or_else(|| mid.as_ref().and_then(|k| machines.iter().position(|m| m.mid.as_ref() == Some(k))))
                .or_else(|| host.as_ref().and_then(|h| machines.iter().position(|m| m.hosts.contains(h))))
                .or_else(|| {
                    devices
                        .iter()
                        .enumerate()
                        .find(|(d, dev)| {
                            machines[of_device[*d]].paired.is_empty()
                                && dev.name.as_deref().is_some_and(|n| n.trim().eq_ignore_ascii_case(pc.name.trim()))
                        })
                        .map(|(d, _)| of_device[d])
                })
        };
        let m = match found {
            Some(m) => m,
            None => {
                let mut fresh = Machine::default();
                if !pc.shared {
                    fresh.mid = mid;
                    fresh.add_host(host);
                }
                machines.push(fresh);
                machines.len() - 1
            }
        };
        machines[m].paired.push(i);
    }

    // Machines with a paired row first, in paired order, then the others in
    // device order (the order before 2.10).
    let mut order: Vec<usize> = (0..machines.len()).collect();
    order.sort_by_key(|&m| {
        let mm = &machines[m];
        match mm.paired.first() {
            Some(p) => (0, *p),
            None => (1, mm.devices.first().copied().unwrap_or(usize::MAX)),
        }
    });
    let mut desk = Vec::new();
    let mut servers = Vec::new();
    for m in order {
        let pcs: Vec<&PairedRow> = machines[m].paired.iter().map(|&i| &paired[i]).collect();
        let devs: Vec<&DeviceRow> = machines[m].devices.iter().map(|&i| &devices[i]).collect();
        let key = machines[m].mid.clone().or_else(|| machines[m].hosts.first().cloned());
        let row = machine_computer(&pcs, &devs, key, connected, now);
        if row.kind == "desktop" { desk.push(row) } else { servers.push(row) }
    }
    rows.extend(desk);
    rows.extend(servers);
    rows
}

fn cloud_computer(c: CloudRow, connected: &HashSet<String>) -> ConsoleComputer {
    let online = c.device_id.as_ref().is_some_and(|id| connected.contains(id));
    let runtime_id = c.device_id.clone();
    let address = c.tailnet_ip.filter(|ip| is_mesh_ip(ip));
    ConsoleComputer {
        name: c.device_name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "Cloud computer".to_string()),
        machine_key: format!("cloud:{}", c.id),
        aliases: c.device_id.iter().cloned().collect(),
        member_devices: c.device_id.iter().cloned().collect(),
        member_paired: Vec::new(),
        paired_id: None,
        whole_disk: None,
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

/// The device the relay should use for a machine: one holding a relay
/// first, then a Desktop (it serves host data), then one that reported
/// lately; ties go to the oldest.
fn preferred_device<'a>(devs: &[&'a DeviceRow], connected: &HashSet<String>, now: DateTime<Utc>) -> Option<&'a DeviceRow> {
    let score = |d: &DeviceRow| {
        (connected.contains(&d.id) as u8) * 4
            + ((d.runtime_type.as_deref() == Some("desktop")) as u8) * 2
            + (device_online(d, connected, now) as u8)
    };
    let mut best: Option<&'a DeviceRow> = None;
    for d in devs {
        if best.map_or(true, |b| score(d) > score(b)) {
            best = Some(d);
        }
    }
    best
}

/// One console row for one machine: its paired rows (code pairing) and its
/// runtime devices (Desktop, `gizzi pair`, `ao fabric pair`).
fn machine_computer(
    pcs: &[&PairedRow],
    devs: &[&DeviceRow],
    key: Option<String>,
    connected: &HashSet<String>,
    now: DateTime<Utc>,
) -> ConsoleComputer {
    let pc = pcs.first().copied();
    let device = preferred_device(devs, connected, now);
    let shared = pc.is_some_and(|p| p.shared);
    let online = pcs.iter().any(|p| recent(p.last_seen_at, now)) || devs.iter().any(|d| device_online(d, connected, now));
    let is_desktop = devs.iter().any(|d| d.runtime_type.as_deref() == Some("desktop"));
    let os_source = pcs
        .iter()
        .find_map(|p| p.os.clone())
        .or_else(|| device.and_then(|d| d.platform.clone()))
        .or_else(|| devs.iter().find_map(|d| d.platform.clone()));
    let (os, os_family) = os_label(os_source.as_deref());
    let address = pcs
        .iter()
        .filter_map(|p| p.mesh_ip.clone())
        .chain(device.and_then(|d| d.tailnet_ip.clone()))
        .chain(devs.iter().filter_map(|d| d.tailnet_ip.clone()))
        .find(|ip| is_mesh_ip(ip));
    let last_sample = pcs
        .iter()
        .map(|p| (p.last_sample.clone(), p.last_sample_at))
        .chain(devs.iter().map(|d| (d.last_sample.clone(), d.last_sample_at)))
        .fold((None, None), |acc: Sample, s: Sample| {
            let newer = s.0.is_some() && (acc.0.is_none() || s.1 > acc.1);
            if newer { s } else { acc }
        })
        .0;
    let group = if is_desktop {
        GROUP_DESK
    } else if shared {
        GROUP_SHARED
    } else {
        GROUP_SERVERS
    };
    // A shared row's linked device is not in the caller's own device list,
    // but its stored link still names the runtime.
    let runtime_id = device.map(|d| d.id.clone()).or_else(|| pc.and_then(|p| p.runtime_device_id.clone()));
    let id = pc.map(|p| p.id.clone()).or_else(|| devs.first().map(|d| d.id.clone())).unwrap_or_default();
    let name = pc
        .map(|p| p.name.clone())
        .or_else(|| devs.iter().find_map(|d| d.name.clone().or_else(|| d.hostname.clone()).filter(|n| !n.trim().is_empty())))
        .unwrap_or_else(|| (if is_desktop { "Desktop" } else { "Computer" }).to_string());
    let member_paired: Vec<String> = pcs.iter().map(|p| p.id.clone()).collect();
    let member_devices: Vec<String> = devs.iter().map(|d| d.id.clone()).collect();
    let aliases: Vec<String> =
        member_paired.iter().chain(member_devices.iter()).filter(|a| **a != id).cloned().collect();
    let machine_key = if shared { format!("paired:{id}") } else { key.unwrap_or_else(|| format!("id:{id}")) };
    ConsoleComputer {
        kind: if is_desktop { "desktop" } else { "paired" },
        group: group.to_string(),
        name,
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
        paired_id: pc.map(|p| p.id.clone()),
        whole_disk: None,
        aliases,
        machine_key,
        member_devices,
        member_paired,
        id,
    }
}

type Sample = (Option<serde_json::Value>, Option<DateTime<Utc>>);

/// The console row `id` names for this caller: by row id or by any alias
/// (step 2.10). Console routes use it so an old id still reaches the machine.
pub async fn find_machine(
    db: &sqlx::PgPool,
    user_id: &str,
    organization_id: Option<&str>,
    id: &str,
) -> Result<Option<ConsoleComputer>, sqlx::Error> {
    let rows = load_console(db, user_id, organization_id).await?;
    Ok(rows.into_iter().find(|r| r.id == id || r.aliases.iter().any(|a| a == id)))
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
