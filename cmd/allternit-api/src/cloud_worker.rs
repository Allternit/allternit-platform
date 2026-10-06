//! Always-on Fabric Transport worker (`gizzi-cloud`).
//!
//! Started by allternit-api when this process is **not** the laptop desktop
//! sidecar (no `ALLTERNIT_DESKTOP_ACCESS_TOKEN`). Provisioned/paired boxes
//! therefore claim `compute.cloud` jobs without an operator command.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tracing::{info, warn};

use crate::config::AppConfig;
use crate::db::DbHandle;

pub fn should_run_cloud_worker(config: &AppConfig) -> bool {
    decide_cloud_worker(
        std::env::var("ALLTERNIT_FABRIC_CLOUD_WORKER").ok().as_deref(),
        config.desktop_access_token().is_some(),
    )
}

fn decide_cloud_worker(force: Option<&str>, has_desktop_token: bool) -> bool {
    match force.map(|v| v.trim()) {
        Some("0") | Some("false") | Some("FALSE") => false,
        Some("1") | Some("true") | Some("TRUE") => true,
        _ => !has_desktop_token,
    }
}

fn gizzi_bin() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("ALLTERNIT_GIZZI_BIN") {
        let path = PathBuf::from(p);
        if path.is_file() {
            return Some(path);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in ["gizzi-code", "gizzi-code.exe"] {
                let cand = dir.join(name);
                if cand.is_file() {
                    return Some(cand);
                }
            }
        }
    }
    which_in_path("gizzi-code")
}

fn which_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(name);
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

/// Provision the gizzi-cloud principal and keep a fabric-worker process alive.
pub fn spawn_cloud_fabric_worker(db: DbHandle, config: Arc<AppConfig>, api_port: u16) {
    let Some(bin) = gizzi_bin() else {
        warn!("gizzi-cloud worker not started: gizzi-code binary not found (set ALLTERNIT_GIZZI_BIN)");
        return;
    };
    tokio::spawn(async move {
        loop {
            match start_once(&db, &config, &bin, api_port).await {
                Ok(status) => {
                    warn!(?status, "gizzi-cloud worker exited; restarting in 2s");
                }
                Err(err) => {
                    warn!(%err, "gizzi-cloud worker failed to start; retrying in 5s");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
}

async fn start_once(
    db: &DbHandle,
    _config: &AppConfig,
    bin: &PathBuf,
    api_port: u16,
) -> Result<std::process::ExitStatus, String> {
    let db = db.clone();
    let token = tokio::task::spawn_blocking(move || -> Result<String, String> {
        let mut conn = db.connect().map_err(|e| e.to_string())?;
        let id = allternit_cowork_runtime::sqlite_store::ensure_gizzi_cloud_principal(
            &mut conn, "default",
        )
        .map_err(|e| e.message)?;
        allternit_cowork_runtime::sqlite_store::provision_principal_token(&mut conn, &id)
            .map_err(|e| e.message)
    })
    .await
    .map_err(|e| e.to_string())??;

    let workspace = _config.cloud_workspace_dir();
    let _ = std::fs::create_dir_all(&workspace);
    info!(bin = %bin.display(), "starting gizzi-cloud fabric-worker");
    let mut child = tokio::process::Command::new(bin)
        .arg("fabric-worker")
        .arg("--compute-mode")
        .arg("cloud")
        .env("GIZZI_COMPUTE_MODE", "cloud")
        .env("ALLTERNIT_GIZZI_TOKEN", token)
        .env(
            "ALLTERNIT_API_URL",
            format!("http://127.0.0.1:{api_port}"),
        )
        .env(
            "ALLTERNIT_CLOUD_WORKSPACE",
            workspace.to_string_lossy().to_string(),
        )
        // Lifeline: gizzi exits when its stdin reaches EOF, and the kernel
        // closes our end however this process dies (kill -9, crash, a boot
        // smoke test killing it). kill_on_drop alone only covers a clean
        // shutdown, so every other exit left the worker running under launchd.
        .env("GIZZI_PARENT_LIFELINE", "stdin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| e.to_string())?;
    // Held, never written: dropping it would end the worker.
    let lifeline = child.stdin.take();
    // Drain the worker's output into our log; an unread pipe fills up and
    // blocks the worker on its next write.
    if let Some(out) = child.stdout.take() {
        tokio::spawn(forward_worker_output(out, "stdout"));
    }
    if let Some(err) = child.stderr.take() {
        tokio::spawn(forward_worker_output(err, "stderr"));
    }
    let status = child.wait().await.map_err(|e| e.to_string());
    drop(lifeline);
    status
}

async fn forward_worker_output<R: tokio::io::AsyncRead + Unpin>(stream: R, which: &'static str) {
    use tokio::io::AsyncBufReadExt;
    let mut lines = tokio::io::BufReader::new(stream).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        info!(target: "gizzi_cloud_worker", stream = which, "{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_sidecar_does_not_autostart_cloud_worker() {
        assert!(!decide_cloud_worker(None, true));
        assert!(!decide_cloud_worker(Some("0"), false));
    }

    #[test]
    fn hosted_api_autostarts_cloud_worker() {
        assert!(decide_cloud_worker(None, false));
        assert!(decide_cloud_worker(Some("1"), true));
    }
}
