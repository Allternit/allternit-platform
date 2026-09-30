//! ExecutionEnvironmentV1 resolution (kernel ABI 1.0.0, `environment.schema`).
//!
//! Every spawned node gets one resolved environment: a worktree/cwd, an env
//! allowlist (never inheriting the whole process env), the filesystem mounts it
//! may touch, and a network policy id. The resolved document is written next to
//! the session log and recorded in the ledger so a replay can show exactly what
//! the node ran with. The document is built as JSON conforming to the frozen
//! schema, so no contract type is redefined here.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde_json::{json, Value};

use crate::core::ids::create_event_id;
use crate::core::types::{Actor, ActorType, AllternitEvent, EventScope};

pub const ENV_RESOLVED_EVENT: &str = "ExecutionEnvironmentResolved";
/// Network policy every node gets unless a caller supplies its own id: only
/// public destinations, enforced by the shared egress guard (`crate::egress`).
pub const DEFAULT_NETWORK_POLICY_ID: &str = "net:public-only";
/// Env keys any node may see. Everything else must be granted explicitly.
pub const BASELINE_ENV_KEYS: &[&str] = &[
    "PATH", "HOME", "USER", "LOGNAME", "SHELL", "TERM", "LANG", "LC_ALL", "LC_CTYPE", "TZ", "TMPDIR", "COLORTERM",
];

pub struct EnvRequest<'a> {
    /// Node / session identifier the environment belongs to.
    pub node_id: &'a str,
    pub workdir: &'a Path,
    pub worktree: bool,
    /// Extra env keys granted to this node (beyond the baseline and `ALLTERNIT_*`).
    pub extra_env_keys: &'a [String],
    /// Extra filesystem mounts (absolute paths) besides the workdir.
    pub mounts: &'a [PathBuf],
    pub secret_refs: &'a [String],
    pub network_policy_id: Option<&'a str>,
}

fn valid_env_key(k: &str) -> bool {
    let mut c = k.chars();
    matches!(c.next(), Some(f) if f.is_ascii_uppercase() || f == '_')
        && c.all(|x| x.is_ascii_uppercase() || x.is_ascii_digit() || x == '_')
}

fn sanitize_id(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || "._:@-".contains(c) { c } else { '-' })
        .collect();
    if !out.chars().next().is_some_and(|c| c.is_ascii_alphanumeric()) {
        out.insert(0, 'x');
    }
    out.truncate(200);
    out
}

/// Resolve the environment document for one node.
pub fn resolve(req: &EnvRequest<'_>) -> Value {
    let mut keys: BTreeSet<String> = BASELINE_ENV_KEYS.iter().map(|s| s.to_string()).collect();
    for k in req.extra_env_keys {
        if valid_env_key(k) {
            keys.insert(k.clone());
        }
    }
    let workdir = req.workdir.to_string_lossy().to_string();
    let mut scope = vec![format!("fs:{workdir}")];
    for m in req.mounts {
        let r = format!("fs:{}", m.to_string_lossy());
        if !scope.contains(&r) {
            scope.push(r);
        }
    }
    json!({
        "schema_id": "allternit.kernel.ExecutionEnvironmentV1",
        "schema_version": "1.0.0",
        "environment_id": format!("env:{}", sanitize_id(req.node_id)),
        "allowed_env_keys": keys.into_iter().collect::<Vec<_>>(),
        "secret_refs": req.secret_refs.iter().map(|s| sanitize_id(s)).collect::<Vec<_>>(),
        "cwd": workdir,
        "tmp_scope": format!("{}/.allternit/tmp/{}", workdir.trim_end_matches('/'), sanitize_id(req.node_id)),
        "filesystem_scope": scope,
        "inherit_process_env": false,
        "network_policy_id": req.network_policy_id.unwrap_or(DEFAULT_NETWORK_POLICY_ID),
        "extensions": { "worktree": req.worktree },
    })
}

/// Whether `key` may pass into the node under `env` (`ALLTERNIT_*` always may).
pub fn key_allowed(env: &Value, key: &str) -> bool {
    key.starts_with("ALLTERNIT_")
        || env["allowed_env_keys"]
            .as_array()
            .is_some_and(|a| a.iter().any(|k| k.as_str() == Some(key)))
}

/// Filter a process environment down to the node's allowlist.
pub fn filter_env(env: &Value, process_env: impl IntoIterator<Item = (String, String)>) -> Vec<(String, String)> {
    process_env.into_iter().filter(|(k, _)| key_allowed(env, k)).collect()
}

/// Structural check against the frozen schema's required fields and patterns.
pub fn validate(env: &Value) -> Result<(), String> {
    for f in [
        "schema_id", "schema_version", "environment_id", "allowed_env_keys", "secret_refs", "cwd", "tmp_scope",
        "filesystem_scope", "inherit_process_env",
    ] {
        if env.get(f).is_none() {
            return Err(format!("missing {f}"));
        }
    }
    if env["schema_id"] != "allternit.kernel.ExecutionEnvironmentV1" {
        return Err("schema_id".into());
    }
    if env["inherit_process_env"] != Value::Bool(false) {
        return Err("inherit_process_env must be false".into());
    }
    let keys = env["allowed_env_keys"].as_array().ok_or("allowed_env_keys")?;
    if !keys.iter().all(|k| k.as_str().is_some_and(valid_env_key)) {
        return Err("bad env key".into());
    }
    Ok(())
}

pub fn resolved_event(wih_id: Option<&str>, env: &Value) -> AllternitEvent {
    AllternitEvent {
        event_id: create_event_id(),
        ts: Utc::now().to_rfc3339(),
        actor: Actor { r#type: ActorType::Gate, id: "exec-env".to_string() },
        scope: Some(EventScope { wih_id: wih_id.map(String::from), ..Default::default() }),
        r#type: ENV_RESOLVED_EVENT.to_string(),
        payload: env.clone(),
        provenance: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req<'a>(extra: &'a [String], mounts: &'a [PathBuf]) -> EnvRequest<'a> {
        EnvRequest {
            node_id: "ao-my slug",
            workdir: Path::new("/w/tree"),
            worktree: true,
            extra_env_keys: extra,
            mounts,
            secret_refs: &[],
            network_policy_id: None,
        }
    }

    #[test]
    fn resolves_a_valid_document_that_never_inherits_env() {
        let env = resolve(&req(&["MY_TOKEN".into(), "bad-key".into()], &[PathBuf::from("/data/ro")]));
        validate(&env).unwrap();
        assert_eq!(env["environment_id"], "env:ao-my-slug");
        assert_eq!(env["network_policy_id"], DEFAULT_NETWORK_POLICY_ID);
        assert_eq!(env["filesystem_scope"], json!(["fs:/w/tree", "fs:/data/ro"]));
        assert!(key_allowed(&env, "MY_TOKEN"));
        assert!(!key_allowed(&env, "bad-key"));
        assert_eq!(env["extensions"]["worktree"], true);
    }

    #[test]
    fn filter_drops_everything_not_allowlisted() {
        let env = resolve(&req(&[], &[]));
        let out = filter_env(
            &env,
            [("PATH", "/bin"), ("AWS_SECRET_ACCESS_KEY", "s"), ("ALLTERNIT_COMMRAILS_WIH", "w"), ("GITHUB_TOKEN", "t")]
                .map(|(k, v)| (k.to_string(), v.to_string())),
        );
        let keys: Vec<_> = out.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["PATH", "ALLTERNIT_COMMRAILS_WIH"]);
    }

    #[test]
    fn matches_the_conformance_example_shape() {
        let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../spec/Contracts/kernel/v1/conformance/examples/valid/ExecutionEnvironmentV1.json");
        let ex: Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
        validate(&ex).unwrap();
        let mine = resolve(&req(&[], &[]));
        for k in ex.as_object().unwrap().keys() {
            assert!(mine.get(k).is_some(), "resolved env lacks {k}");
        }
    }

    #[test]
    fn event_is_recorded_under_the_wih() {
        let env = resolve(&req(&[], &[]));
        let ev = resolved_event(Some("wih_1"), &env);
        assert_eq!(ev.r#type, ENV_RESOLVED_EVENT);
        assert_eq!(ev.scope.unwrap().wih_id.as_deref(), Some("wih_1"));
    }
}
