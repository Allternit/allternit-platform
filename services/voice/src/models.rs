//! Model pack management: manifests with pinned sha256, atomic resumable
//! downloads, and tar.bz2 extraction.
//!
//! Packs live under `~/.allternit/models/voice/<pack>/` and download on
//! first use. The base URL is configurable via `ALLTERNIT_VOICE_MODEL_BASE`
//! (default: our mirror at runtime.allternit.com, falling back to the
//! upstream URLs); hashes are pinned here
//! in code and verified before a file is considered usable.

use serde::Serialize;
use sha2::Digest;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tracing::{info, warn};

/// Upstream sherpa-onnx releases, used as the fallback source.
const DEFAULT_BASE_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download";

/// Our own mirror (R2 bucket `allternit-runtime`), tried first. Files sit
/// flat under it by file name; see `scripts/mirror-voice-packs.sh`.
const MIRROR_BASE_URL: &str = "https://runtime.allternit.com/voice-packs/v1";

/// One downloadable file of a pack. `asset` is the release-asset path
/// (`<tag>/<file name>`) appended to the base URL. Archives (`.tar.bz2`)
/// are extracted into the pack directory after verification.
pub struct PackFile {
    pub asset: &'static str,
    pub sha256: &'static str,
    /// Pinned size in bytes (progress reporting; the sha256 is the check).
    pub size: u64,
    /// Full upstream URL, for files not hosted on the sherpa-onnx releases.
    /// Used only while `ALLTERNIT_VOICE_MODEL_BASE` is unset; a custom base
    /// (a mirror) serves every file at `<base>/<asset>`.
    pub upstream: Option<&'static str>,
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
        description: "Silero VAD + Moonshine tiny EN (quantized) + Smart Turn v3.2",
        files: &[
            PackFile {
                asset: "asr-models/silero_vad.onnx",
                sha256: "9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6",
                size: 643_854,
                upstream: None,
            },
            PackFile {
                asset: "asr-models/sherpa-onnx-moonshine-tiny-en-quantized-2026-02-27.tar.bz2",
                sha256: "9ec31b342d8fa3240c3b81b8f82e1cf7e3ac467c93ca5a999b741d5887164f8d",
                size: 29_858_559,
                upstream: None,
            },
            PackFile {
                // Smart Turn v3.2 end-of-turn model (BSD-2-Clause), used by
                // the voice session layer, not by the HTTP STT/TTS routes.
                asset: "smart-turn/smart-turn-v3.2-cpu.onnx",
                sha256: "2bb026316b14a660486a75b1733cd3fbab8c2fd0314dc9af7be49f8cca967e4f",
                size: 8_679_182,
                upstream: Some(
                    "https://huggingface.co/pipecat-ai/smart-turn-v3/resolve/f766f81d3cfdf7737ac64aad813d91bbfd56bf93/smart-turn-v3.2-cpu.onnx",
                ),
            },
        ],
    },
    Pack {
        name: "tts",
        description: "Kokoro-82M v1.0 fp32 (Apache-2.0), run by the allternit-tts program",
        files: &[PackFile {
            asset: "tts-models/kokoro-multi-lang-v1_0.tar.bz2",
            sha256: "c5f7e2d2caf082bc1d20fb70334a61d99d20b484500aad32e7cf84c128ea3298",
            size: 349_906_910,
            upstream: None,
        }],
    },
    Pack {
        name: "voices",
        description: "Pocket TTS (Kyutai, CC-BY-4.0) ONNX int8 for custom voices with consent on file",
        files: &[
            PackFile {
                asset: "pocket-tts/bundle.json",
                sha256: "bab643150f437f37df080a710520ff39ed9ebd9a339f8ebdc739f7eddfc28b3f",
                size: 24_381,
                upstream: Some("https://huggingface.co/KevinAHM/pocket-tts-onnx/resolve/58a6d00cf13d239b6748cb0769f35c580a8f606c/onnx/english_2026-04/bundle.json"),
            },
            PackFile {
                asset: "pocket-tts/tokenizer.model",
                sha256: "d461765ae179566678c93091c5fa6f2984c31bbe990bf1aa62d92c64d91bc3f6",
                size: 59_339,
                upstream: Some("https://huggingface.co/KevinAHM/pocket-tts-onnx/resolve/58a6d00cf13d239b6748cb0769f35c580a8f606c/onnx/english_2026-04/tokenizer.model"),
            },
            PackFile {
                asset: "pocket-tts/bos_before_voice.npy",
                sha256: "f46edf4f7007b7ba4ea58831f49d003e59e167b4641c44bb3addfe9231a780b1",
                size: 4_224,
                upstream: Some("https://huggingface.co/KevinAHM/pocket-tts-onnx/resolve/58a6d00cf13d239b6748cb0769f35c580a8f606c/onnx/english_2026-04/bos_before_voice.npy"),
            },
            PackFile {
                asset: "pocket-tts/mimi_encoder.onnx",
                sha256: "853e2ca623b8782d94c3745ec6133bfdff7ce33d9b11128bd29ea03f28d76e3d",
                size: 39_768_446,
                upstream: Some("https://huggingface.co/KevinAHM/pocket-tts-onnx/resolve/58a6d00cf13d239b6748cb0769f35c580a8f606c/onnx/english_2026-04/mimi_encoder.onnx"),
            },
            PackFile {
                asset: "pocket-tts/text_conditioner.onnx",
                sha256: "4ecee995fb69f85c7a7493d11f7b5ee15d9950facc7ab3f5c9c49ef1e03847bb",
                size: 16_388_344,
                upstream: Some("https://huggingface.co/KevinAHM/pocket-tts-onnx/resolve/58a6d00cf13d239b6748cb0769f35c580a8f606c/onnx/english_2026-04/text_conditioner.onnx"),
            },
            PackFile {
                asset: "pocket-tts/flow_lm_main_int8.onnx",
                sha256: "f9bd8106b79a0192c1c43399ab938fb24900a95c1c599870d75a884e99000116",
                size: 76_341_079,
                upstream: Some("https://huggingface.co/KevinAHM/pocket-tts-onnx/resolve/58a6d00cf13d239b6748cb0769f35c580a8f606c/onnx/english_2026-04/flow_lm_main_int8.onnx"),
            },
            PackFile {
                asset: "pocket-tts/flow_lm_flow_int8.onnx",
                sha256: "3dd781ee5abee9e195320bf0106bebd6372a852b3b36352524ee78b40554635d",
                size: 9_962_530,
                upstream: Some("https://huggingface.co/KevinAHM/pocket-tts-onnx/resolve/58a6d00cf13d239b6748cb0769f35c580a8f606c/onnx/english_2026-04/flow_lm_flow_int8.onnx"),
            },
            PackFile {
                asset: "pocket-tts/mimi_decoder_int8.onnx",
                sha256: "3630450a3297a101792a6ac66619ebc70ab916b265e6220c2afaef8b1673f925",
                size: 22_684_077,
                upstream: Some("https://huggingface.co/KevinAHM/pocket-tts-onnx/resolve/58a6d00cf13d239b6748cb0769f35c580a8f606c/onnx/english_2026-04/mimi_decoder_int8.onnx"),
            },
        ],
    },
    Pack {
        name: "accurate",
        description: "Parakeet TDT 0.6B v3 int8 (NVIDIA NeMo transducer, CC-BY-4.0)",
        files: &[PackFile {
            asset: "asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2",
            sha256: "5793d0fd397c5778d2cf2126994d58e9d56b1be7c04d13c7a15bb1b4eafb16bf",
            size: 487_170_055,
            upstream: None,
        }],
    },
];

/// Top-level directories the archives extract to (inside the pack dir).
/// Engines look for model files only inside their own component dir, so
/// e.g. Moonshine's and Kokoro's `tokens.txt` never get mixed up.
pub const VAD_FILE: &str = "silero_vad.onnx";
/// Smart Turn v3.2 end-of-turn model (plain file in the small pack dir).
pub const SMART_TURN_FILE: &str = "smart-turn-v3.2-cpu.onnx";
pub const MOONSHINE_DIR: &str = "sherpa-onnx-moonshine-tiny-en-quantized-2026-02-27";
pub const KOKORO_DIR: &str = "kokoro-multi-lang-v1_0";
pub const PARAKEET_DIR: &str = "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8";

/// Inference threads per model (`ALLTERNIT_VOICE_THREADS`, default 2: the
/// target machine is a 4-core laptop that must stay usable while talking).
pub fn inference_threads() -> i32 {
    std::env::var("ALLTERNIT_VOICE_THREADS")
        .ok()
        .and_then(|s| s.trim().parse::<i32>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(2)
}

static INFERENCE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Held around every recogniser decode and TTS generation so the whole
/// service runs one model at a time, i.e. at most `inference_threads()`
/// inference threads. Calls are short (one VAD segment, one sentence), so
/// STT and TTS interleave instead of piling up cores.
pub fn inference_lock() -> std::sync::MutexGuard<'static, ()> {
    INFERENCE.lock().unwrap_or_else(|e| e.into_inner())
}

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

impl Default for PackManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Download+extract manager with per-pack locking and live progress.
/// Cloning is cheap and clones share status and locks.
#[derive(Clone)]
pub struct PackManager {
    root: PathBuf,
    base: String,
    custom_base: bool,
    client: reqwest::Client,
    statuses: Arc<RwLock<BTreeMap<String, PackStatus>>>,
    locks: Arc<Mutex<BTreeMap<String, Arc<Mutex<()>>>>>,
}

impl PackManager {
    pub fn new() -> Self {
        Self::with_root(model_root())
    }

    /// Manager rooted at an explicit directory (tests, bench).
    pub fn with_root(root: PathBuf) -> Self {
        let custom = std::env::var("ALLTERNIT_VOICE_MODEL_BASE")
            .ok()
            .map(|b| b.trim().trim_end_matches('/').to_string())
            .filter(|b| !b.is_empty());
        Self {
            root,
            custom_base: custom.is_some(),
            base: custom.unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            client: reqwest::Client::builder()
                .user_agent("allternit-voice-service/0.1")
                .build()
                .expect("build reqwest client"),
            statuses: Arc::new(RwLock::new(BTreeMap::new())),
            locks: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub fn root_dir(&self) -> PathBuf {
        self.root.clone()
    }

    pub fn pack_dir(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    /// Live status of all known packs (never triggers downloads).
    pub async fn statuses(&self) -> Vec<PackStatus> {
        let mut out = Vec::new();
        let statuses = self.statuses.read().await;
        for p in PACKS {
            out.push(statuses.get(p.name).cloned().unwrap_or_else(|| {
                if self.is_installed(p.name) {
                    PackStatus {
                        name: p.name.to_string(),
                        state: PackStateKind::Ready,
                        pct: None,
                        error: None,
                        size_bytes: Some(dir_size(&self.pack_dir(p.name))),
                    }
                } else {
                    PackStatus::missing(p.name)
                }
            }));
        }
        out
    }

    /// Cheap on-disk check (no hashing): every plain file is present and
    /// every archive has its extraction marker. Hashes were verified before
    /// the files were moved into place.
    pub fn is_installed(&self, name: &str) -> bool {
        let Some(p) = pack(name) else {
            return false;
        };
        let dir = self.pack_dir(name);
        p.files.iter().all(|f| {
            if is_archive(f) {
                extraction_marker(&dir, f).is_file()
            } else {
                dir.join(file_name(f)).is_file()
            }
        })
    }

    async fn set_status(&self, status: PackStatus) {
        self.statuses
            .write()
            .await
            .insert(status.name.clone(), status);
    }

    /// Blocking variant for `spawn_blocking` / plain threads. Skips the
    /// async path entirely when the pack is already installed; otherwise it
    /// needs a tokio runtime context (any `spawn_blocking` thread has one).
    pub fn ensure_blocking(&self, name: &str) -> Result<PathBuf, String> {
        if self.is_installed(name) {
            return Ok(self.pack_dir(name));
        }
        tokio::runtime::Handle::try_current()
            .map_err(|e| format!("voice pack '{name}' not installed and no tokio runtime: {e}"))?
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
            if let Err(e) = self.ensure_file(&dir, file).await {
                warn!("voice pack {name}: {e}");
                self.set_status(PackStatus {
                    name: name.to_string(),
                    state: PackStateKind::Error,
                    pct: None,
                    error: Some(e.clone()),
                    size_bytes: None,
                })
                .await;
                return Err(e);
            }
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

        let urls = candidate_urls(file, self.custom_base.then_some(self.base.as_str()));
        let part_path = dir.join(format!("{}.part", file_name(file)));
        let mut last_err = String::new();
        for (i, url) in urls.iter().enumerate() {
            match self.download(url, &part_path, &final_path, dir, file).await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    // A hash-mismatching mirror copy was already deleted.
                    if i + 1 < urls.len() && !MIRROR_FALLBACK_LOGGED.swap(true, Ordering::Relaxed) {
                        warn!("voice pack mirror failed ({e}); falling back to upstream");
                    }
                    last_err = e;
                }
            }
        }
        Err(last_err)
    }

    async fn download(
        &self,
        url: &str,
        part_path: &Path,
        final_path: &Path,
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
        if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE && existing > 0 {
            // The `.part` already holds the whole file (crash after the last
            // byte, before verification). Verify it as-is below.
            drop(response);
            return self.finish_file(part_path, final_path, dir, file).await;
        }
        if !status.is_success() {
            return Err(format!("GET {url}: HTTP {status}"));
        }
        let mut file_handle;
        let mut downloaded = existing;
        let mut total = response.content_length().unwrap_or(0);
        if status == reqwest::StatusCode::PARTIAL_CONTENT {
            total += existing;
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
        tokio::io::AsyncWriteExt::flush(&mut file_handle)
            .await
            .map_err(|e| format!("flush partial: {e}"))?;
        drop(file_handle);
        self.finish_file(part_path, final_path, dir, file).await
    }

    /// Verify a fully-downloaded `.part`, then extract it (archives) or
    /// rename it into place (plain files).
    async fn finish_file(
        &self,
        part_path: &Path,
        final_path: &Path,
        dir: &Path,
        file: &PackFile,
    ) -> Result<(), String> {
        let actual = sha256_file(part_path).await?;
        if actual != file.sha256 {
            let _ = tokio::fs::remove_file(part_path).await;
            return Err(format!(
                "sha256 mismatch for {}: expected {}, got {actual}",
                file.asset, file.sha256
            ));
        }

        if is_archive(file) {
            // Extract to a temp dir, then merge into the pack dir atomically
            // enough (verified files, marker only after full success).
            let tmp_extract = dir.join(format!(".extract-{}", file_name(file)));
            let _ = tokio::fs::remove_dir_all(&tmp_extract).await;
            tokio::fs::create_dir_all(&tmp_extract)
                .await
                .map_err(|e| format!("create extract dir: {e}"))?;
            let (archive, dest, to) = (
                part_path.to_path_buf(),
                tmp_extract.clone(),
                dir.to_path_buf(),
            );
            tokio::task::spawn_blocking(move || {
                extract_tar_bz2(&archive, &dest)?;
                merge_extracted(&dest, &to)
            })
            .await
            .map_err(|e| format!("extract task: {e}"))??;
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

    /// Progress across the whole pack: finished files + bytes of this one,
    /// over the pinned pack size.
    async fn set_status_downloading(&self, done: u64, _total_hint: u64, file: &PackFile) {
        let name = pack_name_of(file);
        let pct = pack(name).and_then(|p| {
            let total: u64 = p.files.iter().map(|f| f.size).sum();
            let before: u64 = p
                .files
                .iter()
                .take_while(|f| f.asset != file.asset)
                .map(|f| f.size)
                .sum();
            (total > 0).then(|| ((before + done) as f32 / total as f32).clamp(0.0, 1.0))
        });
        self.set_status(PackStatus {
            name: name.to_string(),
            state: PackStateKind::Downloading,
            pct,
            error: None,
            size_bytes: None,
        })
        .await;
    }
}

static MIRROR_FALLBACK_LOGGED: AtomicBool = AtomicBool::new(false);

/// Source URLs for a file, in the order to try them. A custom base
/// (`ALLTERNIT_VOICE_MODEL_BASE`) is used alone at `<base>/<asset>`;
/// otherwise our mirror (flat file name) first, then the upstream URL.
fn candidate_urls(file: &PackFile, custom_base: Option<&str>) -> Vec<String> {
    if let Some(base) = custom_base {
        return vec![format!("{base}/{}", file.asset)];
    }
    let upstream = match file.upstream {
        Some(full) => full.to_string(),
        None => format!("{DEFAULT_BASE_URL}/{}", file.asset),
    };
    vec![format!("{MIRROR_BASE_URL}/{}", file_name(file)), upstream]
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

/// Stream-decompress a `.tar.bz2` (never buffers the whole archive: the
/// accurate pack is ~490 MB and the RAM budget is ~1 GB). Blocking.
fn extract_tar_bz2(archive: &Path, dest: &Path) -> Result<(), String> {
    let f = std::fs::File::open(archive).map_err(|e| format!("open archive: {e}"))?;
    let bz = bzip2::read::BzDecoder::new(std::io::BufReader::new(f));
    tar::Archive::new(bz)
        .unpack(dest)
        .map_err(|e| format!("unpack {}: {e}", dest.display()))
}

/// Move the extracted top-level entries (e.g. `kokoro-int8-en-v0_19/`) into
/// the pack dir, merging into any directory a previous attempt left behind.
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
    fn urls_default_to_mirror_then_upstream() {
        let small = pack("small").unwrap();
        let vad = candidate_urls(&small.files[0], None);
        assert_eq!(
            vad,
            vec![
                "https://runtime.allternit.com/voice-packs/v1/silero_vad.onnx".to_string(),
                format!("{DEFAULT_BASE_URL}/asr-models/silero_vad.onnx"),
            ]
        );
        let turn = candidate_urls(&small.files[2], None);
        assert!(turn[0].ends_with("/voice-packs/v1/smart-turn-v3.2-cpu.onnx"));
        assert!(turn[1].starts_with("https://huggingface.co/"));
    }

    #[test]
    fn custom_base_is_used_alone() {
        let small = pack("small").unwrap();
        let urls = candidate_urls(&small.files[0], Some("http://m.test/x"));
        assert_eq!(
            urls,
            vec!["http://m.test/x/asr-models/silero_vad.onnx".to_string()]
        );
    }

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

    fn pack_bytes(name: &str) -> u64 {
        pack(name).unwrap().files.iter().map(|f| f.size).sum()
    }

    #[test]
    fn pack_downloads_fit_their_budgets() {
        // Dictation (small) stays light; TTS is Kokoro fp32 (Eoj 2026-10-03:
        // full-size Kokoro, budget = its actual size, ~350 MB).
        assert!(
            pack_bytes("small") <= 40_000_000,
            "small: {}",
            pack_bytes("small")
        );
        assert!(
            pack_bytes("tts") <= 350_000_000,
            "tts: {}",
            pack_bytes("tts")
        );
        assert!(
            pack_bytes("accurate") <= 490_000_000,
            "accurate: {}",
            pack_bytes("accurate")
        );
    }

    #[tokio::test]
    async fn progress_spans_the_whole_pack() {
        let m = PackManager::with_root(tempfile::tempdir().unwrap().keep());
        let small = pack("small").unwrap();
        // Halfway through Moonshine (2nd file): VAD done + half of it.
        let f = &small.files[1];
        m.set_status_downloading(f.size / 2, 0, f).await;
        let st = m.statuses().await;
        let pct = st.iter().find(|p| p.name == "small").unwrap().pct.unwrap();
        let total = pack_bytes("small") as f32;
        let want = (small.files[0].size + f.size / 2) as f32 / total;
        assert!((pct - want).abs() < 1e-4, "{pct} vs {want}");
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

    const ABC: PackFile = PackFile {
        asset: "test/abc.bin",
        sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        size: 3,
        upstream: None,
    };
    const ABC_WRONG: PackFile = PackFile {
        asset: "test/abc.bin",
        sha256: "0000000000000000000000000000000000000000000000000000000000000000",
        size: 3,
        upstream: None,
    };

    #[tokio::test]
    async fn verified_part_is_renamed_into_place() {
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("abc.bin.part");
        let fin = dir.path().join("abc.bin");
        tokio::fs::write(&part, b"abc").await.unwrap();
        let m = PackManager::new();
        m.finish_file(&part, &fin, dir.path(), &ABC).await.unwrap();
        assert!(fin.is_file());
        assert!(!part.exists());
    }

    #[tokio::test]
    async fn sha_mismatch_rejects_and_deletes_part() {
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("abc.bin.part");
        let fin = dir.path().join("abc.bin");
        tokio::fs::write(&part, b"abc").await.unwrap();
        let m = PackManager::new();
        let err = m
            .finish_file(&part, &fin, dir.path(), &ABC_WRONG)
            .await
            .unwrap_err();
        assert!(err.contains("sha256 mismatch"), "{err}");
        assert!(
            !fin.exists(),
            "unverified file must never be moved into place"
        );
        assert!(!part.exists(), "corrupt partial must be discarded");
    }

    #[test]
    fn install_check_uses_markers() {
        assert!(pack("small").is_some() && pack("accurate").is_some());
        let tmp = tempfile::tempdir().unwrap();
        let m = PackManager::with_root(tmp.path().to_path_buf());
        assert!(!m.is_installed("accurate"));
        let dir = m.pack_dir("accurate");
        std::fs::create_dir_all(&dir).unwrap();
        let marker = extraction_marker(&dir, &pack("accurate").unwrap().files[0]);
        std::fs::write(marker, b"ok").unwrap();
        assert!(m.is_installed("accurate"));
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
