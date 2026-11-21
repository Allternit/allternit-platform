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

use crate::cli_catalog::{runtime_registry, RuntimeEntry, RuntimeRegistry};

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

// ─── Plan rules and save validation (contract: "Backend gaps 1-3") ────────────
//
// The same rule as `fits` / `allowed` in gizzi's `runtime-registry.ts`; web,
// iOS and Android mirror it. Validation runs only when the request sets
// `botBrain.runtime` itself: old-shape saves are normalized, never rejected.

/// A 422 body for a bot save: `{ error: code, message }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BrainRejection {
    pub code: &'static str,
    pub message: String,
}

impl BrainRejection {
    pub(crate) fn body(&self) -> Value {
        serde_json::json!({ "error": self.code, "message": self.message })
    }
}

pub(crate) const RUNTIME_UNKNOWN: &str = "runtime_unknown";
pub(crate) const BRAIN_NOT_FIT: &str = "brain_not_fit";
pub(crate) const PLAN_NOT_ALLOWED: &str = "plan_not_allowed_on_runtime";
pub(crate) const RUNTIME_NOT_INSTALLED: &str = "runtime_not_installed";

/// A runtime's own CLI: its `tool`, its id, its aliases, the tool's aliases.
fn is_own_cli(entry: &RuntimeEntry, provider: &str) -> bool {
    let Some(tool) = entry.tool.as_deref() else {
        return false;
    };
    provider == tool
        || provider == entry.id
        || entry.aliases.as_deref().unwrap_or(&[]).iter().any(|a| a == provider)
        || crate::cli_catalog::find(tool).is_some_and(|t| t.aliases.iter().any(|a| a == provider))
}

/// Brain fits runtime: `brains == "any"`, the provider (a `subs-` prefix
/// stripped) is listed, or it is the runtime's own CLI.
pub(crate) fn fits(entry: &RuntimeEntry, provider: &str) -> bool {
    if entry.brains.as_str() == Some("any") {
        return true;
    }
    let p = provider.strip_prefix("subs-").unwrap_or(provider);
    let listed = entry
        .brains
        .as_array()
        .is_some_and(|list| list.iter().any(|b| b.as_str() == Some(p)));
    listed || is_own_cli(entry, p)
}

/// The subscription provider whose plan a brain uses, if it is a plan:
/// `subs-<p>` (vendor ids map to their plan: anthropic → claude, …), or a CLI
/// sign-in whose runtime holds exactly one plan (`claude-cli` → claude,
/// `codex-cli` → chatgpt, `kimi-cli` → kimi, `antigravity` → gemini,
/// `copilot` → copilot).
pub(crate) fn plan_provider(provider: &str, registry: &RuntimeRegistry) -> Option<String> {
    if let Some(rest) = provider.strip_prefix("subs-") {
        let plan = match rest {
            "anthropic" => "claude",
            "openai" => "chatgpt",
            "google" => "gemini",
            "moonshot" => "kimi",
            "github" => "copilot",
            other => other,
        };
        return Some(plan.to_string());
    }
    registry
        .runtimes
        .iter()
        .find(|r| r.tool.as_deref() == Some(provider) && r.plans.len() == 1)
        .map(|r| r.plans[0].clone())
}

/// `fits`, and a plan brain only where the runtime's `plans` list its
/// provider. A runtime's own CLI sign-in always passes.
pub(crate) fn allowed(entry: &RuntimeEntry, provider: &str, registry: &RuntimeRegistry) -> bool {
    if !fits(entry, provider) {
        return false;
    }
    match plan_provider(provider, registry) {
        None => true,
        Some(plan) => is_own_cli(entry, provider) || entry.plans.iter().any(|p| *p == plan),
    }
}

/// A subscription provider's display name (ChatGPT, Claude, Kimi, Gemini, Copilot).
fn plan_name(plan: &str) -> String {
    crate::subscription_routes::provider_disclosure(plan)
        .map(|p| p.name.to_string())
        .unwrap_or_else(|| {
            let mut c = plan.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
        })
}

/// The `provider` of a `provider/model` ref, when it has one.
fn ref_provider(model_ref: &str) -> Option<&str> {
    let (p, m) = model_ref.split_once('/')?;
    (!p.is_empty() && !m.is_empty()).then_some(p)
}

/// The provider of `botBrain.modelRef`: a `provider/model` string, or an
/// object `{providerID, modelID}` (web clients write that shape).
fn brain_ref_provider(brain: &serde_json::Map<String, Value>) -> Option<&str> {
    match brain.get("modelRef")? {
        Value::String(s) => ref_provider(s),
        Value::Object(o) => str_field(o, "providerID").or_else(|| str_field(o, "providerId")),
        _ => None,
    }
}

/// Check a bot create (and the changed part of an update, see
/// [`validate_bot_brain_change`]). Returns the 422 to send, or `Ok` — always `Ok` when the
/// request's `botBrain` has no `runtime` of its own (old shape) or the runtime
/// is `custom`. The brain is `botBrain.modelRef` (string or object), else a
/// `provider/model` `model` in the same request; a brain without a provider prefix is not judged.
pub(crate) fn validate_explicit_runtime(
    config: Option<&Value>,
    model: Option<&str>,
    registry: &RuntimeRegistry,
) -> Result<(), BrainRejection> {
    let Some(brain) = config.and_then(|c| c.get("botBrain")).and_then(Value::as_object) else {
        return Ok(());
    };
    let Some(runtime_id) = str_field(brain, "runtime") else {
        return Ok(());
    };
    if runtime_id == RUNTIME_CUSTOM {
        return Ok(());
    }
    let Some(entry) = registry.get(runtime_id) else {
        return Err(BrainRejection {
            code: RUNTIME_UNKNOWN,
            message: format!("No runtime called {runtime_id}."),
        });
    };
    let provider = brain_ref_provider(brain).or_else(|| model.and_then(ref_provider));
    let Some(provider) = provider else {
        return Ok(());
    };
    if !fits(entry, provider) {
        return Err(BrainRejection {
            code: BRAIN_NOT_FIT,
            message: format!("{} runs {} models only.", entry.name, entry.vendor),
        });
    }
    if !allowed(entry, provider, registry) {
        let plan = plan_provider(provider, registry).unwrap_or_default();
        let own = registry
            .runtimes
            .iter()
            .find(|r| r.id != RUNTIME_ALLTERNIT && r.plans.iter().any(|p| *p == plan))
            .map(|r| r.name.clone())
            .unwrap_or_else(|| "Allternit".to_string());
        return Err(BrainRejection {
            code: PLAN_NOT_ALLOWED,
            message: format!(
                "{}'s plan can't be used inside {}. Use an API key, or pick {own}.",
                plan_name(&plan),
                entry.name
            ),
        });
    }
    Ok(())
}

/// Check a bot update (PUT/PATCH) against the stored record. The rules run
/// only when the request's `botBrain.runtime` or brain (the `modelRef`
/// provider, else a `provider/model` `model`) differs from the stored,
/// normalized botBrain: an existing bot that keeps its runtime and brain is
/// never rejected, even when the pair would fail today's rules (bots saved
/// before the rules, and clients that send the whole config back on a
/// rename). A request with no brain keeps the stored one. A stored record with
/// no botBrain is checked like a create.
pub(crate) fn validate_bot_brain_change(
    config: Option<&Value>,
    model: Option<&str>,
    stored_config: Option<&Value>,
    stored_model: Option<&str>,
    registry: &RuntimeRegistry,
) -> Result<(), BrainRejection> {
    let Some(brain) = config.and_then(|c| c.get("botBrain")).and_then(Value::as_object) else {
        return Ok(());
    };
    let Some(runtime_id) = str_field(brain, "runtime") else {
        return Ok(());
    };
    let stored_brain = stored_config
        .and_then(|c| c.get("botBrain"))
        .filter(|b| b.as_object().is_some_and(|m| !m.is_empty()));
    if let Some(stored_brain) = stored_brain {
        let mut stored_brain = stored_brain.clone();
        normalize_bot_brain(&mut stored_brain, registry);
        let canon = |id: &str| registry.get(id).map(|r| r.id.clone()).unwrap_or_else(|| id.to_string());
        let same_runtime = stored_brain.get("runtime").and_then(Value::as_str).map(canon) == Some(canon(runtime_id));
        let asked = brain_ref_provider(brain).or_else(|| model.and_then(ref_provider));
        let stored = stored_brain
            .as_object()
            .and_then(brain_ref_provider)
            .or_else(|| stored_model.and_then(ref_provider));
        let same_brain = asked.is_none() || asked == stored;
        if same_runtime && same_brain {
            return Ok(());
        }
    }
    validate_explicit_runtime(config, model, registry)
}

/// `warnings` for a successful bot save: `runtime_not_installed` when the
/// request's `botBrain` (normalized) runs on a runtime other than `allternit`
/// whose CLI `installed` says is not on this computer. `installed` gets the
/// catalog tool id (the check `GET /api/v1/runtimes` uses: the tool's binary
/// on PATH). No `botBrain` in the request, or an unknown runtime: no warning.
pub(crate) fn install_warnings(
    config: Option<&Value>,
    registry: &RuntimeRegistry,
    installed: impl Fn(&str) -> bool,
) -> Vec<Value> {
    let Some(brain) = config.and_then(|c| c.get("botBrain")) else {
        return Vec::new();
    };
    let mut brain = brain.clone();
    normalize_bot_brain(&mut brain, registry);
    let Some(entry) = brain
        .get("runtime")
        .and_then(Value::as_str)
        .and_then(|id| registry.get(id))
    else {
        return Vec::new();
    };
    let Some(tool) = entry.tool.as_deref() else {
        return Vec::new();
    };
    if entry.id == RUNTIME_ALLTERNIT || installed(tool) {
        return Vec::new();
    }
    vec![serde_json::json!({
        "code": RUNTIME_NOT_INSTALLED,
        "runtime": entry.id,
        "message": format!(
            "{} isn't installed on this computer. The bot will fail to start until it is.",
            entry.name
        ),
    })]
}

/// The live install check: the catalog tool's binary is on PATH (the same
/// test `provider_routes::subprocess_provider_rows` uses for `installed`).
pub(crate) fn tool_installed(tool: &str) -> bool {
    crate::cli_catalog::find(tool)
        .is_some_and(|t| crate::provider_routes::command_on_path(&t.bin).is_some())
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

    fn check(config: Value) -> Result<(), BrainRejection> {
        validate_explicit_runtime(Some(&config), None, runtime_registry())
    }

    #[test]
    fn bot_save_unknown_runtime_is_422() {
        let err = check(json!({"botBrain": {"runtime": "not-a-runtime"}})).unwrap_err();
        assert_eq!(err.code, "runtime_unknown");
        assert_eq!(err.message, "No runtime called not-a-runtime.");
        assert_eq!(err.body()["error"], "runtime_unknown");
    }

    #[test]
    fn bot_save_brain_not_fit_is_422() {
        let err = check(json!({"botBrain": {"runtime": "claude", "modelRef": "openai/gpt-5.5"}})).unwrap_err();
        assert_eq!(err.code, "brain_not_fit");
        assert_eq!(err.message, "Claude Code runs Anthropic models only.");
        // The request's `provider/model` counts when botBrain has no modelRef.
        let config = json!({"botBrain": {"runtime": "codex"}});
        let err = validate_explicit_runtime(Some(&config), Some("anthropic/claude-sonnet-5-5"), runtime_registry())
            .unwrap_err();
        assert_eq!(err.code, "brain_not_fit");
    }

    #[test]
    fn bot_save_plan_on_foreign_runtime_is_422() {
        let err = check(json!({"botBrain": {"runtime": "hermes", "modelRef": "subs-claude/claude-sonnet-5-5"}}))
            .unwrap_err();
        assert_eq!(err.code, "plan_not_allowed_on_runtime");
        assert_eq!(
            err.message,
            "Claude's plan can't be used inside Hermes. Use an API key, or pick Claude Code."
        );
        // A CLI plan sign-in is a plan too.
        let err = check(json!({"botBrain": {"runtime": "opencode", "modelRef": "codex-cli/gpt-5.5"}})).unwrap_err();
        assert_eq!(err.code, "plan_not_allowed_on_runtime");
        assert!(err.message.starts_with("ChatGPT's plan can't be used inside OpenCode."), "{}", err.message);
    }

    #[test]
    fn bot_save_allowed_brains_pass() {
        for (runtime, model_ref) in [
            ("claude", "subs-claude/claude-sonnet-5-5"),
            ("claude", "claude-cli/claude-sonnet-5-5"),
            ("codex", "codex-cli/gpt-5.5"),
            ("antigravity", "subs-gemini/gemini-3-pro"),
            ("allternit", "subs-chatgpt/gpt-5.5"),
            ("allternit", "claude-cli/claude-opus-5-5"),
            // An API key brain fits any "any" runtime.
            ("hermes", "anthropic/claude-sonnet-5-5"),
            ("opencode", "openrouter/qwen/qwen3-coder"),
        ] {
            assert_eq!(
                check(json!({"botBrain": {"runtime": runtime, "modelRef": model_ref}})),
                Ok(()),
                "{runtime} + {model_ref}"
            );
        }
        // The object form of modelRef is read the same way.
        let err = check(json!({"botBrain": {"runtime": "hermes",
            "modelRef": {"providerID": "subs-kimi", "modelID": "kimi-k3"}}}))
        .unwrap_err();
        assert_eq!(err.code, "plan_not_allowed_on_runtime");
        assert!(err.message.ends_with("Use an API key, or pick Kimi Code."), "{}", err.message);
        // A brain with no provider prefix is not judged.
        assert_eq!(check(json!({"botBrain": {"runtime": "claude", "modelRef": "gpt-5.5"}})), Ok(()));
    }

    #[test]
    fn bot_save_old_shape_is_never_rejected() {
        // No explicit runtime: normalized on save, never a 422, whatever the brain.
        assert_eq!(
            check(json!({"botBrain": {"mode": "native_harness", "harness": "hermes", "modelRef": "subs-claude/x"}})),
            Ok(())
        );
        assert_eq!(check(json!({"botBrain": {"mode": "native_harness", "harness": "not-a-runtime"}})), Ok(()));
        assert_eq!(check(json!({"botBrain": {"runtime": "", "mode": "allternit_cloud"}})), Ok(()));
        assert_eq!(check(json!({"avatar": {"type": "mascot"}})), Ok(()));
        assert_eq!(validate_explicit_runtime(None, Some("openai/gpt-5.5"), runtime_registry()), Ok(()));
        // `custom` is what derivation writes; echoing it back is fine.
        assert_eq!(check(json!({"botBrain": {"runtime": "custom", "harness": "my-cli"}})), Ok(()));
    }

    #[test]
    fn bot_save_unchanged_disallowed_combo_is_not_rejected() {
        // Saved before the rules: Hermes on a Claude plan. A rename that sends
        // the whole config back keeps runtime + brain, so it is never a 422.
        let stored = json!({"botBrain": {"runtime": "hermes", "mode": "native_harness", "harness": "hermes",
            "modelRef": "subs-claude/claude-sonnet-5-5"}});
        let request = stored.clone();
        assert_eq!(validate_bot_brain_change(Some(&request), None, Some(&stored), None, runtime_registry()), Ok(()));
        // Old-shape stored record, alias runtime, object modelRef, same provider: unchanged.
        let old = json!({"botBrain": {"mode": "native_harness", "harness": "hermes", "modelRef": "subs-claude/x"}});
        let request = json!({"botBrain": {"runtime": "hermes",
            "modelRef": {"providerID": "subs-claude", "modelID": "claude-opus-5-5"}}});
        assert_eq!(validate_bot_brain_change(Some(&request), None, Some(&old), None, runtime_registry()), Ok(()));
        // No brain in the request keeps the stored one.
        let request = json!({"botBrain": {"runtime": "hermes"}});
        assert_eq!(validate_bot_brain_change(Some(&request), None, Some(&stored), None, runtime_registry()), Ok(()));
        // The stored brain may live in the agent's `model` column.
        let stored_bare = json!({"botBrain": {"runtime": "hermes"}});
        let request = json!({"botBrain": {"runtime": "hermes"}});
        assert_eq!(
            validate_bot_brain_change(Some(&request), Some("subs-claude/x"), Some(&stored_bare), Some("subs-claude/x"), runtime_registry()),
            Ok(())
        );
    }

    #[test]
    fn bot_save_changed_to_disallowed_plan_is_422() {
        // Brain changes to a plan the runtime can't use.
        let stored = json!({"botBrain": {"runtime": "hermes", "modelRef": "anthropic/claude-sonnet-5-5"}});
        let request = json!({"botBrain": {"runtime": "hermes", "modelRef": "subs-claude/claude-sonnet-5-5"}});
        let err = validate_bot_brain_change(Some(&request), None, Some(&stored), None, runtime_registry()).unwrap_err();
        assert_eq!(err.code, "plan_not_allowed_on_runtime");
        // Runtime changes under an existing plan brain.
        let stored = json!({"botBrain": {"runtime": "claude", "modelRef": "subs-claude/claude-sonnet-5-5"}});
        let request = json!({"botBrain": {"runtime": "hermes", "modelRef": "subs-claude/claude-sonnet-5-5"}});
        let err = validate_bot_brain_change(Some(&request), None, Some(&stored), None, runtime_registry()).unwrap_err();
        assert_eq!(err.message, "Claude's plan can't be used inside Hermes. Use an API key, or pick Claude Code.");
        // A record with no botBrain yet is checked like a create.
        let err = validate_bot_brain_change(Some(&request), None, Some(&json!({"isBot": true})), None, runtime_registry())
            .unwrap_err();
        assert_eq!(err.code, "plan_not_allowed_on_runtime");
        // An unknown runtime is still refused when it is new.
        let request = json!({"botBrain": {"runtime": "not-a-runtime"}});
        let err = validate_bot_brain_change(Some(&request), None, Some(&stored), None, runtime_registry()).unwrap_err();
        assert_eq!(err.code, "runtime_unknown");
    }

    #[test]
    fn bot_save_install_warning_only_when_cli_missing() {
        let config = json!({"botBrain": {"runtime": "opencode"}});
        let warnings = install_warnings(Some(&config), runtime_registry(), |_| false);
        assert_eq!(
            warnings,
            vec![json!({
                "code": "runtime_not_installed",
                "runtime": "opencode",
                "message": "OpenCode isn't installed on this computer. The bot will fail to start until it is.",
            })]
        );
        // The check gets the catalog tool id.
        let asked = std::cell::RefCell::new(Vec::new());
        assert!(install_warnings(Some(&config), runtime_registry(), |tool| {
            asked.borrow_mut().push(tool.to_string());
            true
        })
        .is_empty());
        assert_eq!(asked.into_inner(), vec!["opencode".to_string()]);
        // Old shape is normalized first: native_harness + claude warns for Claude Code.
        let old = json!({"botBrain": {"mode": "native_harness", "harness": "claude"}});
        assert_eq!(install_warnings(Some(&old), runtime_registry(), |_| false)[0]["runtime"], "claude");
        // Allternit, no botBrain, or an unknown runtime: no warning.
        let cloud = json!({"botBrain": {"runtime": "allternit"}});
        assert!(install_warnings(Some(&cloud), runtime_registry(), |_| false).is_empty());
        assert!(install_warnings(Some(&json!({"isBot": true})), runtime_registry(), |_| false).is_empty());
        let custom = json!({"botBrain": {"runtime": "custom", "harness": "my-cli"}});
        assert!(install_warnings(Some(&custom), runtime_registry(), |_| false).is_empty());
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
