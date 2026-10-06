// Pane I/O on the Allternit Factory pane engine: panes perform REAL PTY I/O
// as terminals in the pane engine. Starts a pane engine with a scratch HOME
// (`allternit-factory pane tty ensure`), then drives the service over HTTP via
// tower::oneshot. Needs a built allternit-factory: $ALLTERNIT_FACTORY_BIN, or
// the workspace target (`cargo build -p allternit-factory`).

use allternit_workspace_service::{build_router, AppState};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::json;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};
use tower::ServiceExt;

/// Serializes tests that mutate ALLTERNIT_FACTORY_PANE_SOCKET / SHELL
/// (process-global env).
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct PaneEngine {
    bin: PathBuf,
    home: tempfile::TempDir,
    socket: String,
}

fn factory_binary() -> PathBuf {
    if let Some(bin) = std::env::var_os("ALLTERNIT_FACTORY_BIN") {
        return PathBuf::from(bin);
    }
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    target.join("debug/allternit-factory")
}

fn engine_command(bin: &PathBuf, home: &std::path::Path) -> Command {
    let mut cmd = Command::new(bin);
    cmd.env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_STATE_HOME", home.join(".state"))
        .env_remove("HERDR_SOCKET_PATH")
        .env_remove("HERDR_SESSION");
    cmd
}

fn start_engine() -> PaneEngine {
    let bin = factory_binary();
    assert!(
        bin.is_file(),
        "allternit-factory not found at {} (run `cargo build -p allternit-factory` or set ALLTERNIT_FACTORY_BIN)",
        bin.display()
    );
    // A short HOME keeps the socket path under the Unix socket length limit.
    let home = tempfile::Builder::new().prefix("wsf").tempdir_in("/tmp").unwrap();
    let out = engine_command(&bin, home.path()).args(["pane", "tty", "ensure"]).output().unwrap();
    assert!(out.status.success(), "pane tty ensure failed: {}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let socket = v["socket"].as_str().unwrap().to_string();
    PaneEngine { bin, home, socket }
}

impl Drop for PaneEngine {
    fn drop(&mut self) {
        let _ = engine_command(&self.bin, self.home.path())
            .env("HERDR_SESSION", "ao")
            .args(["pane", "server", "stop"])
            .output();
    }
}

async fn post(app: &axum::Router, uri: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(json!({})))
}

async fn get(app: &axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(json!({})))
}

#[tokio::test]
async fn panes_perform_real_pty_io_on_the_pane_engine() {
    let _guard = ENV_LOCK.lock().unwrap();
    let engine = start_engine();
    std::env::set_var("ALLTERNIT_FACTORY_PANE_SOCKET", &engine.socket);
    // A plain shell: a fresh HOME would put zsh into its first-run wizard.
    std::env::set_var("SHELL", "/bin/sh");
    let app = build_router(AppState::new());

    // Session + pane (default shell).
    let (status, created) = post(&app, "/sessions", json!({ "name": "pane-io" })).await;
    assert_eq!(status, StatusCode::CREATED);
    let session_id = created["session"]["id"].as_str().unwrap().to_string();

    let (status, pane) = post(
        &app,
        &format!("/sessions/{}/panes", session_id),
        json!({ "name": "main" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let pane_id = pane["id"].as_str().unwrap().to_string();

    // Send a command through the pane; real PTY executes it.
    let marker = format!("ws-pane-io-{}", uuid::Uuid::new_v4());
    let (status, _) = post(
        &app,
        &format!("/panes/{}/send", pane_id),
        json!({ "keys": format!("echo {marker}") }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Capture must show REAL shell output, not the old simulated "$ cmd" echo.
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut captured = String::new();
    while Instant::now() < deadline {
        let (status, body) = get(&app, &format!("/panes/{}/capture", pane_id)).await;
        assert_eq!(status, StatusCode::OK);
        captured = body["output"].as_str().unwrap_or("").to_string();
        // Real PTY output contains the command's *result* on its own line.
        if captured.lines().any(|l| l.trim() == marker) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(
        captured.lines().any(|l| l.trim() == marker),
        "no real PTY output for marker; got: {captured:?}"
    );
    assert!(
        !captured.starts_with("$ echo"),
        "fell back to simulated output: {captured:?}"
    );

    // Logs endpoint reads the same real scrollback.
    let (status, logs) = get(&app, &format!("/panes/{}/logs", pane_id)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(logs["logs"].as_str().unwrap_or("").contains(&marker));

    // Delete session closes the pane engine terminal too.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/sessions/{}", session_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let out = engine_command(&engine.bin, engine.home.path())
        .env("HERDR_SESSION", "ao")
        .args(["pane", "tty", "list"])
        .output()
        .unwrap();
    let list: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(list["terminals"], json!([]), "terminal left open: {list}");
}
