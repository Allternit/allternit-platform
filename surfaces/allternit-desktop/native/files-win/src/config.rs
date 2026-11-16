//! What Allternit Desktop hands the helper for one computer (`--config <file>`).
//!
//! The bearer token is never in this file: it comes from `ALLTERNIT_FILES_TOKEN`
//! and later `{"type":"token"}` lines on stdin (see `main.rs`).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelperConfig {
    /// `EndpointsResponse.computer_id` (SPEC "Computer ids"): the runtime device id,
    /// or the `pc_...` id of a mesh-only machine.
    pub computer_id: String,
    /// Shown in Explorer and in the wake prompt.
    pub computer_name: String,
    /// This PC's runtime device id (sent on wake consent and pins).
    pub device_id: String,
    /// This PC's name; the runtime uses it in conflict copy names.
    pub device_name: String,
    /// The transport picked from `GET /computer-files/:id/endpoints`.
    pub transport: TransportConfig,
    /// cloud-api origin, e.g. `https://api.allternit.com` (wake consent).
    pub cloud_api_base: String,
    /// Overrides `%USERPROFILE%\Allternit Files\<computer_name>`.
    #[serde(default)]
    pub sync_root: Option<PathBuf>,
    /// Folder with the badge and location icons (default: `<exe dir>\icons`).
    #[serde(default)]
    pub icons_dir: Option<PathBuf>,
    /// Folder for the helper's state (default: `%LOCALAPPDATA%\Allternit\files-win`).
    #[serde(default)]
    pub state_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportConfig {
    /// `loopback`, `mesh` or `relay` (gateway is for guests, not this helper).
    pub kind: String,
    /// `TransportCandidate.base_url`.
    pub base_url: String,
}
