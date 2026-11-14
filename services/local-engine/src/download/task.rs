//! Background download task and local re-verification.
//!
//! Every byte goes through `allternit-model-source`: `resolve(repo, rev)`
//! gives the ordered sources and the hashes (signed Freeweights manifest when
//! the catalog lists the repo, the publisher's hashes otherwise), and
//! `fetch_verified` downloads each file, checking size and hash before the
//! `.part` file is renamed into place. There is no unverified path.

use crate::cache::model::{CachedModel, FileVerification};
use crate::cache::store::ModelStore;
use crate::download::huggingface::{self, is_weight_file, select_files};
use allternit_model_source::{
    Client, FetchOptions, Resolution, Verification, VerificationState,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::fs;
use tracing::{error, info, warn};

/// Start a download in the background.
///
/// The caller is expected to have already inserted a `CachedModel` record
/// into the store with status `Pending`. This function transitions it to
/// `Downloading`, fetches every selected file through the verified path,
/// records the verification, and finally marks it `Ready` or `Failed`.
pub fn spawn_download_task(
    store: ModelStore,
    model_id: String,
    repo_id: String,
    revision: String,
    quantization: Option<String>,
    models_dir: PathBuf,
) {
    tokio::spawn(async move {
        if let Err(err) = run_download(
            &store,
            &model_id,
            &repo_id,
            &revision,
            quantization.as_deref(),
            &models_dir,
        )
        .await
        {
            error!(%model_id, %err, "download task failed");
            store.set_failed(&model_id, err.to_string()).await;
        }
    });
}

#[derive(Debug, thiserror::Error)]
enum DownloadError {
    #[error("{0}")]
    Files(#[from] huggingface::HuggingFaceError),
    #[error("{0}")]
    Source(#[from] allternit_model_source::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Strength order for the model-level state: the weakest file decides.
fn rank(state: VerificationState) -> u8 {
    match state {
        VerificationState::FreeweightsSigned => 3,
        VerificationState::PublisherHash => 2,
        VerificationState::Pinned => 1,
        VerificationState::Unverified => 0,
    }
}

/// Model-level verification: the weakest weight file (all files when there is
/// no weight file); ties go to the largest file, so `source_used` names where
/// the bulk of the bytes came from.
pub fn aggregate(files: &[FileVerification]) -> Option<Verification> {
    let weights: Vec<&FileVerification> = files.iter().filter(|f| is_weight_file(&f.path)).collect();
    let pool: Vec<&FileVerification> = if weights.is_empty() {
        files.iter().collect()
    } else {
        weights
    };
    pool.into_iter()
        .min_by(|a, b| {
            rank(a.verification.state)
                .cmp(&rank(b.verification.state))
                .then(b.size.cmp(&a.size))
        })
        .map(|f| f.verification.clone())
}

async fn run_download(
    store: &ModelStore,
    model_id: &str,
    repo_id: &str,
    revision: &str,
    quantization: Option<&str>,
    models_dir: &Path,
) -> Result<(), DownloadError> {
    info!(%model_id, %repo_id, %revision, "starting verified model download");
    store.set_downloading(model_id).await;

    let client = Client::new()?;
    let res = client.resolve(repo_id, Some(revision)).await?;
    if let Some(reason) = &res.fallback_reason {
        info!(%repo_id, %reason, "Freeweights not used; checking against the publisher's hashes");
    }
    let files = select_files(&res.files, quantization)?;
    let total_bytes: u64 = files.iter().filter_map(|(_, e)| e.size).sum();
    store.update_progress(model_id, 0, total_bytes).await;

    // No pins and no `allow_unverified`: a file without a hash fails the download.
    let opts = FetchOptions::default();
    let mut done_bytes = 0u64;
    let mut checked = Vec::with_capacity(files.len());
    for (path, expected) in &files {
        let dest = huggingface::safe_model_file(models_dir, repo_id, path)?;
        let part = part_path(&dest);
        // A file already on disk is moved to `.part` so `fetch_verified`
        // re-hashes it (and only fetches what is missing). If every source is
        // unreachable it is moved back untouched; a mismatch deletes it.
        let moved = if fs::metadata(&dest).await.is_ok() {
            let _ = fs::remove_file(&part).await;
            fs::rename(&dest, &part).await?;
            true
        } else {
            false
        };

        let fetch = client.fetch_verified(&res, path, &dest, &opts);
        tokio::pin!(fetch);
        let mut tick = tokio::time::interval(Duration::from_millis(500));
        let outcome = loop {
            tokio::select! {
                r = &mut fetch => break r,
                _ = tick.tick() => {
                    let have = fs::metadata(&part).await.map(|m| m.len()).unwrap_or(0);
                    let have = expected.size.map_or(have, |s| have.min(s));
                    store.update_progress(model_id, done_bytes + have, total_bytes).await;
                }
            }
        };
        let outcome = match outcome {
            Ok(o) => o,
            Err(e) => {
                if moved && fs::metadata(&part).await.is_ok() && fs::metadata(&dest).await.is_err() {
                    let _ = fs::rename(&part, &dest).await;
                }
                return Err(e.into());
            }
        };
        for f in &outcome.failed {
            warn!(%repo_id, %path, source = ?f.source, url = %f.url, reason = %f.reason, "source rejected");
        }
        done_bytes += outcome.size;
        store.update_progress(model_id, done_bytes, total_bytes.max(done_bytes)).await;
        info!(%path, state = ?outcome.verification.state, source = ?outcome.verification.source_used, "file verified");
        checked.push(FileVerification {
            path: path.clone(),
            size: outcome.size,
            sha256: outcome.sha256,
            verification: outcome.verification,
        });
    }

    if let Some(v) = aggregate(&checked) {
        store.set_verification(model_id, v, checked).await;
    }
    store.set_ready(model_id).await;
    info!(%model_id, "model download ready");
    Ok(())
}

fn part_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_os_string();
    s.push(".part");
    PathBuf::from(s)
}

/// Public helper to create the initial `CachedModel` record for a download.
pub fn build_cached_model(
    repo_id: &str,
    revision: &str,
    quantization: Option<String>,
    models_dir: &Path,
) -> CachedModel {
    let id = ModelStore::model_id(repo_id, revision, quantization.as_deref());
    let path = models_dir.join(ModelStore::normalize_repo_id(repo_id));
    CachedModel::new(id.clone(), repo_id, revision, quantization, path)
}

// ─── /models/verify ──────────────────────────────────────────────────────────

/// One local file re-hashed against the resolution.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FileCheck {
    pub path: String,
    pub size: u64,
    pub sha256: String,
    /// sha256 the file should have (manifest or publisher `lfs.oid`).
    pub expected_sha256: Option<String>,
    /// `true` = matched, `false` = mismatched, `None` = nothing to check
    /// against (not listed, or no hash for it).
    pub ok: Option<bool>,
    /// What it was checked against when `ok` is `Some(true)`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<VerificationState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Body of `POST /models/verify`.
#[derive(Debug, Clone, Serialize)]
pub struct VerifyReport {
    pub repo: String,
    /// Revision the hashes are for (the manifest commit when signed).
    pub revision: String,
    /// `true` when every checked file matched and at least one weight file
    /// was checked.
    pub ok: bool,
    /// Contract `verification` when `ok`; `null` otherwise.
    pub verification: Option<Verification>,
    pub source_used: Option<allternit_model_source::SourceKind>,
    pub files: Vec<FileCheck>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Re-hash every file under `dir` against `res` (pure apart from file reads,
/// so it is testable without the network). `downloaded_from` is the record's
/// earlier verification, whose source is kept when the files still match.
pub async fn check_local_files(
    dir: &Path,
    res: &Resolution,
    downloaded_from: Option<&Verification>,
) -> std::io::Result<VerifyReport> {
    let mut paths = Vec::new();
    collect_files(dir, dir, &mut paths)?;
    paths.sort();

    let mut files = Vec::new();
    let mut weakest: Option<VerificationState> = None;
    let mut weight_checked = false;
    for rel in paths {
        let full = dir.join(&rel);
        let (size, sha256, blob) = hash_file(full).await?;
        let expected = res.files.get(&rel);
        let (ok, state, reason) = match expected {
            None => (None, None, Some("not listed for this repo".to_string())),
            Some(e) if e.size.is_some_and(|s| s != size) => (
                Some(false),
                None,
                Some(format!("size {size}, expected {}", e.size.unwrap_or(0))),
            ),
            Some(e) => match (&e.sha256, &e.git_blob_sha1) {
                (Some(want), _) if *want == sha256 => (Some(true), Some(res.basis), None),
                (Some(_), _) => (Some(false), None, Some("sha256 mismatch".into())),
                (None, Some(want)) if *want == blob => (Some(true), Some(res.basis), None),
                (None, Some(_)) => (Some(false), None, Some("git blob id mismatch".into())),
                (None, None) => (None, None, Some("no hash available".into())),
            },
        };
        if let Some(s) = state {
            if is_weight_file(&rel) {
                weight_checked = true;
            }
            weakest = Some(match weakest {
                Some(w) if rank(w) <= rank(s) => w,
                _ => s,
            });
        }
        files.push(FileCheck {
            path: rel,
            size,
            sha256,
            expected_sha256: expected.and_then(|e| e.sha256.clone()),
            ok,
            state,
            reason,
        });
    }

    let mismatched: Vec<&str> = files
        .iter()
        .filter(|f| f.ok == Some(false))
        .map(|f| f.path.as_str())
        .collect();
    let unchecked_weights: Vec<&str> = files
        .iter()
        .filter(|f| f.ok.is_none() && is_weight_file(&f.path))
        .map(|f| f.path.as_str())
        .collect();
    let reason = if files.is_empty() {
        Some("no local files".to_string())
    } else if !mismatched.is_empty() {
        Some(format!("files do not match: {}", mismatched.join(", ")))
    } else if !unchecked_weights.is_empty() {
        Some(format!("no hash to check: {}", unchecked_weights.join(", ")))
    } else if !weight_checked {
        Some("no weight file was checked".to_string())
    } else {
        None
    };
    let ok = reason.is_none();
    let verification = match (ok, weakest, res.sources.first()) {
        (true, Some(state), Some(first)) => {
            let (source_used, source_url) = match downloaded_from {
                Some(v) => (v.source_used, v.source_url.clone()),
                None => (first.kind, first.base_url.clone()),
            };
            Some(Verification {
                state,
                source_used,
                source_url,
                manifest_commit: res.manifest_commit().map(str::to_string),
                checked_at: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            })
        }
        _ => None,
    };
    Ok(VerifyReport {
        repo: res.repo.clone(),
        revision: res.revision.clone(),
        ok,
        source_used: verification.as_ref().map(|v| v.source_used),
        verification,
        files,
        fallback_reason: res.fallback_reason.clone(),
        reason,
    })
}

/// `POST /models/verify`: resolve `repo` and re-hash its local files. A
/// mismatch marks the repo's records `failed` so a tampered model is not run.
pub async fn verify_local(
    store: &ModelStore,
    models_dir: &Path,
    repo: &str,
    revision: Option<&str>,
) -> Result<VerifyReport, String> {
    huggingface::validate_repo_id(repo).map_err(|e| e.to_string())?;
    let dir = models_dir.join(ModelStore::normalize_repo_id(repo));
    if !dir.is_dir() {
        return Err(format!("{repo} is not downloaded"));
    }
    let ids = store.ids_for_repo(repo).await;
    let mut record = None;
    for id in &ids {
        if let Some(m) = store.get(id).await {
            record = Some(m);
            break;
        }
    }
    let revision = revision.or(record.as_ref().map(|m| m.revision.as_str()));
    let client = Client::new().map_err(|e| e.to_string())?;
    let res = client
        .resolve(repo, revision)
        .await
        .map_err(|e| e.to_string())?;
    let report = check_local_files(
        &dir,
        &res,
        record.as_ref().and_then(|m| m.verification.as_ref()),
    )
    .await
    .map_err(|e| e.to_string())?;

    for id in &ids {
        match (&report.verification, &report.reason) {
            (Some(v), _) => {
                let files = report
                    .files
                    .iter()
                    .filter(|f| f.ok == Some(true))
                    .map(|f| FileVerification {
                        path: f.path.clone(),
                        size: f.size,
                        sha256: f.sha256.clone(),
                        verification: Verification {
                            state: f.state.unwrap_or(v.state),
                            ..v.clone()
                        },
                    })
                    .collect();
                store.set_verification(id, v.clone(), files).await;
            }
            (None, Some(reason)) if report.files.iter().any(|f| f.ok == Some(false)) => {
                store
                    .set_failed(id, format!("verification failed: {reason}"))
                    .await;
            }
            _ => {}
        }
    }
    Ok(report)
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<String>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let ty = entry.file_type()?;
        if ty.is_dir() {
            collect_files(root, &path, out)?;
        } else if ty.is_file() {
            let name = path.to_string_lossy();
            if name.ends_with(".part") || entry.file_name() == ".DS_Store" {
                continue;
            }
            if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    Ok(())
}

/// (size, sha256 hex, git blob SHA-1 hex) of a file, off the async runtime.
async fn hash_file(path: PathBuf) -> std::io::Result<(u64, String, String)> {
    tokio::task::spawn_blocking(move || {
        use std::io::Read;
        let size = std::fs::metadata(&path)?.len();
        let mut f = std::fs::File::open(&path)?;
        let mut sha = Sha256::new();
        let mut blob = sha1::Sha1::new();
        blob.update(format!("blob {size}\0").as_bytes());
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            sha.update(&buf[..n]);
            blob.update(&buf[..n]);
        }
        Ok((size, hex::encode(sha.finalize()), hex::encode(blob.finalize())))
    })
    .await
    .map_err(std::io::Error::other)?
}

#[cfg(test)]
mod tests {
    use super::*;
    use allternit_model_source::{ExpectedFile, Source, SourceKind};
    use std::collections::BTreeMap;

    fn sha(bytes: &[u8]) -> String {
        hex::encode(Sha256::digest(bytes))
    }

    fn resolution(files: &[(&str, &[u8])]) -> Resolution {
        let files: BTreeMap<String, ExpectedFile> = files
            .iter()
            .map(|(p, b)| {
                (
                    p.to_string(),
                    ExpectedFile {
                        sha256: Some(sha(b)),
                        size: Some(b.len() as u64),
                        git_blob_sha1: None,
                    },
                )
            })
            .collect();
        Resolution {
            repo: "org/model-GGUF".into(),
            revision: "a".repeat(40),
            basis: VerificationState::FreeweightsSigned,
            manifest: None,
            sources: vec![Source {
                kind: SourceKind::FreeweightsWorker,
                base_url: "https://freeweights.allternit.com/org/model-GGUF/resolve/x/".into(),
            }],
            files,
            fallback_reason: None,
        }
    }

    #[tokio::test]
    async fn verify_passes_then_catches_a_tampered_weight() {
        let dir = tempfile::tempdir().unwrap();
        let weight = b"GGUF fake weights".as_slice();
        std::fs::create_dir_all(dir.path().join("Q4")).unwrap();
        std::fs::write(dir.path().join("Q4/m.gguf"), weight).unwrap();
        std::fs::write(dir.path().join("Q4/m.gguf.part"), b"ignored").unwrap();
        let res = resolution(&[("Q4/m.gguf", weight)]);

        let good = check_local_files(dir.path(), &res, None).await.unwrap();
        assert!(good.ok, "{good:?}");
        let v = good.verification.unwrap();
        assert_eq!(v.state, VerificationState::FreeweightsSigned);
        assert_eq!(v.source_used, SourceKind::FreeweightsWorker);
        assert_eq!(good.files.len(), 1, ".part files are skipped");

        // Same size, one byte flipped.
        std::fs::write(dir.path().join("Q4/m.gguf"), b"GGUF fake weightz").unwrap();
        let bad = check_local_files(dir.path(), &res, None).await.unwrap();
        assert!(!bad.ok);
        assert!(bad.verification.is_none());
        assert_eq!(bad.files[0].ok, Some(false));
        assert!(bad.reason.unwrap().contains("Q4/m.gguf"));
    }

    #[tokio::test]
    async fn verify_needs_a_checked_weight_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.json"), b"{}").unwrap();
        std::fs::write(dir.path().join("extra.gguf"), b"unlisted").unwrap();
        let res = resolution(&[("config.json", b"{}")]);
        let r = check_local_files(dir.path(), &res, None).await.unwrap();
        assert!(!r.ok, "an unlisted weight file is not a pass");
        assert!(r.verification.is_none());
    }
}
