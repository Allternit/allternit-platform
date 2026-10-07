//! A cloud computer offers itself as a Factory peer computer, so
//! `gizzi agents up --on <computer>` can run bots on it (Factory phase 4).
//!
//! Runs only on an Allternit cloud computer (Linux, provisioned mode: the
//! image sets `ALLTERNIT_PROVISIONED=1` and writes `/etc/allternit/provisioned.env`).
//! It lives here, in allternit-api, because cloud computers get new
//! allternit-api builds from runtime packages, while their Desktop shell only
//! changes with a new image.
//!
//! 1. Pair: with no `~/.allternit/computer/paired.json`, ask cloud-api
//!    (`POST /api/v1/computers/paired/self`, with this computer's runtime
//!    device token) and save its answer there, 0600, in the same shape
//!    `allternit computers pair` writes. The Factory engine reads that file
//!    and starts its peer listener on 127.0.0.1:3019.
//! 2. Join the mesh: run `mesh-node` forwarding only the engine's peer port
//!    (3019). The screen (VNC on 5900) is never put on the mesh: the image's
//!    VNC server has no password.
//! 3. Report the mesh address every minute
//!    (`POST /api/v1/computers/paired/:id/report`), as `allternit computers
//!    serve` does. A report the cloud no longer accepts (the pairing was
//!    removed) drops the saved pairing and pairs again.
//!
//! mesh-node is supervised (restarted 5 s after it exits) and dies with this
//! process. `ALLTERNIT_CLOUD_PEER=0` turns all of this off.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

/// The Factory engine's peer port; the only port put on the mesh.
pub const FACTORY_PEER_PORT: u16 = 3019;
const MARKER_FILE: &str = "/etc/allternit/provisioned.env";
const REPORT_EVERY: Duration = Duration::from_secs(60);
const RESTART_AFTER: Duration = Duration::from_secs(5);
const RETRY_PAIRING: Duration = Duration::from_secs(60);
const MESH_HOSTNAME: &str = "allternit-cloud-computer";
/// Where the Allternit Desktop .deb installs its sidecars.
const DEB_MESH_NODE: &str = "/opt/Allternit Desktop/resources/bin/mesh-node";

/// `~/.allternit/computer/paired.json`, same field names as the CLI's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairedConfig {
    pub computer_id: String,
    pub secret: String,
    pub cloud_url: String,
    pub name: String,
    pub control_url: String,
    /// Single use: only needed until the first join.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_key: Option<String>,
    #[serde(default)]
    pub mesh_node: String,
}

/// Whether this process runs on an Allternit cloud computer: the
/// provisioned-mode env flag, or the marker file the image writes.
pub fn is_cloud_computer(env_flag: Option<&str>, marker: Option<&str>) -> bool {
    let on = |v: &str| matches!(v.trim().trim_matches('"'), "1" | "true");
    env_flag.is_some_and(on)
        || marker.is_some_and(|text| {
            text.lines().any(|line| line.trim().strip_prefix("ALLTERNIT_PROVISIONED=").is_some_and(on))
        })
}

fn enabled() -> bool {
    if !cfg!(target_os = "linux") {
        return false;
    }
    if matches!(std::env::var("ALLTERNIT_CLOUD_PEER").as_deref(), Ok("0") | Ok("false")) {
        return false;
    }
    is_cloud_computer(
        std::env::var("ALLTERNIT_PROVISIONED").ok().as_deref(),
        std::fs::read_to_string(MARKER_FILE).ok().as_deref(),
    )
}

/// Start serving this computer to the Factory, if it is a cloud computer.
pub fn spawn_if_cloud_computer() {
    if !enabled() {
        return;
    }
    let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from) else {
        warn!("cloud computer peer: HOME is not set; not offering this computer to the Factory");
        return;
    };
    info!("cloud computer peer: offering this computer to the Factory (peer port {FACTORY_PEER_PORT})");
    tokio::spawn(run(home));
}

pub fn state_dir(home: &Path) -> PathBuf {
    home.join(".allternit").join("computer")
}

pub fn config_path(home: &Path) -> PathBuf {
    state_dir(home).join("paired.json")
}

fn load_config(home: &Path) -> Option<PairedConfig> {
    serde_json::from_str(&std::fs::read_to_string(config_path(home)).ok()?).ok()
}

fn save_config(home: &Path, config: &PairedConfig) -> std::io::Result<()> {
    use std::io::Write;
    let dir = state_dir(home);
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        let tmp = dir.join("paired.json.tmp");
        let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        file.write_all(serde_json::to_string_pretty(config)?.as_bytes())?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        std::fs::rename(&tmp, config_path(home))
    }
    #[cfg(not(unix))]
    {
        let mut file = std::fs::File::create(config_path(home))?;
        file.write_all(serde_json::to_string_pretty(config)?.as_bytes())
    }
}

/// mesh-node: `$ALLTERNIT_MESH_NODE` / `$ALLTERNIT_MESH_NODE_BIN`, next to
/// this binary, the Desktop .deb's sidecars, then PATH.
pub fn find_mesh_node(env: impl Fn(&str) -> Option<String>, exe: Option<PathBuf>, exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    for key in ["ALLTERNIT_MESH_NODE", "ALLTERNIT_MESH_NODE_BIN"] {
        if let Some(v) = env(key).filter(|v| !v.trim().is_empty()) {
            candidates.push(PathBuf::from(v));
        }
    }
    if let Some(dir) = exe.as_deref().and_then(Path::parent) {
        candidates.push(dir.join("mesh-node"));
    }
    candidates.push(PathBuf::from(DEB_MESH_NODE));
    if let Some(path) = env("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|dir| dir.join("mesh-node")));
    }
    candidates.into_iter().find(|p| exists(p))
}

/// mesh-node arguments: on the mesh at 3019, forwarded to the engine's
/// loopback 3019, and nothing else.
pub fn mesh_node_args(home: &Path, config: &PairedConfig) -> Vec<String> {
    let mut args = vec![
        "--hostname".to_string(),
        MESH_HOSTNAME.to_string(),
        "--control-url".to_string(),
        config.control_url.clone(),
        "--data-dir".to_string(),
        state_dir(home).join("mesh").to_string_lossy().into_owned(),
        "--forward".to_string(),
        FACTORY_PEER_PORT.to_string(),
    ];
    if let Some(key) = config.auth_key.as_deref().filter(|k| !k.is_empty()) {
        args.push("--auth-key".to_string());
        args.push(key.to_string());
    }
    args
}

/// The mesh address in mesh-node's `MESH_READY ip=<addr>` line.
pub fn mesh_ready_ip(line: &str) -> Option<&str> {
    line.trim().strip_prefix("MESH_READY ip=").map(str::trim).filter(|ip| !ip.is_empty())
}

#[derive(Debug, PartialEq)]
enum PairError {
    /// cloud-api says this runtime isn't a cloud computer: stop for good.
    Refused(String),
    /// Anything else: try again later.
    Retry(String),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairReply {
    computer_id: String,
    secret: String,
    #[serde(default)]
    cloud_url: Option<String>,
    #[serde(default)]
    name: Option<String>,
    control_url: String,
    #[serde(default)]
    auth_key: Option<String>,
}

async fn pair(cloud: &str, mesh_node: &Path) -> Result<PairedConfig, PairError> {
    let Some(token) = crate::phone_sync::runtime_bearer() else {
        return Err(PairError::Retry("this computer isn't signed in yet (no runtime device token)".into()));
    };
    let reply = reqwest::Client::new()
        .post(format!("{cloud}/api/v1/computers/paired/self"))
        .bearer_auth(token)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| PairError::Retry(format!("cloud-api unreachable: {e}")))?;
    let status = reply.status();
    if status == reqwest::StatusCode::FORBIDDEN {
        let body: serde_json::Value = reply.json().await.unwrap_or_default();
        let message = body["message"].as_str().or(body["error"].as_str()).unwrap_or("refused");
        return Err(PairError::Refused(message.to_string()));
    }
    if !status.is_success() {
        return Err(PairError::Retry(format!("pairing failed ({status})")));
    }
    let r: PairReply = reply.json().await.map_err(|e| PairError::Retry(format!("bad pairing reply: {e}")))?;
    Ok(PairedConfig {
        computer_id: r.computer_id,
        secret: r.secret,
        cloud_url: r.cloud_url.filter(|u| !u.is_empty()).unwrap_or_else(|| cloud.to_string()),
        name: r.name.unwrap_or_else(|| "Cloud computer".to_string()),
        control_url: r.control_url,
        auth_key: r.auth_key,
        mesh_node: mesh_node.to_string_lossy().into_owned(),
    })
}

enum Report {
    Ok,
    /// The cloud doesn't know this pairing any more.
    Unknown,
    Failed(String),
}

async fn report(config: &PairedConfig, mesh_ip: &str) -> Report {
    let url = format!("{}/api/v1/computers/paired/{}/report", config.cloud_url.trim_end_matches('/'), config.computer_id);
    let sent = reqwest::Client::new()
        .post(url)
        .header("x-allternit-computer-secret", &config.secret)
        .json(&serde_json::json!({ "meshIp": mesh_ip, "vncReady": false }))
        .timeout(Duration::from_secs(20))
        .send()
        .await;
    match sent {
        Ok(r) if r.status().is_success() => Report::Ok,
        Ok(r) if r.status() == reqwest::StatusCode::UNAUTHORIZED => Report::Unknown,
        Ok(r) => Report::Failed(format!("report failed ({})", r.status())),
        Err(e) => Report::Failed(format!("report failed: {e}")),
    }
}

async fn run(home: PathBuf) {
    let cloud = crate::phone_sync::cloud_base();
    loop {
        let Some(mesh_node) = find_mesh_node(|k| std::env::var(k).ok(), std::env::current_exe().ok(), |p| p.is_file()) else {
            warn!("cloud computer peer: mesh-node not found (set ALLTERNIT_MESH_NODE); retrying in 5 minutes");
            tokio::time::sleep(Duration::from_secs(300)).await;
            continue;
        };
        let mut config = match load_config(&home) {
            Some(config) => config,
            None => match pair(&cloud, &mesh_node).await {
                Ok(config) => {
                    if let Err(e) = save_config(&home, &config) {
                        warn!("cloud computer peer: couldn't save {}: {e}", config_path(&home).display());
                        tokio::time::sleep(RETRY_PAIRING).await;
                        continue;
                    }
                    info!(computer = %config.computer_id, "cloud computer peer: paired");
                    config
                }
                Err(PairError::Refused(message)) => {
                    warn!("cloud computer peer: cloud-api refused pairing ({message}); not offering this computer");
                    return;
                }
                Err(PairError::Retry(message)) => {
                    info!("cloud computer peer: {message}; retrying in a minute");
                    tokio::time::sleep(RETRY_PAIRING).await;
                    continue;
                }
            },
        };
        config.mesh_node = mesh_node.to_string_lossy().into_owned();
        match serve_once(&home, &mut config).await {
            Served::Unpaired => {
                warn!(computer = %config.computer_id, "cloud computer peer: the cloud no longer knows this pairing; pairing again");
                let _ = std::fs::remove_file(config_path(&home));
            }
            Served::Exited(why) => warn!("cloud computer peer: mesh-node stopped ({why}); restarting"),
        }
        tokio::time::sleep(RESTART_AFTER).await;
    }
}

enum Served {
    Unpaired,
    Exited(String),
}

/// Run mesh-node until it exits (or the pairing is gone), reporting while
/// it's up.
async fn serve_once(home: &Path, config: &mut PairedConfig) -> Served {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let mut command = tokio::process::Command::new(&config.mesh_node);
    command
        .args(mesh_node_args(home, config))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(target_os = "linux")]
    // SAFETY: prctl is async-signal-safe; it only asks the kernel to SIGTERM
    // mesh-node when this process dies (however it dies).
    unsafe {
        command.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => return Served::Exited(format!("couldn't start {}: {e}", config.mesh_node)),
    };
    if let Some(err) = child.stderr.take() {
        tokio::spawn(async move {
            let mut lines = BufReader::new(err).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                warn!(target: "mesh_node", "{line}");
            }
        });
    }
    let Some(out) = child.stdout.take() else {
        return Served::Exited("no stdout".into());
    };
    let mut lines = BufReader::new(out).lines();
    let ip = loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                if let Some(ip) = mesh_ready_ip(&line) {
                    break ip.to_string();
                }
            }
            _ => {
                let status = child.wait().await.map(|s| s.to_string()).unwrap_or_else(|e| e.to_string());
                return Served::Exited(format!("before joining the mesh: {status}"));
            }
        }
    };
    info!(ip = %ip, "cloud computer peer: on the mesh (peer port {FACTORY_PEER_PORT})");
    if config.auth_key.take().is_some() {
        // Joined: the mesh state holds the identity; the key was single use.
        if let Err(e) = save_config(home, config) {
            warn!("cloud computer peer: couldn't save {}: {e}", config_path(home).display());
        }
    }
    // Keep draining stdout so mesh-node never blocks on a full pipe.
    tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });
    loop {
        match report(config, &ip).await {
            Report::Ok => {}
            Report::Unknown => {
                let _ = child.kill().await;
                return Served::Unpaired;
            }
            Report::Failed(why) => warn!("cloud computer peer: {why}"),
        }
        tokio::select! {
            status = child.wait() => {
                return Served::Exited(status.map(|s| s.to_string()).unwrap_or_else(|e| e.to_string()));
            }
            _ = tokio::time::sleep(REPORT_EVERY) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_cloud_computers_are_detected() {
        assert!(is_cloud_computer(Some("1"), None));
        assert!(is_cloud_computer(Some("true"), None));
        assert!(is_cloud_computer(None, Some("ALLTERNIT_PROVISIONED=1\nALLTERNIT_LOCAL_SUBS_GATEWAY=1\n")));
        assert!(!is_cloud_computer(None, None));
        assert!(!is_cloud_computer(Some("0"), Some("ALLTERNIT_PROVISIONED=0")));
        assert!(!is_cloud_computer(Some(""), Some("ALLTERNIT_LOCAL_SUBS_GATEWAY=1")));
    }

    #[test]
    fn mesh_node_forwards_only_the_peer_port() {
        let home = Path::new("/root");
        let mut config = PairedConfig {
            computer_id: "pc_1".into(),
            secret: "s".into(),
            cloud_url: "https://api.allternit.com".into(),
            name: "Cloud computer".into(),
            control_url: "https://mesh.example".into(),
            auth_key: Some("k".into()),
            mesh_node: "/bin/mesh-node".into(),
        };
        let args = mesh_node_args(home, &config);
        assert_eq!(
            args,
            [
                "--hostname", "allternit-cloud-computer", "--control-url", "https://mesh.example",
                "--data-dir", "/root/.allternit/computer/mesh", "--forward", "3019", "--auth-key", "k",
            ]
        );
        assert!(!args.iter().any(|a| a == "5900" || a == "--also"), "the screen never goes on the mesh");
        config.auth_key = None;
        assert!(!mesh_node_args(home, &config).contains(&"--auth-key".to_string()));
    }

    #[test]
    fn paired_json_matches_the_cli() {
        let config: PairedConfig = serde_json::from_str(
            r#"{"computerId":"pc_1","secret":"s","cloudUrl":"https://c","name":"n","controlUrl":"https://m","meshNode":"/x"}"#,
        )
        .unwrap();
        let json = serde_json::to_value(&config).unwrap();
        for key in ["computerId", "secret", "cloudUrl", "name", "controlUrl", "meshNode"] {
            assert!(json.get(key).is_some(), "{key}");
        }
        assert!(json.get("authKey").is_none());
    }

    #[test]
    fn saves_paired_json_private() {
        let home = tempfile::tempdir().unwrap();
        let config = PairedConfig {
            computer_id: "pc_1".into(),
            secret: "s".into(),
            cloud_url: "https://c".into(),
            name: "Cloud computer".into(),
            control_url: "https://m".into(),
            auth_key: None,
            mesh_node: "/x".into(),
        };
        save_config(home.path(), &config).unwrap();
        assert_eq!(load_config(home.path()), Some(config));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(config_path(home.path())).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn finds_mesh_node_by_override_then_beside_the_binary() {
        let env = |k: &str| match k {
            "ALLTERNIT_MESH_NODE" => Some("/custom/mesh-node".to_string()),
            "PATH" => Some("/usr/bin".to_string()),
            _ => None,
        };
        let exe = Some(PathBuf::from("/runtime/bin/allternit-api"));
        assert_eq!(find_mesh_node(env, exe.clone(), |_| true), Some(PathBuf::from("/custom/mesh-node")));
        let no_override = |k: &str| (k == "PATH").then(|| "/usr/bin".to_string());
        assert_eq!(
            find_mesh_node(no_override, exe.clone(), |p| p == Path::new("/runtime/bin/mesh-node")),
            Some(PathBuf::from("/runtime/bin/mesh-node"))
        );
        assert_eq!(find_mesh_node(no_override, exe.clone(), |p| p == Path::new(DEB_MESH_NODE)), Some(PathBuf::from(DEB_MESH_NODE)));
        assert_eq!(find_mesh_node(no_override, exe, |_| false), None);
    }

    #[test]
    fn reads_the_mesh_address() {
        assert_eq!(mesh_ready_ip("MESH_READY ip=100.64.0.7\n"), Some("100.64.0.7"));
        assert_eq!(mesh_ready_ip("PROXY_READY port=1"), None);
    }
}
