//! Repo ids, on-disk layout and file selection for model downloads.
//!
//! Bytes never come from here: every download goes through
//! `allternit-model-source` (`resolve` + `fetch_verified`), see `task.rs`.

use allternit_model_source::ExpectedFile;
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

/// Errors from repo-id validation and file selection.
#[derive(Debug, Error)]
pub enum HuggingFaceError {
    #[error("invalid repo id: {0}")]
    InvalidRepoId(String),
    #[error("{0}")]
    NoMatchingFiles(String),
}

/// Validate that a repo id is safe to use.
///
/// Rejects empty ids, absolute paths, path traversal (`..`), and suspicious
/// characters that could break filesystem layout.
pub fn validate_repo_id(repo_id: &str) -> Result<&str, HuggingFaceError> {
    if repo_id.is_empty() {
        return Err(HuggingFaceError::InvalidRepoId(
            "repo_id must not be empty".into(),
        ));
    }

    if repo_id.starts_with('/') || repo_id.starts_with('\\') {
        return Err(HuggingFaceError::InvalidRepoId(
            "repo_id must not be an absolute path".into(),
        ));
    }

    if repo_id.contains("..") {
        return Err(HuggingFaceError::InvalidRepoId(
            "repo_id must not contain '..'".into(),
        ));
    }

    for part in repo_id.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(HuggingFaceError::InvalidRepoId(format!(
                "repo_id contains invalid segment: {}",
                part
            )));
        }
    }

    Ok(repo_id)
}

/// Return `true` if `child` is contained within `base` after normalization.
pub fn is_inside_base(base: &Path, child: &Path) -> bool {
    let base = base.canonicalize().unwrap_or_else(|_| base.to_path_buf());
    let child = child.canonicalize().unwrap_or_else(|_| child.to_path_buf());
    child.starts_with(&base)
}

/// Ensure a resolved file path stays under the models directory.
pub fn safe_model_file(
    base: &Path,
    repo_id: &str,
    filename: &str,
) -> Result<PathBuf, HuggingFaceError> {
    let normalized = repo_id.replace('/', "--");
    let target = base.join(normalized).join(filename);

    // Reject any filename that tries to escape with `..`.
    for component in target.components() {
        if matches!(component, Component::ParentDir) {
            return Err(HuggingFaceError::InvalidRepoId(
                "filename contains path traversal".into(),
            ));
        }
    }

    if !is_inside_base(base, &target) {
        return Err(HuggingFaceError::InvalidRepoId(
            "resolved path escapes models_dir".into(),
        ));
    }

    Ok(target)
}

/// Return `true` if a file from the Hub should be downloaded for local inference.
pub fn should_download(path: &str) -> bool {
    let lower = path.to_lowercase();
    let name = std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path);

    // Always keep core config/tokenizer files.
    let keep_always = [
        "config.json",
        "tokenizer.json",
        "tokenizer_config.json",
        "generation_config.json",
        "preprocessor_config.json",
        "vocab.json",
        "merges.txt",
    ];
    if keep_always.contains(&name) {
        return true;
    }

    // Weight file extensions.
    let weight_exts = [".safetensors", ".bin", ".gguf", ".mlx", ".pt", ".pth"];
    if weight_exts.iter().any(|ext| lower.ends_with(ext)) {
        return true;
    }

    // Explicitly ignore common non-weight files, including `.json`/`.txt` that
    // are not in the keep-always list.
    let ignore_exts = [
        ".gitattributes",
        ".md",
        ".txt",
        ".png",
        ".jpg",
        ".jpeg",
        ".gif",
        ".svg",
        ".ico",
        ".yaml",
        ".yml",
        ".json",
    ];
    if ignore_exts.iter().any(|ext| lower.ends_with(ext)) {
        return false;
    }

    if lower.starts_with("readme") {
        return false;
    }

    // Default to keeping unknown files; they may be custom model components.
    true
}

/// `true` for weight files (the files the quantization filter applies to).
pub fn is_weight_file(path: &str) -> bool {
    let lower = path.to_lowercase();
    [".safetensors", ".bin", ".gguf", ".mlx", ".pt", ".pth"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

/// Pick the files to download from a resolution's file list (the signed
/// manifest's `files[]`, or the publisher tree when the catalog is not used).
///
/// `should_download` drops docs and images. With a `quantization`, weight
/// files are kept only when their path contains it (case-insensitive), so a
/// GGUF repo with ten quants downloads one; config/tokenizer files stay.
pub fn select_files(
    files: &BTreeMap<String, ExpectedFile>,
    quantization: Option<&str>,
) -> Result<Vec<(String, ExpectedFile)>, HuggingFaceError> {
    let quant = quantization
        .map(|q| q.trim().to_lowercase())
        .filter(|q| !q.is_empty());
    let picked: Vec<(String, ExpectedFile)> = files
        .iter()
        .filter(|(path, _)| should_download(path))
        .filter(|(path, _)| match &quant {
            Some(q) if is_weight_file(path) => path.to_lowercase().contains(q.as_str()),
            _ => true,
        })
        .map(|(p, e)| (p.clone(), e.clone()))
        .collect();
    if let Some(q) = &quant {
        if !picked.iter().any(|(p, _)| is_weight_file(p)) {
            return Err(HuggingFaceError::NoMatchingFiles(format!(
                "no weight file matches quantization {q:?}"
            )));
        }
    }
    if picked.is_empty() {
        return Err(HuggingFaceError::NoMatchingFiles(
            "the repo has no downloadable model files".into(),
        ));
    }
    Ok(picked)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(paths: &[&str]) -> BTreeMap<String, ExpectedFile> {
        paths
            .iter()
            .map(|p| (p.to_string(), ExpectedFile::default()))
            .collect()
    }

    #[test]
    fn quant_filter_keeps_one_quant_and_configs() {
        let f = files(&[
            "README.md",
            "config.json",
            "Q4_K_M/model-Q4_K_M.gguf",
            "Q8_0/model-Q8_0.gguf",
            "mmproj-BF16.gguf",
        ]);
        let picked: Vec<String> = select_files(&f, Some("q4_k_m"))
            .unwrap()
            .into_iter()
            .map(|(p, _)| p)
            .collect();
        assert_eq!(picked, vec!["Q4_K_M/model-Q4_K_M.gguf", "config.json"]);
        assert!(select_files(&f, Some("IQ1_S")).is_err());
        assert_eq!(select_files(&f, None).unwrap().len(), 4);
    }
}
