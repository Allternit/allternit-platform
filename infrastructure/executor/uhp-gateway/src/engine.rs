//! Blocking client for the ao engine (herdr) Unix-domain-socket NDJSON JSON-RPC API.
//!
//! Wire format (verified against `factory/pane/src/api/schema*.rs`):
//! one request line `{"id":..,"method":..,"params":{..}}`, one response line that is either
//! `{"id":..,"result":..}` or `{"id":..,"error":{"code":..,"message":..}}`.
//! Layout nodes serialize as `{"type":"pane","cwd":..,"command":[..],"env":{..}}`
//! (LayoutNode::Pane flattens LayoutPane); `pane.read` takes
//! `{"pane_id","source":"recent","lines":n,"format":"text","strip_ansi":true}`.

use std::fmt;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::de::DeserializeOwned;

static REQUEST_SEQ: AtomicU64 = AtomicU64::new(0);

fn next_id() -> String {
    format!(
        "uhp-{}-{}",
        std::process::id(),
        REQUEST_SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

#[derive(Debug)]
pub enum EngineError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Rpc { code: String, message: String },
    EmptyResponse,
    MissingField(&'static str),
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{err}"),
            Self::Json(err) => write!(f, "{err}"),
            Self::Rpc { code, message } => write!(f, "engine rpc {code}: {message}"),
            Self::EmptyResponse => write!(f, "engine returned an empty response"),
            Self::MissingField(field) => write!(f, "engine response missing field {field}"),
        }
    }
}

impl std::error::Error for EngineError {}

impl From<std::io::Error> for EngineError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<serde_json::Error> for EngineError {
    fn from(err: serde_json::Error) -> Self {
        Self::Json(err)
    }
}

#[derive(Debug, Clone)]
pub struct EngineClient {
    socket: PathBuf,
}

impl EngineClient {
    pub fn new(socket: PathBuf) -> Self {
        Self { socket }
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket
    }

    fn connect(&self) -> std::io::Result<interprocess::local_socket::Stream> {
        connect_local_stream(&self.socket)
    }

    /// One-shot request: connect, write one line, read one line.
    fn call_value(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value, EngineError> {
        let mut stream = self.connect()?;
        let request = serde_json::json!({
            "id": next_id(),
            "method": method,
            "params": params,
        });
        stream.write_all(request.to_string().as_bytes())?;
        stream.write_all(b"\n")?;
        stream.flush()?;

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        let read = reader.read_line(&mut line)?;
        if read == 0 || line.trim().is_empty() {
            return Err(EngineError::EmptyResponse);
        }
        let value: serde_json::Value = serde_json::from_str(&line)?;
        if let Some(error) = value.get("error") {
            return Err(EngineError::Rpc {
                code: error
                    .get("code")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unknown")
                    .to_string(),
                message: error
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unknown engine error")
                    .to_string(),
            });
        }
        Ok(value["result"].clone())
    }

    fn call<T: DeserializeOwned>(&self, method: &str, params: serde_json::Value) -> Result<T, EngineError> {
        let value = self.call_value(method, params)?;
        Ok(serde_json::from_value(value)?)
    }

    // ── async facade (the client is blocking; run it on the blocking pool) ──

    pub async fn ping(&self) -> Result<(), EngineError> {
        let client = self.clone();
        tokio::task::spawn_blocking(move || {
            client.call::<serde_json::Value>("ping", serde_json::json!({}))?;
            Ok(())
        })
        .await
        .map_err(join_err)?
    }

    pub async fn workspace_create(&self, cwd: &str, label: &str) -> Result<(String, String), EngineError> {
        let client = self.clone();
        let cwd = cwd.to_string();
        let label = label.to_string();
        tokio::task::spawn_blocking(move || {
            let result: serde_json::Value = client.call(
                "workspace.create",
                serde_json::json!({
                    "source_workspace_id": null,
                    "cwd": cwd,
                    "focus": false,
                    "label": label,
                    "env": {},
                }),
            )?;
            let workspace_id = result["workspace"]["workspace_id"]
                .as_str()
                .ok_or(EngineError::MissingField("result.workspace.workspace_id"))?
                .to_string();
            let tab_id = result["tab"]["tab_id"]
                .as_str()
                .ok_or(EngineError::MissingField("result.tab.tab_id"))?
                .to_string();
            Ok((workspace_id, tab_id))
        })
        .await
        .map_err(join_err)?
    }

    /// Presence probe: the engine removes dead panes' workspaces, so liveness
    /// is exactly "the workspace is listed" (see `session_alive` in
    /// `factory/pane/src/cli/ao.rs:181`).
    pub async fn workspace_list(&self) -> Result<Vec<serde_json::Value>, EngineError> {
        let client = self.clone();
        tokio::task::spawn_blocking(move || {
            let result: serde_json::Value = client.call("workspace.list", serde_json::json!({}))?;
            Ok(result["workspaces"].as_array().cloned().unwrap_or_default())
        })
        .await
        .map_err(join_err)?
    }

    pub async fn find_workspace(&self, label: &str) -> Result<Option<serde_json::Value>, EngineError> {
        let label = label.to_string();
        Ok(self
            .workspace_list()
            .await?
            .into_iter()
            .find(|ws| ws["label"].as_str() == Some(label.as_str())))
    }

    pub async fn workspace_close(&self, workspace_id: &str) -> Result<(), EngineError> {
        let client = self.clone();
        let workspace_id = workspace_id.to_string();
        tokio::task::spawn_blocking(move || {
            client.call::<serde_json::Value>(
                "workspace.close",
                serde_json::json!({"workspace_id": workspace_id, "close_group": true}),
            )?;
            Ok(())
        })
        .await
        .map_err(join_err)?
    }

    pub async fn layout_apply(
        &self,
        tab_id: &str,
        cwd: &str,
        command: Vec<String>,
        env: Vec<(String, String)>,
    ) -> Result<(), EngineError> {
        let client = self.clone();
        let tab_id = tab_id.to_string();
        let cwd = cwd.to_string();
        tokio::task::spawn_blocking(move || {
            let env: serde_json::Map<String, serde_json::Value> = env
                .into_iter()
                .map(|(key, value)| (key, serde_json::Value::String(value)))
                .collect();
            let root = serde_json::json!({
                "type": "pane",
                "cwd": cwd,
                "command": command,
                "env": env,
            });
            client.call::<serde_json::Value>(
                "layout.apply",
                serde_json::json!({
                    "workspace_id": null,
                    "tab_id": tab_id,
                    "tab_label": null,
                    "focus": false,
                    "root": root,
                }),
            )?;
            Ok(())
        })
        .await
        .map_err(join_err)?
    }

    pub async fn pane_list(&self, workspace_id: &str) -> Result<Vec<String>, EngineError> {
        let client = self.clone();
        let workspace_id = workspace_id.to_string();
        tokio::task::spawn_blocking(move || {
            let result: serde_json::Value = client.call(
                "pane.list",
                serde_json::json!({"workspace_id": workspace_id}),
            )?;
            Ok(result["panes"]
                .as_array()
                .map(|panes| {
                    panes
                        .iter()
                        .filter_map(|pane| pane["pane_id"].as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default())
        })
        .await
        .map_err(join_err)?
    }

    /// Recent pane output with ANSI stripped (`result.read.text`).
    pub async fn pane_read_text(&self, pane_id: &str, lines: u32) -> Result<String, EngineError> {
        let client = self.clone();
        let pane_id = pane_id.to_string();
        tokio::task::spawn_blocking(move || {
            let result: serde_json::Value = client.call(
                "pane.read",
                serde_json::json!({
                    "pane_id": pane_id,
                    "source": "recent",
                    "lines": lines,
                    "format": "text",
                    "strip_ansi": true,
                }),
            )?;
            Ok(result["read"]["text"].as_str().unwrap_or_default().to_string())
        })
        .await
        .map_err(join_err)?
    }
}

fn join_err(err: tokio::task::JoinError) -> EngineError {
    EngineError::Io(std::io::Error::other(err))
}

/// cfg-split connection mirroring `factory/pane/src/ipc.rs::connect_local_stream`.
fn connect_local_stream(path: &Path) -> std::io::Result<interprocess::local_socket::Stream> {
    #[cfg(unix)]
    {
        use interprocess::local_socket::prelude::*;
        let name = path.to_fs_name::<interprocess::local_socket::GenericFilePath>()?;
        Ok(interprocess::local_socket::Stream::connect(name)?)
    }
    #[cfg(windows)]
    {
        use interprocess::local_socket::prelude::*;
        let name = path.to_string_lossy().to_string();
        let name = name.to_ns_name::<interprocess::local_socket::GenericNamespaced>()?;
        Ok(interprocess::local_socket::Stream::connect(name)?)
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "local sockets unsupported on this platform",
        ))
    }
}
