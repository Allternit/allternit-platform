//! Allternit Files drive on Windows (plan step 24).
//!
//! One Cloud Files (cfapi) sync root per Allternit computer, under
//! `%USERPROFILE%\Allternit Files\<computer name>`. Explorer lists it in the
//! navigation pane; files are placeholders until opened.
//!
//! - Listing (FETCH_PLACEHOLDERS) and the change feed come from `files/2`
//!   (`contracts/files-drive/SPEC.md`). They never wake a sleeping computer: the
//!   runtime answers them from the asleep snapshot (SPEC section 9).
//! - Opening a file (FETCH_DATA) reads `GET /items/:id/content` in 4096-aligned
//!   ranges of at most 4 MiB (SPEC section 5, the relay limit).
//! - Content of a sleeping computer needs consent: the helper asks the user first
//!   (ask), wakes without asking (auto) or refuses (never). See [`wake`].
//! - Local edits go back through upload sessions with `base_rev`; a stale write
//!   comes back as a conflict copy (SPEC section 4), which the helper mirrors.
//!
//! The pure logic (ranges, identities, names, index, client, wake rules) builds on
//! every OS so it can be unit tested anywhere; the cfapi parts are Windows only.

pub mod client;
pub mod conflict;
pub mod config;
pub mod identity;
pub mod index;
pub mod paths;
pub mod ranges;
pub mod upload;
pub mod wake;

#[cfg(windows)]
pub mod badges;
#[cfg(windows)]
pub mod cf;
#[cfg(windows)]
pub mod provider;
#[cfg(windows)]
pub mod sync_root;
#[cfg(windows)]
pub mod watcher;

/// Printed in logs and set as the sync root version.
pub const HELPER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Joins the process multithreaded apartment before any WinRT call
/// (StorageFolder, StorageProviderSyncRootManager). Without it the WinRT
/// objects live in no apartment and the first call can crash the helper.
/// Safe to call from any thread, any number of times.
#[cfg(windows)]
pub fn init_winrt() {
    use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
    // S_FALSE (already joined) and RPC_E_CHANGED_MODE (thread is STA) are fine.
    let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
}

/// One JSON line on stdout per event, so Allternit Desktop can follow the helper.
pub fn emit(event: &serde_json::Value) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{event}");
    let _ = out.flush();
}
