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
//!   header, or came from a non-loopback peer). The relay is still the
//!   authenticated owner, so this keeps every feature the web, PWA and iOS
//!   use today and nothing more: `~/Allternit`, agent workspaces
//!   (`~/agents`), the skill/command/webhook/connector/cli-tool folders,
//!   plugin folders (read, manifest writes, whole-plugin delete), the MCP
//!   folders and config files, and a shallow listing of the home folder.
//! - **Local** (the Desktop app on this machine): the home folder, minus a
//!   deny-list of secret stores (SSH and GPG keys, Keychains, cloud CLI
//!   credentials, agent CLI auth files, the Allternit data folders).
//!
//! - **Whole disk** (step 2.11): a relayed caller whose request carries the
//!   cloud's `x-allternit-whole-disk: on` AND a valid cloud relay signature
//!   ([`whole_disk_gate`]). The owner turned whole-disk files on for this
//!   computer: the home folder and every path outside it, minus the secrets
//!   deny-list (applied to every suffix of the path, so `/root/.ssh` and
//!   `/home/other/.aws` are refused), its file name patterns, the system
//!   secret stores in [`SYSTEM_SECRETS`] and the Allternit data folders.
//!   Without the grant a relayed path outside home answers `403
//!   whole_disk_off`, the error the cloud uses.
//!
//! Local and remote policies refuse anything outside the home folder; every
//! policy refuses a write or delete of the home folder (and the disk root)
//! itself. Names are compared in Unicode
//! NFC (APFS treats NFC and NFD spellings as one name). Reads and writes open
//! the file with `O_NOFOLLOW` and then check where the open file really is
//! (`F_GETPATH` on macOS, `/proc/self/fd` on Linux), so a path swapped for a
//! symlink between the check and the open is refused.

use axum::{
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::ffi::OsString;
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use unicode_normalization::UnicodeNormalization;

/// Who is asking. Decided from the request, never from the path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Caller {
    /// The Desktop app (or anything else) on this machine, over loopback,
    /// with no relay or proxy marker.
    Local,
    /// Another device: replayed by the cloud relay (`x-allternit-relayed`),
    /// forwarded by a proxy, or a non-loopback TCP peer.
    Remote,
    /// A relayed owner request the cloud marked whole-disk and signed
    /// ([`WholeDiskGrant`]). Only [`caller_with_grant`] returns it.
    WholeDisk,
}

/// The trusted header the cloud's owner proxy adds while whole-disk files are
/// on for this computer (cloud `computer_file_roots::WHOLE_DISK_HEADER`).
pub const WHOLE_DISK_HEADER: &str = "x-allternit-whole-disk";
/// The cloud's message for `whole_disk_off`, word for word.
pub const WHOLE_DISK_OFF_MESSAGE: &str =
    "This folder is outside the Files drive. Turn on whole-disk browsing for this computer in its Files settings first.";

/// Request extension set by [`whole_disk_gate`] only after the cloud relay
/// signature checked out. Clients can't set an extension.
#[derive(Clone, Copy, Debug)]
pub struct WholeDiskGrant;

/// Largest body the gate buffers to check a signature. Bigger (or unsized)
/// bodies pass through untouched and get no grant.
const MAX_GRANT_BODY: u64 = 8 * 1024 * 1024;

/// Middleware for the file routes: a request with `x-allternit-whole-disk: on`
/// gets [`WholeDiskGrant`] only when it is relayed and its cloud relay
/// signature (`relay_auth::verify_relay`, the 1.8 gate) is valid for this
/// runtime. A local or unsigned caller sending the header gets nothing extra;
/// the request goes on as before. The signature key is the
/// `Arc<dyn RelaySecret>` extension when a layer set one (tests), else
/// [`crate::relay_auth::process_secret`].
pub async fn whole_disk_gate(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let wants = req
        .headers()
        .get(WHOLE_DISK_HEADER)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("on"));
    let relayed = req.headers().contains_key(crate::cowork_devices_routes::RELAYED_HEADER);
    if !wants || !relayed {
        return next.run(req).await;
    }
    // Only a body of known size, at most MAX_GRANT_BODY, is checked. A
    // relayed replay always has one (the relay sends the whole body).
    let sized = axum::body::HttpBody::size_hint(req.body()).upper().is_some_and(|n| n <= MAX_GRANT_BODY);
    if !sized {
        return next.run(req).await;
    }
    let (mut parts, body) = req.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, MAX_GRANT_BODY as usize).await else {
        return (StatusCode::PAYLOAD_TOO_LARGE, Json(json!({"error": "body too large"}))).into_response();
    };
    let secret = parts
        .extensions
        .get::<Arc<dyn crate::relay_auth::RelaySecret>>()
        .cloned()
        .unwrap_or_else(crate::relay_auth::process_secret);
    let path = parts
        .extensions
        .get::<axum::extract::OriginalUri>()
        .map(|u| u.0.path().to_string())
        .unwrap_or_else(|| parts.uri.path().to_string());
    let signed = crate::relay_auth::verify_relay(
        secret.as_ref(),
        &parts.headers,
        parts.method.as_str(),
        &path,
        &bytes,
        crate::relay_auth::unix_now(),
    )
    .is_ok();
    if signed {
        parts.extensions.insert(WholeDiskGrant);
    }
    next.run(axum::extract::Request::from_parts(parts, axum::body::Body::from(bytes))).await
}

/// [`caller`], upgraded to [`Caller::WholeDisk`] for a remote request that
/// [`whole_disk_gate`] granted.
pub fn caller_with_grant(headers: &HeaderMap, peer: Option<SocketAddr>, granted: bool) -> Caller {
    match caller(headers, peer) {
        Caller::Remote if granted => Caller::WholeDisk,
        other => other,
    }
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
            "read_only" => "That path is read-only from another device.",
            "home_itself" => "The home folder itself can't be changed.",
            "unresolvable" => "That path points at something that doesn't exist.",
            "local_only" => "Only a process on this computer can record bot-written files.",
            "whole_disk_off" => WHOLE_DISK_OFF_MESSAGE,
            "disk_root_itself" => "The disk root itself can't be changed.",
            _ => "That path is not allowed.",
        }
    }
}

/// The `403` every file handler answers with for a refused path.
pub fn denied_response(denied: Denied) -> Response {
    if denied.0 == "whole_disk_off" {
        // Same shape as the cloud's relay gate (`computer_file_roots::off_response`).
        return (StatusCode::FORBIDDEN, Json(json!({"error": "whole_disk_off", "message": WHOLE_DISK_OFF_MESSAGE}))).into_response();
    }
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

/// The shared secrets deny-list (`assets/secrets-deny.json`). Gizzi's portable
/// workspace bundle reads the same file, so there is one list to edit.
pub const SECRETS_DENY_JSON: &str = include_str!("../assets/secrets-deny.json");

/// Home-relative locations that hold secrets (`home_paths` in
/// [`SECRETS_DENY_JSON`]). Matched case-insensitively, component by
/// component, so `~/.SSH/id_rsa` is refused on a case-insensitive disk and
/// `~/.sshx` is not confused with `~/.ssh`.
fn home_secrets() -> &'static [Vec<String>] {
    static LIST: std::sync::OnceLock<Vec<Vec<String>>> = std::sync::OnceLock::new();
    LIST.get_or_init(|| {
        #[derive(serde::Deserialize)]
        struct DenyList {
            home_paths: Vec<Vec<String>>,
        }
        // Compiled in, so a broken file fails every test, not a user.
        let list: DenyList = serde_json::from_str(SECRETS_DENY_JSON).expect("assets/secrets-deny.json is valid");
        assert!(list.home_paths.iter().all(|p| !p.is_empty()), "assets/secrets-deny.json has an empty home path");
        list.home_paths
    })
}

/// Folders another device may read and write under (not the folder itself):
/// what allternit-ai `src/plugins/fileSystem.ts` scans and Customize writes
/// (`SKILL_DIR_CANDIDATES`, `COMMAND_DIR_CANDIDATES`,
/// `COMMAND_MARKDOWN_DIR_CANDIDATES`, `WEBHOOK_DIR_CANDIDATES`,
/// `CONNECTOR_DIR_CANDIDATES`, `CLI_TOOL_DIR_CANDIDATES`) and iOS
/// `CapabilitiesClient.skillRoots`.
const REMOTE_RW_DIRS: &[&[&str]] = &[
    &[".allternit", "skills"],
    &[".agents", "skills"],
    &[".codex", "skills"],
    &[".allternit", "commands"],
    &[".agents", "commands"],
    &[".codex", "commands"],
    &[".config", "gizzi-code", "command"],
    &[".config", "gizzi-code", "commands"],
    &[".claude", "commands"],
    &[".allternit", "webhooks"],
    &[".agents", "webhooks"],
    &[".codex", "webhooks"],
    &[".allternit", "connectors"],
    &[".agents", "connectors"],
    &[".codex", "connectors"],
    &[".allternit", "cli-tools"],
    &[".agents", "cli-tools"],
    &[".codex", "cli-tools"],
];

/// Plugin folders (`PLUGIN_DIR_CANDIDATES`, iOS `pluginRoots`). Another
/// device may read anything, write a plugin's manifest (iOS `setEnabled`)
/// and delete a whole plugin folder (iOS `remove`). Any other write is
/// refused: plugins carry code.
const REMOTE_PLUGIN_DIRS: &[&[&str]] = &[
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

/// A plugin's manifest, relative to the plugin folder: the candidates iOS
/// `scanPlugins` reads and `setEnabled` writes back.
const PLUGIN_MANIFESTS: &[&[&str]] = &[&["config.json"], &["plugin.json"], &[".claude-plugin", "plugin.json"]];

/// MCP config files the web and iOS read (`MCP_CONFIG_FILE_CANDIDATES`,
/// `McpServersClient.configFiles`). Exact files, read-only: neither writes
/// them over the files API, and they can carry hooks and tokens.
const REMOTE_MCP_CONFIG_FILES: &[&[&str]] = &[
    &[".config", "gizzi-code", "gizzi.json"],
    &[".config", "gizzi", "gizzi.json"],
    &[".config", "allternit-shell", "allternit.json"],
    &[".config", "gizzi", "allternit.json"],
    &[".config", "gizzi-code", "allternit.json"],
    &[".claude", "settings.json"],
    &[".codex", "config.toml"],
    &[".gemini", "mcp.json"],
];

/// Top-level folders another device may read and write under: the
/// registered root and the agent workspaces (`WORKSPACE_ROOT = 'agents'` in
/// allternit-ai `agent-workspace.service.ts`, HEARTBEAT.md on iOS).
const REMOTE_RW_TOP: &[&str] = &["Allternit", "agents"];

/// Why `open_verified` failed.
#[derive(Debug)]
pub enum OpenError {
    /// The opened file is not where the scope allows (it moved under us).
    Denied(Denied),
    /// The open itself failed (missing, a symlink at the leaf, permissions).
    Io(std::io::Error),
}

/// How many bot-written paths the ledger keeps, and for how long.
const LEDGER_MAX: usize = 5_000;
const LEDGER_MAX_AGE_SECS: i64 = 30 * 24 * 60 * 60;

/// Files an agent's tools (gizzi `write`, `edit`, `apply_patch`, `notebook`,
/// `workspace-edit`) wrote on this computer. gizzi reports each
/// `File.Event.Edited` to `POST /files/bot-writes` over loopback; a relayed
/// caller may then READ that exact file (never write or delete it), so a bot
/// thread's file cards open on the web and phone. Bounded by count and age,
/// keyed by canonical path, and kept in the protected data folder so the
/// files API itself can't edit it.
#[derive(Debug, Default)]
pub struct BotWriteLedger {
    entries: std::sync::Mutex<std::collections::VecDeque<(String, i64)>>,
    file: Option<PathBuf>,
}

impl BotWriteLedger {
    /// A ledger kept in `file` (JSON), loaded now and saved on every record.
    pub fn persisted(file: PathBuf) -> Self {
        let entries = std::fs::read(&file)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Vec<(String, i64)>>(&bytes).ok())
            .unwrap_or_default();
        let ledger = Self { entries: std::sync::Mutex::new(entries.into()), file: Some(file) };
        ledger.prune(&mut ledger.entries.lock().unwrap_or_else(|p| p.into_inner()));
        ledger
    }

    fn key(path: &Path) -> String {
        path.to_string_lossy().nfc().collect()
    }

    fn prune(&self, entries: &mut std::collections::VecDeque<(String, i64)>) {
        let cutoff = chrono::Utc::now().timestamp() - LEDGER_MAX_AGE_SECS;
        entries.retain(|(_, at)| *at >= cutoff);
        while entries.len() > LEDGER_MAX {
            entries.pop_front();
        }
    }

    fn record(&self, canonical: &Path) {
        let key = Self::key(canonical);
        let mut entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        entries.retain(|(path, _)| *path != key);
        entries.push_back((key, chrono::Utc::now().timestamp()));
        self.prune(&mut entries);
        if let Some(file) = &self.file {
            if let Ok(bytes) = serde_json::to_vec(&entries.iter().collect::<Vec<_>>()) {
                if let Some(parent) = file.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let tmp = file.with_extension("json.tmp");
                if std::fs::write(&tmp, bytes).is_ok() {
                    let _ = std::fs::rename(&tmp, file);
                }
            }
        }
    }

    fn contains(&self, canonical: &Path) -> bool {
        let key = Self::key(canonical);
        let cutoff = chrono::Utc::now().timestamp() - LEDGER_MAX_AGE_SECS;
        self.entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .any(|(path, at)| *path == key && *at >= cutoff)
    }
}

/// Resolves and checks file paths for one home folder.
#[derive(Clone, Debug)]
pub struct FilesScope {
    /// Canonical home folder. `None` when the process has no home folder:
    /// every request is then refused.
    home: Option<PathBuf>,
    /// Canonical absolute folders that are always refused (the Allternit
    /// data folders).
    protected: Vec<PathBuf>,
    /// Files agents wrote here; relayed callers may read exactly these.
    bot_writes: Arc<BotWriteLedger>,
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
        let data_dir = std::env::var_os("ALLTERNIT_DATA_DIR")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| dirs::data_dir().map(|d| d.join("allternit")));
        let mut scope = Self::new(dirs::home_dir(), protected);
        if let Some(dir) = data_dir {
            scope.bot_writes = Arc::new(BotWriteLedger::persisted(dir.join("files-bot-writes.json")));
        }
        scope
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
        Self { home, protected, bot_writes: Arc::new(BotWriteLedger::default()) }
    }

    /// Record that an agent's tool wrote `raw`. Only a local caller (gizzi on
    /// this computer) may record, and never a secret location.
    pub fn record_bot_write(&self, raw: &str, caller: Caller) -> Result<(), Denied> {
        if caller != Caller::Local {
            return Err(Denied("local_only"));
        }
        let (canonical, rel) = self.check(raw, Access::Read, caller)?;
        // Never a path outside home (the caller is local, so check refused it).
        let rel = rel.ok_or(Denied("outside_home"))?;
        if rel.is_empty() {
            return Err(Denied("home_itself"));
        }
        self.bot_writes.record(&canonical);
        Ok(())
    }

    /// Turn a caller's path into the canonical path to operate on, or refuse it.
    pub fn authorize(&self, raw: &str, caller: Caller, access: Access) -> Result<PathBuf, Denied> {
        let (canonical, rel) = self.check(raw, access, caller)?;
        if caller == Caller::Remote {
            // check() refused every path outside home for a remote caller.
            let rel = rel.ok_or(Denied("whole_disk_off"))?;
            if let Err(denied) = remote_allows(&rel, access) {
                // A file an agent wrote here: readable, never writable.
                if !(access == Access::Read && self.bot_writes.contains(&canonical)) {
                    return Err(denied);
                }
            }
        }
        Ok(canonical)
    }

    /// Resolution plus the checks every caller shares. Returns the canonical
    /// path and its home-relative parts in NFC; `None` for a path outside
    /// home, which only [`Caller::WholeDisk`] gets (a remote caller gets
    /// `whole_disk_off`, a local one `outside_home`).
    fn check(&self, raw: &str, access: Access, caller: Caller) -> Result<(PathBuf, Option<Vec<String>>), Denied> {
        let home = self.home.as_deref().ok_or(Denied("no_home"))?;
        if raw.contains('\0') {
            return Err(Denied("invalid_path"));
        }
        let expanded = expand(raw, home);
        let lexical = normalize(&expanded)?;
        let canonical = resolve(&lexical, access != Access::Delete)?;
        if self.protected.iter().any(|p| starts_with_ci(&canonical, p)) {
            return Err(Denied("protected"));
        }
        let Ok(rel) = canonical.strip_prefix(home) else {
            return match caller {
                Caller::WholeDisk => {
                    let parts = disk_parts(&canonical);
                    if parts.is_empty() && access != Access::Read {
                        return Err(Denied("disk_root_itself"));
                    }
                    if disk_secret(&parts) {
                        return Err(Denied("protected"));
                    }
                    Ok((canonical, None))
                }
                Caller::Remote => Err(Denied("whole_disk_off")),
                Caller::Local => Err(Denied("outside_home")),
            };
        };
        let rel = nfc_parts(rel);
        if rel.is_empty() && access != Access::Read {
            return Err(Denied("home_itself"));
        }
        if is_secret(&rel) || (caller == Caller::WholeDisk && whole_disk_secret(&rel)) {
            return Err(Denied("protected"));
        }
        Ok((canonical, Some(rel)))
    }

    /// True when `path` (canonical) is outside the home folder: a whole-disk
    /// path. Files drive roots there are disk roots (never shared, never
    /// given to an agent, hidden without the grant).
    pub fn is_disk_path(&self, path: &Path) -> bool {
        match self.home.as_deref() {
            Some(home) => !path.starts_with(home),
            None => true,
        }
    }

    /// `authorize` for `/files/list`. A remote caller may also list the home
    /// folder itself, but only one level deep (the web's `readDir('.')`
    /// probe); `listable` hides the secret entries in it.
    pub fn authorize_list(&self, raw: &str, caller: Caller, recursive: bool) -> Result<PathBuf, Denied> {
        let (canonical, rel) = self.check(raw, Access::Read, caller)?;
        if caller == Caller::Remote {
            let rel = rel.ok_or(Denied("whole_disk_off"))?;
            if rel.is_empty() {
                if recursive {
                    return Err(Denied("outside_roots"));
                }
            } else {
                remote_allows(&rel, Access::Read)?;
            }
        }
        Ok(canonical)
    }

    /// A folder a caller wants to add as a Files drive root
    /// (`POST /api/v1/files/v2/roots`, SPEC section 12). Same resolution and
    /// deny-list as `authorize`, for any caller; and because a root exposes
    /// everything under it, the folder may not be the home folder itself nor
    /// hold a secret store, a protected folder or one of `extra_protected`
    /// (the Files drive's own data folder).
    ///
    /// `caller` is [`Caller::Local`] for the legacy behaviour (home only).
    /// [`Caller::WholeDisk`] also accepts a disk root: a folder outside home
    /// that holds neither the home folder nor a system secret store, so `/`,
    /// `C:\`, `/home`, `/Users` and `/etc` are refused (the drive indexes
    /// every root it adds). [`Caller::Remote`] outside home: `whole_disk_off`.
    pub fn authorize_root(&self, raw: &str, extra_protected: &[PathBuf], caller: Caller) -> Result<PathBuf, Denied> {
        let (canonical, _rel) = self.check(raw, Access::Write, caller)?;
        let home = self.home.as_deref().ok_or(Denied("no_home"))?;
        let parts = disk_parts(&canonical);
        let holds_system_secret = SYSTEM_SECRETS
            .iter()
            .any(|secret| parts.len() < secret.len() && parts.iter().zip(secret.iter()).all(|(a, b)| eq_ci(a, b)));
        let holds_secret = holds_system_secret
            || home_secrets()
                .iter()
                .map(|parts| parts.iter().fold(home.to_path_buf(), |p, c| p.join(c)))
                .chain(std::iter::once(home.join(".allternit")))
                .any(|secret| starts_with_ci(&secret, &canonical));
        let extra: Vec<PathBuf> = extra_protected
            .iter()
            .filter(|p| p.is_absolute())
            .filter_map(|p| resolve(&normalize(p).ok()?, true).ok())
            .collect();
        let holds_protected = self.protected.iter().chain(extra.iter()).any(|p| starts_with_ci(p, &canonical))
            || extra.iter().any(|p| starts_with_ci(&canonical, p));
        if holds_secret || holds_protected {
            return Err(Denied("protected"));
        }
        Ok(canonical)
    }

    /// Open an authorized path (`Read`, or `Write` = create without
    /// truncating) and check where the open file really is. The open uses
    /// `O_NOFOLLOW`, so a leaf swapped for a symlink fails; the check after
    /// the open re-runs the scope on the opened file's real path, so a parent
    /// folder swapped for a symlink is refused. On platforms with no way to
    /// read an open file's path (Windows) only the pre-open check applies.
    pub fn open_verified(&self, canonical: &Path, caller: Caller, access: Access) -> Result<std::fs::File, OpenError> {
        let mut options = std::fs::OpenOptions::new();
        if access == Access::Read {
            options.read(true);
        } else {
            options.write(true).create(true);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let file = options.open(canonical).map_err(OpenError::Io)?;
        match fd_path(&file) {
            Ok(Some(real)) => {
                self.authorize(&real.to_string_lossy(), caller, access).map_err(OpenError::Denied)?;
            }
            Ok(None) => {}
            Err(()) => return Err(OpenError::Denied(Denied("unresolvable"))),
        }
        Ok(file)
    }

    /// True when an entry found while listing an already-authorized folder may
    /// be shown. Lexical only (the entry's parent is already canonical): it
    /// keeps recursive listings out of secret folders and, for remote callers,
    /// inside the allowed tree.
    pub fn listable(&self, entry: &Path, caller: Caller) -> bool {
        if caller == Caller::WholeDisk {
            return self.listable_whole_disk(entry);
        }
        let Some(home) = self.home.as_deref() else { return false };
        let Ok(rel) = entry.strip_prefix(home) else { return false };
        let rel = nfc_parts(rel);
        if is_secret(&rel) || self.protected.iter().any(|p| starts_with_ci(entry, p)) {
            return false;
        }
        // rel.len() == 1: an entry of the shallow home listing.
        caller == Caller::Local || rel.len() == 1 || remote_allows(&rel, Access::Read).is_ok()
    }

    /// [`listable`](Self::listable) for a whole-disk caller: anything but a
    /// secret, a protected folder or a secret file name, in or out of home.
    fn listable_whole_disk(&self, entry: &Path) -> bool {
        if self.protected.iter().any(|p| starts_with_ci(entry, p)) {
            return false;
        }
        match self.home.as_deref().and_then(|home| entry.strip_prefix(home).ok()) {
            Some(rel) => {
                let rel = nfc_parts(rel);
                !is_secret(&rel) && !whole_disk_secret(&rel)
            }
            None => !disk_secret(&disk_parts(entry)),
        }
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

/// System secret stores a whole-disk caller may never open, matched from the
/// disk root, case-insensitively, after a leading `private` (macOS
/// `/private/etc`). Home secrets are matched separately on every suffix.
const SYSTEM_SECRETS: &[&[&str]] = &[
    &["etc", "shadow"],
    &["etc", "gshadow"],
    &["etc", "sudoers"],
    &["etc", "sudoers.d"],
    &["etc", "ssh"],
    &["etc", "master.passwd"],
    &["etc", "allternit"],
    &["var", "lib", "allternit"],
    &["var", "db", "dslocal"],
    &["var", "db", "sudo"],
    &["Windows", "System32", "config"],
];

/// The normal components of an absolute canonical path, in NFC, without a
/// leading macOS `private`. Empty for `/` or a drive root.
fn disk_parts(path: &Path) -> Vec<String> {
    let mut parts: Vec<String> = path
        .components()
        .filter(|c| matches!(c, Component::Normal(_)))
        .map(|c| c.as_os_str().to_string_lossy().nfc().collect::<String>())
        .collect();
    if parts.first().is_some_and(|p| eq_ci(p, "private")) {
        parts.remove(0);
    }
    parts
}

/// A path outside home a whole-disk caller may not touch: the home deny-list
/// at any depth (another user's `.ssh`, `/root/.aws`), a system secret store,
/// or a secret file name.
fn disk_secret(parts: &[String]) -> bool {
    if SYSTEM_SECRETS.iter().any(|s| rel_starts_with_ci(parts, *s)) {
        return true;
    }
    (0..parts.len()).any(|start| is_secret(&parts[start..])) || whole_disk_secret(parts)
}

/// Extra refusals for a whole-disk caller anywhere: the runtime identity
/// folder (`.config/allternit`, which holds this computer's device token) at
/// any depth, and leaf names matching the deny-list's `file_patterns`.
fn whole_disk_secret(parts: &[String]) -> bool {
    if parts.windows(2).any(|w| eq_ci(&w[0], ".config") && eq_ci(&w[1], "allternit")) {
        return true;
    }
    parts.last().is_some_and(|leaf| secret_file_name(leaf))
}

fn file_patterns() -> &'static [String] {
    static LIST: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    LIST.get_or_init(|| {
        #[derive(serde::Deserialize)]
        struct Patterns {
            file_patterns: Vec<String>,
        }
        let list: Patterns = serde_json::from_str(SECRETS_DENY_JSON).expect("assets/secrets-deny.json is valid");
        list.file_patterns.into_iter().map(|p| fold(&p)).collect()
    })
}

/// `name` matches one of the deny-list's file patterns (`*` = any run).
fn secret_file_name(name: &str) -> bool {
    let name = fold(name);
    file_patterns().iter().any(|p| glob(p, &name))
}

fn glob(pattern: &str, name: &str) -> bool {
    let pieces: Vec<&str> = pattern.split('*').collect();
    if pieces.len() == 1 {
        return pattern == name;
    }
    let (first, last) = (pieces[0], pieces[pieces.len() - 1]);
    if !name.starts_with(first) || name.len() < first.len() + last.len() || !name.ends_with(last) {
        return false;
    }
    let mut rest = &name[first.len()..name.len() - last.len()];
    for piece in &pieces[1..pieces.len() - 1] {
        match rest.find(piece) {
            Some(i) => rest = &rest[i + piece.len()..],
            None => return false,
        }
    }
    true
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

/// Home-relative parts of a path, each in Unicode NFC.
fn nfc_parts(rel: &Path) -> Vec<String> {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().nfc().collect::<String>())
        .collect()
}

/// NFC, then lower case: how APFS (and NTFS, mostly) compare names.
fn fold(s: &str) -> String {
    s.nfc().collect::<String>().to_lowercase()
}

fn eq_ci(a: &str, b: &str) -> bool {
    fold(a) == fold(b)
}

/// The real path of an open file. `Ok(None)` where the platform has no way
/// to ask; `Err` when it should have answered and didn't (fail closed).
#[cfg(target_os = "macos")]
fn fd_path(file: &std::fs::File) -> Result<Option<PathBuf>, ()> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::io::AsRawFd;
    let mut buf = vec![0u8; libc::PATH_MAX as usize];
    // SAFETY: F_GETPATH writes at most PATH_MAX bytes into `buf`.
    let rc = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETPATH, buf.as_mut_ptr()) };
    if rc == -1 {
        return Err(());
    }
    let len = buf.iter().position(|&b| b == 0).ok_or(())?;
    Ok(Some(PathBuf::from(std::ffi::OsStr::from_bytes(&buf[..len]))))
}

#[cfg(target_os = "linux")]
fn fd_path(file: &std::fs::File) -> Result<Option<PathBuf>, ()> {
    use std::os::unix::io::AsRawFd;
    std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))
        .map(Some)
        .map_err(|_| ())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn fd_path(_file: &std::fs::File) -> Result<Option<PathBuf>, ()> {
    Ok(None)
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

fn rel_starts_with_ci<S: AsRef<str>>(rel: &[String], prefix: &[S]) -> bool {
    rel.len() >= prefix.len() && rel.iter().zip(prefix).all(|(a, b)| eq_ci(a, b.as_ref()))
}

fn rel_starts_with(rel: &[String], prefix: &[&str]) -> bool {
    rel.len() >= prefix.len() && rel.iter().zip(prefix).all(|(a, b)| a == b)
}

fn is_secret(rel: &[String]) -> bool {
    if home_secrets().iter().any(|secret| rel_starts_with_ci(rel, secret)) {
        return true;
    }
    // ~/.allternit/<anything>.env… (incus-host.env, tart-host.env.disabled…)
    rel.len() >= 2 && eq_ci(&rel[0], ".allternit") && rel[1].to_lowercase().contains(".env")
}

/// The remote policy, on the canonical home-relative parts (NFC). Exact
/// (case-sensitive) matching: the path is already canonical, so on a
/// case-insensitive disk it carries the on-disk case of every existing part.
fn remote_allows(rel: &[String], access: Access) -> Result<(), Denied> {
    // A folder the caller may work inside: the folder itself is read-only
    // (it can be listed, not deleted), everything under it is read/write.
    let inside = |len: usize| {
        if rel.len() > len || access == Access::Read {
            Ok(())
        } else {
            Err(Denied("read_only"))
        }
    };
    if let Some(top) = rel.first() {
        if REMOTE_RW_TOP.contains(&top.as_str()) {
            return inside(1);
        }
    }
    if let Some(dir) = REMOTE_RW_DIRS.iter().find(|dir| rel_starts_with(rel, dir)) {
        return inside(dir.len());
    }
    if let Some(dir) = REMOTE_PLUGIN_DIRS.iter().find(|dir| rel_starts_with(rel, dir)) {
        let tail = &rel[dir.len()..];
        let manifest = tail.len() >= 2
            && PLUGIN_MANIFESTS
                .iter()
                .any(|m| tail[1..].len() == m.len() && tail[1..].iter().zip(m.iter()).all(|(a, b)| a == b));
        return match access {
            Access::Read => Ok(()),
            Access::Delete if tail.len() == 1 => Ok(()),
            Access::Write if manifest => Ok(()),
            _ => Err(Denied("read_only")),
        };
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
    // A code project's shared notes (allternit-ai `CodeCanvasTileNotes`):
    // exactly `<any folder>/.allternit/shared-context.md`, read and write.
    if rel.len() >= 3 && rel[rel.len() - 2] == ".allternit" && rel[rel.len() - 1] == "shared-context.md" {
        return match access {
            Access::Read | Access::Write => Ok(()),
            Access::Delete => Err(Denied("read_only")),
        };
    }
    Err(Denied("outside_roots"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Check-then-use: a folder swapped for a symlink after `authorize`
    /// must be caught when the file is opened.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn a_path_swapped_after_the_check_is_refused_on_open() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let home = root.join("home");
        std::fs::create_dir_all(home.join("Allternit/Inbox")).unwrap();
        std::fs::write(home.join("Allternit/Inbox/note.txt"), "hi").unwrap();
        std::fs::create_dir_all(root.join("outside/Inbox")).unwrap();
        std::fs::write(root.join("outside/Inbox/note.txt"), "SECRET").unwrap();
        std::fs::write(root.join("outside/secret.txt"), "SECRET").unwrap();
        let scope = FilesScope::new(Some(home.clone()), vec![]);

        let checked = scope.authorize("~/Allternit/Inbox/note.txt", Caller::Remote, Access::Read).unwrap();
        std::fs::rename(home.join("Allternit/Inbox"), home.join("Allternit/Inbox.old")).unwrap();
        std::os::unix::fs::symlink(root.join("outside/Inbox"), home.join("Allternit/Inbox")).unwrap();
        assert!(
            matches!(scope.open_verified(&checked, Caller::Remote, Access::Read), Err(OpenError::Denied(_))),
            "a folder swapped for a symlink was opened"
        );

        std::fs::remove_file(home.join("Allternit/Inbox")).unwrap();
        std::fs::rename(home.join("Allternit/Inbox.old"), home.join("Allternit/Inbox")).unwrap();
        let checked = scope.authorize("~/Allternit/Inbox/note.txt", Caller::Remote, Access::Write).unwrap();
        std::fs::remove_file(&checked).unwrap();
        std::os::unix::fs::symlink(root.join("outside/secret.txt"), &checked).unwrap();
        assert!(scope.open_verified(&checked, Caller::Remote, Access::Write).is_err(), "a leaf swapped for a symlink was opened");
        assert_eq!(std::fs::read_to_string(root.join("outside/secret.txt")).unwrap(), "SECRET");
    }

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

    /// The deny-list moved from a const into `assets/secrets-deny.json` (Files
    /// drive step 18) so Gizzi's workspace bundle filters with the same list.
    /// It may only grow: every entry the const had is still refused.
    #[test]
    fn secrets_deny_list_only_grows() {
        let before: &[&[&str]] = &[
            &[".ssh"], &[".gnupg"], &[".aws"], &[".azure"], &[".kube"], &[".netrc"], &[".password-store"],
            &[".npmrc"], &[".pypirc"], &[".git-credentials"], &[".docker", "config.json"], &[".config", "gcloud"],
            &[".config", "gh"], &[".config", "stripe"], &["Library", "Keychains"], &[".claude", ".credentials.json"],
            &[".codex", "auth.json"], &[".gizzi", "credentials.json"], &[".local", "share", "gizzi-code", "auth.json"],
            &[".local", "share", "gizzi-code", "fabric-lease.key"], &[".local", "share", "tailscale"],
            &[".allternit", "phone-gateway.json"], &[".allternit", "subscriptions"], &[".allternit", "incus-client"],
            &[".allternit", "factory", "fabric"], &[".allternit", "factory", "registry.json"],
            &[".allternit", "session-caretaker"], &[".allternit", "system-one"],
        ];
        for entry in before {
            let rel: Vec<String> = entry.iter().map(|s| s.to_string()).collect();
            assert!(is_secret(&rel), "{entry:?} left the secrets deny-list");
            let mut inside = rel.clone();
            inside.push("x".into());
            assert!(is_secret(&inside), "{entry:?}/x is no longer refused");
        }
        let v: serde_json::Value = serde_json::from_str(SECRETS_DENY_JSON).unwrap();
        let patterns: Vec<&str> = v["file_patterns"].as_array().unwrap().iter().filter_map(|p| p.as_str()).collect();
        for p in [".env", ".env.*", "*.pem", "*.key", "id_rsa*", "id_ed25519*", "*.token", "token.json", "credentials.json"] {
            assert!(patterns.contains(&p), "file pattern {p:?} left the deny-list");
        }
        assert!(!is_secret(&[".sshx".to_string()]), "prefix confusion");
    }
}
