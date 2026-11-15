//! Engine packs: signed per-OS inference engines (llama-server builds, the
//! MLX sidecar) that local-engine downloads instead of the Desktop installer
//! bundling them (plan "On-device models", decision 1; format in
//! spec/on-device/engine-pack-spike.md).
//!
//! Feed (same signing and object store shape as Desktop runtime packages):
//!   <feed>/engines/<pack_id>/latest.json (+ .sig)   signed pointer (FeedEntry)
//!   <feed>/engines/<pack_id>/<manifest>  (+ .sig)   signed pack.json
//!   <feed>/engines/objects/<sha256>.gz              every file, gzip, by content
//! Objects live under `engines/` so the runtime object prune never sees them.
//!
//! On disk (`<data_dir>/engines`, so per OS: ~/Library/Application Support,
//! %APPDATA%, ~/.local/share):
//!   <pack_id>/<version>/{pack.json, pack.json.sig, files...}
//!   <pack_id>/state.json   { active, previous, bad[] }
//!   .staging/              downloads in progress (never spawned from)
//!
//! Rules: a pack is installed only after its signature and every file's
//! sha256 check out, and it is checked again before every spawn
//! ([`EnginePacks::resolve`]). A failed check rolls back to the previous
//! version, or to nothing. The production feed and key are fixed; a debug
//! build of local-engine also accepts `ALLTERNIT_ENGINE_PACK_FEED`
//! (file:// or http://) and `ALLTERNIT_ENGINE_PACK_DEV_PUBKEY` for local tests.
//! Release builds ignore both.

pub mod pick;
pub mod routes;
pub mod verify;

use crate::hardware::EngineBackend;
use chrono::{DateTime, Utc};
use ed25519_dalek::VerifyingKey;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};
use verify::{FeedEntry, PackManifest, VerifyError};

pub const DEFAULT_FEED: &str = "https://runtime.allternit.com";

#[derive(Debug, thiserror::Error)]
pub enum PackError {
    #[error("pack {0} is not published for this machine")]
    NotPublished(String),
    #[error("feed: {0}")]
    Feed(String),
    #[error("verification failed: {0}")]
    Verify(#[from] VerifyError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("an install of {0} is already running")]
    Busy(String),
    #[error("{0}")]
    Invalid(String),
}

/// Where packs come from.
#[derive(Debug, Clone)]
enum Source {
    Http { base: String },
    File { root: PathBuf },
}

/// Install progress, shown in `GET /engines/packs` and `GET /status`.
#[derive(Debug, Clone, Serialize)]
pub struct InstallJob {
    pub pack_id: String,
    /// queued, downloading, verifying, installed, failed.
    pub state: String,
    pub version: Option<String>,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub error: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

/// An installed, verified pack ready to spawn.
#[derive(Debug, Clone)]
pub struct ResolvedPack {
    pub pack_id: String,
    pub version: String,
    pub backend: EngineBackend,
    pub dir: PathBuf,
    pub entrypoint: PathBuf,
}

/// One installed pack, as listed by the API.
#[derive(Debug, Clone, Serialize)]
pub struct InstalledPack {
    pub pack_id: String,
    pub engine: String,
    pub backend: String,
    pub version: String,
    pub build: u64,
    pub previous: Option<String>,
    pub dir: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PackState {
    active: Option<String>,
    previous: Option<String>,
    #[serde(default)]
    bad: Vec<String>,
}

struct Inner {
    root: PathBuf,
    keys: Vec<VerifyingKey>,
    source: Source,
    dev_source: bool,
    platform: String,
    jobs: RwLock<HashMap<String, InstallJob>>,
}

#[derive(Clone)]
pub struct EnginePacks {
    inner: Arc<Inner>,
}

fn parse_source(feed: &str, allow_dev: bool) -> Option<Source> {
    let feed = feed.trim().trim_end_matches('/');
    if let Some(rest) = feed.strip_prefix("https://") {
        if !rest.is_empty() {
            return Some(Source::Http { base: feed.to_string() });
        }
    }
    if !allow_dev {
        return None;
    }
    if feed.starts_with("http://") {
        return Some(Source::Http { base: feed.to_string() });
    }
    feed.strip_prefix("file://").map(|p| Source::File { root: PathBuf::from(p) })
}

impl EnginePacks {
    /// Production feed and key; debug builds also read the dev overrides.
    pub fn from_env(data_dir: &Path) -> Self {
        let mut keys = vec![verify::parse_public_key(verify::RUNTIME_PUBLIC_KEY_SPKI).expect("built-in key")];
        let mut source = parse_source(DEFAULT_FEED, false).expect("default feed");
        let mut dev_source = false;
        if cfg!(debug_assertions) {
            if let Ok(feed) = std::env::var("ALLTERNIT_ENGINE_PACK_FEED") {
                match parse_source(&feed, true) {
                    Some(s) => {
                        warn!(%feed, "engine packs: dev feed (debug build only)");
                        source = s;
                        dev_source = true;
                    }
                    None => warn!(%feed, "engine packs: ignoring unusable feed"),
                }
            }
            if let Ok(text) = std::env::var("ALLTERNIT_ENGINE_PACK_DEV_PUBKEY") {
                match verify::parse_public_key(&text) {
                    Some(k) => {
                        warn!("engine packs: trusting a dev signing key (debug build only)");
                        keys.push(k);
                    }
                    None => warn!("engine packs: ALLTERNIT_ENGINE_PACK_DEV_PUBKEY is not an Ed25519 key"),
                }
            }
        }
        Self::with(data_dir.join("engines"), keys, source, dev_source, verify::platform_id())
    }

    fn with(root: PathBuf, keys: Vec<VerifyingKey>, source: Source, dev_source: bool, platform: String) -> Self {
        Self { inner: Arc::new(Inner { root, keys, source, dev_source, platform, jobs: RwLock::new(HashMap::new()) }) }
    }

    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    pub fn platform(&self) -> &str {
        &self.inner.platform
    }

    pub fn dev_source(&self) -> bool {
        self.inner.dev_source
    }

    fn pack_dir(&self, pack_id: &str) -> PathBuf {
        self.inner.root.join(pack_id)
    }

    fn read_state(&self, pack_id: &str) -> PackState {
        std::fs::read(self.pack_dir(pack_id).join("state.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn write_state(&self, pack_id: &str, state: &PackState) -> std::io::Result<()> {
        let dir = self.pack_dir(pack_id);
        std::fs::create_dir_all(&dir)?;
        let tmp = dir.join("state.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(state).unwrap_or_default())?;
        std::fs::rename(tmp, dir.join("state.json"))
    }

    fn pack_ids(&self) -> Vec<String> {
        let Ok(rd) = std::fs::read_dir(&self.inner.root) else { return vec![] };
        let mut ids: Vec<String> = rd
            .filter_map(|e| e.ok())
            .filter(|e| e.path().join("state.json").is_file())
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| !n.starts_with('.'))
            .collect();
        ids.sort();
        ids
    }

    /// Installed packs (signature checked, file hashes not: that happens
    /// before spawn). Cheap enough for `/hardware`.
    pub fn list_installed(&self) -> Vec<InstalledPack> {
        let mut out = Vec::new();
        for id in self.pack_ids() {
            let state = self.read_state(&id);
            let Some(version) = state.active.clone() else { continue };
            let dir = self.pack_dir(&id).join(&version);
            let manifest = std::fs::read(dir.join("pack.json")).ok().and_then(|raw| {
                let sig = std::fs::read_to_string(dir.join("pack.json.sig")).ok()?;
                verify::parse_signed_manifest(&raw, &sig, &self.inner.keys, &self.inner.platform).ok()
            });
            if let Some(m) = manifest {
                out.push(InstalledPack {
                    pack_id: id,
                    engine: m.engine,
                    backend: m.backend,
                    version: m.version,
                    build: m.build,
                    previous: state.previous,
                    dir: dir.to_string_lossy().into_owned(),
                });
            }
        }
        out
    }

    /// The best installed pack for `engine` (`llamacpp` / `mlx`), fully
    /// verified (signature + every file's sha256). Blocking: hashes files.
    /// A pack that fails is rolled back to its previous version; if that
    /// fails too the pack is dropped. `Ok(None)` = nothing installed.
    pub fn resolve(&self, engine: &str) -> Result<Option<ResolvedPack>, PackError> {
        let mut candidates: Vec<InstalledPack> =
            self.list_installed().into_iter().filter(|p| p.engine == engine).collect();
        candidates.sort_by_key(|p| backend_rank(pick::parse_backend(&p.backend)));
        let mut last_err = None;
        for p in candidates {
            match self.verify_active(&p.pack_id) {
                Ok(r) => return Ok(Some(r)),
                Err(e) => last_err = Some(e),
            }
        }
        match last_err {
            Some(e) => Err(e),
            None => Ok(None),
        }
    }

    /// Verify the active version; on failure roll back to `previous`.
    fn verify_active(&self, pack_id: &str) -> Result<ResolvedPack, PackError> {
        let mut state = self.read_state(pack_id);
        loop {
            let Some(version) = state.active.clone() else {
                return Err(PackError::Invalid(format!("{pack_id}: no verified version left")));
            };
            let dir = self.pack_dir(pack_id).join(&version);
            match verify::verify_installed(&dir, &self.inner.keys, &self.inner.platform) {
                Ok(m) if m.pack_id == pack_id && m.version == version => {
                    let entrypoint = dir.join(verify::safe_rel(&m.entrypoint)?);
                    return Ok(ResolvedPack {
                        pack_id: pack_id.to_string(),
                        version,
                        backend: pick::parse_backend(&m.backend).unwrap_or(EngineBackend::Cpu),
                        dir,
                        entrypoint,
                    });
                }
                other => {
                    let why = match other {
                        Err(e) => e.to_string(),
                        Ok(_) => "pack id or version does not match its folder".into(),
                    };
                    warn!(%pack_id, %version, %why, "engine pack failed verification; rolling back");
                    let _ = std::fs::remove_dir_all(&dir);
                    if !state.bad.contains(&version) {
                        state.bad.push(version.clone());
                    }
                    state.active = state.previous.take();
                    let _ = self.write_state(pack_id, &state);
                    if state.active.is_none() {
                        return Err(PackError::Invalid(format!("{pack_id} {version}: {why}")));
                    }
                }
            }
        }
    }

    pub async fn jobs(&self) -> Vec<InstallJob> {
        let mut jobs: Vec<InstallJob> = self.inner.jobs.read().await.values().cloned().collect();
        jobs.sort_by(|a, b| a.pack_id.cmp(&b.pack_id));
        jobs
    }

    async fn job_running(&self, pack_id: &str) -> bool {
        self.inner
            .jobs
            .read()
            .await
            .get(pack_id)
            .is_some_and(|j| matches!(j.state.as_str(), "queued" | "downloading" | "verifying"))
    }

    async fn update_job(&self, pack_id: &str, f: impl FnOnce(&mut InstallJob)) {
        if let Some(job) = self.inner.jobs.write().await.get_mut(pack_id) {
            f(job);
        }
    }

    /// Start installing the first of `choices` the feed has, in the
    /// background. Returns at once; progress is in [`Self::jobs`].
    pub async fn start_install(&self, choices: Vec<String>) -> Result<String, PackError> {
        let first = choices.first().cloned().ok_or_else(|| PackError::Invalid("no pack to install".into()))?;
        for id in &choices {
            if self.job_running(id).await {
                return Err(PackError::Busy(id.clone()));
            }
        }
        self.inner.jobs.write().await.insert(
            first.clone(),
            InstallJob {
                pack_id: first.clone(),
                state: "queued".into(),
                version: None,
                bytes_done: 0,
                bytes_total: 0,
                error: None,
                started_at: Utc::now(),
                finished_at: None,
            },
        );
        let this = self.clone();
        let task_first = first.clone();
        tokio::spawn(async move {
            let mut job_id = task_first;
            let mut outcome = Err(PackError::Invalid("no pack tried".into()));
            for (i, id) in choices.iter().enumerate() {
                if i > 0 {
                    // Move the job to the next choice so progress shows the right pack.
                    let mut jobs = this.inner.jobs.write().await;
                    if let Some(mut job) = jobs.remove(&job_id) {
                        job.pack_id = id.clone();
                        jobs.insert(id.clone(), job);
                    }
                    job_id = id.clone();
                }
                outcome = this.install(id).await;
                match &outcome {
                    Err(PackError::NotPublished(_)) => continue,
                    _ => break,
                }
            }
            let now = Utc::now();
            match outcome {
                Ok(m) => {
                    info!(pack_id = %m.pack_id, version = %m.version, "engine pack installed");
                    this.update_job(&job_id, |j| {
                        j.state = "installed".into();
                        j.version = Some(m.version);
                        j.bytes_done = j.bytes_total;
                        j.finished_at = Some(now);
                    })
                    .await;
                }
                Err(e) => {
                    warn!(pack_id = %job_id, error = %e, "engine pack install failed");
                    this.update_job(&job_id, |j| {
                        j.state = "failed".into();
                        j.error = Some(e.to_string());
                        j.finished_at = Some(now);
                    })
                    .await;
                }
            }
            crate::routes::hardware::invalidate().await;
        });
        Ok(first)
    }

    async fn fetch(&self, rel: &str) -> Result<Option<Vec<u8>>, PackError> {
        match &self.inner.source {
            Source::Http { base } => {
                let url = format!("{base}/{rel}");
                let res = http_client()
                    .get(&url)
                    .send()
                    .await
                    .map_err(|e| PackError::Feed(format!("{url}: {e}")))?;
                if res.status() == reqwest::StatusCode::NOT_FOUND {
                    return Ok(None);
                }
                if !res.status().is_success() {
                    return Err(PackError::Feed(format!("{url}: HTTP {}", res.status())));
                }
                let body = res.bytes().await.map_err(|e| PackError::Feed(format!("{url}: {e}")))?;
                Ok(Some(body.to_vec()))
            }
            Source::File { root } => {
                let path = root.join(verify::safe_rel(rel)?);
                match tokio::fs::read(&path).await {
                    Ok(b) => Ok(Some(b)),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                    Err(e) => Err(e.into()),
                }
            }
        }
    }

    /// Download `engines/objects/<sha>.gz` to `dest.gz`, then gunzip into
    /// `dest`, checking size and sha256 of the result.
    async fn fetch_object(&self, sha: &str, size: u64, dest: &Path) -> Result<(), PackError> {
        let rel = format!("engines/objects/{sha}.gz");
        let gz = dest.with_extension("download.gz");
        match &self.inner.source {
            Source::Http { base } => {
                let url = format!("{base}/{rel}");
                let res = http_client()
                    .get(&url)
                    .send()
                    .await
                    .map_err(|e| PackError::Feed(format!("{url}: {e}")))?;
                if !res.status().is_success() {
                    return Err(PackError::Feed(format!("{url}: HTTP {}", res.status())));
                }
                let mut file = tokio::fs::File::create(&gz).await?;
                let mut stream = res.bytes_stream();
                while let Some(chunk) = stream.next().await {
                    let chunk = chunk.map_err(|e| PackError::Feed(format!("{url}: {e}")))?;
                    tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
                }
                tokio::io::AsyncWriteExt::flush(&mut file).await?;
            }
            Source::File { root } => {
                tokio::fs::copy(root.join(verify::safe_rel(&rel)?), &gz).await?;
            }
        }
        let (gz2, dest2, sha2) = (gz.clone(), dest.to_path_buf(), sha.to_string());
        let res = tokio::task::spawn_blocking(move || -> Result<(), PackError> {
            let mut dec = flate2::read::GzDecoder::new(std::fs::File::open(&gz2)?);
            let mut out = std::fs::File::create(&dest2)?;
            let mut buf = vec![0u8; 1 << 20];
            let mut written = 0u64;
            loop {
                let n = dec.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                out.write_all(&buf[..n])?;
                written += n as u64;
            }
            out.sync_all()?;
            if written != size || verify::sha256_file(&dest2)? != sha2 {
                return Err(VerifyError::Mismatch(dest2.to_string_lossy().into_owned()).into());
            }
            Ok(())
        })
        .await
        .map_err(|e| PackError::Invalid(e.to_string()))?;
        let _ = tokio::fs::remove_file(&gz).await;
        res
    }

    /// Download, verify and atomically activate the newest `pack_id`.
    async fn install(&self, pack_id: &str) -> Result<PackManifest, PackError> {
        if !pack_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') || pack_id.is_empty() {
            return Err(PackError::Invalid(format!("bad pack id {pack_id}")));
        }
        self.update_job(pack_id, |j| j.state = "downloading".into()).await;
        let keys = &self.inner.keys;

        // latest.json and its .sig are published one after the other: read the pair twice before failing.
        let mut entry_raw = None;
        for attempt in 0..2 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
            let Some(raw) = self.fetch(&format!("engines/{pack_id}/latest.json")).await? else {
                return Err(PackError::NotPublished(pack_id.into()));
            };
            let sig = self.fetch(&format!("engines/{pack_id}/latest.json.sig")).await?.unwrap_or_default();
            if verify::verify_signature(&raw, &String::from_utf8_lossy(&sig), keys) {
                entry_raw = Some(raw);
                break;
            }
        }
        let entry_raw = entry_raw.ok_or(PackError::Verify(VerifyError::BadSignature))?;
        let entry: FeedEntry =
            serde_json::from_slice(&entry_raw).map_err(|e| PackError::Feed(format!("latest.json: {e}")))?;
        if entry.pack_id.as_deref().is_some_and(|id| id != pack_id) {
            return Err(PackError::Feed("latest.json names another pack".into()));
        }

        let mut state = self.read_state(pack_id);
        if state.bad.contains(&entry.version) {
            return Err(PackError::Invalid(format!("{pack_id} {} failed verification here before", entry.version)));
        }
        if state.active.as_deref() == Some(entry.version.as_str()) {
            let id = pack_id.to_string();
            let this = self.clone();
            if let Ok(Ok(r)) = tokio::task::spawn_blocking(move || this.verify_active(&id)).await {
                let dir = r.dir.clone();
                let raw = std::fs::read(dir.join("pack.json"))?;
                let sig = std::fs::read_to_string(dir.join("pack.json.sig"))?;
                return Ok(verify::parse_signed_manifest(&raw, &sig, keys, &self.inner.platform)?);
            }
            state = self.read_state(pack_id);
        }

        let manifest_rel = format!("engines/{pack_id}/{}", verify::safe_rel(&entry.manifest)?.to_string_lossy());
        let raw = self
            .fetch(&manifest_rel)
            .await?
            .ok_or_else(|| PackError::Feed(format!("{manifest_rel} missing")))?;
        if verify::sha256_hex(&raw) != entry.manifest_sha256 {
            return Err(PackError::Feed("manifest checksum does not match latest.json".into()));
        }
        let sig = String::from_utf8_lossy(&self.fetch(&format!("{manifest_rel}.sig")).await?.unwrap_or_default()).into_owned();
        let m = verify::parse_signed_manifest(&raw, &sig, keys, &self.inner.platform)?;
        if m.pack_id != pack_id || m.version != entry.version || m.build != entry.build {
            return Err(PackError::Feed("manifest does not match latest.json".into()));
        }

        let total: u64 = m.files.values().map(|f| f.size).sum();
        self.update_job(pack_id, |j| {
            j.version = Some(m.version.clone());
            j.bytes_total = total;
        })
        .await;

        let staging = self.inner.root.join(".staging").join(format!("{pack_id}-{}", m.version));
        let _ = tokio::fs::remove_dir_all(&staging).await;
        tokio::fs::create_dir_all(&staging).await?;
        let result = self.stage_and_activate(pack_id, &m, &raw, &sig, &staging, state).await;
        let _ = tokio::fs::remove_dir_all(&staging).await;
        result.map(|_| m)
    }

    async fn stage_and_activate(
        &self,
        pack_id: &str,
        m: &PackManifest,
        raw: &[u8],
        sig: &str,
        staging: &Path,
        mut state: PackState,
    ) -> Result<(), PackError> {
        // Files the active version already has (same hash) are copied, not downloaded.
        let active_dir = state.active.as_ref().map(|v| self.pack_dir(pack_id).join(v));
        let active_files = active_dir
            .as_ref()
            .and_then(|d| std::fs::read(d.join("pack.json")).ok())
            .and_then(|b| serde_json::from_slice::<PackManifest>(&b).ok())
            .map(|am| am.files)
            .unwrap_or_default();
        for (rel, f) in &m.files {
            let dst = staging.join(verify::safe_rel(rel)?);
            if let Some(parent) = dst.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            let reuse = active_files
                .iter()
                .find(|(_, af)| af.sha256 == f.sha256 && af.size == f.size)
                .and_then(|(arel, _)| Some(active_dir.as_ref()?.join(verify::safe_rel(arel).ok()?)));
            let copied = match reuse {
                Some(src) => tokio::fs::copy(&src, &dst).await.is_ok(),
                None => false,
            };
            if !copied {
                self.fetch_object(&f.sha256, f.size, &dst).await?;
            }
            let size = f.size;
            self.update_job(pack_id, |j| j.bytes_done += size).await;
        }
        tokio::fs::write(staging.join("pack.json"), raw).await?;
        tokio::fs::write(staging.join("pack.json.sig"), sig).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let ep = staging.join(verify::safe_rel(&m.entrypoint)?);
            tokio::fs::set_permissions(&ep, std::fs::Permissions::from_mode(0o755)).await?;
        }

        self.update_job(pack_id, |j| j.state = "verifying".into()).await;
        let (dir, keys, platform) = (staging.to_path_buf(), self.inner.keys.clone(), self.inner.platform.clone());
        tokio::task::spawn_blocking(move || verify::verify_installed(&dir, &keys, &platform))
            .await
            .map_err(|e| PackError::Invalid(e.to_string()))??;

        // Atomic switch: rename the verified dir into place, then flip state.json.
        let final_dir = self.pack_dir(pack_id).join(&m.version);
        tokio::fs::create_dir_all(self.pack_dir(pack_id)).await?;
        if final_dir.exists() {
            tokio::fs::remove_dir_all(&final_dir).await?;
        }
        tokio::fs::rename(staging, &final_dir).await?;
        if state.active.as_deref() != Some(m.version.as_str()) {
            state.previous = state.active.take();
        }
        state.active = Some(m.version.clone());
        self.write_state(pack_id, &state)?;
        self.prune(pack_id, &state);
        Ok(())
    }

    /// Keep only the active and previous versions.
    fn prune(&self, pack_id: &str, state: &PackState) {
        let Ok(rd) = std::fs::read_dir(self.pack_dir(pack_id)) else { return };
        for e in rd.filter_map(|e| e.ok()) {
            let name = e.file_name().to_string_lossy().into_owned();
            let keep = state.active.as_deref() == Some(name.as_str()) || state.previous.as_deref() == Some(name.as_str());
            if e.path().is_dir() && !keep {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }

    /// Remove an installed pack (all versions).
    pub async fn remove(&self, pack_id: &str) -> Result<bool, PackError> {
        if !pack_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') || pack_id.is_empty() {
            return Err(PackError::Invalid(format!("bad pack id {pack_id}")));
        }
        if self.job_running(pack_id).await {
            return Err(PackError::Busy(pack_id.into()));
        }
        let dir = self.pack_dir(pack_id);
        let existed = dir.exists();
        if existed {
            tokio::fs::remove_dir_all(&dir).await?;
        }
        self.inner.jobs.write().await.remove(pack_id);
        crate::routes::hardware::invalidate().await;
        Ok(existed)
    }
}

fn backend_rank(b: Option<EngineBackend>) -> u8 {
    match b {
        Some(EngineBackend::Cuda) => 0,
        Some(EngineBackend::Rocm) => 1,
        Some(EngineBackend::Metal) => 2,
        Some(EngineBackend::Vulkan) => 3,
        Some(EngineBackend::Sycl) => 4,
        Some(EngineBackend::Cpu) => 5,
        None => 6,
    }
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .read_timeout(std::time::Duration::from_secs(60))
        .build()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use ed25519_dalek::{Signer, SigningKey};
    use std::collections::BTreeMap;
    use verify::FileEntry;

    fn sign(k: &SigningKey, data: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(k.sign(data).to_bytes())
    }

    fn gz(data: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    /// Publish one version of a pack into a file feed, signed by `k`.
    fn publish(feed: &Path, k: &SigningKey, version: &str, binary: &[u8], tamper_object: bool) {
        let id = "llamacpp-cpu-linux-x64";
        let sha = verify::sha256_hex(binary);
        let objects = feed.join("engines/objects");
        std::fs::create_dir_all(&objects).unwrap();
        let body = if tamper_object { gz(b"evil") } else { gz(binary) };
        std::fs::write(objects.join(format!("{sha}.gz")), body).unwrap();
        let mut files = BTreeMap::new();
        files.insert("bin/llama-server".to_string(), FileEntry { sha256: sha, size: binary.len() as u64 });
        let m = PackManifest {
            format: 1, kind: "engine-pack".into(), pack_id: id.into(), engine: "llamacpp".into(),
            backend: "cpu".into(), version: version.into(), build: version.len() as u64, platform: "linux-x64".into(),
            shell_api: 1, os: "linux".into(), arch: "x64".into(), min_os: String::new(),
            entrypoint: "bin/llama-server".into(), source_commit: String::new(), files,
        };
        let raw = serde_json::to_vec(&m).unwrap();
        let dir = feed.join("engines").join(id);
        std::fs::create_dir_all(dir.join("manifests")).unwrap();
        std::fs::write(dir.join(format!("manifests/{version}.json")), &raw).unwrap();
        std::fs::write(dir.join(format!("manifests/{version}.json.sig")), sign(k, &raw)).unwrap();
        let entry = serde_json::json!({"format": 2, "pack_id": id, "version": version, "build": m.build,
            "manifest": format!("manifests/{version}.json"), "manifestSha256": verify::sha256_hex(&raw)});
        let entry = serde_json::to_vec(&entry).unwrap();
        std::fs::write(dir.join("latest.json"), &entry).unwrap();
        std::fs::write(dir.join("latest.json.sig"), sign(k, &entry)).unwrap();
    }

    fn packs(home: &Path, feed: &Path, k: &SigningKey) -> EnginePacks {
        EnginePacks::with(
            home.join("engines"),
            vec![k.verifying_key()],
            Source::File { root: feed.to_path_buf() },
            true,
            "linux-x64".into(),
        )
    }

    #[tokio::test]
    async fn install_verifies_activates_and_rolls_back() {
        let (home, feed) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let k = SigningKey::from_bytes(&[3u8; 32]);
        let p = packs(home.path(), feed.path(), &k);
        let id = "llamacpp-cpu-linux-x64";

        publish(feed.path(), &k, "b1", b"binary-one", false);
        let m = p.install(id).await.unwrap();
        assert_eq!(m.version, "b1");
        let r = p.resolve("llamacpp").unwrap().unwrap();
        assert_eq!(std::fs::read(&r.entrypoint).unwrap(), b"binary-one");

        // A tampered object never gets activated; b1 stays active.
        publish(feed.path(), &k, "b2", b"binary-two", true);
        assert!(p.install(id).await.is_err());
        assert_eq!(p.resolve("llamacpp").unwrap().unwrap().version, "b1");
        assert!(!home.path().join("engines/.staging").join(format!("{id}-b2")).exists());

        // A good b2 installs; b1 is kept as previous.
        publish(feed.path(), &k, "b2", b"binary-two", false);
        p.install(id).await.unwrap();
        let r = p.resolve("llamacpp").unwrap().unwrap();
        assert_eq!(r.version, "b2");

        // The binary changes on disk after install: spawn-time verify rolls back to b1.
        std::fs::write(&r.entrypoint, b"binary-xxx").unwrap();
        let r = p.resolve("llamacpp").unwrap().unwrap();
        assert_eq!(r.version, "b1");
        assert_eq!(std::fs::read(&r.entrypoint).unwrap(), b"binary-one");
        // b2 is now marked bad and is not installed again.
        assert!(p.install(id).await.is_err());
    }

    #[tokio::test]
    async fn feed_signed_by_another_key_is_refused() {
        let (home, feed) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let trusted = SigningKey::from_bytes(&[3u8; 32]);
        let attacker = SigningKey::from_bytes(&[4u8; 32]);
        publish(feed.path(), &attacker, "b1", b"binary-one", false);
        let p = packs(home.path(), feed.path(), &trusted);
        assert!(matches!(p.install("llamacpp-cpu-linux-x64").await, Err(PackError::Verify(VerifyError::BadSignature))));
        assert!(p.resolve("llamacpp").unwrap().is_none());
        assert!(matches!(p.install("llamacpp-cuda-linux-x64").await, Err(PackError::NotPublished(_))));
    }

    #[test]
    fn release_builds_take_only_https_feeds() {
        assert!(parse_source("https://runtime.allternit.com/", false).is_some());
        assert!(parse_source("http://127.0.0.1:8000", false).is_none());
        assert!(parse_source("file:///tmp/feed", false).is_none());
        assert!(parse_source("file:///tmp/feed", true).is_some());
    }
}
