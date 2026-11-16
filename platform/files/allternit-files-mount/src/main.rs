//! `allternit-files-mount`: serve one computer's Files drive as a folder.
//!
//! ```text
//! allternit-files-mount fuse <mountpoint> [--name NAME] [--read-only]   # Linux
//! allternit-files-mount dav [--listen 127.0.0.1:0] [--read-only]
//! ```
//!
//! The connection comes from `ALLTERNIT_FILES_*` environment variables (see
//! `http.rs`), which `gizzi files mount` sets; tokens never go on the command
//! line. `dav` prints one JSON line, `{"url": "...", "pid": N}`, once it is
//! listening, so the caller can show the address.

use std::net::SocketAddr;
use std::sync::Arc;

use allternit_files_mount::{HttpBackend, HttpConfig, Vfs};

const USAGE: &str = "usage:
  allternit-files-mount fuse <mountpoint> [--name NAME] [--read-only]
  allternit-files-mount dav [--listen 127.0.0.1:0] [--read-only]
Connection settings come from ALLTERNIT_FILES_* environment variables; run it through `gizzi files mount`.";

fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    if let Err(e) = run() {
        eprintln!("allternit-files-mount: {e}");
        std::process::exit(1);
    }
}

fn flag_value(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().map(String::as_str) else {
        return Err(USAGE.to_string());
    };
    if cmd == "-h" || cmd == "--help" {
        println!("{USAGE}");
        return Ok(());
    }
    let read_only = args.iter().any(|a| a == "--read-only");
    // Build the blocking HTTP client before any tokio runtime exists.
    let cfg = HttpConfig::from_env()?;
    let backend = Arc::new(HttpBackend::new(cfg)?);
    let vfs = Arc::new(Vfs::new(backend, read_only));
    match cmd {
        "fuse" => {
            let mountpoint = args.get(1).filter(|a| !a.starts_with("--")).ok_or_else(|| USAGE.to_string())?;
            let name = flag_value(&args, "--name").unwrap_or_else(|| "files".to_string());
            fuse(vfs, mountpoint, &name)
        }
        "dav" => {
            let listen: SocketAddr = flag_value(&args, "--listen")
                .unwrap_or_else(|| "127.0.0.1:0".to_string())
                .parse()
                .map_err(|e| format!("--listen: {e}"))?;
            if !listen.ip().is_loopback() {
                return Err("--listen must be a loopback address (127.0.0.1 or ::1): the WebDAV server has no login of its own".into());
            }
            dav(vfs, listen)
        }
        other => Err(format!("unknown command `{other}`\n{USAGE}")),
    }
}

#[cfg(target_os = "linux")]
fn fuse(vfs: Arc<Vfs>, mountpoint: &str, name: &str) -> Result<(), String> {
    let path = std::path::Path::new(mountpoint);
    std::fs::create_dir_all(path).map_err(|e| format!("{mountpoint}: {e}"))?;
    tracing::info!(mountpoint, "mounting");
    allternit_files_mount::fuse::mount(vfs, path, name).map_err(|e| format!("mount {mountpoint}: {e}"))
}

#[cfg(not(target_os = "linux"))]
fn fuse(_vfs: Arc<Vfs>, _mountpoint: &str, _name: &str) -> Result<(), String> {
    Err("FUSE mounts are Linux only. Use `dav` (WebDAV) here.".into())
}

fn dav(vfs: Arc<Vfs>, listen: SocketAddr) -> Result<(), String> {
    let secret = format!("{:032x}", rand::random::<u128>());
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().map_err(|e| format!("runtime: {e}"))?;
    // The last reference to the blocking HTTP client must drop outside the runtime.
    let _keep = vfs.clone();
    rt.block_on(async move {
        let listener = tokio::net::TcpListener::bind(listen).await.map_err(|e| format!("listen {listen}: {e}"))?;
        let addr = listener.local_addr().map_err(|e| e.to_string())?;
        let url = format!("http://{addr}/{secret}/");
        println!("{}", serde_json::json!({ "url": url, "pid": std::process::id() }));
        tracing::info!(%addr, "WebDAV ready");
        allternit_files_mount::dav::serve(vfs, listener, &secret).await.map_err(|e| e.to_string())
    })
}
