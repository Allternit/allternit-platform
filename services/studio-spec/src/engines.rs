use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The engine catalog shipped with the crate.
pub const ENGINES_JSON: &str = include_str!("../engines.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallState {
    NotInstalled,
    Installing,
    Installed,
    Failed,
}

/// What `GET /engines` returns for one engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngineInfo {
    pub id: String,
    pub name: String,
    pub license: String,
    pub version: String,
    pub methods: Vec<String>,
    /// `apple-silicon`, `cuda-linux`
    pub hardware: Vec<String>,
    pub install_state: InstallState,
    #[serde(default)]
    pub limits: BTreeMap<String, serde_json::Value>,
    /// false = never shipped or published by us (upstream AGPL engines).
    #[serde(default = "yes")]
    pub published: bool,
    /// How the engine runs: `embedded`, `separate_process`.
    #[serde(default)]
    pub process: Option<String>,
    #[serde(default)]
    pub default: bool,
    /// Backends listed for the engine, each with a status.
    #[serde(default)]
    pub backends: Vec<Backend>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Backend {
    pub id: String,
    /// `planned`, `available`, or `pending_decision` (waits on Eoj, F14-prime).
    pub status: String,
}

fn yes() -> bool {
    true
}

pub fn catalog() -> Vec<EngineInfo> {
    serde_json::from_str(ENGINES_JSON).expect("engines.json is valid")
}

pub fn find(id: &str) -> Option<EngineInfo> {
    catalog().into_iter().find(|e| e.id == id)
}
