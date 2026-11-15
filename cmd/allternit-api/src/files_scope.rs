//! Path scoping for the legacy `/api/v1/files/*` routes and the computer
//! file-transfer routes.
//!
//! Every path a caller sends is turned into one canonical absolute path
//! before any I/O: `~` and relative paths are joined to the home folder,
//! any `..` component is refused outright, and symlinks are resolved (for a
//! target that does not exist yet, the deepest existing ancestor is
//! canonicalized and the rest is appended). The policy is then checked on
//! that canonical path, component by component, so `/home/a` never matches
//! `/home/ab`.
//!
//! Two callers, two policies:
//!
//! - **Remote** (the request was replayed by a relay, carries a forwarding
//!   header, or came from a non-loopback peer): only the registered roots
//!   (`~/Allternit` and everything under it), the read-only plugin folders,
//!   and the exact MCP and HEARTBEAT.md files the iOS app manages.
//! - **Local** (the Desktop app on this machine): the home folder, minus a
//!   deny-list of secret stores (SSH and GPG keys, Keychains, cloud CLI
//!   credentials, agent CLI auth files, the Allternit data folders).
//!
//! Both policies refuse anything outside the home folder, and both refuse a
//! write or delete of the home folder itself.

use axum::{
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::ffi::OsString;
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};

/// Who is asking. Decided from the request, never from the path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Caller {
    /// The Desktop app (or anything else) on this machine, over loopback,
    /// with no relay or proxy marker.
    Local,
    /// Another device: replayed by the cloud relay (`x-allternit-relayed`),
    /// forwarded by a proxy, or a non-loopback TCP peer.
    Remote,
}

/// What the handler is about to do with the path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// list, read, raw, exists.
    Read,
    /// write, mkdir. A symlink at the target is followed and the target it
    /// points at is what gets checked.
    Write,
    /// delete. A symlink at the target is NOT followed: the link itself is
    /// what gets removed, so the link's own location is what gets checked.
    Delete,
}

/// Why a path was refused. Returned to the caller as `403`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Denied(pub &'static str);

impl Denied {
    pub fn message(self) -> &'static str {
        match self.0 {
            "invalid_path" => "That path is not valid.",
            "parent_dir" => "Paths can't use '..'.",
            "no_home" => "This computer has no home folder to serve files from.",
            "outside_home" => "That path is outside the home folder.",
            "protected" => "That path holds credentials or app data and can't be opened through the files API.",
            "outside_roots" => "That path is not in a shared Allternit folder.",
            "read_only" => "That folder is read-only from another device.",
            "home_itself" => "The home folder itself can't be changed.",
            "unresolvable" => "That path points at something that doesn't exist.",
            _ => "That path is not allowed.",
        }
    }
}

/// The `403` every file handler answers with for a refused path.
pub fn denied_response(denied: Denied) -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({
            "error": "path_not_allowed",
            "reason": denied.0,
            "message": denied.message(),
        })),
    )
        .into_response()
}

/// Decide the caller from the request. `peer` is the TCP peer from
/// `ConnectInfo`; every production listener sets it, so a missing peer counts
/// as remote outside tests (same rule as `auth::peer_is_loopback`).
pub fn caller(headers: &HeaderMap, peer: Option<SocketAddr>) -> Caller {
    let peer_is_loopback = match peer {
        Some(addr) => addr.ip().to_canonical().is_loopback(),
        None => cfg!(test),
    };
    // `has_forwarding_headers` includes `x-allternit-relayed`, the marker the
    // Desktop relay and agent-daemon set on every request they replay.
    if peer_is_loopback && !crate::auth::has_forwarding_headers(headers) {
        Caller::Local
    } else {
        Caller::Remote
    }
}

/// Home-relative locations that hold secrets. Matched case-insensitively,
/// component by component, so `~/.SSH/id_rsa` is refused on a
/// case-insensitive disk and `~/.sshx` is not confused with `~/.ssh`.
const HOME_SECRETS: &[&[&str]] = &[
    &[".ssh"],
    &[".gnupg"],
    &[".aws"],
    &[".azure"],
    &[".kube"],
    &[".netrc"],
    &[".password-store"],
    &[".npmrc"],
    &[".pypirc"],
    &[".git-credentials"],
    &[".docker", "config.json"],
    &[".config", "gcloud"],
    &[".config", "gh"],
    &[".config", "stripe"],
    &["Library", "Keychains"],
    // Agent CLI credentials.
    &[".claude", ".credentials.json"],
    &[".codex", "auth.json"],
    &[".gizzi", "credentials.json"],
    &[".local", "share", "gizzi-code", "auth.json"],
    &[".local", "share", "gizzi-code", "fabric-lease.key"],
    &[".local", "share", "tailscale"],
    // Allternit's own tokens, keys and private state under ~/.allternit.
    // The rest of ~/.allternit (skills, plugins, mcps, commands, agents)
    // stays reachable for the Desktop app.
    &[".allternit", "phone-gateway.json"],
    &[".allternit", "subscriptions"],
    &[".allternit", "incus-client"],
    &[".allternit", "factory", "fabric"],
    &[".allternit", "factory", "registry.json"],
    &[".allternit", "session-caretaker"],
    &[".allternit", "system-one"],
];

/// Folders another device may read but not change: where plugins are
/// discovered (`PLUGIN_DIR_CANDIDATES` in allternit-ai `src/plugins/fileSystem.ts`
/// and `pluginRoots` in iOS `CapabilitiesClient.swift`).
const REMOTE_READ_ONLY_DIRS: &[&[&str]] = &[
    &[".allternit", "plugins"],
    &[".agents", "plugins"],
    &[".codex", "plugins"],
];

/// MCP server folders iOS lists and edits (`McpServersClient.serverFolders`).
/// Another device may list the folder, read its direct children and write or
/// delete its direct `*.json` children. Nothing deeper.
const REMOTE_MCP_DIRS: &[&[&str]] = &[
    &[".allternit", "mcps"],
    &[".agents", "mcps"],
    &[".codex", "mcps"],
];

/// MCP config files iOS reads (`McpServersClient.configFiles`). Exact files,
/// read-only: iOS never writes them (servers found there are not editable),
/// and they can carry hooks and tokens.
const REMOTE_MCP_CONFIG_FILES: &[&[&str]] = &[
    &[".config", "allternit-shell", "allternit.json"],
    &[".config", "gizzi", "allternit.json"],
    &[".config", "gizzi-code", "allternit.json"],
    &[".claude", "settings.json"],
    &[".codex", "config.toml"],
    &[".gemini", "mcp.json"],
];

/// The registered root another device may read and write under.
const REMOTE_ROOT: &str = "Allternit";

/// Resolves and checks file paths for one home folder.
#[derive(Clone, Debug)]
pub struct FilesScope {
    /// Canonical home folder. `None` when the process has no home folder:
    /// every request is then refused.
    home: Option<PathBuf>,
    /// Canonical absolute folders that are always refused (the Allternit
    /// data folders).
    protected: Vec<PathBuf>,
}

impl FilesScope {
    /// The scope for this process: `dirs::home_dir()`, with the Allternit
    /// data folders protected (`ALLTERNIT_DATA_DIR`, `<data_dir>/allternit`,
    /// and the Desktop's Electron userData `<data_dir>/@allternit`).
    pub fn from_env() -> Self {
        let mut protected = Vec::new();
        if let Some(dir) = std::env::var_os("ALLTERNIT_DATA_DIR").filter(|v| !v.is_empty()) {
            protected.push(PathBuf::from(dir));
        }
        if let Some(data) = dirs::data_dir() {
            protected.push(data.join("allternit"));
            protected.push(data.join("@allternit"));
        }
        Self::new(dirs::home_dir(), protected)
    }

    /// A scope for an explicit home folder and protected folders.
    pub fn new(home: Option<PathBuf>, protected: Vec<PathBuf>) -> Self {
        let home = home
            .filter(|h| h.is_absolute())
            .and_then(|h| resolve(&normalize(&h).ok()?, true).ok());
        let protected = protected
            .into_iter()
            .filter(|p| p.is_absolute())
            .filter_map(|p| resolve(&normalize(&p).ok()?, true).ok())
            .collect();
        Self { home, protected }
    }

    /// Turn a caller's path into the canonical path to operate on, or refuse it.
    pub fn authorize(&self, raw: &str, caller: Caller, access: Access) -> Result<PathBuf, Denied> {
        let home = self.home.as_deref().ok_or(Denied("no_home"))?;
        if raw.contains('\0') {
            return Err(Denied("invalid_path"));
        }
        let expanded = expand(raw, home);
        let lexical = normalize(&expanded)?;
        let canonical = resolve(&lexical, access != Access::Delete)?;
        let rel = canonical.strip_prefix(home).map_err(|_| Denied("outside_home"))?;
        let rel: Vec<String> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        if rel.is_empty() && access != Access::Read {
            return Err(Denied("home_itself"));
        }
        if is_secret(&rel) || self.protected.iter().any(|p| starts_with_ci(&canonical, p)) {
            return Err(Denied("protected"));
        }
        if caller == Caller::Remote {
            remote_allows(&rel, access)?;
        }
        Ok(canonical)
    }

    /// True when an entry found while listing an already-authorized folder may
    /// be shown. Lexical only (the entry's parent is already canonical): it
    /// keeps recursive listings out of secret folders and, for remote callers,
    /// inside the allowed tree.
    pub fn listable(&self, entry: &Path, caller: Caller) -> bool {
        let Some(home) = self.home.as_deref() else { return false };
        let Ok(rel) = entry.strip_prefix(home) else { return false };
        let rel: Vec<String> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        if is_secret(&rel) || self.protected.iter().any(|p| starts_with_ci(entry, p)) {
            return false;
        }
        caller == Caller::Local || remote_allows(&rel, Access::Read).is_ok()
    }
}

/// Paths on another computer (a cloud computer or VM guest) that the
/// computer file routes pull or push. Nothing on the guest can be resolved
/// from here, so the check is lexical: an absolute path, no `..`, no NUL, and
/// none of the secret locations anywhere in it (case-insensitive), so
/// `/home/user/.ssh/authorized_keys` and `/root/.aws/credentials` are refused.
pub fn check_guest_path(raw: &str) -> Result<(), Denied> {
    if raw.is_empty() || raw.contains('\0') {
        return Err(Denied("invalid_path"));
    }
    let unified = raw.replace('\\', "/");
    let absolute = unified.starts_with('/')
        || unified.starts_with("~/")
        || unified == "~"
        || unified.as_bytes().get(1) == Some(&b':');
    if !absolute {
        return Err(Denied("invalid_path"));
    }
    let parts: Vec<String> = unified
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .map(str::to_string)
        .collect();
    if parts.iter().any(|p| p == "..") {
        return Err(Denied("parent_dir"));
    }
    for start in 0..parts.len() {
        if is_secret(&parts[start..]) {
            return Err(Denied("protected"));
        }
    }
    let lower: Vec<String> = parts.iter().map(|p| p.to_ascii_lowercase()).collect();
    if lower.len() >= 2 && lower[0] == "etc" && matches!(lower[1].as_str(), "shadow" | "gshadow" | "sudoers") {
        return Err(Denied("protected"));
    }
    Ok(())
}

/// `~`, `~/x`, absolute and home-relative paths, joined the way the routes
/// always have. `~//etc` is `~/etc`, never `/etc`.
fn expand(raw: &str, home: &Path) -> PathBuf {
    if raw == "~" {
        home.to_path_buf()
    } else if let Some(rest) = raw.strip_prefix("~/") {
        home.join(rest.trim_start_matches('/'))
    } else if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        home.join(raw)
    }
}

/// Drop `.` components and refuse `..` outright. A lexical `..` can't be
/// resolved safely before symlinks are (`a/link/..` is not `a`), and no
/// client needs one.
fn normalize(path: &Path) -> Result<PathBuf, Denied> {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => return Err(Denied("parent_dir")),
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    Ok(out)
}

/// Canonicalize the deepest existing ancestor and append the rest. With
/// `follow_leaf == false` the last component is kept as written (a symlink
/// there stays a symlink), only its parent is resolved. A path that exists
/// but can't be canonicalized (a dangling symlink) is refused, so a write
/// can never follow a dangling link out of an allowed folder.
fn resolve(path: &Path, follow_leaf: bool) -> Result<PathBuf, Denied> {
    let (base, leaf) = match (follow_leaf, path.parent(), path.file_name()) {
        (false, Some(parent), Some(name)) => (parent.to_path_buf(), Some(name.to_os_string())),
        _ => (path.to_path_buf(), None),
    };
    let mut existing = base;
    let mut rest: Vec<OsString> = Vec::new();
    while std::fs::symlink_metadata(&existing).is_err() {
        match existing.file_name() {
            Some(name) => rest.push(name.to_os_string()),
            None => break,
        }
        if !existing.pop() {
            break;
        }
    }
    let mut out = std::fs::canonicalize(&existing).map_err(|_| Denied("unresolvable"))?;
    for name in rest.into_iter().rev() {
        out.push(name);
    }
    if let Some(name) = leaf {
        out.push(name);
    }
    Ok(out)
}

fn eq_ci(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// `path` is `prefix` or inside it, compared component by component and
/// case-insensitively.
fn starts_with_ci(path: &Path, prefix: &Path) -> bool {
    let mut path = path.components();
    for want in prefix.components() {
        match path.next() {
            Some(got) if eq_ci(&got.as_os_str().to_string_lossy(), &want.as_os_str().to_string_lossy()) => {}
            _ => return false,
        }
    }
    true
}

fn rel_starts_with_ci(rel: &[String], prefix: &[&str]) -> bool {
    rel.len() >= prefix.len() && rel.iter().zip(prefix).all(|(a, b)| eq_ci(a, b))
}

fn rel_starts_with(rel: &[String], prefix: &[&str]) -> bool {
    rel.len() >= prefix.len() && rel.iter().zip(prefix).all(|(a, b)| a == b)
}

fn is_secret(rel: &[String]) -> bool {
    if HOME_SECRETS.iter().any(|secret| rel_starts_with_ci(rel, secret)) {
        return true;
    }
    // ~/.allternit/<anything>.env… (incus-host.env, tart-host.env.disabled…)
    rel.len() >= 2 && eq_ci(&rel[0], ".allternit") && rel[1].to_lowercase().contains(".env")
}

/// The remote policy, on the canonical home-relative components. Exact
/// (case-sensitive) matching: the path is already canonical, so on a
/// case-insensitive disk it carries the on-disk case of every existing part.
fn remote_allows(rel: &[String], access: Access) -> Result<(), Denied> {
    // ~/Allternit: the folder itself is readable, everything under it is
    // readable and writable.
    if rel_starts_with(rel, &[REMOTE_ROOT]) {
        return if rel.len() > 1 || access == Access::Read {
            Ok(())
        } else {
            Err(Denied("read_only"))
        };
    }
    if REMOTE_READ_ONLY_DIRS.iter().any(|dir| rel_starts_with(rel, dir)) {
        return if access == Access::Read { Ok(()) } else { Err(Denied("read_only")) };
    }
    for dir in REMOTE_MCP_DIRS {
        if !rel_starts_with(rel, dir) {
            continue;
        }
        return match (rel.len() - dir.len(), access) {
            (0, Access::Read) | (1, Access::Read) => Ok(()),
            (1, _) if rel[rel.len() - 1].to_lowercase().ends_with(".json") => Ok(()),
            (0 | 1, _) => Err(Denied("read_only")),
            _ => Err(Denied("outside_roots")),
        };
    }
    if REMOTE_MCP_CONFIG_FILES.iter().any(|file| rel.len() == file.len() && rel_starts_with(rel, file)) {
        return if access == Access::Read { Ok(()) } else { Err(Denied("read_only")) };
    }
    // An agent's HEARTBEAT.md, the exact file iOS and the web's
    // HeartbeatScheduler read and write: agents/<id>/.allternit/governance/HEARTBEAT.md.
    if rel.len() == 5
        && rel[0] == "agents"
        && rel[2] == ".allternit"
        && rel[3] == "governance"
        && rel[4] == "HEARTBEAT.md"
    {
        return Ok(());
    }
    Err(Denied("outside_roots"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guest_paths_refuse_secrets_and_parent_dirs() {
        for bad in [
            "",
            "relative/file",
            "/home/user/../root/x",
            "/home/user/.ssh/authorized_keys",
            "/root/.AWS/credentials",
            "C:/Users/me/.ssh/id_rsa",
            "C:\\Users\\me\\.ssh\\id_rsa",
            "/etc/shadow",
            "/a\0b",
        ] {
            assert!(check_guest_path(bad).is_err(), "{bad:?} should be refused");
        }
        for ok in ["/tmp/report.pdf", "/home/user/Downloads/a.txt", "C:/Users/Public/Downloads/a.txt", "~/Desktop/a.png"] {
            assert!(check_guest_path(ok).is_ok(), "{ok:?} should be allowed");
        }
    }
}
