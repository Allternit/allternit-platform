//! Per-peer Unix-domain inbox socket.
//!
//! A `PeerSocket` binds a single UDS path and accepts newline-delimited JSON
//! `PeerEnvelope`s.  The companion `send_envelope` function delivers to a peer
//! socket with a short timeout and returns a `DeliveryReceipt`.

use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(unix)]
use anyhow::{bail, Context};
use anyhow::Result;
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};
#[cfg(unix)]
use tokio::time::timeout;

use crate::core::ids::create_event_id;

/// A plain-text message delivered between local peers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerEnvelope {
    pub message_id: String,
    /// Socket path the sender is listening on, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    pub from: String,
    pub to: String,
    pub body: String,
    pub sent_at: String,
}

impl PeerEnvelope {
    /// Build a new envelope with a generated id and current timestamp.
    pub fn new(from: impl Into<String>, to: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            message_id: create_event_id(),
            reply_to: None,
            from: from.into(),
            to: to.into(),
            body: body.into(),
            sent_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    /// Set the reply-to socket path.
    pub fn with_reply_to(mut self, socket: &Path) -> Self {
        self.reply_to = Some(socket.to_string_lossy().to_string());
        self
    }
}

/// Result of attempting to deliver a message to a peer inbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryReceipt {
    pub delivered: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A bound UDS listener for one peer (Unix only).
#[cfg(unix)]
pub struct PeerSocket {
    listener: UnixListener,
    socket_path: PathBuf,
}

#[cfg(unix)]
impl PeerSocket {
    /// Bind to `socket_path`.  Removes any stale socket file first.
    pub async fn bind(socket_path: impl AsRef<Path>) -> Result<Self> {
        let socket_path = socket_path.as_ref().to_path_buf();
        ensure_socket_dir(&socket_path)?;
        if socket_path.exists() {
            std::fs::remove_file(&socket_path)
                .with_context(|| format!("removing stale socket {}", socket_path.display()))?;
        }

        // macOS UDS paths are limited to ~104 bytes.  Fall back to a short
        // tmp path if the workspace-root path is too long.
        let bind_path = short_socket_path(&socket_path);
        if bind_path != socket_path {
            ensure_private_fallback_dir(bind_path.parent().expect("fallback path has a parent"))?;
            if bind_path.exists() {
                std::fs::remove_file(&bind_path)
                    .with_context(|| format!("removing stale socket {}", bind_path.display()))?;
            }
        }
        let listener = UnixListener::bind(&bind_path)
            .with_context(|| format!("binding UDS {}", bind_path.display()))?;
        // Only the owning user may connect (bind leaves the mode to umask).
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bind_path, std::fs::Permissions::from_mode(0o600))
                .with_context(|| format!("chmod 600 {}", bind_path.display()))?;
        }

        Ok(Self {
            listener,
            socket_path: bind_path,
        })
    }

    /// Accept one envelope.  Returns the envelope and the path of the socket
    /// that accepted it.
    pub async fn accept_envelope(&mut self) -> Result<(PeerEnvelope, PathBuf)> {
        let (stream, _) = self.listener.accept().await?;
        let path = self.socket_path.clone();
        let envelope = read_envelope(stream).await?;
        Ok((envelope, path))
    }

    /// Path the socket is bound to (may differ from the requested path if a
    /// tmp fallback was used).
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

/// Deliver an envelope to `socket_path`, waiting at most `timeout_duration`.
#[cfg(unix)]
pub async fn send_envelope(
    socket_path: &Path,
    envelope: &PeerEnvelope,
    timeout_duration: Duration,
) -> Result<DeliveryReceipt> {
    let bind_path = short_socket_path(socket_path);
    if !bind_path.exists() {
        return Ok(DeliveryReceipt {
            delivered: false,
            error: Some(format!("peer socket does not exist: {}", bind_path.display())),
        });
    }

    let fut = async {
        let mut stream = UnixStream::connect(&bind_path).await?;
        let line = serde_json::to_string(envelope)?;
        stream.write_all(line.as_bytes()).await?;
        stream.write_all(b"\n").await?;
        stream.shutdown().await?;
        Ok::<(), anyhow::Error>(())
    };

    match timeout(timeout_duration, fut).await {
        Ok(Ok(())) => Ok(DeliveryReceipt {
            delivered: true,
            error: None,
        }),
        Ok(Err(e)) => Ok(DeliveryReceipt {
            delivered: false,
            error: Some(format!("uds send failed: {}", e)),
        }),
        Err(_) => Ok(DeliveryReceipt {
            delivered: false,
            error: Some(format!(
                "uds send timed out after {}ms",
                timeout_duration.as_millis()
            )),
        }),
    }
}

#[cfg(unix)]
async fn read_envelope(stream: UnixStream) -> Result<PeerEnvelope> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let n = reader.read_line(&mut line).await?;
    if n == 0 {
        bail!("peer socket closed before sending envelope");
    }
    let envelope: PeerEnvelope = serde_json::from_str(&line)
        .with_context(|| format!("invalid peer envelope JSON: {}", line.trim()))?;
    Ok(envelope)
}

#[cfg(unix)]
fn ensure_socket_dir(socket_path: &Path) -> Result<()> {
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

/// Per-user fallback directory for over-long socket paths:
/// `/tmp/allternit-peers-<uid>` (a shared `/tmp/allternit-peers` would let any
/// local user pre-create it and read or hijack peer traffic).
#[cfg(unix)]
fn fallback_dir() -> PathBuf {
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    PathBuf::from("/tmp").join(format!("allternit-peers-{uid}"))
}

/// Create the fallback dir 0700, or verify an existing one is a real
/// directory owned by us, and tighten it to 0700.
#[cfg(unix)]
fn ensure_private_fallback_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    match std::fs::symlink_metadata(dir) {
        Ok(meta) => {
            // SAFETY: getuid has no preconditions and cannot fail.
            let uid = unsafe { libc::getuid() };
            if !meta.file_type().is_dir() || meta.uid() != uid {
                bail!(
                    "peer socket fallback dir {} is not a directory owned by uid {}; refusing to use it",
                    dir.display(),
                    uid
                );
            }
            if meta.mode() & 0o777 != 0o700 {
                std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
                    .with_context(|| format!("chmod 700 {}", dir.display()))?;
            }
        }
        Err(_) => {
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(dir)
                .with_context(|| format!("creating {}", dir.display()))?;
        }
    }
    Ok(())
}

/// macOS limits UDS paths to about 104 bytes.  If `socket_path` is too long,
/// mirror it under the per-user [`fallback_dir`] using the basename.  Pure:
/// the directory is created (0700) only by `PeerSocket::bind`.
#[cfg(unix)]
fn short_socket_path(socket_path: &Path) -> PathBuf {
    const MAX_LEN: usize = 100;
    let s = socket_path.to_string_lossy();
    if s.len() <= MAX_LEN {
        return socket_path.to_path_buf();
    }
    let basename = socket_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "peer.sock".to_string());
    fallback_dir().join(basename)
}

/// Non-Unix platforms have no UDS in tokio. Delivery reports as failed so
/// callers surface a useful message; peers still receive messages via the
/// durable Bus inbox + HTTP polling fallback.
#[cfg(not(unix))]
pub async fn send_envelope(
    _socket_path: &Path,
    _envelope: &PeerEnvelope,
    _timeout_duration: Duration,
) -> Result<DeliveryReceipt> {
    Ok(DeliveryReceipt {
        delivered: false,
        error: Some("uds peer delivery is not supported on this platform".to_string()),
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn round_trip_envelope() {
        let tmp = TempDir::new().unwrap();
        let sock = tmp.path().join("peer.sock");
        let mut listener = PeerSocket::bind(&sock).await.unwrap();

        let sent = PeerEnvelope::new("alice", "bob", "hello there");

        let handle = tokio::spawn(async move {
            listener.accept_envelope().await.unwrap()
        });

        let receipt = send_envelope(&sock, &sent, Duration::from_secs(2)).await.unwrap();
        assert!(receipt.delivered, "{:?}", receipt.error);

        let (received, _) = handle.await.unwrap();
        assert_eq!(received.message_id, sent.message_id);
        assert_eq!(received.from, "alice");
        assert_eq!(received.body, "hello there");
    }

    #[tokio::test]
    async fn bound_socket_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let sock = tmp.path().join("peer.sock");
        let _listener = PeerSocket::bind(&sock).await.unwrap();
        let mode = std::fs::metadata(&sock).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "socket mode {mode:o}");
    }

    #[tokio::test]
    async fn long_path_falls_back_to_private_per_user_dir() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let long = tmp.path().join("x".repeat(120)).join(format!("spawngate-{}.sock", std::process::id()));
        let listener = PeerSocket::bind(&long).await.unwrap();
        let bound = listener.socket_path().to_path_buf();
        assert_eq!(bound.parent().unwrap(), fallback_dir());
        let dir_mode = std::fs::metadata(fallback_dir()).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "dir mode {dir_mode:o}");
        let sock_mode = std::fs::metadata(&bound).unwrap().permissions().mode() & 0o777;
        assert_eq!(sock_mode, 0o600);
        // Senders compute the same path and can still deliver.
        let receipt = send_envelope(&long, &PeerEnvelope::new("a", "b", "x"), Duration::from_millis(500))
            .await
            .unwrap();
        assert!(receipt.delivered, "{:?}", receipt.error);
        drop(listener);
        let _ = std::fs::remove_file(&bound);
    }

    #[tokio::test]
    async fn missing_socket_returns_undelivered() {
        let receipt = send_envelope(
            Path::new("/tmp/allternit-peers/does-not-exist.sock"),
            &PeerEnvelope::new("a", "b", "x"),
            Duration::from_millis(100),
        )
        .await
        .unwrap();
        assert!(!receipt.delivered);
    }
}
