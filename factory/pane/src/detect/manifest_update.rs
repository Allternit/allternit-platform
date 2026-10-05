use std::{
    cmp::Ordering,
    collections::BTreeMap,
    fmt, fs,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use super::{agent_label, Agent};

pub(crate) const MANIFEST_ENGINE_VERSION: u32 = 3;

#[derive(Debug, Clone)]
pub(crate) struct ManifestVersion(String);

impl ManifestVersion {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err("version must not be empty".to_string());
        }
        for segment in trimmed.split('.') {
            if segment.is_empty() {
                return Err(format!("version {trimmed:?} contains an empty segment"));
            }
            if !segment.chars().all(|ch| ch.is_ascii_digit()) {
                return Err(format!("version {trimmed:?} must be dotted numeric"));
            }
            segment
                .parse::<u64>()
                .map_err(|_| format!("version {trimmed:?} contains an oversized segment"))?;
        }
        Ok(Self(trimmed.to_string()))
    }
}

impl fmt::Display for ManifestVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ManifestVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

impl Serialize for ManifestVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl Ord for ManifestVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        let mut left = self.0.split('.');
        let mut right = other.0.split('.');

        loop {
            match (left.next(), right.next()) {
                (Some(left), Some(right)) => {
                    let left = left.parse::<u64>().unwrap_or(0);
                    let right = right.parse::<u64>().unwrap_or(0);
                    match left.cmp(&right) {
                        Ordering::Equal => {}
                        ordering => return ordering,
                    }
                }
                (Some(left), None) => {
                    let left = left.parse::<u64>().unwrap_or(0);
                    if left == 0 {
                        continue;
                    }
                    return Ordering::Greater;
                }
                (None, Some(right)) => {
                    let right = right.parse::<u64>().unwrap_or(0);
                    if right == 0 {
                        continue;
                    }
                    return Ordering::Less;
                }
                (None, None) => return Ordering::Equal,
            }
        }
    }
}

impl PartialOrd for ManifestVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for ManifestVersion {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for ManifestVersion {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManifestUpdateCommit {
    pub(crate) agent: Agent,
    pub(crate) version: ManifestVersion,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) struct ManifestUpdateStatus {
    pub(crate) last_check_unix: Option<u64>,
    pub(crate) last_result: Option<String>,
    #[serde(default)]
    pub(crate) agents: BTreeMap<String, AgentRemoteStatus>,
}

impl ManifestUpdateStatus {
    pub(crate) fn agent_status(&self, agent: Agent) -> Option<AgentRemoteStatus> {
        self.agents.get(agent_label(agent)).cloned()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AgentRemoteStatus {
    pub(crate) cached_version: Option<String>,
    pub(crate) attempted_version: Option<String>,
    pub(crate) last_checked_unix: Option<u64>,
    pub(crate) last_result: String,
    pub(crate) last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManifestUpdateOutput {
    pub(crate) checked: Vec<Agent>,
    pub(crate) updated: Vec<ManifestUpdateCommit>,
    pub(crate) status: ManifestUpdateStatus,
}

pub(crate) fn check_and_update() -> Result<ManifestUpdateOutput, String> {
    check_and_update_local()
}

/// ao P0: offline replacement for the upstream remote-catalog check. The
/// catalog fetch of https://herdr.dev/agent-detection/index.toml is gone;
/// this validates the locally cached remote manifests (the state-dir cache
/// that overrides the bundled ones) and refreshes the persisted status.
/// Nothing can be newly downloaded here — agents are reported "current" when
/// their cache parses and "failed" otherwise.
fn check_and_update_local() -> Result<ManifestUpdateOutput, String> {
    let mut status = load_status();
    let check_time = now_unix();
    status.last_check_unix = Some(check_time);
    status.last_result = Some("checked".to_string());

    let checked = Agent::SCREEN_MANIFEST_AGENTS.to_vec();
    for agent in &checked {
        let agent_id = agent_label(*agent).to_string();
        if !remote_manifest_path(*agent).exists() {
            continue;
        }
        match fs::read_to_string(remote_manifest_path(*agent))
            .map_err(|err| err.to_string())
            .and_then(|content| {
                super::manifest::parse_remote_manifest_for_agent(*agent, &content)
                    .map(|parsed| parsed.version)
            }) {
            Ok(version) => {
                status.agents.insert(
                    agent_id,
                    AgentRemoteStatus {
                        cached_version: Some(version.to_string()),
                        attempted_version: None,
                        last_checked_unix: Some(check_time),
                        last_result: "current".to_string(),
                        last_error: None,
                    },
                );
            }
            Err(err) => {
                status.agents.insert(
                    agent_id,
                    AgentRemoteStatus {
                        cached_version: cached_remote_version(*agent)
                            .map(|version| version.to_string()),
                        attempted_version: None,
                        last_checked_unix: Some(check_time),
                        last_result: "failed".to_string(),
                        last_error: Some(err),
                    },
                );
            }
        }
    }

    if let Err(err) = save_status(&status) {
        tracing::warn!("failed to save agent detection manifest update status: {err}");
        status.last_result = Some(format!("failed_to_save_status: {err}"));
    }
    Ok(ManifestUpdateOutput {
        checked,
        updated: Vec::new(),
        status,
    })
}

fn process_agent_manifest(
    agent: Agent,
    content: &str,
    _check_time: u64,
) -> Result<Option<ManifestUpdateCommit>, String> {
    let parsed = super::manifest::parse_remote_manifest_for_agent(agent, content)?;
    if let Some(current) = cached_remote_version(agent) {
        match parsed.version.cmp(&current) {
            Ordering::Less => {
                return Err(format!(
                    "remote version {} is older than cached {current}",
                    parsed.version
                ))
            }
            Ordering::Equal => {
                let committed = fs::read_to_string(remote_manifest_path(agent)).unwrap_or_default();
                if committed != content {
                    return Err(format!(
                        "remote version {} changed content without a version bump",
                        parsed.version
                    ));
                }
                return Ok(None);
            }
            Ordering::Greater => {}
        }
    }

    commit_remote_manifest(agent, content)?;
    Ok(Some(ManifestUpdateCommit {
        agent,
        version: parsed.version,
    }))
}

pub(crate) fn load_status() -> ManifestUpdateStatus {
    let path = status_path();
    let Ok(content) = fs::read_to_string(&path) else {
        return ManifestUpdateStatus::default();
    };
    toml::from_str(&content).unwrap_or_else(|err| {
        tracing::warn!(
            path = %path.display(),
            "failed to parse agent detection manifest status: {err}"
        );
        ManifestUpdateStatus::default()
    })
}

fn save_status(status: &ManifestUpdateStatus) -> Result<(), String> {
    let path = status_path();
    let parent = path
        .parent()
        .ok_or_else(|| format!("status path {} has no parent", path.display()))?;
    fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    let content = toml::to_string_pretty(status).map_err(|err| err.to_string())?;
    atomic_write(&path, content.as_bytes())
}

pub(crate) fn status_path() -> PathBuf {
    state_root().join("status.toml")
}

pub(crate) fn remote_manifest_path(agent: Agent) -> PathBuf {
    state_root()
        .join("remote")
        .join(format!("{}.toml", agent_label(agent)))
}

pub(crate) fn cached_remote_version(agent: Agent) -> Option<ManifestVersion> {
    let content = fs::read_to_string(remote_manifest_path(agent)).ok()?;
    super::manifest::parse_remote_manifest_for_agent(agent, &content)
        .ok()
        .map(|parsed| parsed.version)
}

fn commit_remote_manifest(agent: Agent, content: &str) -> Result<(), String> {
    let path = remote_manifest_path(agent);
    let parent = path
        .parent()
        .ok_or_else(|| format!("remote manifest path {} has no parent", path.display()))?;
    fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    atomic_write(&path, content.as_bytes())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("path {} has no parent", path.display()))?;
    fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    let tmp_path = parent.join(format!(
        ".{}.{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("manifest"),
        std::process::id(),
        now_nanos()
    ));
    {
        let mut file = fs::File::create(&tmp_path).map_err(|err| err.to_string())?;
        if let Err(err) = file.write_all(bytes).and_then(|_| file.sync_all()) {
            let _ = fs::remove_file(&tmp_path);
            return Err(err.to_string());
        }
    }
    fs::rename(&tmp_path, path).map_err(|err| {
        let _ = fs::remove_file(&tmp_path);
        err.to_string()
    })?;
    if let Err(err) = sync_parent_dir(parent) {
        tracing::warn!(
            path = %path.display(),
            error = %err,
            "agent detection manifest committed but parent directory sync failed"
        );
    }
    Ok(())
}

fn sync_parent_dir(parent: &Path) -> Result<(), String> {
    let dir = match fs::File::open(parent) {
        Ok(dir) => dir,
        Err(err) if directory_sync_unsupported(&err) => return Ok(()),
        Err(err) => return Err(format!("failed to open parent directory for sync: {err}")),
    };
    match dir.sync_all() {
        Ok(()) => Ok(()),
        Err(err) if directory_sync_unsupported(&err) => Ok(()),
        Err(err) => Err(format!("failed to sync parent directory: {err}")),
    }
}

fn directory_sync_unsupported(err: &std::io::Error) -> bool {
    // PermissionDenied: Windows cannot open directories via `File::open`
    // (os error 5), so directory sync is effectively unsupported there.
    matches!(
        err.kind(),
        std::io::ErrorKind::Unsupported
            | std::io::ErrorKind::InvalidInput
            | std::io::ErrorKind::PermissionDenied
    )
}

fn state_root() -> PathBuf {
    crate::config::state_dir().join("agent-detection")
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn remote_manifest(version: &str, contains: &str) -> String {
        remote_manifest_for("codex", version, contains)
    }

    fn remote_manifest_for(agent: &str, version: &str, contains: &str) -> String {
        format!(
            r#"
id = "{agent}"
version = "{version}"
min_engine_version = 1
updated_at = "2026-06-10T12:00:00Z"

[[rules]]
id = "idle"
state = "idle"
contains = ["{contains}"]
"#
        )
    }

    fn with_state_dir<T>(name: &str, f: impl FnOnce() -> T) -> T {
        let _guard = crate::config::test_config_env_lock().lock().unwrap();
        let old_config = std::env::var_os("XDG_CONFIG_HOME");
        let old_state = std::env::var_os("XDG_STATE_HOME");
        let dir = std::env::temp_dir().join(format!(
            "herdr-manifest-update-{name}-{}",
            std::process::id()
        ));
        let config_dir = dir.join("config");
        let state_dir = dir.join("state");
        let _ = fs::remove_dir_all(&dir);
        std::env::set_var("XDG_CONFIG_HOME", &config_dir);
        std::env::set_var("XDG_STATE_HOME", &state_dir);
        crate::detect::manifest::reload_manifests();
        let result = f();
        match old_config {
            Some(value) => std::env::set_var("XDG_CONFIG_HOME", value),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        match old_state {
            Some(value) => std::env::set_var("XDG_STATE_HOME", value),
            None => std::env::remove_var("XDG_STATE_HOME"),
        }
        crate::detect::manifest::reload_manifests();
        let _ = fs::remove_dir_all(&dir);
        result
    }

    #[test]
    fn manifest_version_compares_dotted_numeric_segments() {
        assert!(
            ManifestVersion::parse("2026.6.10.1").unwrap()
                > ManifestVersion::parse("2026.6.9.9").unwrap()
        );
        assert!(ManifestVersion::parse("1.2.0").unwrap() == ManifestVersion::parse("1.2").unwrap());
        assert!(ManifestVersion::parse("1.2.1").unwrap() > ManifestVersion::parse("1.2").unwrap());
    }

    #[test]
    fn manifest_version_rejects_non_numeric_segments() {
        assert!(ManifestVersion::parse("").is_err());
        assert!(ManifestVersion::parse("2026.06.alpha").is_err());
        assert!(ManifestVersion::parse("2026..06").is_err());
        assert!(ManifestVersion::parse("2026.999999999999999999999999999999").is_err());
    }

    #[test]
    fn process_agent_manifest_commits_newer_manifest_atomically() {
        with_state_dir("commit-newer", || {
            let content = remote_manifest("9999.01.01.1", "ready");
            let commit = process_agent_manifest(Agent::Codex, &content, 1)
                .unwrap()
                .unwrap();

            assert_eq!(commit.agent, Agent::Codex);
            assert_eq!(
                commit.version,
                ManifestVersion::parse("9999.01.01.1").unwrap()
            );
            assert_eq!(
                fs::read_to_string(remote_manifest_path(Agent::Codex)).unwrap(),
                content
            );
        });
    }





    #[test]
    fn process_agent_manifest_rejects_downgrade_and_keeps_cached_manifest() {
        with_state_dir("reject-downgrade", || {
            let current = remote_manifest("9999.01.01.1", "current");
            process_agent_manifest(Agent::Codex, &current, 1).unwrap();

            let older = remote_manifest("9999.01.01.0", "older");
            assert!(process_agent_manifest(Agent::Codex, &older, 2).is_err());
            assert_eq!(
                fs::read_to_string(remote_manifest_path(Agent::Codex)).unwrap(),
                current
            );
        });
    }

    #[test]
    fn process_agent_manifest_rejects_equal_version_content_change() {
        with_state_dir("reject-equal-change", || {
            let current = remote_manifest("9999.01.01.1", "current");
            process_agent_manifest(Agent::Codex, &current, 1).unwrap();

            let changed = remote_manifest("9999.01.01.1", "changed");
            assert!(process_agent_manifest(Agent::Codex, &changed, 2).is_err());
            assert_eq!(
                fs::read_to_string(remote_manifest_path(Agent::Codex)).unwrap(),
                current
            );
        });
    }

    #[test]
    fn process_agent_manifest_skips_same_version_same_content() {
        with_state_dir("skip-same", || {
            let current = remote_manifest("9999.01.01.1", "current");
            process_agent_manifest(Agent::Codex, &current, 1).unwrap();

            let result = process_agent_manifest(Agent::Codex, &current, 2).unwrap();
            assert!(result.is_none());
        });
    }



    #[test]
    fn check_and_update_verifies_local_cache_without_network() {
        with_state_dir("offline-check", || {
            let content = remote_manifest("9999.01.01.1", "offline-ready");
            process_agent_manifest(Agent::Codex, &content, 1).unwrap();

            let output = check_and_update().unwrap();
            assert!(output.updated.is_empty());
            let codex = output.status.agent_status(Agent::Codex).unwrap();
            assert_eq!(codex.last_result, "current");
            assert_eq!(codex.cached_version.as_deref(), Some("9999.01.01.1"));
        });
    }
}
