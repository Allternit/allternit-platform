//! Where the kernel's Ed25519 signing seed lives (CONTRACT.md section 12.3, G0 answer 3).
//!
//! | Host | Store | Who holds the seed |
//! |---|---|---|
//! | Allternit Desktop (Mac, Windows, Linux) | [`KeyStore::Desktop`] | Electron main keeps it in the OS keystore (macOS Keychain, Windows DPAPI, Linux Secret Service, through `secure-store.ts`) and writes it to this process's stdin at launch. Never on disk, never in the environment. |
//! | Headless (cloud computers, servers) | [`KeyStore::File`] | Hex seed in a 0600 file: `$ALLTERNIT_KERNEL_SIGNING_KEY`, else `<data dir>/kernel/authority-ed25519.key`. |
//!
//! allternit-api does not talk to an OS keystore itself (same rule as `token_crypto.rs`): the
//! sidecar binary is re-signed per build, so a keystore item it owned would prompt on every update,
//! and cloud computers have no keystore at all. Desktop's main process already owns the Keychain
//! for every other Desktop secret, so it owns this one too.
//!
//! Desktop sets [`KEY_STDIN_ENV`] and writes `kernel-seed:<64 hex>` as the second stdin line, after
//! the D16 human-proof secret (see `subscription_routes::read_desktop_human_proof_from_stdin`).
//! In that mode the file is never read or created: a missing seed is `kernel_unavailable` (fail
//! closed). Moving an old 0600 file into the keystore is Desktop's job (`kernel-key.ts`); if a copy
//! of the same seed is still on disk here, it is removed. The seed is never logged.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use allternit_factory_engine::receipts::sign::ReceiptSigner;

/// Env flag Desktop sets when it hands the seed over on stdin.
pub const KEY_STDIN_ENV: &str = "ALLTERNIT_KERNEL_KEY_STDIN";
/// Optional path override for the headless key file.
pub const SIGNING_KEY_ENV: &str = "ALLTERNIT_KERNEL_SIGNING_KEY";
const KEY_FILE: &str = "kernel/authority-ed25519.key";
const SEED_LINE_PREFIX: &str = "kernel-seed:";

static DESKTOP_SEED: OnceLock<[u8; 32]> = OnceLock::new();

/// True when Desktop launched this process and hands the seed over on stdin.
pub fn desktop_handoff() -> bool {
    std::env::var(KEY_STDIN_ENV).as_deref() == Ok("1")
}

/// Parse `kernel-seed:<64 hex>`. Anything else is `None`.
pub fn parse_seed_line(line: &str) -> Option<[u8; 32]> {
    let raw = hex::decode(line.trim().strip_prefix(SEED_LINE_PREFIX)?).ok()?;
    raw.try_into().ok()
}

/// Take the seed line Desktop wrote on stdin. First valid line wins; the line itself is not kept.
pub fn accept_seed_line(line: &str) -> bool {
    match parse_seed_line(line) {
        Some(seed) => {
            let _ = DESKTOP_SEED.set(seed);
            tracing::info!("kernel signing key handed over by Desktop");
            true
        }
        None => {
            tracing::warn!("kernel signing key missing on stdin; kernel approvals stay unavailable");
            false
        }
    }
}

/// Headless key file: `$ALLTERNIT_KERNEL_SIGNING_KEY`, else `<data dir>/kernel/authority-ed25519.key`.
pub fn key_path(data_dir: &Path) -> PathBuf {
    std::env::var_os(SIGNING_KEY_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| data_dir.join(KEY_FILE))
}

/// Which store this process uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyStore {
    /// Seed from Desktop's OS keystore, over stdin.
    Desktop,
    /// Hex seed in a 0600 file (generated on first use).
    File(PathBuf),
}

impl KeyStore {
    pub fn select(data_dir: &Path) -> Self {
        if desktop_handoff() {
            KeyStore::Desktop
        } else {
            KeyStore::File(key_path(data_dir))
        }
    }

    /// For the one log line that says which store is in use (never the key).
    pub fn describe(&self) -> String {
        match self {
            KeyStore::Desktop => "OS keystore via Allternit Desktop".to_string(),
            KeyStore::File(p) => format!("0600 file {}", p.display()),
        }
    }

    /// Load the signer. `data_dir` is only used to find a stale file copy in Desktop mode.
    pub fn load(&self, data_dir: &Path) -> Result<ReceiptSigner, String> {
        match self {
            KeyStore::Desktop => load_desktop(DESKTOP_SEED.get(), &key_path(data_dir)),
            KeyStore::File(path) => ReceiptSigner::load_or_create(path, true).map_err(|e| e.to_string()),
        }
    }
}

/// Desktop mode: use the handed-over seed only. Never read the file as a fallback and never
/// create one. A file holding the same seed is a leftover copy and is removed.
fn load_desktop(seed: Option<&[u8; 32]>, stale_file: &Path) -> Result<ReceiptSigner, String> {
    let Some(seed) = seed else {
        return Err("Allternit Desktop has not handed the kernel signing key over (OS keystore locked or unavailable)".into());
    };
    let signer = ReceiptSigner::from_seed(*seed);
    if stale_file.exists() {
        match ReceiptSigner::load_or_create(stale_file, false) {
            Ok(on_disk) if on_disk.kid() == signer.kid() => match std::fs::remove_file(stale_file) {
                Ok(()) => tracing::info!("removed the old kernel key file; the key lives in the OS keystore now"),
                Err(e) => tracing::warn!(error = %e, "could not remove the old kernel key file"),
            },
            _ => tracing::warn!(
                path = %stale_file.display(),
                "a different kernel key file is on disk; left in place, not used"
            ),
        }
    }
    Ok(signer)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEED_HEX: &str = "0101010101010101010101010101010101010101010101010101010101010101";

    #[test]
    fn seed_line_parses_only_the_framed_form() {
        assert_eq!(parse_seed_line(&format!("kernel-seed:{SEED_HEX}\n")), Some([1u8; 32]));
        assert_eq!(parse_seed_line(SEED_HEX), None);
        assert_eq!(parse_seed_line("kernel-seed:"), None);
        assert_eq!(parse_seed_line("kernel-seed:abcd"), None);
        assert_eq!(parse_seed_line("kernel-seed:zz"), None);
    }

    #[test]
    fn desktop_mode_without_a_seed_fails_closed_and_never_creates_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(KEY_FILE);
        assert!(load_desktop(None, &file).is_err());
        assert!(!file.exists());
    }

    #[test]
    fn desktop_mode_removes_a_leftover_copy_of_the_same_seed() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("authority-ed25519.key");
        std::fs::write(&file, SEED_HEX).unwrap();
        let signer = load_desktop(Some(&[1u8; 32]), &file).unwrap();
        assert_eq!(signer.kid(), ReceiptSigner::from_seed([1u8; 32]).kid());
        assert!(!file.exists());
    }

    #[test]
    fn desktop_mode_leaves_a_different_key_file_alone() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("authority-ed25519.key");
        std::fs::write(&file, SEED_HEX).unwrap();
        let signer = load_desktop(Some(&[2u8; 32]), &file).unwrap();
        assert_eq!(signer.kid(), ReceiptSigner::from_seed([2u8; 32]).kid());
        assert!(file.exists());
    }
}
