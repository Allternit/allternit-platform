//! allternit-model-source: where Allternit apps get model bytes, and how sure
//! they are that the bytes are right.
//!
//! Contract: `contracts/model-source/README.md`. In short:
//! 1. [`Client::resolve`] builds an ordered source list. When the Freeweights
//!    manifest is reachable and its minisign signature verifies, the list is the
//!    Freeweights worker URL, then the manifest `sources[]` by priority, and
//!    every file is checked against the signed sha256 (`freeweights_signed`).
//!    When the catalog is unreachable (or does not list the repo), the list is
//!    the publisher (Hugging Face; models, else datasets) and files are checked
//!    against the HF tree API: `lfs.oid` sha256 for LFS files, the git blob id
//!    (`oid`) for the rest (`publisher_hash`). A manifest that is reachable but
//!    fails verification is an error, never a silent fallback.
//! 2. [`Client::fetch_verified`] downloads one file, resumable, hashing as it
//!    streams. A hash or size mismatch deletes the file and moves on to the
//!    next source. Bytes with no hash at all are an error unless the caller
//!    sets [`FetchOptions::allow_unverified`].

pub mod manifest;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub use manifest::{
    canonical_json, parse_manifest, verify_manifest, verify_manifest_with_key, License, Manifest,
    ManifestFile, ManifestSource, Torrent, Upstream, FREEWEIGHTS_KEY_ID, FREEWEIGHTS_PUBLIC_KEY,
};

pub const DEFAULT_BASE_URL: &str = "https://freeweights.allternit.com";
pub const DEFAULT_PUBLISHER_URL: &str = "https://huggingface.co";
/// Env var that overrides [`DEFAULT_BASE_URL`].
pub const BASE_URL_ENV: &str = "ALLTERNIT_FREEWEIGHTS_URL";
/// Timeout for catalog, manifest and tree requests, and for connecting / each
/// read during a download.
pub const NETWORK_TIMEOUT: Duration = Duration::from_secs(5);

/// How a downloaded file was checked. Never blank.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationState {
    /// sha256 + size from a Freeweights manifest whose minisign signature verified.
    FreeweightsSigned,
    /// sha256 from the publisher (Hugging Face tree API `lfs.oid`); catalog not used.
    PublisherHash,
    /// sha256 the caller supplied (e.g. a model registry pin).
    Pinned,
    /// No hash was available and the caller explicitly allowed it.
    Unverified,
}

/// Which kind of source the bytes came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// `https://freeweights.allternit.com/{org}/{repo}/resolve/{commit}/{path}` (302s to the best source).
    FreeweightsWorker,
    /// Manifest `sources[]` entry of type `hosted` (Freeweights origin or a registered mirror).
    ManifestHosted,
    /// Manifest `sources[]` entry of type `source` (upstream at the pinned commit).
    ManifestSource,
    /// Publisher fallback `https://huggingface.co/{repo}/resolve/{rev}/{path}` (catalog unreachable).
    Publisher,
}

/// The `verification` object local-engine / allternit-api responses carry.
/// Shape: `contracts/model-source/model-source.schema.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verification {
    pub state: VerificationState,
    pub source_used: SourceKind,
    /// Base URL of the source used (before redirects; no signed query strings).
    pub source_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_commit: Option<String>,
    /// UTC, `YYYY-MM-DDTHH:MM:SSZ`.
    pub checked_at: String,
}

/// One place to fetch from. The file URL is `base_url` + the percent-encoded path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub kind: SourceKind,
    pub base_url: String,
}

impl Source {
    pub fn file_url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, encode_path(path))
    }
}

/// What a file must hash to, when known.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpectedFile {
    pub sha256: Option<String>,
    pub size: Option<u64>,
    /// Publisher git blob id (SHA-1 of `"blob {size}\0" + content`), for
    /// non-LFS files at the publisher. Checked only when `sha256` is absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_blob_sha1: Option<String>,
}

/// Result of [`Client::resolve`]: ordered sources plus the hashes to check against.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Resolution {
    pub repo: String,
    /// The manifest commit, or the revision asked of the publisher.
    pub revision: String,
    /// `FreeweightsSigned` with a verified manifest, otherwise `PublisherHash`.
    pub basis: VerificationState,
    pub manifest: Option<Manifest>,
    /// sha256 (hex) of the signed manifest bytes as served, when `manifest` is
    /// set. Job specs pin a base model to it (`base.manifest_sha256`).
    #[serde(default)]
    pub manifest_sha256: Option<String>,
    /// Ordered: try the first, fall through on any failure or mismatch.
    pub sources: Vec<Source>,
    pub files: BTreeMap<String, ExpectedFile>,
    /// Why the catalog was not used (unreachable, not listed, other revision).
    pub fallback_reason: Option<String>,
}

impl Resolution {
    pub fn manifest_commit(&self) -> Option<&str> {
        self.manifest.as_ref().map(|m| m.upstream.commit.as_str())
    }
}

#[derive(Debug, Clone, Default)]
pub struct FetchOptions {
    /// Caller pin. Used when the resolution has no hash for the file
    /// (state `pinned`); must agree with it when it has one.
    pub expected_sha256: Option<String>,
    /// Allow a download with no hash at all (state `unverified`). Off by default.
    pub allow_unverified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailedAttempt {
    pub source: SourceKind,
    pub url: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FetchOutcome {
    pub path: PathBuf,
    pub size: u64,
    pub sha256: String,
    pub verification: Verification,
    /// Sources tried before the one that worked.
    pub failed: Vec<FailedAttempt>,
}

/// Minimal catalog row (`{base}/api/catalog` → `{"models": [...]}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogEntry {
    pub id: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub total_size: Option<u64>,
    #[serde(default)]
    pub manifest_url: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("manifest signature: {0}")]
    Signature(String),
    #[error("manifest signed by key {found}, expected {expected}")]
    KeyId { expected: String, found: String },
    #[error("manifest is not canonical: {0}")]
    NotCanonical(String),
    #[error("manifest: {0}")]
    Manifest(String),
    #[error("manifest id {found} does not match requested repo {requested}")]
    RepoMismatch { requested: String, found: String },
    #[error("invalid repo {0:?} (want org/name)")]
    InvalidRepo(String),
    #[error("invalid file path {0:?}")]
    InvalidPath(String),
    #[error("{0} is not listed in the signed manifest")]
    FileNotListed(String),
    #[error("no hash available for {0}; pass expected_sha256 or allow_unverified")]
    NoHash(String),
    #[error("pinned sha256 {pinned} disagrees with {basis:?} sha256 {known} for {path}")]
    PinConflict {
        path: String,
        pinned: String,
        known: String,
        basis: VerificationState,
    },
    #[error("every source failed for {path}: {attempts:?}")]
    AllSourcesFailed {
        path: String,
        attempts: Vec<FailedAttempt>,
    },
    #[error("http: {0}")]
    Http(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Tries for small GETs (catalog, `.minisig`) before giving up.
const SMALL_GET_TRIES: u32 = 3;

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Freeweights + publisher client. Cheap to clone.
#[derive(Clone)]
pub struct Client {
    base: String,
    publisher: String,
    public_key: String,
    http: reqwest::Client,
}

impl Client {
    /// Base URL from [`BASE_URL_ENV`] or [`DEFAULT_BASE_URL`]; publisher = Hugging Face.
    pub fn new() -> Result<Self> {
        let base = std::env::var(BASE_URL_ENV)
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        Self::with_urls(base, DEFAULT_PUBLISHER_URL)
    }

    /// Explicit Freeweights base and publisher base (tests, self-hosted mirrors).
    pub fn with_urls(base: impl Into<String>, publisher: impl Into<String>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(NETWORK_TIMEOUT)
            .read_timeout(NETWORK_TIMEOUT)
            .user_agent(concat!(
                "allternit-model-source/",
                env!("CARGO_PKG_VERSION")
            ))
            .build()
            .map_err(|e| Error::Http(e.to_string()))?;
        Ok(Self {
            base: base.into().trim_end_matches('/').to_string(),
            publisher: publisher.into().trim_end_matches('/').to_string(),
            public_key: FREEWEIGHTS_PUBLIC_KEY.to_string(),
            http,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    /// `{base}/api/catalog`.
    pub async fn catalog(&self) -> Result<Vec<CatalogEntry>> {
        #[derive(Deserialize)]
        struct Catalog {
            models: Vec<CatalogEntry>,
        }
        let bytes = self
            .get_small(&format!("{}/api/catalog", self.base))
            .await?;
        let c: Catalog = serde_json::from_slice(&bytes)
            .map_err(|e| Error::Http(format!("catalog JSON: {e}")))?;
        Ok(c.models)
    }

    /// Fetch and verify `{base}/manifests/{org}/{repo}.json` + `.json.minisig`.
    /// `Ok(Err(reason))` = catalog unreachable or repo not listed (callers may
    /// fall back). `Err` = the manifest exists but failed verification.
    pub async fn fetch_manifest(
        &self,
        repo: &str,
    ) -> Result<std::result::Result<Manifest, String>> {
        Ok(self.fetch_manifest_sha(repo).await?.map(|(m, _)| m))
    }

    /// [`Client::fetch_manifest`] plus the sha256 (hex) of the signed
    /// manifest bytes, the value a job spec's `base.manifest_sha256` pins.
    pub async fn fetch_manifest_sha(
        &self,
        repo: &str,
    ) -> Result<std::result::Result<(Manifest, String), String>> {
        check_repo(repo)?;
        let url = format!("{}/manifests/{}.json", self.base, encode_path(repo));
        let resp = match self.http.get(&url).timeout(NETWORK_TIMEOUT).send().await {
            Ok(r) => r,
            Err(e) => return Ok(Err(format!("catalog unreachable: {e}"))),
        };
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(Err(format!("{repo} is not in the Freeweights catalog")));
        }
        if !status.is_success() {
            return Ok(Err(format!("catalog unreachable: HTTP {status}")));
        }
        let bytes = match resp.bytes().await {
            Ok(b) => b,
            Err(e) => return Ok(Err(format!("catalog unreachable: {e}"))),
        };
        // From here on the catalog answered: a missing or bad signature is an error.
        let sig = self
            .get_small(&format!("{url}.minisig"))
            .await
            .map_err(|e| {
                Error::Signature(format!(
                    "manifest found but its .minisig could not be fetched: {e}"
                ))
            })?;
        let sig =
            String::from_utf8(sig).map_err(|_| Error::Signature(".minisig is not UTF-8".into()))?;
        let m = verify_manifest_with_key(&bytes, &sig, &self.public_key)?;
        if m.id != repo {
            return Err(Error::RepoMismatch {
                requested: repo.into(),
                found: m.id,
            });
        }
        Ok(Ok((m, hex::encode(Sha256::digest(&bytes)))))
    }

    /// Ordered sources for `repo` at `rev` (`None` = the catalog's pinned commit,
    /// or `main` at the publisher). See the module docs for the order.
    pub async fn resolve(&self, repo: &str, rev: Option<&str>) -> Result<Resolution> {
        let reason = match self.fetch_manifest_sha(repo).await? {
            Ok((m, sha)) => {
                let commit = m.upstream.commit.clone();
                if rev.is_none_or(|r| r == "main" || r == commit) {
                    return Ok(self.signed_resolution(m, sha));
                }
                format!(
                    "revision {} is not the catalog's pinned commit {commit}",
                    rev.unwrap_or_default()
                )
            }
            Err(reason) => reason,
        };
        let rev = rev.unwrap_or("main");
        // Models first; a repo the models API does not know may be a dataset.
        let (prefix, files) = match self.publisher_tree("models", repo, rev).await {
            Some(files) => ("", files),
            None => match self.publisher_tree("datasets", repo, rev).await {
                Some(files) => ("datasets/", files),
                None => ("", BTreeMap::new()),
            },
        };
        Ok(Resolution {
            repo: repo.into(),
            revision: rev.into(),
            basis: VerificationState::PublisherHash,
            manifest: None,
            manifest_sha256: None,
            sources: vec![Source {
                kind: SourceKind::Publisher,
                base_url: format!(
                    "{}/{prefix}{}/resolve/{}/",
                    self.publisher,
                    encode_path(repo),
                    urlencoding::encode(rev)
                ),
            }],
            files,
            fallback_reason: Some(reason),
        })
    }

    fn signed_resolution(&self, m: Manifest, manifest_sha256: String) -> Resolution {
        let commit = m.upstream.commit.clone();
        let mut sources = vec![Source {
            kind: SourceKind::FreeweightsWorker,
            base_url: format!("{}/{}/resolve/{}/", self.base, encode_path(&m.id), commit),
        }];
        let mut listed: Vec<&ManifestSource> =
            m.sources.iter().filter(|s| s.url.is_some()).collect();
        listed.sort_by_key(|s| (s.priority, s.url.clone()));
        for s in listed {
            let kind = match s.kind.as_str() {
                "hosted" => SourceKind::ManifestHosted,
                "source" => SourceKind::ManifestSource,
                _ => continue, // `seeded` = magnet only; not an HTTP source
            };
            let mut base = s.url.clone().unwrap_or_default();
            if !base.ends_with('/') {
                base.push('/');
            }
            sources.push(Source {
                kind,
                base_url: base,
            });
        }
        let files = m
            .files
            .iter()
            .map(|f| {
                (
                    f.path.clone(),
                    ExpectedFile {
                        sha256: Some(f.sha256.clone()),
                        size: Some(f.size),
                        git_blob_sha1: None,
                    },
                )
            })
            .collect();
        Resolution {
            repo: m.id.clone(),
            revision: commit,
            basis: VerificationState::FreeweightsSigned,
            manifest: Some(m),
            manifest_sha256: Some(manifest_sha256),
            sources,
            files,
            fallback_reason: None,
        }
    }

    /// Hugging Face tree API (`kind` = `models` or `datasets`): sha256 for LFS
    /// files (`lfs.oid`), git blob SHA-1 (`oid`) for the rest, size for all.
    /// `None` = tree unavailable (fetches then need a pin or allow_unverified).
    async fn publisher_tree(
        &self,
        kind: &str,
        repo: &str,
        rev: &str,
    ) -> Option<BTreeMap<String, ExpectedFile>> {
        #[derive(Deserialize)]
        struct Lfs {
            oid: String,
            size: u64,
        }
        #[derive(Deserialize)]
        struct Entry {
            #[serde(rename = "type")]
            kind: String,
            path: String,
            #[serde(default)]
            size: Option<u64>,
            #[serde(default)]
            oid: Option<String>,
            #[serde(default)]
            lfs: Option<Lfs>,
        }
        let url = format!(
            "{}/api/{kind}/{}/tree/{}?recursive=1",
            self.publisher,
            encode_path(repo),
            urlencoding::encode(rev)
        );
        let bytes = self.get_small(&url).await.ok()?;
        let entries = serde_json::from_slice::<Vec<Entry>>(&bytes).ok()?;
        let files = entries
            .into_iter()
            .filter(|e| e.kind == "file")
            .map(|e| {
                let exp = match e.lfs {
                    Some(l) if manifest::is_hex(&l.oid, 64) => ExpectedFile {
                        sha256: Some(l.oid),
                        size: Some(l.size),
                        git_blob_sha1: None,
                    },
                    // LFS entry with an unusable oid: the git oid is the pointer's, not the data's.
                    Some(_) => ExpectedFile::default(),
                    None => ExpectedFile {
                        sha256: None,
                        size: e.size,
                        git_blob_sha1: e.oid.filter(|o| manifest::is_hex(o, 40)),
                    },
                };
                (e.path, exp)
            })
            .collect();
        Some(files)
    }

    /// Download `path` to `dest`, trying `res.sources` in order. Resumes from
    /// `dest` + `.part` with a Range request; hashes while streaming; on a size
    /// or sha256 mismatch deletes the partial file and tries the next source.
    pub async fn fetch_verified(
        &self,
        res: &Resolution,
        path: &str,
        dest: &Path,
        opts: &FetchOptions,
    ) -> Result<FetchOutcome> {
        check_path(path)?;
        let known = res.files.get(path).cloned();
        if res.basis == VerificationState::FreeweightsSigned && known.is_none() {
            return Err(Error::FileNotListed(path.into()));
        }
        let known = known.unwrap_or_default();
        let pin = opts
            .expected_sha256
            .as_ref()
            .map(|s| s.trim().to_ascii_lowercase());
        let (sha, state) = match (known.sha256.clone(), pin) {
            (Some(k), Some(p)) if k != p => {
                return Err(Error::PinConflict {
                    path: path.into(),
                    pinned: p,
                    known: k,
                    basis: res.basis,
                })
            }
            (Some(k), _) => (Some(k), res.basis),
            (None, Some(p)) if manifest::is_hex(&p, 64) => (Some(p), VerificationState::Pinned),
            (None, Some(p)) => {
                return Err(Error::InvalidPath(format!(
                    "expected_sha256 {p:?} is not 64 hex"
                )))
            }
            // Publisher git blob id: needs the size to build the `blob {size}\0` header.
            (None, None) if known.git_blob_sha1.is_some() && known.size.is_some() => {
                (None, res.basis)
            }
            (None, None) if opts.allow_unverified => (None, VerificationState::Unverified),
            (None, None) => return Err(Error::NoHash(path.into())),
        };
        // A pin says nothing about size; only trust the size that came with the hash basis.
        let size = if state == VerificationState::Pinned {
            None
        } else {
            known.size
        };
        let check = Check {
            sha256: sha,
            size,
            git_blob: known.git_blob_sha1.clone().zip(known.size),
        };

        if let Some(dir) = dest.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        let part = part_path(dest);
        let mut failed = Vec::new();
        for source in &res.sources {
            let url = source.file_url(path);
            let mut result = self.download(&url, &part, &check).await;
            if let Err(Attempt::Mismatch { resumed: true, .. }) = &result {
                // The partial bytes may have come from an earlier source: retry this one from 0.
                let _ = tokio::fs::remove_file(&part).await;
                result = self.download(&url, &part, &check).await;
            }
            match result {
                Ok((n, digest)) => {
                    tokio::fs::rename(&part, dest).await?;
                    return Ok(FetchOutcome {
                        path: dest.to_path_buf(),
                        size: n,
                        sha256: digest,
                        verification: Verification {
                            state,
                            source_used: source.kind,
                            source_url: source.base_url.clone(),
                            manifest_commit: res.manifest_commit().map(str::to_string),
                            checked_at: now_utc(),
                        },
                        failed,
                    });
                }
                Err(a) => {
                    if matches!(a, Attempt::Mismatch { .. }) {
                        let _ = tokio::fs::remove_file(&part).await;
                    }
                    failed.push(FailedAttempt {
                        source: source.kind,
                        url,
                        reason: a.to_string(),
                    });
                }
            }
        }
        Err(Error::AllSourcesFailed {
            path: path.into(),
            attempts: failed,
        })
    }

    /// One attempt at one URL. Returns (bytes, sha256 hex) of the complete `.part`.
    async fn download(
        &self,
        url: &str,
        part: &Path,
        check: &Check,
    ) -> std::result::Result<(u64, String), Attempt> {
        let size = check.size;
        let mut have = tokio::fs::metadata(part)
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        if size.is_some_and(|s| have > s) {
            tokio::fs::remove_file(part).await.map_err(Attempt::io)?;
            have = 0;
        }
        let mut req = self.http.get(url);
        if have > 0 {
            req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
        }
        let mut resp = req
            .send()
            .await
            .map_err(|e| Attempt::Transport(e.to_string()))?;
        let status = resp.status();
        let mut hasher = Hashers::new(check);
        let mut written = 0u64;
        let resumed;
        let mut file = if status == reqwest::StatusCode::PARTIAL_CONTENT && have > 0 {
            let range_ok = resp
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.starts_with(&format!("bytes {have}-")));
            if !range_ok {
                return Err(Attempt::Mismatch {
                    resumed: true,
                    why: "server sent a different range".into(),
                });
            }
            written = hash_existing(part, &mut hasher)
                .await
                .map_err(Attempt::io)?;
            resumed = true;
            tokio::fs::OpenOptions::new()
                .append(true)
                .open(part)
                .await
                .map_err(Attempt::io)?
        } else if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE && have > 0 {
            // `.part` may already be complete; let the hash decide.
            written = hash_existing(part, &mut hasher)
                .await
                .map_err(Attempt::io)?;
            return finish(hasher, written, check, true);
        } else if status.is_success() {
            resumed = false;
            tokio::fs::File::create(part).await.map_err(Attempt::io)?
        } else {
            return Err(Attempt::Transport(format!("HTTP {status}")));
        };
        loop {
            let chunk = match resp.chunk().await {
                Ok(Some(c)) => c,
                Ok(None) => break,
                Err(e) => {
                    let _ = file.flush().await;
                    return Err(Attempt::Transport(e.to_string()));
                }
            };
            written += chunk.len() as u64;
            if size.is_some_and(|s| written > s) {
                return Err(Attempt::Mismatch {
                    resumed,
                    why: format!("more than {} bytes", size.unwrap_or(0)),
                });
            }
            hasher.update(&chunk);
            file.write_all(&chunk).await.map_err(Attempt::io)?;
        }
        file.flush().await.map_err(Attempt::io)?;
        drop(file);
        finish(hasher, written, check, resumed)
    }

    /// GET a small file (catalog, `.minisig`). Retries up to [`SMALL_GET_TRIES`]
    /// times on a transport error, HTTP 429 or 5xx, so one dropped request does not
    /// fail a signed download. A 4xx other than 429 fails at once.
    async fn get_small(&self, url: &str) -> Result<Vec<u8>> {
        let mut last = String::new();
        for attempt in 0..SMALL_GET_TRIES {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(400 * (1 << attempt))).await;
            }
            let resp = match self.http.get(url).timeout(NETWORK_TIMEOUT).send().await {
                Ok(r) => r,
                Err(e) => {
                    last = e.to_string();
                    continue;
                }
            };
            let status = resp.status();
            if !status.is_success() {
                last = format!("{url}: HTTP {status}");
                if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                    continue;
                }
                return Err(Error::Http(last));
            }
            match resp.bytes().await {
                Ok(b) => return Ok(b.to_vec()),
                Err(e) => last = e.to_string(),
            }
        }
        Err(Error::Http(format!("{last} (after {SMALL_GET_TRIES} tries)")))
    }
}

enum Attempt {
    Transport(String),
    Mismatch { resumed: bool, why: String },
    Io(String),
}

impl Attempt {
    fn io(e: std::io::Error) -> Self {
        Attempt::Io(e.to_string())
    }
}

impl std::fmt::Display for Attempt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Attempt::Transport(s) => write!(f, "transport: {s}"),
            Attempt::Mismatch { why, .. } => write!(f, "mismatch: {why}"),
            Attempt::Io(s) => write!(f, "io: {s}"),
        }
    }
}

/// What one download must match.
struct Check {
    sha256: Option<String>,
    size: Option<u64>,
    /// (git blob SHA-1 hex, size) from the publisher tree.
    git_blob: Option<(String, u64)>,
}

/// sha256 always; git blob SHA-1 too when the publisher gave one.
struct Hashers {
    sha256: Sha256,
    blob: Option<sha1::Sha1>,
}

impl Hashers {
    fn new(check: &Check) -> Self {
        let blob = check.git_blob.as_ref().map(|(_, size)| {
            let mut h = sha1::Sha1::new();
            h.update(format!("blob {size}\0").as_bytes());
            h
        });
        Self {
            sha256: Sha256::new(),
            blob,
        }
    }

    fn update(&mut self, bytes: &[u8]) {
        self.sha256.update(bytes);
        if let Some(b) = &mut self.blob {
            b.update(bytes);
        }
    }
}

fn finish(
    hasher: Hashers,
    written: u64,
    check: &Check,
    resumed: bool,
) -> std::result::Result<(u64, String), Attempt> {
    let digest = hex::encode(hasher.sha256.finalize());
    let (sha, size) = (check.sha256.as_deref(), check.size);
    if let Some(s) = size {
        if written != s {
            return Err(Attempt::Mismatch {
                resumed,
                why: format!("size {written}, expected {s}"),
            });
        }
    }
    if let Some(want) = sha {
        if digest != want {
            return Err(Attempt::Mismatch {
                resumed,
                why: format!("sha256 {digest}, expected {want}"),
            });
        }
    }
    if let (Some((want, _)), Some(blob)) = (&check.git_blob, hasher.blob) {
        let got = hex::encode(blob.finalize());
        if &got != want {
            return Err(Attempt::Mismatch {
                resumed,
                why: format!("git blob {got}, expected {want}"),
            });
        }
    }
    Ok((written, digest))
}

async fn hash_existing(part: &Path, hasher: &mut Hashers) -> std::io::Result<u64> {
    let mut f = tokio::fs::File::open(part).await?;
    let mut buf = vec![0u8; 1 << 16];
    let mut n = 0u64;
    loop {
        let r = f.read(&mut buf).await?;
        if r == 0 {
            return Ok(n);
        }
        hasher.update(&buf[..r]);
        n += r as u64;
    }
}

fn part_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_os_string();
    s.push(".part");
    PathBuf::from(s)
}

fn now_utc() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Percent-encode each `/`-separated segment.
fn encode_path(path: &str) -> String {
    path.split('/')
        .map(|s| urlencoding::encode(s).into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

pub(crate) fn check_repo(repo: &str) -> Result<()> {
    let ok_part = |p: &str| {
        !p.is_empty()
            && p != "."
            && p != ".."
            && p.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    };
    match repo.split_once('/') {
        Some((org, name)) if ok_part(org) && ok_part(name) => Ok(()),
        _ => Err(Error::InvalidRepo(repo.into())),
    }
}

pub(crate) fn check_path(path: &str) -> Result<()> {
    let bad = path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == "..");
    if bad {
        Err(Error::InvalidPath(path.into()))
    } else {
        Ok(())
    }
}
