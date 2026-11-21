//! `GET /v1/host/snapshot`: CPU, memory, disks, network and OS in one read.
//!
//! CPU use and network throughput are rates, so they need two readings.
//! [`Sampler`] keeps the previous reading: a long-lived caller (the 60 s
//! heartbeat) makes one `Sampler` and calls [`Sampler::sample`] each tick, and
//! every tick reports the average since the last one at no extra cost.
//! [`snapshot`] is the one-shot form: it takes two readings ~200 ms apart.
//!
//! Both block the calling thread (sysinfo is synchronous); async callers wrap
//! them in `tokio::task::spawn_blocking`.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use sysinfo::{Disks, Networks, System, IS_SUPPORTED_SYSTEM, MINIMUM_CPU_UPDATE_INTERVAL};

use crate::os::{is_hidden_mount, os_info, pick_primary_disk, system_drive};
use crate::{CpuInfo, DiskInfo, HostSnapshot, MemoryInfo, NetRate, ProbeError, Result};

/// Gap between the two readings [`snapshot`] takes. sysinfo needs at least
/// [`MINIMUM_CPU_UPDATE_INTERVAL`] between CPU refreshes; the larger wins.
pub const SNAPSHOT_INTERVAL: Duration = Duration::from_millis(200);

/// Cumulative per-interface byte counters: name -> (rx total, tx total).
pub type NetCounters = HashMap<String, (u64, u64)>;

/// Keeps the previous reading so CPU and network rates come from two points.
///
/// Calls to [`Sampler::sample`] should be at least
/// [`MINIMUM_CPU_UPDATE_INTERVAL`] apart (200 ms on most hosts); closer calls
/// give a noisy CPU figure.
pub struct Sampler {
    sys: System,
    networks: Networks,
    disks: Disks,
    prev_net: NetCounters,
    prev_at: Instant,
}

impl Sampler {
    /// Takes the first reading. Returns [`ProbeError::Unsupported`] on a host
    /// that is not Linux, macOS or Windows.
    pub fn new() -> Result<Self> {
        if !IS_SUPPORTED_SYSTEM {
            return Err(ProbeError::Unsupported(format!(
                "snapshot on {}",
                std::env::consts::OS
            )));
        }
        // Fails with Unsupported on any OS family we do not handle.
        os_info()?;
        let mut sys = System::new();
        // Full CPU refresh once so the CPU list and brand are filled in.
        sys.refresh_cpu();
        let networks = Networks::new_with_refreshed_list();
        let prev_net = read_counters(&networks);
        Ok(Self {
            sys,
            networks,
            disks: Disks::new(),
            prev_net,
            prev_at: Instant::now(),
        })
    }

    /// Takes a reading and reports rates since the previous one.
    pub fn sample(&mut self) -> Result<HostSnapshot> {
        let os = os_info()?;

        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();

        // refresh_list picks up interfaces and mounts that appeared since the
        // last tick (docker bridges, USB disks); it also reads the counters.
        self.networks.refresh_list();
        self.networks.refresh();
        let now = Instant::now();
        let cur_net = read_counters(&self.networks);
        let net = net_rate(&self.prev_net, &cur_net, now.duration_since(self.prev_at));
        self.prev_net = cur_net;
        self.prev_at = now;

        self.disks.refresh_list();
        let disk = build_disks(self.disks.iter().map(|d| RawDisk {
            mount: d.mount_point().to_string_lossy().into_owned(),
            fs: d.file_system().to_string_lossy().into_owned(),
            total_bytes: d.total_space(),
            available_bytes: d.available_space(),
        }));
        let primary_disk = pick_primary_disk(
            &disk.iter().map(|d| d.mount.as_str()).collect::<Vec<_>>(),
            os.family,
            system_drive().as_deref(),
        )
        .map(|i| disk[i].mount.clone());

        let model = self
            .sys
            .cpus()
            .first()
            .map(|c| c.brand().trim().to_string())
            .filter(|b| !b.is_empty());
        let cpu = CpuInfo {
            percent: clamp_percent(self.sys.global_cpu_info().cpu_usage() as f64),
            cores: self.sys.cpus().len() as u32,
            load1: load1(),
            model,
        };

        Ok(HostSnapshot {
            sampled_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            uptime_sec: System::uptime(),
            os,
            cpu,
            memory: MemoryInfo {
                used_bytes: self.sys.used_memory(),
                total_bytes: self.sys.total_memory(),
            },
            disk,
            primary_disk,
            net,
        })
    }
}

/// One-shot snapshot: two readings [`SNAPSHOT_INTERVAL`] apart (~200 ms).
/// For repeated sampling keep a [`Sampler`] instead.
pub fn snapshot() -> Result<HostSnapshot> {
    let mut sampler = Sampler::new()?;
    std::thread::sleep(SNAPSHOT_INTERVAL.max(MINIMUM_CPU_UPDATE_INTERVAL));
    sampler.sample()
}

fn read_counters(networks: &Networks) -> NetCounters {
    networks
        .iter()
        .map(|(name, data)| {
            (
                name.clone(),
                (data.total_received(), data.total_transmitted()),
            )
        })
        .collect()
}

#[cfg(not(target_os = "windows"))]
fn load1() -> Option<f64> {
    Some(round_to(System::load_average().one, 100.0))
}

#[cfg(target_os = "windows")]
fn load1() -> Option<f64> {
    // Windows has no load average; sysinfo returns zeros there.
    None
}

/// Loopback interfaces: `lo` (Linux), `lo0` (macOS), "Loopback
/// Pseudo-Interface 1" (Windows).
pub fn is_loopback(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if lower.contains("loopback") {
        return true;
    }
    match lower.strip_prefix("lo") {
        Some(rest) => rest.chars().all(|c| c.is_ascii_digit()),
        None => false,
    }
}

/// Bytes per second summed over non-loopback interfaces present in both
/// readings. A counter that went down (interface reset, driver reload)
/// contributes 0 for that interval instead of a huge bogus number. Zero
/// elapsed time gives 0.
pub fn net_rate(prev: &NetCounters, cur: &NetCounters, elapsed: Duration) -> NetRate {
    let secs = elapsed.as_secs_f64();
    if secs <= 0.0 {
        return NetRate::default();
    }
    let (mut rx, mut tx) = (0u64, 0u64);
    for (name, &(cur_rx, cur_tx)) in cur {
        if is_loopback(name) {
            continue;
        }
        let Some(&(prev_rx, prev_tx)) = prev.get(name) else {
            continue;
        };
        rx = rx.saturating_add(cur_rx.checked_sub(prev_rx).unwrap_or(0));
        tx = tx.saturating_add(cur_tx.checked_sub(prev_tx).unwrap_or(0));
    }
    NetRate {
        rx_bytes_per_sec: round_to(rx as f64 / secs, 10.0),
        tx_bytes_per_sec: round_to(tx as f64 / secs, 10.0),
    }
}

/// A volume as sysinfo reports it, before filtering.
#[derive(Debug, Clone)]
pub struct RawDisk {
    pub mount: String,
    pub fs: String,
    pub total_bytes: u64,
    pub available_bytes: u64,
}

/// Drops pseudo/hidden mounts and zero-size volumes, keeps the first entry
/// per mount point, and computes used = total - available.
pub fn build_disks(raw: impl IntoIterator<Item = RawDisk>) -> Vec<DiskInfo> {
    let mut out: Vec<DiskInfo> = Vec::new();
    for d in raw {
        if d.total_bytes == 0 || is_hidden_mount(&d.mount, &d.fs) {
            continue;
        }
        if out.iter().any(|o| o.mount == d.mount) {
            continue;
        }
        out.push(DiskInfo {
            used_bytes: d.total_bytes.saturating_sub(d.available_bytes),
            total_bytes: d.total_bytes,
            fs: Some(d.fs).filter(|f| !f.is_empty()),
            mount: d.mount,
        });
    }
    out
}

fn clamp_percent(p: f64) -> f64 {
    if p.is_finite() {
        round_to(p.clamp(0.0, 100.0), 10.0)
    } else {
        0.0
    }
}

fn round_to(v: f64, scale: f64) -> f64 {
    (v * scale).round() / scale
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counters(items: &[(&str, u64, u64)]) -> NetCounters {
        items
            .iter()
            .map(|(n, rx, tx)| (n.to_string(), (*rx, *tx)))
            .collect()
    }

    #[test]
    fn rate_sums_interfaces_over_elapsed_time() {
        let prev = counters(&[("eth0", 1_000, 500), ("wlan0", 0, 0)]);
        let cur = counters(&[("eth0", 3_000, 1_500), ("wlan0", 1_000, 0)]);
        let r = net_rate(&prev, &cur, Duration::from_secs(2));
        assert_eq!(r.rx_bytes_per_sec, 1_500.0);
        assert_eq!(r.tx_bytes_per_sec, 500.0);
    }

    #[test]
    fn rate_skips_loopback_new_and_reset_interfaces() {
        let prev = counters(&[("lo", 0, 0), ("lo0", 0, 0), ("en0", 10_000, 10_000)]);
        let cur = counters(&[
            ("lo", 9_999_999, 9_999_999),
            ("lo0", 9_999_999, 9_999_999),
            // counter reset: went down
            ("en0", 100, 200),
            // appeared this tick: no previous point
            ("docker0", 5_000, 5_000),
        ]);
        let r = net_rate(&prev, &cur, Duration::from_secs(1));
        assert_eq!(r, NetRate::default());
    }

    #[test]
    fn rate_with_zero_elapsed_is_zero() {
        let c = counters(&[("eth0", 100, 100)]);
        assert_eq!(
            net_rate(&HashMap::new(), &c, Duration::ZERO),
            NetRate::default()
        );
    }

    #[test]
    fn rate_rounds_to_one_decimal() {
        let prev = counters(&[("eth0", 0, 0)]);
        let cur = counters(&[("eth0", 1_000, 1)]);
        let r = net_rate(&prev, &cur, Duration::from_millis(300));
        assert_eq!(r.rx_bytes_per_sec, 3_333.3);
        assert_eq!(r.tx_bytes_per_sec, 3.3);
    }

    #[test]
    fn loopback_names() {
        assert!(is_loopback("lo"));
        assert!(is_loopback("lo0"));
        assert!(is_loopback("Loopback Pseudo-Interface 1"));
        assert!(!is_loopback("eth0"));
        assert!(!is_loopback("en0"));
        assert!(!is_loopback("local-bridge"));
    }

    #[test]
    fn disks_filtered_deduped_and_used_computed() {
        let raw = vec![
            RawDisk {
                mount: "/".into(),
                fs: "ext4".into(),
                total_bytes: 100,
                available_bytes: 40,
            },
            RawDisk {
                mount: "/run".into(),
                fs: "tmpfs".into(),
                total_bytes: 10,
                available_bytes: 10,
            },
            RawDisk {
                mount: "/snap/core22/1".into(),
                fs: "squashfs".into(),
                total_bytes: 5,
                available_bytes: 0,
            },
            RawDisk {
                mount: "/".into(),
                fs: "ext4".into(),
                total_bytes: 999,
                available_bytes: 0,
            },
            RawDisk {
                mount: "/mnt/empty".into(),
                fs: "".into(),
                total_bytes: 0,
                available_bytes: 0,
            },
            RawDisk {
                mount: "/data".into(),
                fs: "".into(),
                total_bytes: 50,
                available_bytes: 80,
            },
        ];
        let d = build_disks(raw);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].mount, "/");
        assert_eq!(d[0].used_bytes, 60);
        assert_eq!(d[0].total_bytes, 100);
        assert_eq!(d[0].fs.as_deref(), Some("ext4"));
        assert_eq!(d[1].mount, "/data");
        assert_eq!(d[1].used_bytes, 0, "available > total saturates to 0");
        assert_eq!(d[1].fs, None);
    }

    #[test]
    fn percent_clamped_and_rounded() {
        assert_eq!(clamp_percent(12.345), 12.3);
        assert_eq!(clamp_percent(-1.0), 0.0);
        assert_eq!(clamp_percent(140.0), 100.0);
        assert_eq!(clamp_percent(f64::NAN), 0.0);
    }

    /// Live read on the build host: shape checks only, values vary.
    #[test]
    fn live_snapshot_has_sane_shape() {
        match snapshot() {
            Ok(s) => {
                assert!(s.cpu.cores > 0);
                assert!(s.memory.total_bytes > 0);
                assert!(s.memory.used_bytes <= s.memory.total_bytes);
                assert!((0.0..=100.0).contains(&s.cpu.percent));
                assert!(!s.os.arch.is_empty());
                assert!(s.sampled_at.ends_with('Z'));
                if let Some(p) = &s.primary_disk {
                    assert!(s.disk.iter().any(|d| &d.mount == p));
                }
            }
            Err(ProbeError::Unsupported(_)) => {}
            Err(e) => panic!("snapshot failed: {e}"),
        }
    }
}
