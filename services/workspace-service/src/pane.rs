//! Client for the Allternit Factory pane engine (`allternit-factory pane`).
//!
//! Every workspace pane is a terminal on the pane engine: a real PTY that
//! also shows in the Factory's agent wall (workspace `ws-<session>-…`), the
//! same thing as a Desktop terminal tile or a Gizzi PTY. The pane engine serves
//! one JSON request per connection on its socket (`factory.terminal.*`).
//!
//! The socket is `$ALLTERNIT_FACTORY_PANE_SOCKET`, else whatever
//! `allternit-factory pane tty ensure` reports (it starts the engine when it
//! is down). The binary is `$ALLTERNIT_FACTORY_BIN`, else `allternit-factory`
//! on `PATH`.

use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use uuid::Uuid;

static SOCKET: Mutex<Option<PathBuf>> = Mutex::new(None);

fn factory_bin() -> PathBuf {
    std::env::var_os("ALLTERNIT_FACTORY_BIN")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("allternit-factory"))
}

async fn ensure() -> Result<PathBuf, String> {
    let bin = factory_bin();
    let out = tokio::time::timeout(
        Duration::from_secs(30),
        tokio::process::Command::new(&bin)
            .args(["pane", "tty", "ensure"])
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| "the Factory pane engine did not start within 30s".to_string())?
    .map_err(|e| format!("the Factory pane engine could not be started ({}: {e})", bin.display()))?;
    if !out.status.success() {
        return Err(format!(
            "the Factory pane engine could not be started: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    v["socket"]
        .as_str()
        .map(PathBuf::from)
        .ok_or_else(|| "pane tty ensure reported no socket".to_string())
}

async fn connect() -> Result<UnixStream, String> {
    if let Some(path) = std::env::var_os("ALLTERNIT_FACTORY_PANE_SOCKET").filter(|v| !v.is_empty()) {
        let path = PathBuf::from(path);
        return UnixStream::connect(&path)
            .await
            .map_err(|e| format!("the Factory pane engine is not reachable at {}: {e}", path.display()));
    }
    let cached = SOCKET.lock().unwrap_or_else(|p| p.into_inner()).clone();
    if let Some(path) = cached {
        if let Ok(stream) = UnixStream::connect(&path).await {
            return Ok(stream);
        }
    }
    let path = ensure().await?;
    let stream = UnixStream::connect(&path)
        .await
        .map_err(|e| format!("the Factory pane engine is not reachable at {}: {e}", path.display()))?;
    *SOCKET.lock().unwrap_or_else(|p| p.into_inner()) = Some(path);
    Ok(stream)
}

async fn open(method: &str, params: Value) -> Result<BufReader<UnixStream>, String> {
    let mut stream = connect().await?;
    let frame = json!({ "id": Uuid::new_v4().to_string(), "method": method, "params": params });
    stream
        .write_all(format!("{frame}\n").as_bytes())
        .await
        .map_err(|e| format!("pane engine write: {e}"))?;
    Ok(BufReader::new(stream))
}

async fn read_result(reader: &mut BufReader<UnixStream>) -> Result<Value, String> {
    let mut buf = String::new();
    let n = reader.read_line(&mut buf).await.map_err(|e| format!("pane engine read: {e}"))?;
    if n == 0 {
        return Err("the pane engine closed the connection".into());
    }
    let resp: Value = serde_json::from_str(buf.trim()).map_err(|e| e.to_string())?;
    if let Some(err) = resp.get("error") {
        return Err(err["message"].as_str().unwrap_or("unknown").to_string());
    }
    Ok(resp.get("result").cloned().unwrap_or(Value::Null))
}

/// One `factory.terminal.*` call.
pub async fn call(method: &str, params: Value) -> Result<Value, String> {
    let mut reader = open(method, params).await?;
    read_result(&mut reader).await
}

/// The terminal's kept output (scrollback, raw, up to 2 MiB).
pub async fn read_output(terminal_id: &str) -> Result<String, String> {
    let mut reader = open(
        "factory.terminal.output",
        json!({ "terminal_id": terminal_id, "replay": true, "follow": false }),
    )
    .await?;
    read_result(&mut reader).await?;
    let mut out = String::new();
    let mut line = String::new();
    while reader.read_line(&mut line).await.map_err(|e| format!("pane engine read: {e}"))? > 0 {
        if let Ok(frame) = serde_json::from_str::<Value>(line.trim()) {
            if frame["type"] == "output" {
                out.push_str(frame["data"].as_str().unwrap_or_default());
            }
        }
        line.clear();
    }
    Ok(out)
}
