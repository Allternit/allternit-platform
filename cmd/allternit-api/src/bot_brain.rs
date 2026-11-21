//! Bot brain normalizer and bot kind (Harness & Bots 2.0, contract:
//! `CONTRACT-harness-bots-2.md`, "Bot record" and "Normalizer rule").
//!
//! A bot's brain lives in `agents.config.botBrain` (a JSON key, no column).
//! Old clients write `mode` / `harness` / `uhpHarnessId`; new clients also
//! write `runtime` (plus `access`, `autoSwitchedFrom`). Every save that carries
//! `botBrain` runs [`normalize_bot_brain`], so the record always has both
//! shapes and they agree:
//!
//! - `runtime` missing: derive it. `allternit_cloud` becomes `allternit`;
//!   `native_harness` + h becomes the runtime whose `legacy.harness` or id
//!   (or alias) is h, else `custom`; `uhp_harness` becomes the runtime whose
//!   `legacy.uhp` holds `uhpHarnessId`, else `custom`.
//! - `runtime` present: recompute `mode` / `harness` from it. A runtime this
//!   server does not know falls back to `native_harness` + that id (a newer
//!   client may know runtimes this build does not); `custom` keeps the stored
//!   `mode` / `harness` as they are.
//!
//! These are the rules of `resolveRuntime` in gizzi's
//! `cmd/gizzi-code/src/runtime/providers/discovery/runtime-registry.ts`, plus
//! the unknown-runtime fallback above. Every other key (`nativeSessionId`,
//! `modelRef`, `effort`, `access`, `autoSwitchedFrom`, anything newer) is
//! kept. `config.agentView` is its own top-level key, so a `botBrain`-only
//! save never touches it (see [`merge_config`]).
//!
//! `botKind` is computed, never stored: `connected` when the bot has a vendor
//! execution binding (`bot_execution_bindings.type = 'vendor'`, the wire name
//! that stays `vendor`), else `allternit`.

use std::collections::HashSet;

use rusqlite::{params, Connection};
use serde_json::Value;

use crate::cli_catalog::{runtime_registry, RuntimeRegistry};

pub(crate) const MODE_CLOUD: &str = "allternit_cloud";
pub(crate) const MODE_NATIVE: &str = "native_harness";
pub(crate) const MODE_UHP: &str = "uhp_harness";
pub(crate) const RUNTIME_ALLTERNIT: &str = "allternit";
pub(crate) const RUNTIME_CUSTOM: &str = "custom";

pub(crate) const BOT_KIND_ALLTERNIT: &str = "allternit";
pub(crate) const BOT_KIND_CONNECTED: &str = "connected";

fn str_field<'a>(map: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a str> {
    map.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// Fill in the missing half of a `botBrain` object in place (see the module
/// docs for the rule). A value that is not an object, or an empty object, is
/// left alone.
pub(crate) fn normalize_bot_brain(brain: &mut Value, registry: &RuntimeRegistry) {
    let Some(map) = brain.as_object_mut() else {
        return;
    };
    if map.is_empty() {
        return;
    }

    if let Some(runtime) = str_field(map, "runtime").map(str::to_string) {
        match registry.get(&runtime) {
            Some(entry) if entry.id == RUNTIME_ALLTERNIT => {
                map.remove("harness");
                map.remove("uhpHarnessId");
                map.insert("mode".into(), MODE_CLOUD.into());
                map.insert("runtime".into(), entry.id.clone().into());
            }
            Some(entry) if entry.adapter.kind == "unsupported" => {
                let uhp = entry
                    .legacy_uhp()
                    .first()
                    .cloned()
                    .unwrap_or_else(|| entry.id.clone());
                map.insert("mode".into(), MODE_UHP.into());
                map.insert("uhpHarnessId".into(), uhp.into());
                map.insert("runtime".into(), entry.id.clone().into());
            }
            Some(entry) => {
                let harness = entry.legacy_harness().unwrap_or(entry.id.as_str()).to_string();
                map.insert("mode".into(), MODE_NATIVE.into());
                map.insert("harness".into(), harness.into());
                map.insert("runtime".into(), entry.id.clone().into());
            }
            // `custom` is what derivation writes for a harness the registry
            // has no entry for: the stored mode/harness are the truth.
            None if runtime == RUNTIME_CUSTOM => {}
            None => {
                map.insert("mode".into(), MODE_NATIVE.into());
                map.insert("harness".into(), runtime.into());
            }
        }
        return;
    }

    let derived = match str_field(map, "mode") {
        Some(MODE_NATIVE) if str_field(map, "harness").is_some() => {
            let harness = str_field(map, "harness").unwrap_or_default();
            registry
                .runtimes
                .iter()
                .find(|r| r.legacy_harness() == Some(harness))
                .or_else(|| registry.get(harness))
                .map(|r| r.id.clone())
                .unwrap_or_else(|| RUNTIME_CUSTOM.to_string())
        }
        Some(MODE_UHP) => str_field(map, "uhpHarnessId")
            .and_then(|uhp| {
                registry
                    .runtimes
                    .iter()
                    .find(|r| r.legacy_uhp().iter().any(|u| u == uhp))
            })
            .map(|r| r.id.clone())
            .unwrap_or_else(|| RUNTIME_CUSTOM.to_string()),
        _ => RUNTIME_ALLTERNIT.to_string(),
    };
    map.insert("runtime".into(), derived.into());
}

/// Normalize `config.botBrain` (when present) against the embedded registry.
pub(crate) fn normalize_config(config: &mut Value) {
    if let Some(brain) = config.get_mut("botBrain") {
        normalize_bot_brain(brain, runtime_registry());
    }
}

/// The agent config merge used by `PUT`/`PATCH /agents/:id`: the patch is
/// merged over the stored config one top-level key at a time (never a
/// wholesale replace), and a patch that carries `botBrain` is normalized.
/// `agentView` and every other stored key survive a `botBrain`-only save.
pub(crate) fn merge_config(stored: Option<Value>, patch: Option<Value>) -> Option<Value> {
    let patch_has_brain = patch.as_ref().is_some_and(|p| p.get("botBrain").is_some());
    let mut merged = match (stored, patch) {
        (Some(Value::Object(mut stored)), Some(Value::Object(patch))) => {
            stored.extend(patch);
            Some(Value::Object(stored))
        }
        (stored, None) => stored,
        (_, body) => body,
    };
    if patch_has_brain {
        if let Some(config) = merged.as_mut() {
            normalize_config(config);
        }
    }
    merged
}

/// `botKind` for one agent: `connected` when it has a vendor execution
/// binding owned by `owner`. A store error reads as `allternit`.
pub(crate) fn bot_kind(conn: &Connection, agent_id: &str, owner: &str) -> &'static str {
    let connected: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM bot_execution_bindings WHERE bot_id = ?1 AND owner = ?2 AND type = 'vendor')",
            params![agent_id, owner],
            |r| r.get(0),
        )
        .unwrap_or(false);
    kind_for(connected)
}

/// Every agent id of `owner` with a vendor execution binding (one query for
/// the agent list).
pub(crate) fn connected_bot_ids(conn: &Connection, owner: &str) -> HashSet<String> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT DISTINCT bot_id FROM bot_execution_bindings WHERE owner = ?1 AND type = 'vendor'",
    ) else {
        return HashSet::new();
    };
    stmt.query_map(params![owner], |r| r.get::<_, String>(0))
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

pub(crate) fn kind_for(connected: bool) -> &'static str {
    if connected {
        BOT_KIND_CONNECTED
    } else {
        BOT_KIND_ALLTERNIT
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli_catalog::parse_runtime_registry;
    use serde_json::json;

    fn normalized(brain: Value) -> Value {
        let mut brain = brain;
        normalize_bot_brain(&mut brain, runtime_registry());
        brain
    }

    #[test]
    fn bot_brain_old_shape_gets_runtime() {
        assert_eq!(normalized(json!({"mode": "allternit_cloud"}))["runtime"], "allternit");
        let native = normalized(json!({"mode": "native_harness", "harness": "claude", "nativeSessionId": "s-1"}));
        assert_eq!(native["runtime"], "claude");
        assert_eq!(native["harness"], "claude");
        assert_eq!(native["nativeSessionId"], "s-1");
        // An alias resolves to the canonical runtime id.
        assert_eq!(normalized(json!({"mode": "native_harness", "harness": "agy"}))["runtime"], "antigravity");
        // A harness the registry has no entry for keeps its harness, runtime `custom`.
        let unknown = normalized(json!({"mode": "native_harness", "harness": "not-a-runtime"}));
        assert_eq!(unknown["runtime"], "custom");
        assert_eq!(unknown["harness"], "not-a-runtime");
        assert_eq!(normalized(json!({"mode": "uhp_harness", "uhpHarnessId": "nope"}))["runtime"], "custom");
    }

    #[test]
    fn bot_brain_new_shape_gets_mode_and_harness() {
        let codex = normalized(json!({"runtime": "codex", "access": "plan", "modelRef": "openai/gpt-5.5"}));
        assert_eq!(codex["mode"], "native_harness");
        assert_eq!(codex["harness"], "codex");
        assert_eq!(codex["access"], "plan");
        assert_eq!(codex["modelRef"], "openai/gpt-5.5");

        let cloud = normalized(json!({"runtime": "allternit", "mode": "native_harness", "harness": "claude"}));
        assert_eq!(cloud["mode"], "allternit_cloud");
        assert!(cloud.get("harness").is_none(), "{cloud}");

        // An alias is stored as the canonical id.
        let agy = normalized(json!({"runtime": "agy", "autoSwitchedFrom": "claude"}));
        assert_eq!(agy["runtime"], "antigravity");
        assert_eq!(agy["harness"], "antigravity");
        assert_eq!(agy["autoSwitchedFrom"], "claude");

        // A runtime with no adapter runs through UHP.
        let dsh = normalized(json!({"runtime": "dsh"}));
        assert_eq!(dsh["mode"], "uhp_harness");
        assert_eq!(dsh["uhpHarnessId"], "dsh");
    }

    #[test]
    fn bot_brain_legacy_ids_map_both_ways() {
        let registry = parse_runtime_registry(
            &json!({"version": 3, "runtimes": [
                {"id": "allternit", "name": "Allternit", "vendor": "Allternit", "category": "native", "tool": null,
                 "adapter": {"kind": "native"}, "brains": "any", "access": [], "more": false,
                 "systemLayer": {"via": "none"}, "look": {"default": "terminal"}},
                {"id": "newname", "name": "New", "vendor": "V", "category": "coding", "tool": "x",
                 "adapter": {"kind": "acp"}, "brains": "any", "access": [], "more": false,
                 "systemLayer": {"via": "acp-session"}, "look": {"default": "terminal"},
                 "legacy": {"harness": "oldname", "uhp": ["uhp-old"]}}
            ]})
            .to_string(),
        );
        let mut old = json!({"mode": "native_harness", "harness": "oldname"});
        normalize_bot_brain(&mut old, &registry);
        assert_eq!(old["runtime"], "newname");
        let mut uhp = json!({"mode": "uhp_harness", "uhpHarnessId": "uhp-old"});
        normalize_bot_brain(&mut uhp, &registry);
        assert_eq!(uhp["runtime"], "newname");
        let mut new = json!({"runtime": "newname"});
        normalize_bot_brain(&mut new, &registry);
        assert_eq!(new["mode"], "native_harness");
        assert_eq!(new["harness"], "oldname");
    }

    #[test]
    fn bot_brain_unknown_runtime_falls_back_to_native_harness() {
        let brain = normalized(json!({"runtime": "future-cli", "mode": "allternit_cloud"}));
        assert_eq!(brain["mode"], "native_harness");
        assert_eq!(brain["harness"], "future-cli");
        assert_eq!(brain["runtime"], "future-cli");
        // `custom` never overwrites the harness it was derived from.
        let custom = normalized(json!({"runtime": "custom", "mode": "native_harness", "harness": "my-cli"}));
        assert_eq!(custom["mode"], "native_harness");
        assert_eq!(custom["harness"], "my-cli");
    }

    #[test]
    fn bot_brain_empty_or_absent_is_left_alone() {
        assert_eq!(normalized(json!({})), json!({}));
        assert_eq!(normalized(Value::Null), Value::Null);
        let mut config = json!({"isBot": true, "agentView": {"view": "runtime", "look": "terminal"}});
        let before = config.clone();
        normalize_config(&mut config);
        assert_eq!(config, before);
        // A save without botBrain does not touch the stored one.
        let merged = merge_config(
            Some(json!({"botBrain": {"mode": "native_harness", "harness": "claude"}})),
            Some(json!({"avatar": {"type": "mascot"}})),
        )
        .unwrap();
        assert!(merged["botBrain"].get("runtime").is_none(), "{merged}");
    }

    #[test]
    fn bot_brain_agent_view_survives_bot_brain_only_merge() {
        let stored = json!({
            "isBot": true,
            "agentView": {"view": "split", "look": "desktop"},
            "botBrain": {"mode": "native_harness", "harness": "codex", "runtime": "codex", "access": "plan"},
        });
        // An old client saves only botBrain, in the old shape.
        let patch = json!({"botBrain": {"mode": "native_harness", "harness": "claude"}});
        let merged = merge_config(Some(stored), Some(patch)).unwrap();
        assert_eq!(merged["agentView"], json!({"view": "split", "look": "desktop"}));
        assert_eq!(merged["isBot"], true);
        assert_eq!(merged["botBrain"]["harness"], "claude");
        assert_eq!(merged["botBrain"]["runtime"], "claude");
    }

    #[test]
    fn bot_kind_is_connected_only_with_a_vendor_binding() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE bot_execution_bindings (id TEXT PRIMARY KEY, owner TEXT NOT NULL, bot_id TEXT NOT NULL, type TEXT NOT NULL DEFAULT 'vendor');
             INSERT INTO bot_execution_bindings VALUES ('b1', 'user-a', 'bot-v', 'vendor');
             INSERT INTO bot_execution_bindings VALUES ('b2', 'user-a', 'bot-n', 'allternit');",
        )
        .unwrap();
        assert_eq!(bot_kind(&conn, "bot-v", "user-a"), "connected");
        assert_eq!(bot_kind(&conn, "bot-n", "user-a"), "allternit");
        assert_eq!(bot_kind(&conn, "bot-v", "user-b"), "allternit");
        assert_eq!(bot_kind(&conn, "bot-none", "user-a"), "allternit");
        assert_eq!(connected_bot_ids(&conn, "user-a"), HashSet::from(["bot-v".to_string()]));
        // No table (an old store) reads as allternit, never an error.
        let empty = Connection::open_in_memory().unwrap();
        assert_eq!(bot_kind(&empty, "bot-v", "user-a"), "allternit");
        assert!(connected_bot_ids(&empty, "user-a").is_empty());
    }
}
