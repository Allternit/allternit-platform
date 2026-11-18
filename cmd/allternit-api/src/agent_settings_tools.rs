//! Settings (Primitives Wave D, D2): gizzi's `settings` tool.
//!
//! Keys and their risk class come from `contracts/settings/definitions.json`
//! (decision 12). The risk gate is enforced here, on the server:
//!
//! * `safe` account keys are applied through the existing preference handlers
//!   (`PUT /agent-preferences`, `PUT /cowork-preferences`, called in process
//!   as the session's owner, so their checks and side effects run as for a
//!   Settings save; no new storage). `safe` device and runtime keys come back
//!   `apply_on_device` for the client that sent the turn (decision 13).
//! * `confirm` keys (security, privacy, instructions) are never applied here:
//!   they come back `needs_confirmation`, and the user's own tap applies them
//!   under their sign-in. A run an automation started may not even ask: those
//!   come back `invalid` with code `automation_run`.
//! * `locked` keys (money, account) come back `locked` with the Settings link.
//! * An unknown key or a value the key does not take comes back `invalid`.
//!
//! Only [`account_field`] maps a key to a preference write, and it names safe
//! keys only; the tests check that no `confirm` or `locked` key has a writer.
//!
//! Whose settings they are comes from the session id ([`resolve_owner`]),
//! never from the model.
//!
//! * `POST /bot-routing/settings/read` `{sessionId, keys?}` →
//!   `{settings: [definition + value?], unknown: [key]}`.
//! * `POST /bot-routing/settings/write`
//!   `{sessionId, toolCallId, changes ≤5 [{key, value}], automationRun?}` →
//!   `{toolCallId, changes: [{key, label, outcome, scope, value?, previous?, reason?, code?, settings_path?}]}`.
//!   `previous` is the value before the write when the server knows it
//!   (account keys); the card uses it for Undo.

use std::collections::HashSet;
use std::sync::{Arc, LazyLock};

use axum::body::Body;
use axum::extract::{Extension, State};
use axum::http::{header, Method, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tower::ServiceExt;
use tracing::warn;

use crate::agent_user_tools::{resolve_owner, Owner};
use crate::auth::AuthUser;
use crate::AppState;

// ---- limits ----

/// Changes one write may carry.
pub const MAX_CHANGES: usize = 5;
/// Keys one read may name.
pub const MAX_READ_KEYS: usize = 50;
/// A string setting with no `max` in the definitions.
const DEFAULT_STRING_MAX: usize = 1_000;
/// A list setting with no `max` in the definitions.
const DEFAULT_LIST_MAX: usize = 200;
/// One list item.
const LIST_ITEM_MAX: usize = 2_048;
const MAX_TOOL_CALL_ID: usize = 200;

const AGENT_PREFS: &str = "/agent-preferences";
const COWORK_PREFS: &str = "/cowork-preferences";

// ---- definitions ----

static DEFINITIONS_JSON: &str = include_str!("../../../contracts/settings/definitions.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Class {
    Safe,
    Confirm,
    Locked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Account,
    Device,
    Runtime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Boolean,
    String,
    Enum,
    List,
}

/// One setting, as the read returns it (the contract's `store` is left out).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Definition {
    pub key: String,
    pub class: Class,
    pub scope: Scope,
    #[serde(rename = "type")]
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<usize>,
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings_path: Option<String>,
}

#[derive(Deserialize)]
struct DefinitionsFile {
    settings: Vec<Definition>,
}

/// The contract's settings. A contract that fails to parse leaves the list
/// empty, so every key comes back `invalid` (nothing is applied).
pub static DEFINITIONS: LazyLock<Vec<Definition>> = LazyLock::new(|| match serde_json::from_str::<DefinitionsFile>(DEFINITIONS_JSON) {
    Ok(f) => f.settings,
    Err(e) => {
        warn!("settings: contracts/settings/definitions.json does not parse: {e}");
        Vec::new()
    }
});

pub fn definition(key: &str) -> Option<&'static Definition> {
    DEFINITIONS.iter().find(|d| d.key == key)
}

/// The preference handler and field a safe account key is written through.
/// Safe keys only: a `confirm` or `locked` key must never appear here.
pub fn account_field(key: &str) -> Option<(&'static str, &'static str)> {
    match key {
        "chat.answer_style" => Some((AGENT_PREFS, "response_style")),
        "browser.preferred" => Some((COWORK_PREFS, "preferred_browser")),
        "browser.open_links_in_app" => Some((COWORK_PREFS, "open_links_in_app")),
        _ => None,
    }
}

// ---- the gate ----

/// What happens to one change.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// Unknown key, a value the key does not take, or a key named twice.
    Invalid(String),
    /// Money or account: only the Settings page changes it.
    Locked,
    /// The user's own tap applies it.
    NeedsConfirmation,
    /// A confirm key asked for by a run an automation started.
    AutomationRefused,
    /// The client that sent the turn applies it (device and runtime keys).
    OnDevice,
    /// Applied here, through this preference handler and field.
    Account { path: &'static str, field: &'static str },
}

/// Check a value against its definition.
pub fn check_value(def: &Definition, value: &Value) -> Result<(), String> {
    match def.kind {
        Kind::Boolean => value.as_bool().map(|_| ()).ok_or_else(|| format!("{} takes true or false.", def.key)),
        Kind::Enum => {
            let allowed = def.values.as_deref().unwrap_or_default();
            match value.as_str() {
                Some(v) if allowed.iter().any(|a| a == v) => Ok(()),
                _ => Err(format!("{} takes one of: {}.", def.key, allowed.join(", "))),
            }
        }
        Kind::String => {
            let max = def.max.unwrap_or(DEFAULT_STRING_MAX);
            match value.as_str() {
                Some(v) if v.chars().count() <= max => Ok(()),
                Some(_) => Err(format!("{} can be at most {max} characters.", def.key)),
                None => Err(format!("{} takes text.", def.key)),
            }
        }
        Kind::List => {
            let max = def.max.unwrap_or(DEFAULT_LIST_MAX);
            let Some(items) = value.as_array() else {
                return Err(format!("{} takes a list of text items.", def.key));
            };
            if items.len() > max {
                return Err(format!("{} can hold at most {max} items.", def.key));
            }
            if items.iter().any(|i| !i.as_str().is_some_and(|s| s.chars().count() <= LIST_ITEM_MAX)) {
                return Err(format!("{} takes a list of text items.", def.key));
            }
            Ok(())
        }
    }
}

/// The risk gate for one change. `automation`: a scheduled run started the
/// turn. The class decides before the value does, so a locked key is never
/// echoed back or written whatever value was sent.
pub fn decide(key: &str, value: &Value, automation: bool) -> Decision {
    let Some(def) = definition(key) else {
        return Decision::Invalid(format!("There's no setting \"{}\". Read the settings to see the keys.", clip(key, 100)));
    };
    match def.class {
        Class::Locked => Decision::Locked,
        Class::Confirm => {
            if let Err(reason) = check_value(def, value) {
                return Decision::Invalid(reason);
            }
            if automation {
                Decision::AutomationRefused
            } else {
                Decision::NeedsConfirmation
            }
        }
        Class::Safe => {
            if let Err(reason) = check_value(def, value) {
                return Decision::Invalid(reason);
            }
            match def.scope {
                Scope::Device | Scope::Runtime => Decision::OnDevice,
                Scope::Account => match account_field(key) {
                    Some((path, field)) => Decision::Account { path, field },
                    None => Decision::Invalid(format!("{} can't be changed from a chat yet. Open Settings.", def.label)),
                },
            }
        }
    }
}

/// A truthy `automationRun` (the runtime's marker of a scheduled turn).
fn from_run(marker: Option<&Value>) -> bool {
    match marker {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(Value::String(s)) => !s.trim().is_empty(),
        Some(_) => true,
    }
}

fn clip(s: &str, cap: usize) -> String {
    s.chars().take(cap).collect()
}

const AUTOMATION_RUN_REFUSAL: &str =
    "This run was started by a scheduled task, so it can't ask to change a security, privacy or instructions setting. Tell the user in your reply instead.";

/// One change's result entry. `previous` and `value` go out only when the
/// outcome uses them; a locked or invalid change never echoes its value.
pub fn result_entry(key: &str, decision: &Decision, value: &Value, previous: Option<Value>, applied: Option<Result<Value, String>>) -> Value {
    let def = definition(key);
    let mut out = Map::new();
    out.insert("key".into(), json!(key));
    if let Some(d) = def {
        out.insert("label".into(), json!(d.label));
        out.insert("scope".into(), json!(d.scope));
    }
    let settings_path = def.and_then(|d| d.settings_path.clone());
    let mut put = |k: &str, v: Value| {
        out.insert(k.into(), v);
    };
    match decision {
        Decision::Invalid(reason) => {
            put("outcome", json!("invalid"));
            put("reason", json!(reason));
        }
        Decision::AutomationRefused => {
            put("outcome", json!("invalid"));
            put("code", json!("automation_run"));
            put("reason", json!(AUTOMATION_RUN_REFUSAL));
        }
        Decision::Locked => {
            put("outcome", json!("locked"));
            put("reason", json!("This is changed only in Settings. Give the user the link; you can't change it."));
            if let Some(p) = settings_path {
                put("settings_path", json!(p));
            }
        }
        Decision::NeedsConfirmation => {
            put("outcome", json!("needs_confirmation"));
            put("value", value.clone());
            if let Some(p) = previous {
                put("previous", p);
            }
            put("reason", json!("The user confirms this on the card."));
            if let Some(p) = settings_path {
                put("settings_path", json!(p));
            }
        }
        Decision::OnDevice => {
            put("outcome", json!("apply_on_device"));
            put("value", value.clone());
        }
        Decision::Account { .. } => match applied {
            Some(Ok(now)) => {
                put("outcome", json!("applied"));
                put("value", now);
                if let Some(p) = previous {
                    put("previous", p);
                }
            }
            Some(Err(reason)) => {
                put("outcome", json!("invalid"));
                put("reason", json!(reason));
            }
            None => {
                put("outcome", json!("invalid"));
                put("reason", json!("The change was not applied."));
            }
        },
    }
    Value::Object(out)
}

// ---- current values (the existing preference loaders) ----

/// The stored value of an account key, or None when the server doesn't keep it.
pub fn account_value(conn: &rusqlite::Connection, owner: &str, key: &str) -> Option<Value> {
    use crate::agent_preferences_routes::load_preferences_row;
    use crate::cowork_preferences_routes::load_prefs;
    let agent = || load_preferences_row(conn, owner).ok().flatten();
    match key {
        "chat.answer_style" => Some(json!(agent().map(|r| r.0).unwrap_or_else(|| "balanced".into()))),
        "instructions.custom" => Some(json!(agent().map(|r| r.1).unwrap_or_default())),
        "browser.preferred" => Some(json!(load_prefs(conn, owner).preferred_browser)),
        "browser.open_links_in_app" => Some(json!(load_prefs(conn, owner).open_links_in_app)),
        "instructions.global" => Some(json!(load_prefs(conn, owner).global_instructions)),
        "security.require_trusted_devices" => Some(json!(load_prefs(conn, owner).require_trusted_devices)),
        "files.trusted_folders" => Some(json!(load_prefs(conn, owner).trusted_folders)),
        "browser.allowed_sites" => Some(json!(load_prefs(conn, owner).allowed_sites)),
        "memory.capture.channels" => Some(json!(crate::memory_capture::settings(conn, owner).channels)),
        "memory.capture.gizzi" => Some(json!(crate::memory_capture::settings(conn, owner).gizzi)),
        "memory.capture.agent_folders" => Some(json!(crate::memory_capture::settings(conn, owner).agent_folders)),
        "memory.capture.vendors" => Some(json!(crate::memory_capture::settings(conn, owner).vendors)),
        "memory.dreaming" => crate::memory_dream::dreaming_enabled(conn, owner).ok().map(Value::Bool),
        _ => None,
    }
}

// ---- routes ----

pub fn agent_settings_tools_router() -> Router<Arc<AppState>> {
    Router::new().route("/bot-routing/settings/read", post(read_route)).route("/bot-routing/settings/write", post(write_route))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadBody {
    pub session_id: String,
    #[serde(default)]
    pub keys: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct ChangeBody {
    pub key: String,
    #[serde(default)]
    pub value: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteBody {
    pub session_id: String,
    #[serde(default)]
    pub tool_call_id: String,
    pub changes: Vec<ChangeBody>,
    /// The calling turn was started by a scheduled run (the runtime sends it,
    /// never the model).
    #[serde(default)]
    pub automation_run: Option<Value>,
}

fn err(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
    (status, Json(json!({ "error": message.into(), "code": code }))).into_response()
}

async fn owner_for(state: &AppState, user: &AuthUser, session_id: &str) -> Result<Owner, Response> {
    let db = state.db.clone();
    let (uid, sid) = (user.user_id.clone(), session_id.to_string());
    let paired = crate::relay_auth::process_secret().paired_owner();
    match tokio::task::spawn_blocking(move || resolve_owner(&db, &uid, &sid, paired.as_deref())).await {
        Ok(Ok(o)) => Ok(o),
        Ok(Err((status, code, msg))) => Err(err(status, code, msg)),
        Err(e) => Err(err(StatusCode::BAD_GATEWAY, "failed", e.to_string())),
    }
}

async fn read_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<ReadBody>) -> Response {
    if body.keys.as_ref().is_some_and(|k| k.len() > MAX_READ_KEYS) {
        return err(StatusCode::BAD_REQUEST, "too_many_keys", format!("Read at most {MAX_READ_KEYS} keys at a time."));
    }
    let owner = match owner_for(&state, &user, &body.session_id).await {
        Ok(o) => o,
        Err(r) => return r,
    };
    let (wanted, unknown): (Vec<&'static Definition>, Vec<String>) = match body.keys.as_ref() {
        None => (DEFINITIONS.iter().collect(), Vec::new()),
        Some(keys) => {
            let mut seen = HashSet::new();
            let (mut found, mut unknown) = (Vec::new(), Vec::new());
            for k in keys.iter().map(|k| k.trim()).filter(|k| seen.insert(k.to_string())) {
                match definition(k) {
                    Some(d) => found.push(d),
                    None => unknown.push(clip(k, 100)),
                }
            }
            (found, unknown)
        }
    };
    let db = state.db.clone();
    let owner_id = owner.user_id.clone();
    let settings = tokio::task::spawn_blocking(move || -> Vec<Value> {
        let conn = db.connect().ok();
        wanted
            .into_iter()
            .map(|d| {
                let mut entry = serde_json::to_value(d).unwrap_or_else(|_| json!({ "key": d.key }));
                // Locked keys are links only: no value is read for them.
                let value = match (d.class, d.scope, conn.as_ref()) {
                    (Class::Locked, _, _) | (_, Scope::Device | Scope::Runtime, _) | (_, _, None) => None,
                    (_, Scope::Account, Some(c)) => account_value(c, &owner_id, &d.key),
                };
                if let (Some(v), Some(obj)) = (value, entry.as_object_mut()) {
                    obj.insert("value".into(), v);
                }
                entry
            })
            .collect()
    })
    .await
    .unwrap_or_default();
    Json(json!({ "settings": settings, "unknown": unknown })).into_response()
}

/// Call an existing preference handler in process, as `owner`.
async fn call_preferences(state: &Arc<AppState>, owner: &str, path: &'static str, body: Value) -> Result<Value, String> {
    let router = if path == AGENT_PREFS {
        crate::agent_preferences_routes::agent_preferences_router()
    } else {
        crate::cowork_preferences_routes::cowork_preferences_router()
    }
    .with_state(state.clone());
    let mut req = Request::builder()
        .method(Method::PUT)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .map_err(|e| e.to_string())?;
    req.extensions_mut().insert(AuthUser {
        user_id: owner.to_string(),
        email: None,
        name: None,
        avatar_url: None,
        tenant_id: None,
        organization_id: None,
        organization_role: None,
        organization_slug: None,
    });
    let resp = match router.oneshot(req).await {
        Ok(r) => r,
        Err(never) => match never {},
    };
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 4 * 1024 * 1024).await.map_err(|e| e.to_string())?;
    let payload: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if status.is_success() {
        Ok(payload)
    } else {
        let msg = payload["message"].as_str().or(payload["error"].as_str()).unwrap_or("The setting was not saved.");
        Err(msg.to_string())
    }
}

async fn write_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<WriteBody>) -> Response {
    if body.changes.is_empty() || body.changes.len() > MAX_CHANGES {
        return err(StatusCode::BAD_REQUEST, "bad_changes", format!("Send 1 to {MAX_CHANGES} changes."));
    }
    let tool_call_id = body.tool_call_id.trim().to_string();
    if tool_call_id.len() > MAX_TOOL_CALL_ID {
        return err(StatusCode::BAD_REQUEST, "bad_tool_call_id", "toolCallId is too long");
    }
    let owner = match owner_for(&state, &user, &body.session_id).await {
        Ok(o) => o,
        Err(r) => return r,
    };
    let automation = from_run(body.automation_run.as_ref()) || crate::routine_local_scheduler::session_running_automation(body.session_id.trim());

    let mut seen = HashSet::new();
    let mut results = Vec::with_capacity(body.changes.len());
    for change in &body.changes {
        let key = change.key.trim();
        let decision = if !seen.insert(key.to_string()) {
            Decision::Invalid(format!("{} is changed twice in this call; send it once.", clip(key, 100)))
        } else {
            decide(key, &change.value, automation)
        };
        let (previous, applied) = match &decision {
            Decision::NeedsConfirmation => (current(&state, &owner.user_id, key).await, None),
            &Decision::Account { path, field } => {
                let previous = current(&state, &owner.user_id, key).await;
                let mut put = Map::new();
                put.insert(field.to_string(), change.value.clone());
                let applied = call_preferences(&state, &owner.user_id, path, Value::Object(put))
                    .await
                    .map(|saved| saved.get(field).cloned().unwrap_or_else(|| change.value.clone()));
                if let Err(e) = &applied {
                    warn!("settings: {key} was not applied: {e}");
                }
                (previous, Some(applied))
            }
            _ => (None, None),
        };
        results.push(result_entry(key, &decision, &change.value, previous, applied));
    }
    Json(json!({ "toolCallId": tool_call_id, "changes": results })).into_response()
}

async fn current(state: &AppState, owner: &str, key: &str) -> Option<Value> {
    let (db, owner, key) = (state.db.clone(), owner.to_string(), key.to_string());
    tokio::task::spawn_blocking(move || db.connect().ok().and_then(|c| account_value(&c, &owner, &key))).await.ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(class: Class) -> Vec<&'static str> {
        DEFINITIONS.iter().filter(|d| d.class == class).map(|d| d.key.as_str()).collect()
    }

    #[test]
    fn the_contract_parses_and_has_every_class() {
        assert!(DEFINITIONS.len() >= 20);
        for c in [Class::Safe, Class::Confirm, Class::Locked] {
            assert!(!keys(c).is_empty(), "{c:?}");
        }
        let unique: HashSet<_> = DEFINITIONS.iter().map(|d| &d.key).collect();
        assert_eq!(unique.len(), DEFINITIONS.len(), "keys are unique");
        for d in DEFINITIONS.iter().filter(|d| d.kind == Kind::Enum) {
            assert!(d.values.as_ref().is_some_and(|v| !v.is_empty()), "{} has values", d.key);
        }
        for d in DEFINITIONS.iter().filter(|d| d.class != Class::Safe) {
            assert!(d.settings_path.is_some(), "{} links to Settings", d.key);
        }
    }

    #[test]
    fn no_confirm_or_locked_key_has_a_writer() {
        for k in keys(Class::Confirm).into_iter().chain(keys(Class::Locked)) {
            assert_eq!(account_field(k), None, "{k} must never be written by the server");
        }
        for k in ["chat.answer_style", "browser.preferred", "browser.open_links_in_app"] {
            let d = definition(k).unwrap();
            assert_eq!((d.class, d.scope), (Class::Safe, Scope::Account), "{k}");
        }
    }

    #[test]
    fn every_safe_account_key_is_wired() {
        for d in DEFINITIONS.iter().filter(|d| d.class == Class::Safe && d.scope == Scope::Account) {
            assert!(account_field(&d.key).is_some(), "{} has a preference writer", d.key);
        }
    }

    #[test]
    fn confirm_and_locked_are_never_applied_even_from_automation() {
        for automation in [false, true] {
            for k in keys(Class::Locked) {
                let d = decide(k, &json!("anything"), automation);
                assert_eq!(d, Decision::Locked, "{k}");
                let out = result_entry(k, &d, &json!("hunter2"), None, None);
                assert_eq!(out["outcome"], "locked");
                assert!(out.get("value").is_none(), "a locked value is never echoed");
                assert!(out["settings_path"].as_str().is_some());
            }
        }
        let global = decide("instructions.global", &json!("Always answer in French."), false);
        assert_eq!(global, Decision::NeedsConfirmation);
        let out = result_entry("instructions.global", &global, &json!("Always answer in French."), Some(json!("")), None);
        assert_eq!((out["outcome"].as_str(), out["previous"].as_str()), (Some("needs_confirmation"), Some("")));
        assert_eq!(decide("security.require_trusted_devices", &json!(false), true), Decision::AutomationRefused);
        let out = result_entry("files.trusted_folders", &Decision::AutomationRefused, &json!(["/"]), None, None);
        assert_eq!((out["outcome"].as_str(), out["code"].as_str()), (Some("invalid"), Some("automation_run")));
        for k in keys(Class::Confirm) {
            let d = definition(k).unwrap();
            let sample = match d.kind {
                Kind::Boolean => json!(true),
                Kind::Enum => json!(d.values.as_ref().unwrap()[0]),
                Kind::String => json!("x"),
                Kind::List => json!(["/tmp"]),
            };
            for automation in [false, true] {
                assert!(matches!(decide(k, &sample, automation), Decision::NeedsConfirmation | Decision::AutomationRefused), "{k}");
            }
        }
    }

    #[test]
    fn unknown_keys_and_bad_values_are_invalid() {
        assert!(matches!(decide("account.balance_hack", &json!(1), false), Decision::Invalid(_)));
        assert!(matches!(decide("appearance.theme", &json!("purple"), false), Decision::Invalid(_)));
        assert!(matches!(decide("browser.open_links_in_app", &json!("yes"), false), Decision::Invalid(_)));
        assert!(matches!(decide("instructions.global", &json!(42), false), Decision::Invalid(_)));
        let out = result_entry("nope", &decide("nope", &json!(1), false), &json!(1), None, None);
        assert_eq!(out["outcome"], "invalid");
        assert!(out.get("value").is_none());
    }

    #[test]
    fn safe_keys_apply_on_the_server_or_on_the_device() {
        assert_eq!(decide("chat.answer_style", &json!("concise"), true), Decision::Account { path: AGENT_PREFS, field: "response_style" });
        assert_eq!(decide("browser.preferred", &json!("chrome"), false), Decision::Account { path: COWORK_PREFS, field: "preferred_browser" });
        assert_eq!(decide("appearance.theme", &json!("dark"), false), Decision::OnDevice);
        assert_eq!(decide("gizzi.verbose", &json!(true), false), Decision::OnDevice);
        let out = result_entry("appearance.theme", &Decision::OnDevice, &json!("dark"), None, None);
        assert_eq!((out["outcome"].as_str(), out["value"].as_str(), out["scope"].as_str()), (Some("apply_on_device"), Some("dark"), Some("device")));
        let d = decide("chat.answer_style", &json!("concise"), false);
        let out = result_entry("chat.answer_style", &d, &json!("concise"), Some(json!("balanced")), Some(Ok(json!("concise"))));
        assert_eq!((out["outcome"].as_str(), out["value"].as_str(), out["previous"].as_str()), (Some("applied"), Some("concise"), Some("balanced")));
        let out = result_entry("chat.answer_style", &d, &json!("concise"), None, Some(Err("db down".into())));
        assert_eq!(out["outcome"], "invalid");
    }

    #[test]
    fn automation_marker_is_truthy_only_when_set() {
        assert!(!from_run(None));
        assert!(!from_run(Some(&Value::Null)));
        assert!(!from_run(Some(&json!(false))));
        assert!(!from_run(Some(&json!(" "))));
        assert!(from_run(Some(&json!({ "id": "r-1", "runId": "run-1" }))));
        assert!(from_run(Some(&json!(true))));
    }

    #[test]
    fn answer_style_is_read_from_agent_preferences() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::DbHandle::new(dir.path().join("t.db")).unwrap();
        let conn = db.connect().unwrap();
        assert_eq!(account_value(&conn, "u1", "chat.answer_style"), Some(json!("balanced")));
        conn.execute("INSERT INTO user_agent_preferences (user_id, response_style, custom_instructions) VALUES ('u1', 'concise', '')", []).unwrap();
        assert_eq!(account_value(&conn, "u1", "chat.answer_style"), Some(json!("concise")));
        assert_eq!(account_value(&conn, "u1", "appearance.theme"), None);
    }
}
