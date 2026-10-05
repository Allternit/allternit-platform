//! Port of `platform/packages/native-sessions/src/fingerprint.ts`.
//!
//! The fingerprint is a sha256 over path, stat triple (size, truncated mtime
//! ms, inode), and — for directories — sorted child names with their stat
//! triples (skipping `.lock`/`-shm`/`-wal` siblings). It is a *staleness*
//! token, not a content hash: two identical fingerprints mean "no detected
//! change", differing fingerprints mean "re-scan".

use std::path::Path;

use sha2::{Digest, Sha256};

/// Fingerprint a single path (`fingerprint.ts` fingerprintPath).
pub fn fingerprint_path(target: &Path) -> String {
    let mut hash = Sha256::new();
    add_path(&mut hash, target);
    hex_digest(hash)
}

/// Fingerprint several paths in order (`fingerprint.ts` fingerprintPaths).
pub fn fingerprint_paths(targets: &[&Path]) -> String {
    let mut hash = Sha256::new();
    for target in targets {
        add_path(&mut hash, target);
    }
    hex_digest(hash)
}

fn hex_digest(hash: Sha256) -> String {
    let bytes = hash.finalize();
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{:02x}", b));
    }
    out
}

fn add_path(hash: &mut Sha256, target: &Path) {
    hash.update(target.to_string_lossy().as_bytes());
    hash.update("\0");
    let Ok(st) = std::fs::metadata(target) else {
        hash.update("missing");
        return;
    };
    hash.update(st.len().to_string().as_bytes());
    hash.update("\0");
    hash.update(mtime_ms(&st).to_string().as_bytes());
    hash.update("\0");
    hash.update(ino(&st).to_string().as_bytes());
    if st.is_dir() {
        let mut names: Vec<String> = match std::fs::read_dir(target) {
            Ok(entries) => entries
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect(),
            Err(_) => Vec::new(),
        };
        names.sort();
        for name in names {
            if name.ends_with(".lock") || name.ends_with("-shm") || name.ends_with("-wal") {
                continue;
            }
            let child = target.join(&name);
            hash.update(name.as_bytes());
            hash.update("\0");
            match std::fs::metadata(&child) {
                Ok(child_st) => {
                    hash.update(child_st.len().to_string().as_bytes());
                    hash.update("\0");
                    hash.update(mtime_ms(&child_st).to_string().as_bytes());
                    hash.update("\0");
                }
                Err(_) => {
                    hash.update(name.as_bytes());
                    hash.update("\0missing");
                }
            }
        }
    }
}

fn mtime_ms(st: &std::fs::Metadata) -> u64 {
    st.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(unix)]
fn ino(st: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    st.ino()
}

#[cfg(not(unix))]
fn ino(_st: &std::fs::Metadata) -> u64 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_stable_and_sensitive_to_change() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("wire.jsonl");
        std::fs::write(&file, "{}\n").unwrap();

        let a = fingerprint_path(&file);
        let b = fingerprint_path(&file);
        assert_eq!(a, b, "same stat must fingerprint identically");
        assert_eq!(a.len(), 64);

        std::fs::write(&file, "{}\n{}\n").unwrap();
        let c = fingerprint_path(&file);
        assert_ne!(a, c, "size change must change the fingerprint");
    }

    #[test]
    fn fingerprint_directory_covers_children() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("session_abc");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("state.json"), "{}").unwrap();

        let a = fingerprint_path(&sub);
        std::fs::write(sub.join("wire.jsonl"), "{}").unwrap();
        let b = fingerprint_path(&sub);
        assert_ne!(a, b, "new child must change the directory fingerprint");
    }

    #[test]
    fn fingerprint_missing_path_is_distinct_and_stable() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope");
        let a = fingerprint_path(&missing);
        let b = fingerprint_path(&missing);
        assert_eq!(a, b);
        assert_ne!(a, fingerprint_path(dir.path()));
    }

    /// Cross-implementation parity gate, driven by
    /// `tests/ao_visibility_parity/run.sh`: the script computes the expected
    /// fingerprints with the REAL TS implementation (`fingerprint.ts` via
    /// bun) over a shared fixture and passes them in via env. Skips (passes)
    /// when the env is absent so plain `cargo test` stays hermetic.
    #[test]
    fn fingerprint_parity_with_ts() {
        let Ok(expected_single) = std::env::var("AO_FP_PARITY_SINGLE") else {
            return;
        };
        let expected_multi = std::env::var("AO_FP_PARITY_MULTI").unwrap();
        let target = std::env::var("AO_FP_PARITY_TARGET").unwrap();
        let target2 = std::env::var("AO_FP_PARITY_TARGET2").unwrap();
        assert_eq!(fingerprint_path(std::path::Path::new(&target)), expected_single);
        assert_eq!(
            fingerprint_paths(&[
                std::path::Path::new(&target2),
                std::path::Path::new(&target),
            ]),
            expected_multi
        );
    }
}
