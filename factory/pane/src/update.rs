//! Residue of the removed self-updater (ao P0 gut list).
//!
//! Upstream `src/update.rs` (~3800 lines) fetched https://herdr.dev update
//! manifests, ran `herdr update`, and drove background version checks. All of
//! that phone-home machinery is gone: ao updates ship via the harness.
//!
//! What remains here, because other engine code still uses it:
//! - `Version`: semver parse/compare used by release-notes preview detection
//!   and plugin manifest gating.
//! - `is_package_manager_managed_exe_path`: pure local path heuristic used by
//!   the remote-attach installer to decide whether the running binary can seed
//!   a remote install.


// ---------------------------------------------------------------------------
// Version
// ---------------------------------------------------------------------------

/// Parsed semver version for comparison.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.strip_prefix('v').unwrap_or(s);
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() != 3 {
            return None;
        }
        Some(Self {
            major: parts[0].parse().ok()?,
            minor: parts[1].parse().ok()?,
            patch: parts[2].parse().ok()?,
        })
    }

    pub fn current() -> Self {
        Self::parse(crate::build_info::BASE_VERSION).expect("invalid CARGO_PKG_VERSION")
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

// ---------------------------------------------------------------------------
// Installation manager detection (local path heuristics only, no network)
// ---------------------------------------------------------------------------

