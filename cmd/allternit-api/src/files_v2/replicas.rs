//! Replicas on the runtime (SPEC sections 10, 13 and 15): the host role, the
//! registrations on both ends, and this computer's Files key.
//!
//! - **Registration.** `POST /replicas` takes a `CreateReplicaRequest`. The id is
//!   derived from (source, root, host) (`replica::replica_id`), so the client
//!   sends the same body to the host and to the source and both agree without
//!   talking. On the host it opens an empty sealed store; on the source it records
//!   where to push (`host_base_url`, loopback or mesh). Without `host_base_url`
//!   the host is relay-only (a cloud computer) and the source pushes through
//!   cloud-api's no-wake peer relay (`sync::peer_base`).
//! - **Host role.** `PUT/GET /replicas/:id/blobs/:locator`, `POST
//!   /replicas/:id/have-need`, `PUT/GET /replicas/:id/head`. The host keeps
//!   ciphertext named by locator and checks only what it can without a key
//!   (`replica::check_sealed`). Reads carry `x-allternit-served-by: replica`; the
//!   reader opens them with the Files key, so content is served while the source
//!   sleeps without the host ever reading it.
//! - **Files key.** `GET /replicas/key` answers this computer's `KeyShareRequest`
//!   (X25519 public key and the 6-digit code). The user's app forwards it to
//!   cloud-api, another device approves, and the app delivers the `KeyEnvelope`
//!   with `PUT /replicas/key`. The envelope is stored; the key is unwrapped in
//!   memory when needed and never written unwrapped.
//! - **No wake, no activity.** Nothing here sends a wake ticket or touches idle
//!   (`replica_tests::replication_never_wakes_or_touches_activity`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use allternit_files_core::{
    CreateReplicaRequest, ErrorCode, HaveNeedRequest, HaveNeedResponse, KeyEnvelope, KeyShareRequest, Replica,
    ReplicaHostInfo, ReplicaMode, ReplicaState, ReplicasResponse,
};
use allternit_files_engine::crypto::FilesRootKey;
use allternit_files_engine::replica::{self, HostStore, WRAP_ALG};
use allternit_files_engine::FilesError;
use axum::body::Bytes;
use axum::extract::rejection::{BytesRejection, JsonRejection};
use axum::extract::{DefaultBodyLimit, Path as UrlPath};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, post};
use axum::{Extension, Json, Router};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use curve25519_dalek::montgomery::MontgomeryPoint;
use rand::RngCore;
use rusqlite::{params, Connection, OptionalExtension};

use super::routes::run;
use super::{ApiError, Fail, FilesV2, Inner, Need, Req, BIG_BODY_LIMIT, UNLOCK_HEADER};

type Files = Extension<Arc<FilesV2>>;

/// Response header on every read a replica host answers (SPEC section 10).
pub const SERVED_BY_HEADER: &str = "x-allternit-served-by";
/// Bytes this computer will hold for other computers' replicas. Unset: half of
/// the free space on the data volume. `0` turns hosting off.
pub const HOST_BYTES_ENV: &str = "ALLTERNIT_FILES_REPLICA_HOST_BYTES";

pub(crate) fn routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    let big = || DefaultBodyLimit::max(BIG_BODY_LIMIT);
    Router::new()
        .route("/replicas", axum::routing::get(list).post(create))
        .route("/replicas/restore", post(super::sync::restore_route))
        .route("/replicas/key", axum::routing::get(key_request).put(install_key))
        .route("/replicas/:replica_id", delete(remove))
        .route("/replicas/:replica_id/sync", post(super::sync::sync_now))
        .route("/replicas/:replica_id/have-need", post(have_need))
        .route("/replicas/:replica_id/head", axum::routing::get(get_head).put(put_head))
        .route("/replicas/:replica_id/blobs/:locator", axum::routing::get(get_blob).put(put_blob).layer(big()))
}

// ---- the registry (replicas.sqlite) ----

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS replicas(
  replica_id TEXT NOT NULL,
  role TEXT NOT NULL CHECK(role IN ('host','source')),
  source_computer_id TEXT NOT NULL,
  root_id TEXT NOT NULL,
  host_device_id TEXT NOT NULL,
  mode TEXT NOT NULL,
  state TEXT NOT NULL,
  key_id TEXT NOT NULL DEFAULT '',
  last_cursor TEXT,
  last_synced_at TEXT,
  stored_bytes INTEGER NOT NULL DEFAULT 0,
  host_base_url TEXT,
  last_error TEXT,
  created_at TEXT NOT NULL,
  PRIMARY KEY(replica_id, role));
CREATE TABLE IF NOT EXISTS jobs(
  job_id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  state TEXT NOT NULL,
  done_bytes INTEGER NOT NULL DEFAULT 0,
  total_bytes INTEGER NOT NULL DEFAULT 0,
  error TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL);
"#;

pub(crate) fn db(data_dir: &Path) -> rusqlite::Result<Connection> {
    std::fs::create_dir_all(data_dir).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    let c = Connection::open(data_dir.join("replicas.sqlite"))?;
    c.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))?;
    c.busy_timeout(std::time::Duration::from_secs(10))?;
    c.execute_batch(SCHEMA)?;
    Ok(c)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    Host,
    Source,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Role::Host => "host",
            Role::Source => "source",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Row {
    pub replica: Replica,
    pub role: Role,
    pub host_base_url: Option<String>,
    pub last_error: Option<String>,
}

fn parse_enum<T: Copy>(all: &[T], name: fn(&T) -> &'static str, s: &str) -> Option<T> {
    all.iter().copied().find(|v| name(v) == s)
}

const COLS: &str = "replica_id, role, source_computer_id, root_id, host_device_id, mode, state, key_id, last_cursor, \
                    last_synced_at, stored_bytes, host_base_url, last_error";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    let role: String = r.get(1)?;
    let mode: String = r.get(5)?;
    let state: String = r.get(6)?;
    Ok(Row {
        replica: Replica {
            replica_id: r.get(0)?,
            source_computer_id: r.get(2)?,
            root_id: r.get(3)?,
            host_device_id: r.get(4)?,
            mode: parse_enum(ReplicaMode::ALL, ReplicaMode::as_str, &mode).unwrap_or(ReplicaMode::Full),
            state: parse_enum(ReplicaState::ALL, ReplicaState::as_str, &state).unwrap_or(ReplicaState::Error),
            last_cursor: r.get(8)?,
            last_synced_at: r.get(9)?,
            stored_bytes: r.get::<_, i64>(10)?.max(0) as u64,
            key_id: r.get(7)?,
        },
        role: if role == "host" { Role::Host } else { Role::Source },
        host_base_url: r.get(11)?,
        last_error: r.get(12)?,
    })
}

pub(crate) fn rows(c: &Connection, role: Option<Role>) -> rusqlite::Result<Vec<Row>> {
    let mut s = c.prepare(&format!(
        "SELECT {COLS} FROM replicas WHERE (?1 IS NULL OR role = ?1) ORDER BY created_at, replica_id, role"
    ))?;
    let v = s.query_map(params![role.map(Role::as_str)], row)?.collect();
    v
}

pub(crate) fn get(c: &Connection, replica_id: &str, role: Role) -> rusqlite::Result<Option<Row>> {
    c.query_row(&format!("SELECT {COLS} FROM replicas WHERE replica_id = ?1 AND role = ?2"), params![replica_id, role.as_str()], row)
        .optional()
}

/// Records a sync outcome on the source's row.
pub(crate) fn set_source_state(
    c: &Connection,
    replica_id: &str,
    state: ReplicaState,
    progress: Option<(&str, &str, u64, &str)>,
    last_error: Option<&str>,
) -> rusqlite::Result<()> {
    match progress {
        Some((cursor, synced_at, stored_bytes, key_id)) => c.execute(
            "UPDATE replicas SET state = ?2, last_cursor = ?3, last_synced_at = ?4, stored_bytes = ?5, key_id = ?6, last_error = NULL \
             WHERE replica_id = ?1 AND role = 'source'",
            params![replica_id, state.as_str(), cursor, synced_at, stored_bytes as i64, key_id],
        )?,
        None => c.execute(
            "UPDATE replicas SET state = ?2, last_error = ?3 WHERE replica_id = ?1 AND role = 'source'",
            params![replica_id, state.as_str(), last_error],
        )?,
    };
    Ok(())
}

fn now(v: &Inner) -> String {
    v.drive.now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn host_dir(data_dir: &Path, replica_id: &str) -> PathBuf {
    data_dir.join("replica-host").join(replica_id)
}

/// The sealed store for a replica this computer hosts, or `not_found`.
fn hosted(v: &Inner, replica_id: &str) -> Result<(Row, HostStore), Fail> {
    let c = db(&v.cfg.data_dir)?;
    let row = get(&c, replica_id, Role::Host)?
        .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "This computer does not host that replica."))?;
    Ok((row, HostStore::open(host_dir(&v.cfg.data_dir, replica_id))?))
}

// ---- hosting capacity (SPEC section 15) ----

fn free_bytes(dir: &Path) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let Ok(c) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else { return 0 };
        // SAFETY: `s` is a plain C struct statvfs fills in; `c` is NUL-terminated.
        let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
            return 0;
        }
        (s.f_bavail as u64).saturating_mul(s.f_frsize as u64)
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        0
    }
}

/// What this computer offers as a replica host, for `GET /storage` and the heartbeat.
pub(crate) fn host_info(data_dir: &Path) -> ReplicaHostInfo {
    let used: u64 = db(data_dir)
        .and_then(|c| rows(&c, Some(Role::Host)))
        .unwrap_or_default()
        .iter()
        .filter_map(|r| HostStore::open(host_dir(data_dir, &r.replica.replica_id)).ok())
        .map(|s| s.used_bytes())
        .sum();
    let configured = std::env::var(HOST_BYTES_ENV).ok().and_then(|v| v.trim().parse::<u64>().ok());
    match configured {
        Some(0) => ReplicaHostInfo { enabled: false, capacity_bytes: 0, used_bytes: used },
        Some(n) => ReplicaHostInfo { enabled: true, capacity_bytes: n, used_bytes: used },
        None => ReplicaHostInfo { enabled: true, capacity_bytes: used + free_bytes(data_dir) / 2, used_bytes: used },
    }
}

// ---- registration ----

fn wire(rows: Vec<Row>) -> ReplicasResponse {
    ReplicasResponse { replicas: rows.into_iter().map(|r| r.replica).collect() }
}

/// Every replica this computer hosts and every replica of its own roots.
async fn list(Extension(files): Files, _req: Req) -> Result<Json<ReplicasResponse>, ApiError> {
    run(&files, move |v| {
        let c = db(&v.cfg.data_dir)?;
        let mut all = rows(&c, None)?;
        for r in all.iter_mut().filter(|r| r.role == Role::Host) {
            if let Ok(s) = HostStore::open(host_dir(&v.cfg.data_dir, &r.replica.replica_id)) {
                r.replica.stored_bytes = s.used_bytes();
            }
        }
        Ok(Json(wire(all)))
    })
    .await
}

pub(crate) fn check_base_url(url: &str) -> Result<String, ApiError> {
    let u = url.trim().trim_end_matches('/');
    let parsed = url::Url::parse(u).map_err(|_| ApiError::invalid("host_base_url is not a URL."))?;
    if !matches!(parsed.scheme(), "http" | "https") || !u.ends_with("/api/v1/files/v2") {
        return Err(ApiError::invalid("host_base_url must be an http(s) URL ending in /api/v1/files/v2."));
    }
    if u.contains("/runtime-devices/") || u.contains("/proxy") {
        return Err(ApiError::invalid("host_base_url must be the host's loopback or mesh address, not a relay URL."));
    }
    Ok(u.to_string())
}

async fn create(Extension(files): Files, _req: Req, body: Result<Json<CreateReplicaRequest>, JsonRejection>) -> Result<Json<ReplicasResponse>, ApiError> {
    let Json(body) = body?;
    let base_url = body.host_base_url.as_deref().map(check_base_url).transpose()?;
    run(&files, move |v| {
        let me = v.cfg.computer_id.as_str();
        let (is_host, is_source) = (body.host_device_id == me, body.source_computer_id == me);
        if !is_host && !is_source {
            return Err(ApiError::invalid("Send this to the replica's host or to its source computer.").into());
        }
        let id = replica::replica_id(&body.source_computer_id, &body.root_id, &body.host_device_id);
        let mode = parse_enum(ReplicaMode::ALL, ReplicaMode::as_str, body.mode.as_str()).unwrap_or(ReplicaMode::Full);
        let c = db(&v.cfg.data_dir)?;
        let at = now(v);
        if is_host {
            let info = host_info(&v.cfg.data_dir);
            if !info.enabled {
                return Err(ApiError::forbidden("Replica hosting is turned off on this computer.").into());
            }
            HostStore::open(host_dir(&v.cfg.data_dir, &id))?;
            c.execute(
                "INSERT INTO replicas(replica_id, role, source_computer_id, root_id, host_device_id, mode, state, created_at) \
                 VALUES(?1,'host',?2,?3,?4,?5,'syncing',?6) ON CONFLICT(replica_id, role) DO UPDATE SET mode = excluded.mode",
                params![id, body.source_computer_id, body.root_id, body.host_device_id, mode.as_str(), at],
            )?;
        }
        if is_source {
            let root = v.drive.root(&body.root_id)?;
            if !root.policy.replica {
                return Err(ApiError::forbidden("This folder's policy does not allow replicas.").into());
            }
            // No host_base_url on another computer's replica: the host is
            // reachable only through the cloud relay (a cloud computer), and
            // passes go through cloud-api's peer relay (`sync::peer_base`).
            c.execute(
                "INSERT INTO replicas(replica_id, role, source_computer_id, root_id, host_device_id, mode, state, host_base_url, created_at) \
                 VALUES(?1,'source',?2,?3,?4,?5,'syncing',?6,?7) \
                 ON CONFLICT(replica_id, role) DO UPDATE SET mode = excluded.mode, host_base_url = COALESCE(excluded.host_base_url, replicas.host_base_url)",
                params![id, body.source_computer_id, body.root_id, body.host_device_id, mode.as_str(), base_url, at],
            )?;
        }
        let mine: Vec<Row> = rows(&c, None)?.into_iter().filter(|r| r.replica.replica_id == id).collect();
        Ok(Json(wire(mine)))
    })
    .await
}

async fn remove(Extension(files): Files, _req: Req, UrlPath(replica_id): UrlPath<String>) -> Result<StatusCode, ApiError> {
    run(&files, move |v| {
        let c = db(&v.cfg.data_dir)?;
        let n = c.execute("DELETE FROM replicas WHERE replica_id = ?1", params![replica_id])?;
        if n == 0 {
            return Err(ApiError::new(ErrorCode::NotFound, "No such replica on this computer.").into());
        }
        HostStore::open(host_dir(&v.cfg.data_dir, &replica_id))?.destroy()?;
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

// ---- host role ----

fn served_by_replica(mut resp: Response) -> Response {
    resp.headers_mut().insert(SERVED_BY_HEADER, axum::http::HeaderValue::from_static("replica"));
    resp
}

async fn have_need(
    Extension(files): Files,
    _req: Req,
    UrlPath(replica_id): UrlPath<String>,
    body: Result<Json<HaveNeedRequest>, JsonRejection>,
) -> Result<Json<HaveNeedResponse>, ApiError> {
    let Json(body) = body?;
    if body.hashes.len() > 10_000 {
        return Err(ApiError::invalid("At most 10000 locators per request."));
    }
    run(&files, move |v| {
        let (_, store) = hosted(v, &replica_id)?;
        let (have, need): (Vec<String>, Vec<String>) = body.hashes.into_iter().partition(|l| store.has_blob(l));
        Ok(Json(HaveNeedResponse { have, need }))
    })
    .await
}

async fn put_blob(
    Extension(files): Files,
    _req: Req,
    UrlPath((replica_id, locator)): UrlPath<(String, String)>,
    body: Result<Bytes, BytesRejection>,
) -> Result<StatusCode, ApiError> {
    let body = body?;
    run(&files, move |v| {
        let (row, store) = hosted(v, &replica_id)?;
        let info = host_info(&v.cfg.data_dir);
        if info.used_bytes.saturating_add(body.len() as u64) > info.capacity_bytes && !store.has_blob(&locator) {
            return Err(ApiError::new(ErrorCode::QuotaExceeded, "This computer has no more room for replicas.").into());
        }
        let key = (!row.replica.key_id.is_empty()).then_some(row.replica.key_id.as_str());
        let (header, _) = store.put_blob(&locator, &body, key)?;
        if row.replica.key_id.is_empty() {
            db(&v.cfg.data_dir)?.execute(
                "UPDATE replicas SET key_id = ?2 WHERE replica_id = ?1 AND role = 'host' AND key_id = ''",
                params![replica_id, header.key_id],
            )?;
        }
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

async fn get_blob(Extension(files): Files, _req: Req, UrlPath((replica_id, locator)): UrlPath<(String, String)>) -> Result<Response, ApiError> {
    let blob = run(&files, move |v| {
        let (_, store) = hosted(v, &replica_id)?;
        store.get_blob(&locator)?.ok_or_else(|| ApiError::new(ErrorCode::NotFound, "That sealed chunk is not on this host.").into())
    })
    .await?;
    Ok(served_by_replica(([(header::CONTENT_TYPE, "application/octet-stream")], blob).into_response()))
}

async fn put_head(
    Extension(files): Files,
    _req: Req,
    UrlPath(replica_id): UrlPath<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<StatusCode, ApiError> {
    let body = body?;
    run(&files, move |v| {
        let (row, store) = hosted(v, &replica_id)?;
        let key = (!row.replica.key_id.is_empty()).then_some(row.replica.key_id.as_str());
        let key_id = store.put_head(&body, key)?;
        // A new head means the source finished a pass: the copy is current.
        db(&v.cfg.data_dir)?.execute(
            "UPDATE replicas SET key_id = ?2, state = 'current', last_synced_at = ?3 WHERE replica_id = ?1 AND role = 'host'",
            params![replica_id, key_id, now(v)],
        )?;
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

async fn get_head(Extension(files): Files, _req: Req, UrlPath(replica_id): UrlPath<String>) -> Result<Response, ApiError> {
    let (head, synced) = run(&files, move |v| {
        let (row, store) = hosted(v, &replica_id)?;
        let head = store
            .get_head()?
            .ok_or_else(|| ApiError::new(ErrorCode::ReplicaUnavailable, "This replica has not received its first copy yet."))?;
        Ok((head, row.replica.last_synced_at))
    })
    .await?;
    let mut resp = served_by_replica(([(header::CONTENT_TYPE, "application/octet-stream")], head).into_response());
    if let Some(at) = synced.and_then(|s| axum::http::HeaderValue::from_str(&s).ok()) {
        resp.headers_mut().insert("x-allternit-replica-synced-at", at);
    }
    Ok(resp)
}

// ---- this computer's Files key (SPEC section 13) ----

fn key_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("files-key")
}

/// This computer's X25519 secret, made once, readable only by this user.
fn device_secret(data_dir: &Path) -> Result<[u8; 32], FilesError> {
    let dir = key_dir(data_dir);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("device.x25519");
    if let Ok(b) = std::fs::read(&path) {
        if let Ok(k) = <[u8; 32]>::try_from(b.as_slice()) {
            return Ok(k);
        }
    }
    let mut k = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut k);
    let tmp = path.with_extension("tmp");
    {
        use std::io::Write;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(&k)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, &path)?;
    Ok(k)
}

pub(crate) fn device_pub(data_dir: &Path) -> Result<[u8; 32], FilesError> {
    Ok(MontgomeryPoint::mul_base_clamped(device_secret(data_dir)?).to_bytes())
}

fn b64_32(s: &str) -> Option<[u8; 32]> {
    URL_SAFE_NO_PAD.decode(s.trim_end_matches('=')).ok().and_then(|b| <[u8; 32]>::try_from(b.as_slice()).ok())
}

fn open_envelope(data_dir: &Path, device_id: &str, env: &KeyEnvelope) -> Result<FilesRootKey, FilesError> {
    let unavailable = |m: &str| FilesError::KeyUnavailable(m.to_string());
    if env.wrap_alg != WRAP_ALG {
        return Err(unavailable("unknown key wrap algorithm"));
    }
    if env.device_id != device_id {
        return Err(unavailable("this envelope is for another device"));
    }
    let eph = b64_32(&env.ephemeral_pub).ok_or_else(|| unavailable("bad ephemeral key"))?;
    let secret = device_secret(data_dir)?;
    let mine = MontgomeryPoint::mul_base_clamped(secret).to_bytes();
    let shared = MontgomeryPoint(eph).mul_clamped(secret).to_bytes();
    replica::unwrap_root_key(&env.key_id, &shared, &eph, &mine, device_id, &env.nonce, &env.wrapped_key)
}

/// The Files root key, unwrapped in memory from the stored envelope.
/// `key_unavailable` until the user has approved this computer.
pub(crate) fn files_key(v: &Inner) -> Result<FilesRootKey, Fail> {
    let raw = match std::fs::read(key_dir(&v.cfg.data_dir).join("envelope.json")) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ApiError::new(
                ErrorCode::KeyUnavailable,
                "This computer does not have your Files key yet. Approve it from another of your devices.",
            )
            .into())
        }
        Err(e) => return Err(FilesError::from(e).into()),
    };
    let env: KeyEnvelope = serde_json::from_slice(&raw).map_err(|_| FilesError::KeyUnavailable("stored envelope is damaged".into()))?;
    Ok(open_envelope(&v.cfg.data_dir, &v.cfg.computer_id, &env)?)
}

async fn key_request(Extension(files): Files, req: Req) -> Result<Json<KeyShareRequest>, ApiError> {
    run(&files, move |v| {
        let public = device_pub(&v.cfg.data_dir)?;
        Ok(Json(KeyShareRequest {
            device_id: v.cfg.computer_id.clone(),
            device_pub: URL_SAFE_NO_PAD.encode(public),
            device_name: req.device_name.clone().unwrap_or_else(|| "Allternit computer".to_string()),
            confirm_code: replica::confirm_code(&public),
        }))
    })
    .await
}

async fn install_key(
    Extension(files): Files,
    _req: Req,
    headers: HeaderMap,
    body: Result<Json<KeyEnvelope>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let Json(env) = body?;
    let unlocked = headers.get(UNLOCK_HEADER).and_then(|h| h.to_str().ok()).is_some_and(|s| !s.trim().is_empty());
    run(&files, move |v| {
        // Proves the envelope is for this computer before anything is stored.
        let frk = open_envelope(&v.cfg.data_dir, &v.cfg.computer_id, &env)?;
        let path = key_dir(&v.cfg.data_dir).join("envelope.json");
        if let Ok(raw) = std::fs::read(&path) {
            let old: Option<KeyEnvelope> = serde_json::from_slice(&raw).ok();
            // Swapping in a different key re-keys every replica: it needs a fresh unlock.
            if old.is_some_and(|o| o.key_id != frk.key_id()) && !unlocked {
                return Err(ApiError::new(ErrorCode::ProtectedRootLocked, "Unlock on your device to replace this computer's Files key.").into());
            }
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec(&env).expect("envelope serializes")).map_err(FilesError::from)?;
        std::fs::rename(&tmp, &path).map_err(FilesError::from)?;
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

/// Roots a source replicates, after the read check (used by the sync loop).
pub(crate) fn source_rows(v: &Inner) -> Result<Vec<Row>, Fail> {
    let c = db(&v.cfg.data_dir)?;
    Ok(rows(&c, Some(Role::Source))?)
}

/// The guard a replication pass runs under: the root must still exist and allow replicas.
pub(crate) fn check_source_root(v: &Inner, req: &Req, root_id: &str) -> Result<allternit_files_engine::RootRecord, Fail> {
    let root = v.guard(req, root_id, Need::Read)?;
    if !root.policy.replica {
        return Err(ApiError::forbidden("This folder's policy no longer allows replicas.").into());
    }
    Ok(root)
}
