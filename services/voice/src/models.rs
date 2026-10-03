//! Model pack management: manifests with pinned sha256, atomic resumable
//! downloads, and tar.bz2 extraction.
//!
//! Packs live under `~/.allternit/models/voice/<pack>/` and download on
//! first use. The base URL is configurable via `ALLTERNIT_VOICE_MODEL_BASE`
//! (Phase 2 moves hosting to runtime.allternit.com); hashes are pinned here
//! in code and verified before a file is considered usable.

use serde::Serialize;
use sha2::Digest;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tracing::{info, warn};

const DEFAULT_BASE_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download";

/// One downloadable file of a pack. `asset` is the release-asset path
/// (`<tag>/<file name>`) appended to the base URL. Archives (`.tar.bz2`)
/// are extracted into the pack directory after verification.
pub struct PackFile {
    pub asset: &'static str,
    pub sha256: &'static str,
}

/// A named model pack.
pub struct Pack {
    pub name: &'static str,
    pub description: &'static str,
    pub files: &'static [PackFile],
}

/// sha256 hashes are pinned after downloading from the upstream k2-fsa
/// sherpa-onnx releases and verifying size + decodability.
pub const PACKS: &[Pack] = &[
    Pack {
        name: "small",
        description: "Silero VAD + Moonshine tiny EN (quantized) + Kokoro-82M int8 EN",
        files: &[
            PackFile {
                asset: "asr-models/silero_vad.onnx",
                sha256: "9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6",
            },
            PackFile {
                asset: "asr-models/sherpa-onnx-moonshine-tiny-en-quantized-2026-02-27.tar.bz2",
                sha256: "9ec31b342d8fa3240c3b81b8f82e1cf7e3ac467c93ca5a999b741d5887164f8d",
            },
            PackFile {
                asset: "tts-models/kokoro-int8-en-v0_19.tar.bz2",
                sha256: "c9f0dd393615805b0bab050c340834d5e684e732aec91c0e860cd30e982c08bd",
            },
        ],
    },
    Pack {
        name: "accurate",
        description: "Parakeet TDT 0.6B v3 int8 (NVIDIA NeMo transducer, CC-BY-4.0)",
        files: &[PackFile {
            asset: "asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2",
            sha256: "5793d0fd397c5778d2cf2126994d58e9d56b1be7c04d13c7a15bb1b4eafb16bf",
        }],
    },
];

pub fn pack(name: &str) -> Option<&'static Pack> {
    PACKS.iter().find(|p| p.name == name)
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PackStateKind {
    Missing,
    Downloading,
    Ready,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct PackStatus {
    pub name: String,
    pub state: PackStateKind,
    /// 0..1 across all files of the pack; `None` when not downloading.
    pub pct: Option<f32>,
    pub error: Option<String>,
    pub size_bytes: Option<u64>,
}

impl PackStatus {
    fn missing(name: &str) -> Self {
        Self {
            name: name.to_string(),
            state: PackStateKind::Missing,
            pct: None,
            error: None,
            size_bytes: None,
        }
    }
}

/// Download+extract manager with per-pack locking and live progress.
pub struct PackManager {
    root: PathBuf,
    base: String,
    client: reqwest::Client,
    statuses: Arc<RwLock<BTreeMap<String, PackStatus>>>,
    locks: Arc<Mutex<BTreeMap<String, Arc<Mutex<()>>>>>,
}

impl PackManager {
    pub fn new() -> Self {
        let root = model_root();
        let base = std::env::var("ALLTERNIT_VOICE_MODEL_BASE")
            .unwrap_or_else(|_| DEFAULT_BASE_URL.to_string())
            .trim_end_matches('/')
            .to_string();
        Self {
            root,
            base,
            client: reqwest::Client::builder()
                .user_agent("allternit-voice-service/0.1")
                .build()
                .expect("build reqwest client"),
            statuses: Arc::new(RwLock::new(BTreeMap::new())),
            locks: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub fn pack_dir(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    /// Live status of all known packs (never triggers downloads).
    pub async fn statuses(&self) -> Vec<PackStatus> {
        let mut out = Vec::new();
        let statuses = self.statuses.read().await;
        for p in PACKS {
            out.push(
                statuses
                    .get(p.name)
                    .cloned()
                    .unwrap_or_else(|| PackStatus::missing(p.name)),
            );
        }
        out
    }

    async fn set_status(&self, status: PackStatus) {
        self.statuses
            .write()
            .await
            .insert(status.name.clone(), status);
    }

    /// Blocking variant for use inside `spawn_blocking` / bench threads.
    pub fn ensure_blocking(&self, name: &str) -> Result<PathBuf, String> {
        tokio::runtime::Handle::try_current()
            .map_err(|e| format!("no tokio runtime: {e}"))?
            .block_on(self.ensure(name))
    }

    /// Ensure a pack is fully downloaded, verified, and extracted.
    pub async fn ensure(&self, name: &str) -> Result<PathBuf, String> {
        let p = pack(name).ok_or_else(|| format!("unknown pack: {name}"))?;
        let lock = {
            let mut locks = self.locks.lock().await;
            locks
                .entry(name.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = lock.lock().await;

        let dir = self.pack_dir(name);
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| format!("create pack dir: {e}"))?;

        for file in p.files {
            self.ensure_file(&dir, file).await?;
        }
        self.set_status(PackStatus {
            name: name.to_string(),
            state: PackStateKind::Ready,
            pct: None,
            error: None,
            size_bytes: Some(dir_size(&dir)),
        })
        .await;
        Ok(dir)
    }

    async fn ensure_file(&self, dir: &Path, file: &PackFile) -> Result<(), String> {
        let final_path = dir.join(file_name(file));
        if final_path.is_file() && sha256_file(&final_path).await? == file.sha256 {
            return Ok(()); // already downloaded (single file, e.g. VAD)
        }
        if is_archive(file) {
            // Verify extracted contents instead of the (deleted) archive.
            if extraction_marker(dir, file).is_file() {
                return Ok(());
            }
        }

        let url = format!("{}/{}", self.base, file.asset);
        let part_path = dir.join(format!("{}.part", file_name(file)));
        self.download(&url, &part_path, &final_path, dir, file)
            .await
    }

    async fn download(
        &self,
        url: &str,
        part_path: &PathBuf,
        final_path: &PathBuf,
        dir: &Path,
        file: &PackFile,
    ) -> Result<(), String> {
        // Resume: keep any existing `.part` and continue from its size when
        // the server honours Range; restart from scratch when it does not.
        let existing = tokio::fs::metadata(part_path)
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        let mut request = self.client.get(url);
        if existing > 0 {
            request = request.header("Range", format!("bytes={existing}-"));
        }
        let response = request
            .send()
            .await
            .map_err(|e| format!("GET {url}: {e}"))?;
        let status = response.status();
        let mut file_handle;
        let mut downloaded = existing;
        if status == reqwest::StatusCode::PARTIAL_CONTENT {
            file_handle = tokio::fs::OpenOptions::new()
                .append(true)
                .open(part_path)
                .await
                .map_err(|e| format!("open partial: {e}"))?;
        } else {
            if existing > 0 {
                warn!("server ignored Range; restarting download of {url}");
            }
            downloaded = 0;
            file_handle = tokio::fs::File::create(part_path)
                .await
                .map_err(|e| format!("create partial: {e}"))?;
        }
        if !status.is_success() && status != reqwest::StatusCode::PARTIAL_CONTENT {
            return Err(format!("GET {url}: HTTP {status}"));
        }

        let total = existing + response.content_length().unwrap_or(0);
        self.set_status_downloading(downloaded, total, file).await;

        use futures_util::StreamExt;
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("download body: {e}"))?;
            tokio::io::AsyncWriteExt::write_all(&mut file_handle, &chunk)
                .await
                .map_err(|e| format!("write partial: {e}"))?;
            downloaded += chunk.len() as u64;
            self.set_status_downloading(downloaded, total, file).await;
        }
        drop(file_handle);

        let actual = sha256_file(part_path).await?;
        if actual != file.sha256 {
            let _ = tokio::fs::remove_file(part_path).await;
            self.set_status(PackStatus {
                name: pack_name_of(file).to_string(),
                state: PackStateKind::Error,
                pct: None,
                error: Some(format!(
                    "sha256 mismatch for {}: expected {}, got {}",
                    file.asset, file.sha256, actual
                )),
                size_bytes: None,
            })
            .await;
            return Err(format!("sha256 mismatch for {}", file.asset));
        }

        if is_archive(file) {
            // Extract to a temp dir, then merge into the pack dir atomically
            // enough (verified files, marker only after full success).
            let tmp_extract = dir.join(format!(".extract-{}", file_name(file)));
            let _ = tokio::fs::remove_dir_all(&tmp_extract).await;
            tokio::fs::create_dir_all(&tmp_extract)
                .await
                .map_err(|e| format!("create extract dir: {e}"))?;
            extract_tar_bz2(part_path, &tmp_extract).await?;
            merge_extracted(&tmp_extract, dir)?;
            tokio::fs::remove_dir_all(&tmp_extract)
                .await
                .map_err(|e| format!("clean extract dir: {e}"))?;
            tokio::fs::remove_file(part_path)
                .await
                .map_err(|e| format!("remove archive: {e}"))?;
            tokio::fs::write(extraction_marker(dir, file), b"ok\n")
                .await
                .map_err(|e| format!("write marker: {e}"))?;
            info!("extracted {} into {}", file.asset, dir.display());
        } else {
            tokio::fs::rename(part_path, final_path)
                .await
                .map_err(|e| format!("rename into place: {e}"))?;
        }
        Ok(())
    }

    async fn set_status_downloading(&self, done: u64, total_hint: u64, file: &PackFile) {
        // Fall back to pinned upstream sizes when the server omits length.
        let totals: BTreeMap<&str, u64> = [
            ("asr-models/silero_vad.onnx", 643_854),
            (
                "asr-models/sherpa-onnx-moonshine-tiny-en-quantized-2026-02-27.tar.bz2",
                29_858_559,
            ),
            ("tts-models/kokoro-int8-en-v0_19.tar.bz2", 103_248_205),
            (
                "asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2",
                487_170_055,
            ),
        ]
        .into_iter()
        .collect();
        let total = if total_hint > 0 {
            total_hint
        } else {
            totals.get(file.asset).copied().unwrap_or(0)
        };
        let pct = if total > 0 {
            Some((done as f32 / total as f32).clamp(0.0, 1.0))
        } else {
            None
        };
        self.set_status(PackStatus {
            name: pack_name_of(file).to_string(),
            state: PackStateKind::Downloading,
            pct,
            error: None,
            size_bytes: None,
        })
        .await;
    }
}

fn model_root() -> PathBuf {
    if let Ok(explicit) = std::env::var("ALLTERNIT_VOICE_MODEL_DIR") {
        return PathBuf::from(explicit);
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".allternit").join("models").join("voice")
}

fn file_name(file: &PackFile) -> &str {
    file.asset.rsplit('/').next().unwrap_or(file.asset)
}

fn is_archive(file: &PackFile) -> bool {
    file.asset.ends_with(".tar.bz2")
}

/// Which pack a file belongs to (manifest lookup by asset path).
fn pack_name_of(file: &PackFile) -> &'static str {
    PACKS
        .iter()
        .find(|p| p.files.iter().any(|f| f.asset == file.asset))
        .map(|p| p.name)
        .unwrap_or("unknown")
}

fn extraction_marker(dir: &Path, file: &PackFile) -> PathBuf {
    dir.join(format!(".{}.extracted", file_name(file)))
}

/// sha256 of a file, hex-encoded.
pub async fn sha256_file(path: &Path) -> Result<String, String> {
    use tokio::io::AsyncReadExt;
    let mut f = tokio::fs::File::open(path)
        .await
        .map_err(|e| format!("open {}: {e}", path.display()))?;
    let mut hasher = sha2::Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).await.map_err(|e| format!("read: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

/// sha256 of an in-memory blob (used by tests and the bench harness).
pub fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = sha2::Sha256::new();
    hasher.update(bytes);
    hex(&hasher.finalize())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

async fn extract_tar_bz2(archive: &Path, dest: &Path) -> Result<(), String> {
    use tokio::io::AsyncReadExt;
    let f = tokio::fs::File::open(archive)
        .await
        .map_err(|e| format!("open archive: {e}"))?;
    let mut reader = f;
    let mut raw = Vec::new();
    reader
        .read_to_end(&mut raw)
        .await
        .map_err(|e| format!("read archive: {e}"))?;
    let bz = bzip2::read::BzDecoder::new(&raw[..]);
    let mut archive = tar::Archive::new(bz);
    archive
        .unpack(dest)
        .map_err(|e| format!("unpack {}: {e}", dest.display()))?;
    Ok(())
}

/// Move extracted `<top-dir>/*` into the pack dir (flatten one level).
fn merge_extracted(from: &Path, to: &Path) -> Result<(), String> {
    let entries: Vec<_> = std::fs::read_dir(from)
        .map_err(|e| format!("read extract dir: {e}"))?
        .filter_map(|e| e.ok())
        .collect();
    for entry in entries {
        let path = entry.path();
        let target = to.join(entry.file_name());
        if path.is_dir() {
            if target.exists() {
                // merge recursively (e.g. espeak-ng-data over itself)
                merge_dir(&path, &target)?;
                std::fs::remove_dir_all(&path).map_err(|e| format!("remove tmp dir: {e}"))?;
            } else {
                std::fs::rename(&path, &target).map_err(|e| format!("move dir: {e}"))?;
            }
        } else if !target.exists() {
            std::fs::rename(&path, &target).map_err(|e| format!("move file: {e}"))?;
        }
    }
    Ok(())
}

fn merge_dir(from: &Path, to: &Path) -> Result<(), String> {
    for entry in std::fs::read_dir(from).map_err(|e| format!("read dir: {e}"))? {
        let entry = entry.map_err(|e| format!("read entry: {e}"))?;
        let path = entry.path();
        let target = to.join(entry.file_name());
        if path.is_dir() {
            std::fs::create_dir_all(&target).map_err(|e| format!("mkdir: {e}"))?;
            merge_dir(&path, &target)?;
        } else if !target.exists() {
            std::fs::rename(&path, &target).map_err(|e| format!("move: {e}"))?;
        }
    }
    Ok(())
}

fn dir_size(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .filter_map(|e| e.ok())
        .map(|e| {
            let p = e.path();
            if p.is_dir() {
                dir_size(&p)
            } else {
                e.metadata().map(|m| m.len()).unwrap_or(0)
            }
        })
        .sum()
}

/// Find a file in `dir` (recursive) whose name ends with any of `suffixes`.
pub fn find_file(dir: &Path, suffixes: &[&str]) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file(&path, suffixes) {
                return Some(found);
            }
        } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if suffixes.iter().any(|s| name.ends_with(s)) {
                return Some(path);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_hashes_are_hex_and_files_unique() {
        for p in PACKS {
            let mut seen = std::collections::HashSet::new();
            for f in p.files {
                assert_eq!(f.sha256.len(), 64, "sha256 must be 64 hex chars");
                assert!(f.sha256.chars().all(|c| c.is_ascii_hexdigit()));
                assert!(seen.insert(f.asset), "duplicate asset {}", f.asset);
                assert!(pack_name_of(f) == p.name);
            }
        }
    }

    #[test]
    fn sha256_bytes_matches_known_vector() {
        // SHA-256("") — sanity check for the hasher plumbing.
        assert_eq!(
            sha256_bytes(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_bytes(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[tokio::test]
    async fn sha256_file_matches_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.bin");
        tokio::fs::write(&path, b"abc").await.unwrap();
        assert_eq!(sha256_file(&path).await.unwrap(), sha256_bytes(b"abc"));
    }

    #[test]
    fn find_file_locates_by_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("nested");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("tokens.txt"), b"").unwrap();
        let found = find_file(dir.path(), &["tokens.txt"]).unwrap();
        assert!(found.ends_with("tokens.txt"));
        assert!(find_file(dir.path(), &["missing.onnx"]).is_none());
    }
}
