//! Daemon configuration: identity location, cloud URL, and the `node.core`
//! scopes (fs roots, exec toggle, desktop launch command, pane engine socket).
//!
//! Everything is overridable through `~/.config/allternit/node-config.json`
//! (or `ALLTERNIT_NODE_CONFIG`). Missing fields fall back to safe defaults:
//! fs access is scoped to the user's home directory and exec is enabled.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NodeConfig {
    /// Cloud API base URL the relay connects out to.
    pub cloud_api_url: String,
    /// Path of the paired-runtime identity the daemon adopts
    /// (`runtime-identity.json`, same file the desktop app / agent-daemon
    /// uses).
    pub identity_path: PathBuf,
    /// Filesystem roots the `node.fs` scope is allowed to touch. Paths
    /// outside every root are rejected.
    pub fs_roots: Vec<PathBuf>,
    /// Whether `node.exec` (arbitrary command execution) is served.
    pub exec_enabled: bool,
    /// Optional explicit desktop-launch override; when unset the platform
    /// default is used (`open -na "Allternit Desktop"` on macOS, the
    /// `allternit-desktop` binary on Linux).
    pub desktop_launch: Option<String>,
    /// The Factory pane engine socket that serves terminals. Unset: the
    /// engine's own default paths are probed (see `engine::socket_candidates`).
    /// When no engine answers, terminals fall back to in-process PTYs.
    pub pane_socket: Option<PathBuf>,
}

impl Default for NodeConfig {
    fn default() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        Self {
            cloud_api_url: "https://api.allternit.com".to_string(),
            identity_path: PathBuf::from(&home)
                .join(".config")
                .join("allternit")
                .join("runtime-identity.json"),
            fs_roots: vec![PathBuf::from(&home)],
            exec_enabled: true,
            desktop_launch: None,
            pane_socket: None,
        }
    }
}

impl NodeConfig {
    /// Load configuration from `ALLTERNIT_NODE_CONFIG` (or the default
    /// `~/.config/allternit/node-config.json`), overlaying environment
    /// overrides. A missing file is not an error — defaults apply.
    pub fn load() -> anyhow::Result<Self> {
        let mut config = Self::default();
        let path = std::env::var("ALLTERNIT_NODE_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                PathBuf::from(home)
                    .join(".config")
                    .join("allternit")
                    .join("node-config.json")
            });
        if path.exists() {
            let raw = std::fs::read_to_string(&path)?;
            let file_config: NodeConfig = serde_json::from_str(&raw)
                .map_err(|error| anyhow::anyhow!("invalid node config {}: {error}", path.display()))?;
            config = file_config;
        }
        if let Ok(url) = std::env::var("ALLTERNIT_CLOUD_API_URL") {
            config.cloud_api_url = url.trim_end_matches('/').to_string();
        }
        if let Ok(identity) = std::env::var("ALLTERNIT_RUNTIME_IDENTITY_PATH") {
            config.identity_path = PathBuf::from(identity);
        }
        Ok(config)
    }

    /// Resolve `path` against the configured fs roots: the path must exist
    /// under (or be creatable under) one of the roots once canonicalized.
    /// Non-existent leaf components are canonicalized through their nearest
    /// existing ancestor so writes into new files still scope-check.
    pub fn scope_check(&self, path: &std::path::Path) -> anyhow::Result<PathBuf> {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            let home = self
                .fs_roots
                .first()
                .cloned()
                .unwrap_or_else(|| PathBuf::from("."));
            home.join(path)
        };
        let canonical = canonicalize_missing_ok(&absolute)?;
        for root in &self.fs_roots {
            let root_canonical = canonicalize_missing_ok(root)?;
            if canonical.starts_with(&root_canonical) {
                return Ok(canonical);
            }
        }
        anyhow::bail!(
            "path {} is outside the configured fs roots",
            path.display()
        )
    }
}

/// Canonicalize `path`, tolerating a non-existent tail by walking up to the
/// nearest existing ancestor and re-appending the missing components.
fn canonicalize_missing_ok(path: &std::path::Path) -> anyhow::Result<PathBuf> {
    let mut existing = path;
    let mut missing: Vec<std::ffi::OsString> = Vec::new();
    loop {
        match std::fs::canonicalize(existing) {
            Ok(base) => {
                let mut resolved = base;
                for component in missing.into_iter().rev() {
                    resolved.push(component);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match existing.file_name() {
                    Some(name) => {
                        missing.push(name.to_os_string());
                        existing = match existing.parent() {
                            Some(parent) => parent,
                            None => anyhow::bail!("path {} does not resolve", path.display()),
                        };
                    }
                    None => anyhow::bail!("path {} does not resolve", path.display()),
                }
            }
            Err(error) => return Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_check_allows_inside_root_and_rejects_outside() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        let config = NodeConfig {
            fs_roots: vec![root.clone()],
            ..NodeConfig::default()
        };
        // Existing path inside the root.
        let inside = root.join("note.txt");
        std::fs::write(&inside, "hi").unwrap();
        assert!(config.scope_check(&inside).is_ok());
        // New file inside the root (missing tail canonicalizes via ancestor).
        assert!(config.scope_check(&root.join("new").join("file.txt")).is_ok());
        // Outside the root — including via `..` — is rejected.
        let outside = temp.path().join("other.txt");
        std::fs::write(&outside, "hi").unwrap();
        assert!(config.scope_check(&outside).is_err());
        assert!(config.scope_check(&root.join("..").join("other.txt")).is_err());
    }
}
