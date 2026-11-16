//! `GET /storage`: `StorageDiscovery` (SPEC section 15) from the volumes that
//! hold this computer's roots, plus bytes per root from the index.

use std::sync::Arc;

use allternit_files_core::{ReplicaHostInfo, StorageDiscovery, Volume};
use axum::{Extension, Json};

use super::routes::run;
use super::{meta, ApiError, FilesV2, Req};

pub async fn storage(Extension(files): Extension<Arc<FilesV2>>, _req: Req) -> Result<Json<StorageDiscovery>, ApiError> {
    run(&files, move |v| {
        let paths: Vec<std::path::PathBuf> = v.drive.roots()?.into_iter().map(|r| r.local_path).collect();
        Ok(Json(StorageDiscovery {
            computer_id: v.cfg.computer_id.clone(),
            reported_at: v.drive.now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            volumes: volumes(&paths),
            root_usage: meta::root_usage(&v.cfg.data_dir.join("engine"))?,
            // Replica hosting is step 15.
            replica_host: ReplicaHostInfo { enabled: false, capacity_bytes: 0, used_bytes: 0 },
        }))
    })
    .await
}

#[cfg(unix)]
fn volumes(paths: &[std::path::PathBuf]) -> Vec<Volume> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    let mut out: Vec<(u64, Volume)> = Vec::new();
    for p in paths {
        let Ok(md) = std::fs::metadata(p) else { continue };
        let dev = md.dev();
        if out.iter().any(|(d, _)| *d == dev) {
            continue;
        }
        // The mount point: the highest ancestor still on this device.
        let mut mount = p.clone();
        while let Some(parent) = mount.parent() {
            match std::fs::metadata(parent) {
                Ok(m) if m.dev() == dev => mount = parent.to_path_buf(),
                _ => break,
            }
        }
        let Ok(c) = std::ffi::CString::new(p.as_os_str().as_bytes()) else { continue };
        // SAFETY: `s` is a plain C struct that statvfs fills in; `c` is NUL-terminated.
        let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
            continue;
        }
        let frsize = s.f_frsize as u64;
        let m = mount.to_string_lossy();
        let external = ["/Volumes/", "/media/", "/run/media/", "/mnt/"].iter().any(|pre| m.starts_with(pre));
        let label = if external {
            mount.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "External disk".into())
        } else {
            "This computer".to_string()
        };
        out.push((
            dev,
            Volume {
                volume_id: format!("dev-{dev:x}"),
                label,
                kind: if external { allternit_files_core::VolumeKind::External } else { allternit_files_core::VolumeKind::Internal },
                total_bytes: (s.f_blocks as u64).saturating_mul(frsize),
                free_bytes: (s.f_bavail as u64).saturating_mul(frsize),
                removable: external,
                encrypted: None,
            },
        ));
    }
    out.into_iter().map(|(_, v)| v).collect()
}

#[cfg(not(unix))]
fn volumes(_paths: &[std::path::PathBuf]) -> Vec<Volume> {
    Vec::new()
}
