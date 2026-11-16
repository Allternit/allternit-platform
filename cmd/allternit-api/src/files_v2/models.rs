//! Models, datasets and checkpoints, and sending a file to a computer
//! (Files drive plan step 18; SPEC sections 3 and 7).
//!
//! - **Package manifest** (`allternit.model-package/1`). One JSON shape for
//!   three jobs: the hand-off a phone or another computer reads before it pulls
//!   a package (`GET /models/handoff/:item_id`), the permission file a package
//!   carries once it lands ([`MARKER`] inside the package folder), and the
//!   seam to the on-device model lane (their importer reads the same file).
//! - **Transfer is not execute.** `grant.transfer` lets the bytes move;
//!   `grant.execute` lets a model engine load them. A package whose permission
//!   file says `execute: false` is refused with `execute_denied` where models
//!   are imported or launched on this runtime ([`import_gate`],
//!   [`launch_gate`], called from `local_engine_routes.rs`). A transfer never
//!   widens the grant: the result runs only if the source package could run
//!   and the person sending it allowed it.
//! - **Pull on the destination.** `POST /models/transfer` and `POST /route` run
//!   on the computer that receives. It reads the source's manifests, asks its
//!   own chunk store which chunks it already has (dedupe), downloads only the
//!   rest, and commits each file through the engine's upload session, so every
//!   chunk and the whole-file hash are checked. Progress is a `JobRef`
//!   (`GET /jobs/:job_id`). Phones have no runtime: they read the hand-off and
//!   pull the same chunks themselves.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use allternit_files_core::{
    ChunkRef, ErrorBody, ErrorCode, ItemKind, JobRef, JobRefKind, JobRefState, Manifest, OnConflict, PrincipalKind,
    RootKind, StatResponse, TransportKind, UploadCreateRequest, UploadStatus,
};
use allternit_files_engine::{chunker, EditingDevice, ItemRecord};
use axum::body::Bytes;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Path as UrlPath, Query};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use sha2::Digest;

use super::routes::run;
use super::{memory, permissions, status_for, ApiError, Fail, FilesV2, Inner, Need, Req};

/// `format` of every package manifest.
pub const PACKAGE_FORMAT: &str = "allternit.model-package/1";
/// The permission file inside a package folder.
pub const MARKER: &str = ".allternit-package.json";
/// Where transfers land when no folder is named: `~/Allternit/Downloads/Models`.
pub const MODELS_FOLDER: &str = "Models";
const MAX_FILES: usize = 10_000;
/// Chunk refs per upload request (engine `MANIFEST_PAGE`).
const REFS_PER_REQUEST: usize = 10_000;
const MAX_JOBS: usize = 256;

// ---- the package manifest ----

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageKind {
    #[default]
    Model,
    Dataset,
    Checkpoint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeightsFormat {
    Gguf,
    Mlx,
    Safetensors,
    Onnx,
    Other,
}

/// What the holder of a package may do with it (SPEC section 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    /// The bytes may be copied to another device or computer.
    pub transfer: bool,
    /// A model engine may load or run it. Never implied by `transfer`.
    pub execute: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageFile {
    /// Relative to the package folder, `/`-separated, no `..`.
    pub path: String,
    pub size: u64,
    /// blake3 of the whole file (the Files drive content hash).
    pub content_hash: String,
    /// sha256 of the whole file, for importers that check sha256 (Freeweights, Android).
    pub sha256: String,
    /// The file's item on the source computer: fetch it with
    /// `GET /items/:item_id/manifest` and `GET /chunks/:hash`, or
    /// `GET /items/:item_id/content`. Left out in a landed package's permission file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageSource {
    pub computer_id: String,
    pub item_id: String,
    pub rev: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageManifest {
    pub format: String,
    /// `pkg_` + 24 hex of blake3 over the sorted `path\tcontent_hash` lines:
    /// the same bytes give the same id on every device.
    pub package_id: String,
    pub name: String,
    pub kind: PackageKind,
    pub weights_format: WeightsFormat,
    pub total_bytes: u64,
    pub files: Vec<PackageFile>,
    pub grant: Grant,
    pub source: PackageSource,
    pub created_at: String,
}

/// The package id for a file list (see [`PackageManifest::package_id`]).
pub fn package_id(files: &[PackageFile]) -> String {
    let mut lines: Vec<String> = files.iter().map(|f| format!("{}\t{}\n", f.path, f.content_hash)).collect();
    lines.sort();
    format!("pkg_{}", &chunker::hash_bytes(lines.concat().as_bytes())[..24])
}

fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A path an importer can join under its own folder without leaving it.
fn safe_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.contains('\0')
        && path.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
        && path.rsplit('/').next() != Some(MARKER)
}

impl PackageManifest {
    /// Every rule an importer relies on. A transfer refuses a manifest that
    /// fails any of them (`manifest_invalid`).
    pub fn validate(&self) -> Result<(), String> {
        if self.format != PACKAGE_FORMAT {
            return Err(format!("format is {:?}, expected {PACKAGE_FORMAT:?}", self.format));
        }
        if self.name.trim().is_empty() || self.name.contains('/') || self.name.contains('\\') || self.name == "." || self.name == ".." {
            return Err("name must be a plain folder name".into());
        }
        if self.files.is_empty() {
            return Err("a package has at least one file".into());
        }
        if self.files.len() > MAX_FILES {
            return Err(format!("a package has at most {MAX_FILES} files"));
        }
        let mut seen = HashSet::new();
        let mut total = 0u64;
        for f in &self.files {
            if !safe_relative(&f.path) {
                return Err(format!("{:?} is not a safe relative path", f.path));
            }
            if !seen.insert(f.path.to_lowercase()) {
                return Err(format!("{:?} is listed twice", f.path));
            }
            if !chunker::is_hash(&f.content_hash) {
                return Err(format!("{:?} has no blake3 content_hash", f.path));
            }
            if !is_sha256(&f.sha256) {
                return Err(format!("{:?} has no sha256", f.path));
            }
            total = total.checked_add(f.size).ok_or("total size overflows")?;
        }
        if total != self.total_bytes {
            return Err(format!("files add up to {total} bytes, total_bytes says {}", self.total_bytes));
        }
        if self.package_id != package_id(&self.files) {
            return Err("package_id does not match the files".into());
        }
        Ok(())
    }
}

fn weights_format(files: &[PackageFile], name: &str) -> WeightsFormat {
    let has = |ext: &str| files.iter().any(|f| f.path.to_lowercase().ends_with(ext));
    if has(".gguf") {
        WeightsFormat::Gguf
    } else if name.to_lowercase().contains("mlx") || has(".mlpackage") {
        WeightsFormat::Mlx
    } else if has(".safetensors") {
        WeightsFormat::Safetensors
    } else if has(".onnx") {
        WeightsFormat::Onnx
    } else {
        WeightsFormat::Other
    }
}

/// Reads a permission file. Anything but a valid manifest is an error, and
/// callers treat an error as "may not run".
pub fn read_marker(path: &Path) -> Result<PackageManifest, String> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.len() > 8 * 1024 * 1024 {
        return Err("not a regular permission file".into());
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let m: PackageManifest = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if m.format != PACKAGE_FORMAT {
        return Err(format!("unknown format {:?}", m.format));
    }
    Ok(m)
}

// ---- execute enforcement (local-engine import and launch) ----

fn execute_denied() -> ApiError {
    ApiError::new(
        ErrorCode::ExecuteDenied,
        "This model was copied here without permission to run. Allow it to run from this computer first.",
    )
}

/// `execute_denied` when `path` (a package folder, a file in one, or a
/// folder that holds packages one level down) carries a permission file
/// without `execute`. A broken permission file fails closed. No permission
/// file at all (the engine's own downloads) is not restricted.
pub fn check_execute_path(path: &Path) -> Result<(), ApiError> {
    let decide = |marker: &Path| match read_marker(marker) {
        Ok(m) if m.grant.execute => Ok(()),
        Ok(_) => Err(execute_denied()),
        Err(_) => Err(ApiError::new(
            ErrorCode::ExecuteDenied,
            "This model's permission file can't be read, so it won't run.",
        )),
    };
    let start = if path.is_dir() { Some(path) } else { path.parent() };
    // The folder itself and up to four folders above it (weights nested in a package).
    let mut dir = start;
    for _ in 0..5 {
        let Some(d) = dir else { break };
        let marker = d.join(MARKER);
        if std::fs::symlink_metadata(&marker).is_ok() {
            return decide(&marker);
        }
        dir = d.parent();
    }
    // A folder that holds packages: every package in it must be allowed.
    if path.is_dir() {
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                let marker = entry.path().join(MARKER);
                if entry.path().is_dir() && std::fs::symlink_metadata(&marker).is_ok() {
                    decide(&marker)?;
                }
            }
        }
    }
    Ok(())
}

/// For `POST /api/local-engine/models/import`: `Some(response)` refuses the
/// import of a package that may not run.
pub fn import_gate(body: &[u8]) -> Option<Response> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    let path = v.get("path")?.as_str()?.trim();
    if path.is_empty() {
        return None;
    }
    check_execute_path(Path::new(path)).err().map(IntoResponse::into_response)
}

/// For `POST /api/local-engine/runtimes/launch`: checks `recipe.model_path`
/// and the path local-engine has on record for `model_id`. If the engine's
/// list can't be read the launch goes on and fails at the engine itself.
pub async fn launch_gate(body: &[u8], engine_url: &str) -> Option<Response> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    if let Some(p) = v.pointer("/recipe/model_path").and_then(serde_json::Value::as_str) {
        if let Err(e) = check_execute_path(Path::new(p)) {
            return Some(e.into_response());
        }
    }
    let model_id = v.get("model_id")?.as_str()?.trim().to_string();
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(5)).build().ok()?;
    let models: serde_json::Value = client.get(format!("{engine_url}/models")).send().await.ok()?.json().await.ok()?;
    launch_gate_with(&models, &model_id).err().map(IntoResponse::into_response)
}

/// The launch check on local-engine's `GET /models` body.
pub(crate) fn launch_gate_with(models: &serde_json::Value, model_id: &str) -> Result<(), ApiError> {
    let path = models
        .get("models")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .find(|m| m.get("id").and_then(serde_json::Value::as_str) == Some(model_id))
        .and_then(|m| m.get("path"))
        .and_then(serde_json::Value::as_str);
    match path {
        Some(p) => check_execute_path(Path::new(p)),
        None => Ok(()),
    }
}

// ---- building a hand-off ----

fn sha_cache() -> &'static Mutex<HashMap<(String, String), String>> {
    static CACHE: OnceLock<Mutex<HashMap<(String, String), String>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// sha256 of a file, checked against the index's blake3 in the same pass so a
/// file that changed after it was indexed is not described with stale hashes.
fn file_sha256(v: &Inner, rec: &ItemRecord) -> Result<String, Fail> {
    let key = (rec.id.clone(), rec.rev());
    if let Some(hit) = sha_cache().lock().expect("sha cache").get(&key) {
        return Ok(hit.clone());
    }
    let path = v.drive.local_path(&rec.id)?;
    let mut sha = sha2::Sha256::new();
    let file = std::fs::File::open(&path).map_err(|e| Fail::Files(e.into()))?;
    let chunked = chunker::chunk_reader(file, |c| {
        sha.update(&c.data);
        Ok(())
    })?;
    let expected = rec.content_hash.clone().unwrap_or_else(|| chunker::hash_bytes(&[]));
    if chunked.content_hash != expected || chunked.size != rec.size {
        let _ = v.drive.reconcile_paths(&[path]);
        return Err(ApiError::new(ErrorCode::RevMismatch, "This file changed while it was being read. Try again.").into());
    }
    let hex = hex::encode(sha.finalize());
    let mut cache = sha_cache().lock().expect("sha cache");
    if cache.len() > 4096 {
        cache.clear();
    }
    cache.insert(key, hex.clone());
    Ok(hex)
}

/// Files under `folder` with their package paths. Skips symlinks that leave
/// the root and links into other roots; the top-level permission file is
/// returned separately (`Some(Err)` when it is unreadable).
fn walk(
    v: &Inner,
    folder: &ItemRecord,
    prefix: &str,
    out: &mut Vec<(String, ItemRecord)>,
    marker: &mut Option<Result<PackageManifest, String>>,
) -> Result<(), Fail> {
    let mut children = v.drive.list(&folder.id)?;
    children.sort_by(|a, b| a.name.cmp(&b.name));
    for child in children {
        if child.link_escapes || child.root_link.is_some() {
            continue;
        }
        let path = if prefix.is_empty() { child.name.clone() } else { format!("{prefix}/{}", child.name) };
        match child.kind {
            ItemKind::Folder => walk(v, &child, &path, out, marker)?,
            ItemKind::File if child.name == MARKER => {
                if prefix.is_empty() {
                    *marker = Some(read_marker(&v.drive.local_path(&child.id)?));
                }
            }
            ItemKind::File => {
                out.push((path, child));
                if out.len() > MAX_FILES {
                    return Err(ApiError::invalid(format!("A package can hold at most {MAX_FILES} files.")).into());
                }
            }
        }
    }
    Ok(())
}

/// The hand-off for `item_id` (a package folder, or one file such as a GGUF).
/// `execute` asks for the run permission to travel with it; it is granted
/// only to the owner's own callers and only if the package can run here
/// (it sits in the Models root, or its permission file allows it).
pub(crate) fn build_manifest(v: &Inner, req: &Req, item_id: &str, execute: bool, kind: Option<PackageKind>) -> Result<PackageManifest, Fail> {
    let (root, item) = v.guard_item(req, item_id, Need::Read)?;
    if !permissions(&root, &item).transfer {
        return Err(ApiError::forbidden("This item can't be copied off this computer.").into());
    }
    let mut found = Vec::new();
    let mut marker = None;
    let name = match item.kind {
        ItemKind::File => {
            found.push((item.name.clone(), item.clone()));
            Path::new(&item.name).file_stem().and_then(|s| s.to_str()).unwrap_or(&item.name).to_string()
        }
        ItemKind::Folder => {
            walk(v, &item, "", &mut found, &mut marker)?;
            item.name.clone()
        }
    };
    if found.is_empty() {
        return Err(ApiError::invalid("This folder has no files to send.").into());
    }
    let runs_here = match &marker {
        Some(Ok(m)) => m.grant.execute,
        Some(Err(_)) => false,
        None => root.kind == RootKind::Models,
    };
    let own = matches!(req.principal.kind, PrincipalKind::Owner | PrincipalKind::OwnDevice);
    let mut files = Vec::with_capacity(found.len());
    for (path, rec) in &found {
        files.push(PackageFile {
            path: path.clone(),
            size: rec.size,
            content_hash: rec.content_hash.clone().unwrap_or_else(|| chunker::hash_bytes(&[])),
            sha256: file_sha256(v, rec)?,
            item_id: Some(rec.id.clone()),
            rev: Some(rec.rev()),
        });
    }
    let landed = marker.and_then(Result::ok);
    let name = landed.as_ref().map(|m| m.name.clone()).unwrap_or(name);
    let kind = kind.or(landed.as_ref().map(|m| m.kind)).unwrap_or_default();
    let m = PackageManifest {
        format: PACKAGE_FORMAT.into(),
        package_id: package_id(&files),
        weights_format: weights_format(&files, &name),
        total_bytes: files.iter().map(|f| f.size).sum(),
        name,
        kind,
        grant: Grant { transfer: true, execute: execute && runs_here && own },
        source: PackageSource { computer_id: v.cfg.computer_id.clone(), item_id: item.id.clone(), rev: item.rev() },
        created_at: v.drive.now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        files,
    };
    m.validate().map_err(|e| ApiError::new(ErrorCode::ManifestInvalid, e))?;
    Ok(m)
}

type Files = Extension<Arc<FilesV2>>;

#[derive(Deserialize)]
pub(crate) struct HandoffQuery {
    #[serde(default)]
    execute: bool,
    #[serde(default)]
    kind: Option<PackageKind>,
}

/// `GET /models/handoff/:item_id[?execute=true][&kind=dataset]`.
pub(crate) async fn handoff(
    Extension(files): Files,
    req: Req,
    UrlPath(item_id): UrlPath<String>,
    q: Result<Query<HandoffQuery>, QueryRejection>,
) -> Result<Json<PackageManifest>, ApiError> {
    let q = q.map(|Query(v)| v).map_err(ApiError::from)?;
    run(&files, move |v| Ok(Json(build_manifest(v, &req, &item_id, q.execute, q.kind)?))).await
}

#[derive(Deserialize)]
pub(crate) struct GrantRequest {
    item_id: String,
    execute: bool,
}

/// `POST /models/grant`: allow (or stop) a landed package from running. Only
/// from this computer itself, never over the relay or from a guest or agent.
pub(crate) async fn grant(
    Extension(files): Files,
    req: Req,
    body: Result<Json<GrantRequest>, JsonRejection>,
) -> Result<Json<PackageManifest>, ApiError> {
    let body = body.map(|Json(b)| b).map_err(ApiError::from)?;
    if req.relayed || !matches!(req.principal.kind, PrincipalKind::Owner | PrincipalKind::OwnDevice) {
        return Err(ApiError::forbidden("Allow a model to run from the computer it is on."));
    }
    run(&files, move |v| {
        let (root, folder) = v.guard_item(&req, &body.item_id, Need::Write)?;
        if folder.kind != ItemKind::Folder {
            return Err(ApiError::invalid("Pick the model's folder.").into());
        }
        let child = v
            .drive
            .list(&folder.id)?
            .into_iter()
            .find(|c| c.kind == ItemKind::File && c.name == MARKER)
            .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "This folder is not a copied model package."))?;
        let mut m = read_marker(&v.drive.local_path(&child.id)?).map_err(|_| execute_denied())?;
        m.grant.execute = body.execute;
        let bytes = serde_json::to_vec_pretty(&m).map_err(|_| ApiError::internal())?;
        let device = EditingDevice { name: req.device_name.clone(), utc_offset_minutes: None };
        v.drive.write_small(&root.root_id, &folder.id, MARKER, &bytes, Some(&child.rev()), OnConflict::Replace, &req.principal, &device)?;
        Ok(Json(m))
    })
    .await
}

// ---- jobs ----

fn jobs() -> &'static Mutex<HashMap<String, JobRef>> {
    static JOBS: OnceLock<Mutex<HashMap<String, JobRef>>> = OnceLock::new();
    JOBS.get_or_init(Default::default)
}

/// A transfer or route job on this computer (`GET /jobs/:job_id`).
pub(crate) fn job(job_id: &str) -> Option<JobRef> {
    jobs().lock().expect("jobs").get(job_id).cloned()
}

fn new_job(kind: JobRefKind) -> JobRef {
    let job = JobRef {
        job_id: format!("job_{}", uuid::Uuid::new_v4().simple()),
        kind,
        state: JobRefState::Queued,
        done_bytes: 0,
        total_bytes: 0,
        error: None,
    };
    let mut all = jobs().lock().expect("jobs");
    if all.len() >= MAX_JOBS {
        all.retain(|_, j| matches!(j.state, JobRefState::Queued | JobRefState::Running));
    }
    all.insert(job.job_id.clone(), job.clone());
    job
}

fn update_job(job_id: &str, f: impl FnOnce(&mut JobRef)) {
    if let Some(j) = jobs().lock().expect("jobs").get_mut(job_id) {
        f(j);
    }
}

fn add_done(job_id: &str, n: u64) {
    update_job(job_id, |j| j.done_bytes = (j.done_bytes + n).min(j.total_bytes));
}

// ---- the source computer ----

#[derive(Deserialize, Clone)]
pub(crate) struct SourceTransport {
    kind: TransportKind,
    /// loopback / mesh: the prefix ending in `/api/v1/files/v2`; relay: the
    /// cloud-api proxy URL (`EndpointsResponse` candidates).
    base_url: String,
}

#[derive(Deserialize, Clone)]
pub(crate) struct SourceRef {
    /// cloud-api runtime device id (`EndpointsResponse.computer_id`).
    computer_id: String,
    item_id: String,
    /// Omitted: this computer when `computer_id` is its own, else the relay.
    #[serde(default)]
    transport: Option<SourceTransport>,
}

fn cloud_api_url() -> String {
    std::env::var("ALLTERNIT_CLOUD_API_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "https://api.allternit.com".into())
        .trim_end_matches('/')
        .to_string()
}

/// Mesh and loopback hosts this runtime will call with the owner's token:
/// loopback, the tailnet ranges (100.64.0.0/10, fd7a:115c:a1e0::/48) and
/// MagicDNS names. Anything else goes through the relay.
fn mesh_host_ok(url: &url::Url) -> bool {
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback() || (ip.octets()[0] == 100 && (ip.octets()[1] & 0xc0) == 64),
        Some(url::Host::Ipv6(ip)) => {
            let s = ip.segments();
            ip.is_loopback() || (s[0] == 0xfd7a && s[1] == 0x115c && s[2] == 0xa1e0)
        }
        Some(url::Host::Domain(d)) => {
            let d = d.to_ascii_lowercase();
            d == "localhost" || d.ends_with(".ts.net")
        }
        None => false,
    }
}

enum Target {
    /// This computer: read the engine directly.
    Local,
    /// `<prefix>/...` over loopback or the mesh.
    Direct(String),
    /// cloud-api `POST /runtime-devices/:id/proxy` with a relay envelope.
    Relay(String),
}

struct Source {
    computer_id: String,
    target: Target,
    client: reqwest::Client,
    /// The caller's own `Authorization`, so the source sees the same owner.
    auth: Option<String>,
    device_id: Option<String>,
    req: Req,
}

impl Source {
    fn new(v: &Inner, src: &SourceRef, headers: &HeaderMap, req: &Req) -> Result<Self, ApiError> {
        let cloud = cloud_api_url();
        let relay_url = format!("{cloud}/api/v1/runtime-devices/{}/proxy", urlencoding::encode(&src.computer_id));
        let target = match &src.transport {
            None if src.computer_id == v.cfg.computer_id => Target::Local,
            None => Target::Relay(relay_url),
            Some(t) => match t.kind {
                // A relayed caller names this computer by leaving the transport
                // out; a loopback URL from it would come back here as a local caller.
                TransportKind::Loopback if req.relayed => {
                    return Err(ApiError::forbidden("Leave the transport out to copy from this computer."));
                }
                TransportKind::Loopback | TransportKind::Mesh => {
                    let url = url::Url::parse(t.base_url.trim()).map_err(|_| ApiError::invalid("That transport URL is not valid."))?;
                    if !mesh_host_ok(&url) || !url.path().trim_end_matches('/').ends_with("/api/v1/files/v2") {
                        return Err(ApiError::invalid("Only loopback or mesh Files URLs can be used here; leave the transport out to use the relay."));
                    }
                    Target::Direct(t.base_url.trim().trim_end_matches('/').to_string())
                }
                TransportKind::Relay if t.base_url.trim().trim_end_matches('/') == relay_url => Target::Relay(relay_url),
                TransportKind::Relay => return Err(ApiError::invalid("That relay URL is not this account's cloud relay for the computer.")),
                TransportKind::Gateway => return Err(ApiError::unsupported("Guest share links can't be pulled from a computer.")),
            },
        };
        let header = |name: &str| headers.get(name).and_then(|h| h.to_str().ok()).map(str::to_string);
        Ok(Self {
            computer_id: src.computer_id.clone(),
            target,
            client: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .map_err(|_| ApiError::internal())?,
            auth: header("authorization"),
            device_id: header(crate::cowork_devices_routes::DEVICE_ID_HEADER),
            req: req.clone(),
        })
    }

    /// GET a v2 path (`/items/...`) on the source and return the raw body.
    async fn get(&self, path: &str) -> Result<Bytes, ApiError> {
        let full = format!("/api/v1/files/v2{path}");
        let rb = match &self.target {
            Target::Local => return Err(ApiError::internal()),
            Target::Direct(prefix) => self.client.get(format!("{prefix}{path}")),
            Target::Relay(url) => self.client.post(url).json(&serde_json::json!({
                "method": "GET",
                "path": full,
                "headers": {},
                "body": "",
                "body_encoding": "utf8",
            })),
        };
        let mut rb = rb;
        if let Some(a) = &self.auth {
            rb = rb.header("authorization", a);
        }
        if let Some(d) = &self.device_id {
            rb = rb.header(crate::cowork_devices_routes::DEVICE_ID_HEADER, d);
        }
        // The source must treat a pull for a relayed caller as relayed too:
        // the marker only ever narrows what the source allows.
        if self.req.relayed {
            if let Target::Direct(_) = self.target {
                rb = rb.header(crate::cowork_devices_routes::RELAYED_HEADER, "1");
            }
        }
        let res = rb
            .send()
            .await
            .map_err(|_| ApiError::new(ErrorCode::Offline, "Couldn't reach the computer the file is on."))?;
        let status = res.status();
        let body = res
            .bytes()
            .await
            .map_err(|_| ApiError::new(ErrorCode::Offline, "The computer the file is on stopped answering."))?;
        if status.is_success() {
            return Ok(body);
        }
        Err(match serde_json::from_slice::<ErrorBody>(&body) {
            Ok(b) => ApiError { status: status_for(b.code.clone()), body: b },
            Err(_) => ApiError::new(ErrorCode::Internal, format!("The computer the file is on answered {status}.")),
        })
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, ApiError> {
        let body = self.get(path).await?;
        serde_json::from_slice(&body).map_err(|_| ApiError::new(ErrorCode::ManifestInvalid, "The other computer sent an answer this runtime can't read."))
    }

    async fn handoff(&self, inner: &Arc<Inner>, item_id: &str, execute: bool) -> Result<PackageManifest, ApiError> {
        if let Target::Local = self.target {
            let (req, id) = (self.req.clone(), item_id.to_string());
            return blocking(inner, move |v| build_manifest(v, &req, &id, execute, None)).await;
        }
        self.get_json(&format!("/models/handoff/{}?execute={execute}", urlencoding::encode(item_id))).await
    }

    async fn stat(&self, inner: &Arc<Inner>, item_id: &str) -> Result<ItemSummary, ApiError> {
        if let Target::Local = self.target {
            let (req, id) = (self.req.clone(), item_id.to_string());
            return blocking(inner, move |v| {
                let (root, rec) = v.guard_item(&req, &id, Need::Read)?;
                if !permissions(&root, &rec).transfer {
                    return Err(ApiError::forbidden("This item can't be copied.").into());
                }
                Ok(ItemSummary { name: rec.name.clone(), kind: rec.kind, size: rec.size, content_hash: rec.content_hash.clone() })
            })
            .await;
        }
        let s: StatResponse = self.get_json(&format!("/items/{}", urlencoding::encode(item_id))).await?;
        if !s.item.permissions.transfer {
            return Err(ApiError::forbidden("This item can't be copied."));
        }
        Ok(ItemSummary { name: s.item.name, kind: s.item.kind, size: s.item.size, content_hash: s.item.content_hash })
    }

    async fn manifest(&self, inner: &Arc<Inner>, item_id: &str, page: Option<String>) -> Result<Manifest, ApiError> {
        if let Target::Local = self.target {
            let (req, id) = (self.req.clone(), item_id.to_string());
            return blocking(inner, move |v| {
                v.guard_item(&req, &id, Need::Read)?;
                Ok(v.drive.manifest(&id, page.as_deref())?)
            })
            .await;
        }
        let mut path = format!("/items/{}/manifest", urlencoding::encode(item_id));
        if let Some(p) = page {
            path.push_str(&format!("?page_token={}", urlencoding::encode(&p)));
        }
        self.get_json(&path).await
    }

    async fn chunk(&self, inner: &Arc<Inner>, hash: &str) -> Result<Vec<u8>, ApiError> {
        let data = if let Target::Local = self.target {
            let h = hash.to_string();
            blocking(inner, move |v| Ok(v.drive.get_chunk(&h)?)).await?
        } else {
            self.get(&format!("/chunks/{hash}")).await?.to_vec()
        };
        if chunker::hash_bytes(&data) != hash {
            return Err(ApiError::new(ErrorCode::IntegrityFailed, "A piece of the file arrived damaged."));
        }
        Ok(data)
    }
}

struct ItemSummary {
    name: String,
    kind: ItemKind,
    size: u64,
    content_hash: Option<String>,
}

async fn blocking<T: Send + 'static>(
    inner: &Arc<Inner>,
    f: impl FnOnce(&Inner) -> Result<T, Fail> + Send + 'static,
) -> Result<T, ApiError> {
    let inner = inner.clone();
    tokio::task::spawn_blocking(move || f(&inner).map_err(|e| inner.fail(e)))
        .await
        .map_err(|_| ApiError::internal())?
}

// ---- pulling ----

/// Pulls one file into `parent_id`: whole manifest, then only the chunks this
/// computer lacks, then a checked commit. Aborts the upload session on error.
#[allow(clippy::too_many_arguments)]
async fn pull_file(
    inner: &Arc<Inner>,
    job_id: &str,
    src: &Source,
    item_id: &str,
    root_id: &str,
    parent_id: &str,
    name: &str,
    expect: (&str, u64),
) -> Result<(), ApiError> {
    let mut refs: Vec<ChunkRef> = Vec::new();
    let mut page = None;
    let first = loop {
        let m = src.manifest(inner, item_id, page.take()).await?;
        let next = m.next_page_token.clone();
        let head = (m.content_hash.clone(), m.size, m.chunker.clone());
        refs.extend(m.chunks);
        if next.is_none() {
            break head;
        }
        page = next;
    };
    let (content_hash, size, params) = first;
    if content_hash != expect.0 || size != expect.1 {
        return Err(ApiError::new(ErrorCode::RevMismatch, format!("{name} changed on the other computer. Start the copy again.")));
    }
    let total_chunks = refs.len() as u64;
    let head: Vec<ChunkRef> = refs.iter().take(REFS_PER_REQUEST).cloned().collect();
    let upload = UploadCreateRequest {
        root_id: root_id.to_string(),
        parent_id: parent_id.to_string(),
        name: name.to_string(),
        item_id: None,
        base_rev: None,
        size,
        content_hash,
        chunker: params,
        chunks: head,
        total_chunks,
        on_conflict: OnConflict::KeepBoth,
        modified_at: None,
        device_name: None,
        utc_offset_minutes: None,
    };
    let by = src.req.principal.clone();
    let rest = refs.clone();
    let status: UploadStatus = blocking(inner, move |v| {
        let mut st = v.drive.create_upload(upload, &by)?;
        for page in rest.chunks(REFS_PER_REQUEST).skip(1) {
            st = v.drive.append_upload_chunks(&st.session_id, page)?;
        }
        Ok(st)
    })
    .await?;
    let session = status.session_id.clone();
    let result = async {
        let need: HashSet<&str> = status.need.iter().map(String::as_str).collect();
        let mut counted = HashSet::new();
        let have_bytes: u64 =
            refs.iter().filter(|r| !need.contains(r.hash.as_str()) && counted.insert(r.hash.as_str())).map(|r| r.size).sum();
        add_done(job_id, have_bytes);
        for hash in &status.need {
            let data = src.chunk(inner, hash).await?;
            let n = data.len() as u64;
            let (s, h) = (session.clone(), hash.clone());
            blocking(inner, move |v| Ok(v.drive.put_upload_chunk(&s, &h, &data)?)).await?;
            add_done(job_id, n);
        }
        let s = session.clone();
        blocking(inner, move |v| Ok(v.drive.commit_upload(&s)?)).await?;
        Ok::<(), ApiError>(())
    }
    .await;
    if result.is_err() {
        let s = session.clone();
        let _ = blocking(inner, move |v| Ok(v.drive.abort_upload(&s)?)).await;
    }
    result
}

/// Folder `parts` under `top`, made as needed (an existing folder is reused).
async fn ensure_dirs(inner: &Arc<Inner>, top: &str, parts: Vec<String>, req: &Req) -> Result<String, ApiError> {
    let (top, by) = (top.to_string(), req.principal.clone());
    blocking(inner, move |v| {
        let mut at = top;
        for p in parts {
            at = v.drive.mkdir(&at, &p, OnConflict::Replace, &by)?.item.id;
        }
        Ok(at)
    })
    .await
}

enum Plan {
    Model { allow_execute: bool },
    Route,
}

async fn pull(inner: &Arc<Inner>, job_id: &str, src: &Source, item_id: &str, dest: (String, String), plan: Plan) -> Result<(), ApiError> {
    let (root_id, folder_id) = dest;
    update_job(job_id, |j| j.state = JobRefState::Running);
    match plan {
        Plan::Route => {
            let it = src.stat(inner, item_id).await?;
            if it.kind != ItemKind::File {
                return Err(ApiError::invalid("Send one file at a time, or send a folder as a model package."));
            }
            update_job(job_id, |j| j.total_bytes = it.size);
            let hash = it.content_hash.unwrap_or_else(|| chunker::hash_bytes(&[]));
            pull_file(inner, job_id, src, item_id, &root_id, &folder_id, &it.name, (&hash, it.size)).await
        }
        Plan::Model { allow_execute } => {
            let mut m = src.handoff(inner, item_id, allow_execute).await?;
            m.validate().map_err(|e| ApiError::new(ErrorCode::ManifestInvalid, e))?;
            update_job(job_id, |j| j.total_bytes = m.total_bytes);
            let (name, by, f) = (m.name.clone(), src.req.principal.clone(), folder_id.clone());
            let pkg = blocking(inner, move |v| Ok(v.drive.mkdir(&f, &name, OnConflict::KeepBoth, &by)?.item.id)).await?;
            for file in &m.files {
                let mut parts: Vec<String> = file.path.split('/').map(str::to_string).collect();
                let leaf = parts.pop().unwrap_or_default();
                let parent = ensure_dirs(inner, &pkg, parts, &src.req).await?;
                let fid = file.item_id.clone().ok_or_else(|| ApiError::new(ErrorCode::ManifestInvalid, "The package lists a file without its item id."))?;
                pull_file(inner, job_id, src, &fid, &root_id, &parent, &leaf, (&file.content_hash, file.size)).await?;
            }
            // The landed permission file: never wider than what was asked for.
            m.grant.execute = m.grant.execute && allow_execute;
            m.source.computer_id = src.computer_id.clone();
            for f in &mut m.files {
                f.item_id = None;
                f.rev = None;
            }
            let bytes = serde_json::to_vec_pretty(&m).map_err(|_| ApiError::internal())?;
            let (by, root) = (src.req.principal.clone(), root_id.clone());
            blocking(inner, move |v| {
                v.drive.write_small(&root, &pkg, MARKER, &bytes, None, OnConflict::Replace, &by, &EditingDevice::default())?;
                Ok(())
            })
            .await
        }
    }
}

fn start(inner: Arc<Inner>, kind: JobRefKind, src: Source, item_id: String, dest: (String, String), plan: Plan) -> JobRef {
    let job = new_job(kind);
    let id = job.job_id.clone();
    tokio::spawn(async move {
        let out = pull(&inner, &id, &src, &item_id, dest, plan).await;
        update_job(&id, |j| match out {
            Ok(()) => {
                j.state = JobRefState::Done;
                j.done_bytes = j.total_bytes;
            }
            Err(e) => {
                tracing::warn!(job = %id, code = e.body.code.as_str(), "files v2: pull failed");
                j.state = JobRefState::Failed;
                j.error = Some(e.body.code);
            }
        });
    });
    job
}

/// The folder a pull writes into, checked now so a bad target fails the
/// request and not the job.
fn dest_folder(v: &Inner, req: &Req, folder_id: &str) -> Result<(String, String), Fail> {
    let (root, folder) = v.guard_item(req, folder_id, Need::Write)?;
    if folder.kind != ItemKind::Folder {
        return Err(ApiError::invalid("Pick a folder to copy into.").into());
    }
    if memory::is_memory(&root) {
        return Err(memory::refuse("take copied files"));
    }
    Ok((root.root_id, folder.id))
}

fn root_of_kind(v: &Inner, kind: RootKind) -> Result<allternit_files_engine::RootRecord, Fail> {
    v.drive
        .roots()?
        .into_iter()
        .find(|r| r.kind == kind)
        .ok_or_else(|| ApiError::new(ErrorCode::RootNotFound, "That folder is not set up on this computer.").into())
}

#[derive(Deserialize)]
pub(crate) struct TransferRequest {
    source: SourceRef,
    /// Default: `~/Allternit/Downloads/Models`.
    #[serde(default)]
    dest_parent_id: Option<String>,
    /// Let the copy run here (only if it could run on the source). Default off.
    #[serde(default)]
    allow_execute: bool,
}

/// `POST /models/transfer` on the receiving computer: a `model_transfer` job.
pub(crate) async fn transfer(
    Extension(files): Files,
    req: Req,
    headers: HeaderMap,
    body: Result<Json<TransferRequest>, JsonRejection>,
) -> Result<Json<JobRef>, ApiError> {
    let body = body.map(|Json(b)| b).map_err(ApiError::from)?;
    let inner = files.inner().await?;
    let src = Source::new(&inner, &body.source, &headers, &req)?;
    let (r, parent) = (req.clone(), body.dest_parent_id.clone());
    let dest = blocking(&inner, move |v| {
        let folder = match parent {
            Some(id) => id,
            None => {
                let downloads = root_of_kind(v, RootKind::Downloads)?;
                v.guard(&r, &downloads.root_id, Need::Write)?;
                v.drive.mkdir(&downloads.root_item_id, MODELS_FOLDER, OnConflict::Replace, &r.principal)?.item.id
            }
        };
        dest_folder(v, &r, &folder)
    })
    .await?;
    let plan = Plan::Model { allow_execute: body.allow_execute };
    Ok(Json(start(inner, JobRefKind::ModelTransfer, src, body.source.item_id.clone(), dest, plan)))
}

#[derive(Deserialize)]
pub(crate) struct RouteRequest {
    source: SourceRef,
    /// A root on this computer. Default: Inbox.
    #[serde(default)]
    dest_root_id: Option<String>,
}

/// `POST /route` on the receiving computer: send one file from any of the
/// owner's computers into this computer's Inbox (or another root). A `copy` job.
pub(crate) async fn route_file(
    Extension(files): Files,
    req: Req,
    headers: HeaderMap,
    body: Result<Json<RouteRequest>, JsonRejection>,
) -> Result<Json<JobRef>, ApiError> {
    let body = body.map(|Json(b)| b).map_err(ApiError::from)?;
    let inner = files.inner().await?;
    let src = Source::new(&inner, &body.source, &headers, &req)?;
    let (r, root_id) = (req.clone(), body.dest_root_id.clone());
    let dest = blocking(&inner, move |v| {
        let root = match root_id {
            Some(id) => v.drive.root(&id)?,
            None => root_of_kind(v, RootKind::Inbox)?,
        };
        dest_folder(v, &r, &root.root_item_id)
    })
    .await?;
    Ok(Json(start(inner, JobRefKind::Copy, src, body.source.item_id.clone(), dest, Plan::Route)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthUser;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::Router;
    use http_body_util::BodyExt;
    use serde_json::json;
    use std::path::PathBuf;
    use tower::ServiceExt;

    fn user() -> AuthUser {
        AuthUser {
            user_id: "u1".into(),
            email: None,
            name: Some("Eoj".into()),
            avatar_url: None,
            tenant_id: None,
            organization_id: None,
            organization_role: None,
            organization_slug: None,
        }
    }

    /// A drive whose Models root holds `tiny-model/` (a GGUF and a config).
    struct Env {
        _tmp: tempfile::TempDir,
        home: PathBuf,
        files: Arc<FilesV2>,
        app: Router,
        gguf: Vec<u8>,
    }

    fn env() -> Env {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let models = base.join("models");
        std::fs::create_dir_all(base.join("home")).unwrap();
        std::fs::create_dir_all(models.join("tiny-model")).unwrap();
        // Bigger than one chunk's minimum so the manifest has real refs.
        let gguf: Vec<u8> = b"GGUF".iter().copied().chain((0..600_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)).collect();
        std::fs::write(models.join("tiny-model/model.gguf"), &gguf).unwrap();
        std::fs::write(models.join("tiny-model/config.json"), br#"{"arch":"qwen3"}"#).unwrap();
        let cfg = super::super::Config {
            enabled: true,
            data_dir: base.join("data"),
            home: Some(base.join("home")),
            computer_id: "rd_test".into(),
            memory_db: None,
            models_dir: Some(models),
            code_dir: None,
            watch: false,
            clock: Some(Arc::new(|| "2026-11-15T16:42:00Z".parse().unwrap())),
        };
        let files = Arc::new(FilesV2::new(cfg));
        let app = super::super::routes::<()>(files.clone()).layer(Extension(user()));
        Env { home: base.join("home"), _tmp: tmp, files, app, gguf }
    }

    impl Env {
        async fn call(&self, method: &str, uri: &str, body: Option<serde_json::Value>) -> (StatusCode, serde_json::Value) {
            let b = Request::builder().method(method).uri(format!("/files/v2{uri}")).header("content-type", "application/json");
            let body = body.map(|v| Body::from(v.to_string())).unwrap_or_else(Body::empty);
            let res = self.app.clone().oneshot(b.body(body).unwrap()).await.unwrap();
            let status = res.status();
            let bytes = res.into_body().collect().await.unwrap().to_bytes();
            (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
        }

        async fn package_item(&self) -> String {
            let inner = self.files.inner().await.unwrap();
            let models = inner.drive.roots().unwrap().into_iter().find(|r| r.kind == RootKind::Models).expect("models root");
            inner.drive.list(&models.root_item_id).unwrap().into_iter().find(|i| i.name == "tiny-model").expect("tiny-model indexed").id
        }
    }

    fn landed(dir: &Path, execute: bool) -> PackageManifest {
        let files = vec![PackageFile {
            path: "model.gguf".into(),
            size: 9,
            content_hash: chunker::hash_bytes(b"GGUF tiny"),
            sha256: hex::encode(sha2::Sha256::digest(b"GGUF tiny")),
            item_id: None,
            rev: None,
        }];
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("model.gguf"), b"GGUF tiny").unwrap();
        let m = PackageManifest {
            format: PACKAGE_FORMAT.into(),
            package_id: package_id(&files),
            name: "tiny".into(),
            kind: PackageKind::Model,
            weights_format: WeightsFormat::Gguf,
            total_bytes: 9,
            files,
            grant: Grant { transfer: true, execute },
            source: PackageSource { computer_id: "rd_mac".into(), item_id: "it_1".into(), rev: "1-dir".into() },
            created_at: "2026-11-15T16:42:00.000Z".into(),
        };
        std::fs::write(dir.join(MARKER), serde_json::to_vec(&m).unwrap()).unwrap();
        m
    }

    #[test]
    fn transfer_only_package_cannot_execute() {
        let tmp = tempfile::tempdir().unwrap();
        let pkg = tmp.path().join("Downloads/Models/tiny");
        landed(&pkg, false);

        // The package folder, a file in it and the folder above it are all refused.
        for p in [pkg.clone(), pkg.join("model.gguf"), pkg.parent().unwrap().to_path_buf()] {
            let e = check_execute_path(&p).expect_err("transfer-only package must not run");
            assert_eq!(e.body.code, ErrorCode::ExecuteDenied, "{}", p.display());
            assert_eq!(e.status, StatusCode::FORBIDDEN);
        }
        // Launch by the id local-engine has on record for that folder.
        let models = json!({"models": [{"id": "local--tiny--1", "path": pkg}, {"id": "hf--other@main", "path": tmp.path().join("other")}]});
        assert_eq!(launch_gate_with(&models, "local--tiny--1").unwrap_err().body.code, ErrorCode::ExecuteDenied);
        assert!(launch_gate_with(&models, "hf--other@main").is_ok(), "models without a permission file still run");
        // Import through allternit-api's local-engine proxy.
        let refused = import_gate(json!({"path": pkg}).to_string().as_bytes()).expect("import refused");
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);

        // Once allowed, it runs.
        landed(&pkg, true);
        assert!(check_execute_path(&pkg).is_ok());
        assert!(import_gate(json!({"path": pkg}).to_string().as_bytes()).is_none());

        // A damaged permission file fails closed.
        std::fs::write(pkg.join(MARKER), b"{not json").unwrap();
        assert_eq!(check_execute_path(&pkg.join("model.gguf")).unwrap_err().body.code, ErrorCode::ExecuteDenied);
    }

    #[tokio::test]
    async fn handoff_manifest_is_well_formed_for_a_small_model() {
        let env = env();
        let id = env.package_item().await;
        let (status, body) = env.call("GET", &format!("/models/handoff/{id}"), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let m: PackageManifest = serde_json::from_value(body.clone()).unwrap();
        assert_eq!(serde_json::to_value(&m).unwrap(), body, "no fields outside the manifest format");
        m.validate().expect("manifest passes its own rules");

        assert_eq!(m.format, PACKAGE_FORMAT);
        assert_eq!(m.name, "tiny-model");
        assert_eq!(m.weights_format, WeightsFormat::Gguf);
        assert_eq!(m.kind, PackageKind::Model);
        let paths: Vec<&str> = m.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["config.json", "model.gguf"]);
        let gguf = &m.files[1];
        assert_eq!(gguf.size, env.gguf.len() as u64);
        assert_eq!(gguf.content_hash, chunker::hash_bytes(&env.gguf));
        assert_eq!(gguf.sha256, hex::encode(sha2::Sha256::digest(&env.gguf)));
        assert!(gguf.item_id.is_some(), "a phone needs the item id to pull the chunks");
        assert_eq!(m.total_bytes, env.gguf.len() as u64 + br#"{"arch":"qwen3"}"#.len() as u64);
        assert_eq!(m.source.computer_id, "rd_test");
        // Run permission travels only when asked for.
        assert_eq!(m.grant, Grant { transfer: true, execute: false });
        let (_, asked) = env.call("GET", &format!("/models/handoff/{id}?execute=true"), None).await;
        assert_eq!(asked["grant"], json!({"transfer": true, "execute": true}));
        assert_eq!(asked["package_id"], body["package_id"], "same bytes, same package id");
    }

    #[tokio::test]
    async fn transferred_package_lands_without_run_permission() {
        let env = env();
        let id = env.package_item().await;
        let (status, job) = env
            .call("POST", "/models/transfer", Some(json!({"source": {"computer_id": "rd_test", "item_id": id}})))
            .await;
        assert_eq!(status, StatusCode::OK, "{job}");
        let job_id = job["job_id"].as_str().unwrap().to_string();
        assert_eq!(job["kind"], "model_transfer");
        let mut last = json!(null);
        for _ in 0..200 {
            let (_, j) = env.call("GET", &format!("/jobs/{job_id}"), None).await;
            if j["state"] == "done" || j["state"] == "failed" {
                last = j;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        assert_eq!(last["state"], "done", "{last}");
        assert_eq!(last["done_bytes"], last["total_bytes"]);

        let pkg = env.home.join("Allternit/Downloads/Models/tiny-model");
        assert_eq!(std::fs::read(pkg.join("model.gguf")).unwrap(), env.gguf, "bytes arrive intact");
        let marker = read_marker(&pkg.join(MARKER)).unwrap();
        assert_eq!(marker.grant, Grant { transfer: true, execute: false });
        assert!(marker.files.iter().all(|f| f.item_id.is_none()));
        assert_eq!(check_execute_path(&pkg).unwrap_err().body.code, ErrorCode::ExecuteDenied);
    }

    async fn wait_job(env: &Env, job_id: &str) -> serde_json::Value {
        for _ in 0..200 {
            let (_, j) = env.call("GET", &format!("/jobs/{job_id}"), None).await;
            if j["state"] == "done" || j["state"] == "failed" {
                return j;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        panic!("job {job_id} never finished");
    }

    /// Route a file to a computer: one file lands in this computer's Inbox
    /// with the same bytes, and a folder is refused (it goes as a package).
    #[tokio::test]
    async fn routed_file_lands_in_the_inbox() {
        let env = env();
        let pkg = env.package_item().await;
        let inner = env.files.inner().await.unwrap();
        let config = inner.drive.list(&pkg).unwrap().into_iter().find(|i| i.name == "config.json").unwrap().id;

        let (status, job) = env.call("POST", "/route", Some(json!({"source": {"computer_id": "rd_test", "item_id": config}}))).await;
        assert_eq!(status, StatusCode::OK, "{job}");
        assert_eq!(job["kind"], "copy");
        let done = wait_job(&env, job["job_id"].as_str().unwrap()).await;
        assert_eq!(done["state"], "done", "{done}");
        assert_eq!(std::fs::read(env.home.join("Allternit/Inbox/config.json")).unwrap(), br#"{"arch":"qwen3"}"#);

        let (_, job) = env.call("POST", "/route", Some(json!({"source": {"computer_id": "rd_test", "item_id": pkg}}))).await;
        let failed = wait_job(&env, job["job_id"].as_str().unwrap()).await;
        assert_eq!(failed["state"], "failed");
        assert_eq!(failed["error"], "invalid_request");
    }

    /// A relayed caller must not get this runtime to call itself over
    /// loopback, where the call would count as a local one.
    #[tokio::test]
    async fn relayed_caller_cannot_pull_over_loopback() {
        let env = env();
        let id = env.package_item().await;
        let body = json!({"source": {"computer_id": "rd_other", "item_id": id,
            "transport": {"kind": "loopback", "base_url": "http://127.0.0.1:3010/api/v1/files/v2"}}});
        let req = Request::builder()
            .method("POST")
            .uri("/files/v2/models/transfer")
            .header("content-type", "application/json")
            .header(crate::cowork_devices_routes::RELAYED_HEADER, "1")
            .body(Body::from(body.to_string()))
            .unwrap();
        let res = env.app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
        // Not a mesh or loopback host at all: refused for everyone.
        let (status, _) = env
            .call("POST", "/models/transfer", Some(json!({"source": {"computer_id": "rd_other", "item_id": id,
                "transport": {"kind": "mesh", "base_url": "http://169.254.169.254/api/v1/files/v2"}}})))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    /// The grant can't be flipped over the relay: run permission is given on
    /// the computer the model is on.
    #[tokio::test]
    async fn run_permission_is_not_granted_over_the_relay() {
        let env = env();
        let req = Request::builder()
            .method("POST")
            .uri("/files/v2/models/grant")
            .header("content-type", "application/json")
            .header(crate::cowork_devices_routes::RELAYED_HEADER, "1")
            .body(Body::from(json!({"item_id": "it_x", "execute": true}).to_string()))
            .unwrap();
        let res = env.app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }
}
