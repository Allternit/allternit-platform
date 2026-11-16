//! Files drive protocol v2 (`files/2`) on the runtime: `/api/v1/files/v2/*`
//! (`contracts/files-drive/SPEC.md` section 2).
//!
//! The engine (`allternit-files-engine`) does the file work: index, revisions,
//! chunks, uploads, trash, versions, change feed. This module adds what a
//! transport needs around it: the default roots and providers (section 3),
//! principals (section 7), scoping (section 12), permissions, the error mapping
//! (section 16), the long poll and WebSocket feed (section 6), and the side
//! tables for pins and comments.
//!
//! - **Flag.** Everything is off unless `ALLTERNIT_FILES_DRIVE=1`. The runtime
//!   has no per-user flag source; cloud-api holds the real `files_drive` flag
//!   (step 13). Off, every route answers `feature_disabled` and nothing is
//!   created on disk.
//! - **No activity.** Files traffic never resets idle (SPEC sections 1 and 9):
//!   nothing in this module calls `touch_computer_activity`,
//!   `touch_runtime_activity` or any other idle reset.
//!   `tests::files_v2_never_touches_activity` guards it.
//! - **Body limits.** 1 MiB on every v2 route, except the small write and the
//!   chunk PUT, which take 4 MiB + 64 KiB (section 2).

mod errors;
mod events;
mod feed;
mod inbox;
#[cfg(test)]
mod inbox_tests;
mod memory;
mod meta;
mod roots;
mod routes;
pub mod routing;
mod rules;
mod storage;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use allternit_files_core::{
    ComputerState, ErrorCode, Freshness, Item, ItemKind, Permissions, PrincipalKind, PrincipalRef, RootKind, ServedBy,
};
use allternit_files_engine::{ChangeListener, Clock, Drive, DriveOptions, DriveWatcher, FilesError, ItemRecord, RootRecord};
use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;
use axum::http::HeaderMap;
use axum::Router;
use tokio::sync::{broadcast, OnceCell};

use crate::db::DbHandle;
use crate::files_scope::FilesScope;
pub use errors::{status_for, ApiError};
pub use rules::{is_folder_rule, RULE_TRIGGER};

/// Env switch for the whole drive (off by default).
pub const FLAG_ENV: &str = "ALLTERNIT_FILES_DRIVE";
/// Proof of a fresh device unlock for protected roots (section 12).
pub const UNLOCK_HEADER: &str = "x-allternit-unlock";
pub const DEVICE_NAME_HEADER: &str = "x-allternit-device-name";
/// Small write and chunk PUT body limit (section 2).
pub const BIG_BODY_LIMIT: usize = 4 * 1024 * 1024 + 64 * 1024;
/// Every other v2 route.
pub const BODY_LIMIT: usize = 1024 * 1024;

/// What the drive needs from the process. `from_env` for production; tests
/// build one by hand.
#[derive(Clone)]
pub struct Config {
    pub enabled: bool,
    /// Engine index, chunk store, side tables, provider views.
    pub data_dir: PathBuf,
    /// The home folder: default roots live in `<home>/Allternit`.
    pub home: Option<PathBuf>,
    /// `runtime_devices.id` in cloud-api when known (SPEC "Computer ids").
    pub computer_id: String,
    /// The Memory Drive's database (its write path and git history).
    pub memory_db: Option<DbHandle>,
    /// local-engine's model cache.
    pub models_dir: Option<PathBuf>,
    /// The user's code folder.
    pub code_dir: Option<PathBuf>,
    /// Start the filesystem watcher (off in tests).
    pub watch: bool,
    pub clock: Option<Clock>,
}

impl Config {
    pub fn from_env(app_data_dir: &Path, memory_db: Option<DbHandle>) -> Self {
        let enabled = std::env::var(FLAG_ENV).map(|v| v.trim() == "1" || v.trim().eq_ignore_ascii_case("true")).unwrap_or(false);
        let home = dirs::home_dir();
        let models_dir = dirs::data_dir().map(|d| d.join("allternit-local-engine").join("models"));
        let code_dir = std::env::var_os("ALLTERNIT_FILES_CODE_DIR").filter(|v| !v.is_empty()).map(PathBuf::from);
        let computer_id = crate::relay_auth::process_secret().runtime_id().unwrap_or_else(|| "local".to_string());
        Self {
            enabled,
            data_dir: app_data_dir.join("files-v2"),
            home,
            computer_id,
            memory_db,
            models_dir,
            code_dir,
            watch: true,
            clock: None,
        }
    }
}

/// The `/files/v2` routes for this process, nested by the caller under
/// `/api/v1`.
pub fn router<S: Clone + Send + Sync + 'static>(app_data_dir: &Path, db: DbHandle) -> Router<S> {
    routes::routes(Arc::new(FilesV2::new(Config::from_env(app_data_dir, Some(db)))))
}

pub use routes::routes;

/// Shared state for every v2 route. The drive opens on first use.
pub struct FilesV2 {
    pub(crate) cfg: Config,
    inner: OnceCell<Arc<Inner>>,
}

impl FilesV2 {
    pub fn new(cfg: Config) -> Self {
        Self { cfg, inner: OnceCell::new() }
    }

    /// Opens the drive and creates the default roots (section 3), once.
    pub(crate) async fn inner(&self) -> Result<Arc<Inner>, ApiError> {
        if !self.cfg.enabled {
            return Err(ApiError::new(ErrorCode::FeatureDisabled, "The Files drive is not turned on for this computer."));
        }
        self.inner
            .get_or_try_init(|| async {
                let cfg = self.cfg.clone();
                tokio::task::spawn_blocking(move || Inner::open(cfg))
                    .await
                    .map_err(|_| ApiError::internal())?
            })
            .await
            .cloned()
    }
}

/// The open drive.
pub(crate) struct Inner {
    pub cfg: Config,
    pub home: PathBuf,
    pub drive: Arc<Drive>,
    pub meta: Mutex<rusqlite::Connection>,
    pub scope: FilesScope,
    /// Root IDs with new changes, for feed sockets.
    pub feed: broadcast::Sender<String>,
    pub watcher: Option<DriveWatcher>,
    /// Memory Drive revision last copied into its view folder.
    pub memory_rev: Mutex<Option<String>>,
    /// The model Inbox tasks call; `None` is the computer's default model
    /// (`inbox::GizziModel`). Tests put a recorder here.
    pub task_model: std::sync::RwLock<Option<Arc<dyn inbox::TaskModel>>>,
}

impl Inner {
    fn open(cfg: Config) -> Result<Arc<Self>, ApiError> {
        let home = cfg
            .home
            .clone()
            .filter(|h| h.is_absolute())
            .and_then(|h| std::fs::canonicalize(h).ok())
            .ok_or_else(|| ApiError::new(ErrorCode::Internal, "This computer has no home folder to serve files from."))?;
        let fail = |e: FilesError| ApiError::from_files(e, None);
        std::fs::create_dir_all(&cfg.data_dir).map_err(|e| fail(e.into()))?;
        let mut opts = DriveOptions::default();
        // SPEC section 4: without the writer's offset the runtime uses UTC.
        opts.display_offset = chrono::FixedOffset::east_opt(0).expect("utc");
        if let Some(clock) = cfg.clock.clone() {
            opts.clock = clock;
        }
        let drive = Arc::new(Drive::open(cfg.data_dir.join("engine"), opts).map_err(fail)?);
        let meta = meta::open(&cfg.data_dir.join("files_v2.sqlite")).map_err(|e| fail(e.into()))?;
        let mut protected = Vec::new();
        if let Some(dir) = std::env::var_os("ALLTERNIT_DATA_DIR").filter(|v| !v.is_empty()) {
            protected.push(PathBuf::from(dir));
        }
        if let Some(data) = dirs::data_dir() {
            protected.push(data.join("allternit"));
            protected.push(data.join("@allternit"));
        }
        let scope = FilesScope::new(Some(home.clone()), protected);
        let (feed, _) = broadcast::channel(256);
        let tx = feed.clone();
        let listener: ChangeListener = Arc::new(move |c| {
            let _ = tx.send(c.root_id.clone());
        });
        drive.set_change_listener(Some(listener));
        let mut inner = Self {
            cfg: cfg.clone(),
            home,
            drive,
            meta: Mutex::new(meta),
            scope,
            feed,
            watcher: None,
            memory_rev: Mutex::new(None),
            task_model: std::sync::RwLock::new(None),
        };
        inbox::ensure_schema(&inner.meta.lock().expect("meta lock")).map_err(|e| fail(e.into()))?;
        roots::ensure_defaults(&inner).map_err(|e| e.into_api(&inner))?;
        if cfg.watch {
            match DriveWatcher::start(inner.drive.clone(), |e| tracing::warn!(error = %e, "files v2: watcher reconcile failed")) {
                Ok(w) => inner.watcher = Some(w),
                Err(e) => tracing::warn!(error = %e, "files v2: watcher did not start; changes made outside the drive show up on the next scan"),
            }
        }
        let inner = Arc::new(inner);
        // Change events, Inbox drops and folder rules (SPEC sections 7 and 14).
        events::attach(&inner);
        Ok(inner)
    }

    /// The root for `root_id` after the caller's checks (section 12).
    pub fn guard(&self, req: &Req, root_id: &str, need: Need) -> Result<RootRecord, Fail> {
        let root = self.drive.root(root_id)?;
        if root.policy.protected && !req.unlocked {
            return Err(ApiError::new(
                ErrorCode::ProtectedRootLocked,
                "This folder is protected. Unlock it on your device, then try again.",
            )
            .into());
        }
        match need {
            Need::Read if !root.policy.read => Err(ApiError::forbidden("This folder can't be read.").into()),
            Need::Write if !root.policy.write => Err(ApiError::forbidden("This folder is read-only.").into()),
            _ => Ok(root),
        }
    }

    /// An item and its root after the checks.
    pub fn guard_item(&self, req: &Req, item_id: &str, need: Need) -> Result<(RootRecord, ItemRecord), Fail> {
        let item = self.drive.stat(item_id)?;
        let root = self.guard(req, &item.root_id, need)?;
        Ok((root, item))
    }

    /// The wire item, with this caller's permissions and the pins.
    pub fn item(&self, root: &RootRecord, rec: &ItemRecord) -> Item {
        let mut item = rec.to_item(permissions(root, rec));
        let pins = meta::pins(&self.meta.lock().expect("meta lock"), &rec.id).unwrap_or_default();
        if !pins.is_empty() {
            item.pinned_on = Some(pins);
        }
        item
    }

    /// Same, looking the root up.
    pub fn item_any(&self, rec: &ItemRecord) -> Item {
        match self.drive.root(&rec.root_id) {
            Ok(root) => self.item(&root, rec),
            Err(_) => rec.to_item(no_permissions()),
        }
    }

    pub fn fail(&self, e: Fail) -> ApiError {
        e.into_api(self)
    }
}

/// What a handler is about to do in a root.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Need {
    Read,
    Write,
}

/// Error inside a blocking handler body: the engine's (mapped with the drive
/// at hand, so `rev_mismatch` carries `current`) or an already-built one.
pub(crate) enum Fail {
    Files(FilesError),
    Api(ApiError),
}

impl From<FilesError> for Fail {
    fn from(e: FilesError) -> Self {
        Fail::Files(e)
    }
}
impl From<ApiError> for Fail {
    fn from(e: ApiError) -> Self {
        Fail::Api(e)
    }
}
impl From<rusqlite::Error> for Fail {
    fn from(e: rusqlite::Error) -> Self {
        Fail::Files(FilesError::Db(e))
    }
}

impl Fail {
    pub fn into_api(self, inner: &Inner) -> ApiError {
        match self {
            Fail::Api(e) => e,
            Fail::Files(FilesError::RevMismatch { current }) => {
                let item = inner.item_any(&current);
                ApiError::from_files(FilesError::RevMismatch { current }, Some(item))
            }
            Fail::Files(e) => ApiError::from_files(e, None),
        }
    }
}

/// Permissions for an owner or own device (section 7), from the root policy.
/// Files never runs anything, so `execute` is always false here.
pub(crate) fn permissions(root: &RootRecord, item: &ItemRecord) -> Permissions {
    let p = &root.policy;
    let readable = p.read && !item.link_escapes;
    let memory = root.kind == RootKind::MemoryDrive;
    Permissions {
        list: p.read && item.kind == ItemKind::Folder,
        read: readable,
        write: p.write && !(memory && item.kind == ItemKind::Folder),
        delete: p.write && item.parent_id.is_some(),
        share: p.shareable,
        comment: p.read,
        agent_process: p.agent_process,
        transfer: readable,
        execute: false,
    }
}

pub(crate) fn no_permissions() -> Permissions {
    Permissions {
        list: false,
        read: false,
        write: false,
        delete: false,
        share: false,
        comment: false,
        agent_process: false,
        transfer: false,
        execute: false,
    }
}

pub(crate) fn live() -> Freshness {
    Freshness { served_by: ServedBy::Live, computer_state: ComputerState::Awake, snapshot_at: None }
}

/// The caller, decided once per request.
#[derive(Clone, Debug)]
pub(crate) struct Req {
    pub principal: PrincipalRef,
    pub user_id: String,
    /// Replayed by a relay, forwarded, or a non-loopback peer (`files_scope::caller`).
    pub relayed: bool,
    /// Sent `x-allternit-unlock` (section 12).
    pub unlocked: bool,
    pub device_name: Option<String>,
}

fn header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get(name).and_then(|v| v.to_str().ok()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

impl Req {
    pub fn extract(parts: &Parts) -> Result<Self, ApiError> {
        let user = parts
            .extensions
            .get::<crate::auth::AuthUser>()
            .ok_or_else(|| ApiError::new(ErrorCode::Unauthorized, "Sign in to use the Files drive."))?;
        let headers = &parts.headers;
        let peer = parts.extensions.get::<ConnectInfo<std::net::SocketAddr>>().map(|c| c.0);
        let relayed = crate::files_scope::caller(headers, peer) == crate::files_scope::Caller::Remote;
        let device_name = header(headers, DEVICE_NAME_HEADER);
        let principal = match header(headers, crate::cowork_devices_routes::DEVICE_ID_HEADER) {
            Some(device_id) => PrincipalRef {
                kind: PrincipalKind::OwnDevice,
                id: device_id,
                display_name: device_name.clone().or_else(|| user.name.clone()),
            },
            None => PrincipalRef { kind: PrincipalKind::Owner, id: user.user_id.clone(), display_name: user.name.clone() },
        };
        // SPEC section 12 names the proof but not its format. Until the client
        // unlock flow lands, any non-empty value counts as the fresh unlock.
        // TODO(files-drive): verify a signed, short-lived unlock assertion here.
        let unlocked = header(headers, UNLOCK_HEADER).is_some();
        Ok(Self { principal, user_id: user.user_id.clone(), relayed, unlocked, device_name })
    }

    /// Conflict-copy device name: request field, else the header, else the
    /// principal's display name (section 4).
    pub fn device_name_or(&self, field: Option<String>) -> Option<String> {
        field.filter(|s| !s.trim().is_empty()).or_else(|| self.device_name.clone()).or_else(|| self.principal.display_name.clone())
    }
}

#[axum::async_trait]
impl<S: Send + Sync> FromRequestParts<S> for Req {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Req::extract(parts)
    }
}
