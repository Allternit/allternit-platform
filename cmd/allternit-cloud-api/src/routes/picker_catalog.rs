//! `GET /api/v1/picker-catalog`: the one list every model and runtime picker
//! reads (Gizzi Code, Desktop, web, phones). Contract:
//! `docs/contracts/picker-catalog/schema.json` (+ fixtures), explained in
//! `surfaces/docs/architecture/picker-catalog.mdx`.
//!
//! The server decides what is locked, so the rule lives here once:
//! - signed out: every Cloud model and the Allternit Cloud brain are `locked`
//!   with reason "Sign in" (action `sign_in`);
//! - signed in: the lock comes from the account's credits, through the same
//!   `services::cloud_models_entitlement` `/api/v1/billing/credits` reports.
//!   No credits: "No credits left" (action `add_credits`), except providers the
//!   account has its own key for (BYOK routes before the credits gate);
//! - open-lane models (`lane: "open"`) follow their own gate instead
//!   (`services::open_entitlement::load`: terms, plan, allowance, bought
//!   credits), the same one the chat path enforces;
//! - a model the router lists as `hidden` (retired aliases) is `hidden`.
//!
//! `runtimes` come from the `cli-catalog.json` this binary embeds
//! (`routes::runtimes::catalog()`, the same file allternit-api embeds).
//! `cloudModels` are the models `/v1/models` offers (`model_router::offered_models`).
//! allternit-api serves the same shape and adds `installed` / `signedIn`.
//!
//! Public. Strong ETag over the body without `generatedAt`, `If-None-Match`
//! answers 304, `Cache-Control: max-age=21600` (public signed out, private
//! signed in).

use std::sync::Arc;

use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{auth, model_router::ModelInfo, services, ApiError, ApiState};

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new().route("/api/v1/picker-catalog", get(get_picker_catalog))
}

/// Six hours, the contract's cache window.
const MAX_AGE: &str = "max-age=21600";

// ─── Contract shape (schema.json; unknown fields refused) ────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PickerCatalog {
    pub schema: u32,
    pub generated_at: String,
    pub runtimes: Vec<PickerRuntime>,
    pub brains: Vec<PickerBrain>,
    pub cloud_models: Vec<PickerCloudModel>,
    pub on_device: Vec<OnDeviceModel>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PickerRuntime {
    pub id: String,
    pub label: String,
    pub vendor: String,
    pub logo: String,
    pub sign_in: String,
    pub default_model: String,
    pub models: Vec<PickerRuntimeModel>,
    /// Only allternit-api sends these (for its own computer).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signed_in: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickerRuntimeModel {
    pub id: String,
    pub name: String,
    pub aliases: Vec<String>,
    pub context: u64,
    pub output: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickerBrain {
    pub id: String,
    pub label: String,
    pub kind: BrainKind,
    pub access: Access,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrainKind {
    Subscription,
    ApiKey,
    Cloud,
    OnDevice,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PickerCloudModel {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub access: Access,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OnDeviceModel {
    pub id: String,
    pub name: String,
    pub size_bytes: u64,
    pub min_ram_gb: f64,
    pub platforms: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccessState {
    Ok,
    Locked,
    Hidden,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Access {
    pub state: AccessState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
}

impl Access {
    fn ok() -> Self {
        Access { state: AccessState::Ok, reason: None, action: None }
    }
    fn hidden() -> Self {
        Access { state: AccessState::Hidden, reason: None, action: None }
    }
    fn locked(reason: &str, action: &str) -> Self {
        Access { state: AccessState::Locked, reason: Some(reason.to_string()), action: Some(action.to_string()) }
    }
}

// ─── Lock rule ───────────────────────────────────────────────────────────────

/// Whether open-lane models (`lane: "open"`) may run: their own gate
/// (`services::open_entitlement`: terms, plan, allowance, bought credits),
/// not the credits rule, and never through a BYOK key.
#[derive(Debug, Clone, PartialEq)]
pub enum OpenAccess {
    Allowed,
    /// The entitlement's first blocking `reason` code.
    Blocked(String),
    /// The entitlement could not be read.
    Unknown,
}

/// What the caller may run on Allternit Cloud.
#[derive(Debug, Clone, PartialEq)]
pub enum CloudAccess {
    SignedOut,
    /// `allowed` and `byok_providers` from `services::CloudModelsEntitlement`;
    /// `open` from `services::open_entitlement::load`.
    SignedIn { allowed: bool, byok_providers: Vec<String>, open: OpenAccess },
}

impl CloudAccess {
    /// The lock for a standard Cloud model of `provider`.
    fn model_access(&self, provider: &str) -> Access {
        match self {
            CloudAccess::SignedOut => Access::locked("Sign in", "sign_in"),
            CloudAccess::SignedIn { allowed: true, .. } => Access::ok(),
            CloudAccess::SignedIn { byok_providers, .. } if byok_providers.iter().any(|b| b == provider) => Access::ok(),
            CloudAccess::SignedIn { .. } => Access::locked("No credits left", "add_credits"),
        }
    }

    /// The lock for an open-lane model.
    fn open_access(&self) -> Access {
        use services::open_entitlement::{
            REASON_ALLOWANCE_USED, REASON_BOUGHT_CREDITS_REQUIRED, REASON_PLAN_REQUIRED, REASON_TERMS_REQUIRED,
        };
        match self {
            CloudAccess::SignedOut => Access::locked("Sign in", "sign_in"),
            CloudAccess::SignedIn { open: OpenAccess::Allowed, .. } => Access::ok(),
            CloudAccess::SignedIn { open: OpenAccess::Unknown, .. } => Access::locked("Can't check access right now", "retry"),
            CloudAccess::SignedIn { open: OpenAccess::Blocked(code), .. } => match code.as_str() {
                REASON_TERMS_REQUIRED => Access::locked("Accept the open-model terms", "accept_terms"),
                REASON_PLAN_REQUIRED => Access::locked("Needs a paid plan", "upgrade"),
                REASON_ALLOWANCE_USED => Access::locked("Open-model allowance used", "add_credits"),
                REASON_BOUGHT_CREDITS_REQUIRED => Access::locked("No credits left", "add_credits"),
                _ => Access::locked("Not available on this account", "upgrade"),
            },
        }
    }

    /// The Allternit Cloud brain: open when any Cloud model can run.
    fn brain_access(&self) -> Access {
        match self {
            CloudAccess::SignedOut => Access::locked("Sign in", "sign_in"),
            CloudAccess::SignedIn { allowed, byok_providers, open } => {
                if *allowed || !byok_providers.is_empty() || *open == OpenAccess::Allowed {
                    Access::ok()
                } else {
                    Access::locked("No credits left", "add_credits")
                }
            }
        }
    }
}

// ─── Build ───────────────────────────────────────────────────────────────────

/// The on-device models the apps can download (contract fixtures).
pub fn on_device_models() -> Vec<OnDeviceModel> {
    let platforms = |list: &[&str]| -> Vec<String> { list.iter().map(|p| p.to_string()).collect() };
    vec![
        OnDeviceModel {
            id: "gemma-4-e2b-local".into(),
            name: "Gemma 4 E2B".into(),
            size_bytes: 3_200_000_000,
            min_ram_gb: 8.0,
            platforms: platforms(&["web", "macos", "windows", "linux"]),
        },
        OnDeviceModel {
            id: "local-brain-llama-3.2-3b".into(),
            name: "Local Brain".into(),
            size_bytes: 2_000_000_000,
            min_ram_gb: 4.0,
            platforms: platforms(&["macos", "windows", "linux"]),
        },
    ]
}

fn str_of<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// `runtimes[]` of a cli-catalog (v3): each runtime joined to its `tools[]`
/// entry for logo, sign-in and models. A model's `aliases` are the retired ids
/// that now map to it. The native runtime (`tool: null`) has no logo, models
/// or default model (its models are the brains) and signs in with `none`.
pub fn runtimes_from_catalog(catalog: &Value) -> Vec<PickerRuntime> {
    let tools = catalog.get("tools").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]);
    let runtimes = catalog.get("runtimes").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]);
    runtimes
        .iter()
        .map(|rt| {
            let tool = rt
                .get("tool")
                .and_then(Value::as_str)
                .and_then(|id| tools.iter().find(|t| str_of(t, "id") == id));
            let empty = Vec::new();
            let raw_models = tool.and_then(|t| t.get("models")).and_then(Value::as_array).unwrap_or(&empty);
            let retired = tool.and_then(|t| t.get("retired")).and_then(Value::as_object);
            let models: Vec<PickerRuntimeModel> = raw_models
                .iter()
                .map(|m| {
                    let id = str_of(m, "id").to_string();
                    let aliases = retired
                        .map(|r| {
                            r.iter()
                                .filter(|(_, to)| to.as_str() == Some(id.as_str()))
                                .map(|(from, _)| from.clone())
                                .collect()
                        })
                        .unwrap_or_default();
                    PickerRuntimeModel {
                        name: m.get("name").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| id.clone()),
                        aliases,
                        context: m.get("context").and_then(Value::as_u64).unwrap_or(0),
                        output: m.get("output").and_then(Value::as_u64).unwrap_or(0),
                        id,
                    }
                })
                .collect();
            let default_model = raw_models
                .iter()
                .find(|m| m.get("default").and_then(Value::as_bool) == Some(true))
                .or_else(|| raw_models.first())
                .map(|m| str_of(m, "id").to_string())
                .unwrap_or_default();
            PickerRuntime {
                id: str_of(rt, "id").to_string(),
                label: str_of(rt, "name").to_string(),
                vendor: str_of(rt, "vendor").to_string(),
                logo: tool.map(|t| str_of(t, "logo").to_string()).unwrap_or_default(),
                sign_in: tool.map(|t| str_of(t, "signIn")).filter(|s| !s.is_empty()).unwrap_or("none").to_string(),
                default_model,
                models,
                installed: None,
                signed_in: None,
            }
        })
        .collect()
}

/// The four brains, with the Cloud brain locked by the same rule as its models.
pub fn brains(cloud: &CloudAccess) -> Vec<PickerBrain> {
    vec![
        PickerBrain { id: "subscription".into(), label: "My subscription".into(), kind: BrainKind::Subscription, access: Access::ok() },
        PickerBrain { id: "api_key".into(), label: "My API key".into(), kind: BrainKind::ApiKey, access: Access::ok() },
        PickerBrain { id: "allternit-cloud".into(), label: "Allternit Cloud".into(), kind: BrainKind::Cloud, access: cloud.brain_access() },
        PickerBrain { id: "on-device".into(), label: "On this device".into(), kind: BrainKind::OnDevice, access: Access::ok() },
    ]
}

/// Cloud model rows from the offered model list.
pub fn cloud_models(models: &[ModelInfo], cloud: &CloudAccess) -> Vec<PickerCloudModel> {
    models
        .iter()
        .map(|m| {
            let provider = m.provider.clone().unwrap_or_else(|| m.owned_by.clone());
            let hidden = m.extra.get("hidden").and_then(Value::as_bool) == Some(true);
            let open_lane = m.extra.get("lane").and_then(Value::as_str) == Some("open");
            PickerCloudModel {
                id: m.id.clone(),
                name: m.extra.get("name").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| m.id.clone()),
                access: if hidden {
                    Access::hidden()
                } else if open_lane {
                    cloud.open_access()
                } else {
                    cloud.model_access(&provider)
                },
                provider,
            }
        })
        .collect()
}

pub fn build_catalog(catalog: &Value, models: &[ModelInfo], cloud: &CloudAccess, generated_at: String) -> PickerCatalog {
    PickerCatalog {
        schema: 1,
        generated_at,
        runtimes: runtimes_from_catalog(catalog),
        brains: brains(cloud),
        cloud_models: cloud_models(models, cloud),
        on_device: on_device_models(),
    }
}

/// Strong ETag over everything but `generatedAt`, so an unchanged catalog
/// revalidates with 304.
pub fn etag_for(catalog: &PickerCatalog) -> String {
    let mut stable = catalog.clone();
    stable.generated_at.clear();
    let bytes = serde_json::to_vec(&stable).unwrap_or_default();
    let digest = Sha256::digest(&bytes);
    format!("\"pc-{}\"", hex::encode(&digest[..16]))
}

fn etag_matches(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|v| v.trim().trim_start_matches("W/"))
        .any(|v| v == "*" || v == etag)
}

async fn cloud_access(state: &ApiState, headers: &HeaderMap) -> Result<CloudAccess, ApiError> {
    // Any failed or missing sign-in reads as signed out: the catalog still
    // answers, with Cloud locked behind "Sign in".
    let Ok(user) = auth::resolve_user(&state.db, headers).await else {
        return Ok(CloudAccess::SignedOut);
    };
    let balance = services::credit_balance_row(&state.db, &user.id).await?;
    let entitlement = services::cloud_models_entitlement(&state.db, &user.id, balance).await;
    let open = match services::open_entitlement::load(&state.db, &user.id).await {
        Ok(open) => match open.entitlement.reason {
            None => OpenAccess::Allowed,
            Some(code) => OpenAccess::Blocked(code.to_string()),
        },
        Err(error) => {
            tracing::warn!(%error, "open_models unreadable for the picker catalog");
            OpenAccess::Unknown
        }
    };
    Ok(CloudAccess::SignedIn { allowed: entitlement.allowed, byok_providers: entitlement.byok_providers, open })
}

async fn get_picker_catalog(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Result<Response, ApiError> {
    let cloud = cloud_access(&state, &headers).await?;
    let models = super::model_router::offered_models(&state, &headers).await?;
    let generated_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let catalog = build_catalog(super::runtimes::catalog(), &models, &cloud, generated_at);
    let etag = etag_for(&catalog);
    let cache = match cloud {
        CloudAccess::SignedOut => format!("public, {MAX_AGE}"),
        CloudAccess::SignedIn { .. } => format!("private, {MAX_AGE}"),
    };
    let headers_out = [
        (header::ETAG, etag.clone()),
        (header::CACHE_CONTROL, cache),
        (header::VARY, "Authorization".to_string()),
    ];
    if etag_matches(&headers, &etag) {
        return Ok((StatusCode::NOT_MODIFIED, headers_out).into_response());
    }
    Ok((headers_out, Json(catalog)).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIGNED_OUT: &str = include_str!("../../../../docs/contracts/picker-catalog/fixtures/signed-out.json");
    const NO_CREDITS: &str = include_str!("../../../../docs/contracts/picker-catalog/fixtures/no-credits.json");

    fn fixture(text: &str) -> PickerCatalog {
        serde_json::from_str(text).expect("fixture matches the contract structs")
    }

    /// Router rows shaped like the fixture's Cloud models, plus one hidden alias.
    fn models_like(fixture: &PickerCatalog) -> Vec<ModelInfo> {
        let mut rows: Vec<ModelInfo> = fixture
            .cloud_models
            .iter()
            .map(|m| {
                let mut extra = serde_json::Map::new();
                extra.insert("name".into(), Value::String(m.name.clone()));
                ModelInfo {
                    id: m.id.clone(),
                    object: "model".into(),
                    created: 0,
                    owned_by: m.provider.clone(),
                    upstream_id: None,
                    provider: Some(m.provider.clone()),
                    aliases: None,
                    extra,
                }
            })
            .collect();
        let mut hidden = rows[0].clone();
        hidden.id = "allternit/retired-alias".into();
        hidden.extra.insert("hidden".into(), Value::Bool(true));
        rows.push(hidden);
        rows
    }

    fn round_trip(catalog: &PickerCatalog) -> PickerCatalog {
        let text = serde_json::to_string(catalog).unwrap();
        serde_json::from_str(&text).expect("served body parses as the contract")
    }

    #[test]
    fn signed_out_matches_the_signed_out_fixture() {
        let want = fixture(SIGNED_OUT);
        let got = round_trip(&build_catalog(
            super::super::runtimes::catalog(),
            &models_like(&want),
            &CloudAccess::SignedOut,
            "2026-11-27T15:00:00Z".into(),
        ));
        assert_eq!(got.schema, 1);
        assert_eq!(got.brains, want.brains);
        assert_eq!(got.on_device, want.on_device);
        for row in &want.cloud_models {
            let ours = got.cloud_models.iter().find(|m| m.id == row.id).expect("cloud row");
            assert_eq!(ours, row);
        }
        let hidden = got.cloud_models.iter().find(|m| m.id == "allternit/retired-alias").unwrap();
        assert_eq!(hidden.access.state, AccessState::Hidden);
        // Every fixture runtime is served from the embedded catalog with the
        // same label, vendor, logo, sign-in, default and model facts.
        for rt in &want.runtimes {
            let ours = got.runtimes.iter().find(|r| r.id == rt.id).expect("runtime");
            assert_eq!((&ours.label, &ours.vendor, &ours.logo, &ours.sign_in, &ours.default_model), (&rt.label, &rt.vendor, &rt.logo, &rt.sign_in, &rt.default_model));
            assert!(ours.installed.is_none() && ours.signed_in.is_none(), "cloud-api never sends installed");
            for m in &rt.models {
                let om = ours.models.iter().find(|x| x.id == m.id).expect("model");
                assert_eq!((&om.name, om.context, om.output), (&m.name, m.context, m.output));
            }
        }
        // The native runtime has no tool: empty logo/default, sign-in none.
        let native = got.runtimes.iter().find(|r| r.id == "allternit").unwrap();
        assert_eq!((native.sign_in.as_str(), native.models.len()), ("none", 0));
    }

    #[test]
    fn no_credits_account_gets_locked_rows_except_own_key_providers() {
        let want = fixture(NO_CREDITS);
        let models = models_like(&want);
        let cloud = CloudAccess::SignedIn { allowed: false, byok_providers: vec![], open: OpenAccess::Blocked("plan_required".into()) };
        let got = round_trip(&build_catalog(super::super::runtimes::catalog(), &models, &cloud, "x".into()));
        assert_eq!(got.brains, want.brains);
        for row in &want.cloud_models {
            assert_eq!(got.cloud_models.iter().find(|m| m.id == row.id), Some(row));
        }

        let byok = CloudAccess::SignedIn { allowed: false, byok_providers: vec!["openai".into()], open: OpenAccess::Blocked("plan_required".into()) };
        let got = build_catalog(super::super::runtimes::catalog(), &models, &byok, "x".into());
        for m in got.cloud_models.iter().filter(|m| m.access.state != AccessState::Hidden) {
            let want_state = if m.provider == "openai" { AccessState::Ok } else { AccessState::Locked };
            assert_eq!(m.access.state, want_state, "{}", m.id);
        }
    }

    #[test]
    fn open_lane_rows_follow_the_open_gate_not_credits() {
        let mut models = models_like(&fixture(SIGNED_OUT));
        let mut open_row = models[0].clone();
        open_row.id = "allternit/open-model".into();
        open_row.extra.insert("lane".into(), Value::String("open".into()));
        models.push(open_row);
        let find = |c: &PickerCatalog, id: &str| c.cloud_models.iter().find(|m| m.id == id).unwrap().access.clone();

        // Bought credits but a Free plan: standard rows open, the open row locked.
        let free = CloudAccess::SignedIn { allowed: true, byok_providers: vec![], open: OpenAccess::Blocked("plan_required".into()) };
        let got = build_catalog(super::super::runtimes::catalog(), &models, &free, "x".into());
        assert_eq!(find(&got, "allternit/claude-sonnet-5.5"), Access::ok());
        assert_eq!(find(&got, "allternit/open-model"), Access::locked("Needs a paid plan", "upgrade"));

        // Open allowance left, no credits: only the open row runs; the brain is open.
        let open_only = CloudAccess::SignedIn { allowed: false, byok_providers: vec![], open: OpenAccess::Allowed };
        let got = build_catalog(super::super::runtimes::catalog(), &models, &open_only, "x".into());
        assert_eq!(find(&got, "allternit/open-model"), Access::ok());
        assert_eq!(find(&got, "allternit/claude-sonnet-5.5").state, AccessState::Locked);
        assert_eq!(got.brains.iter().find(|b| b.kind == BrainKind::Cloud).unwrap().access, Access::ok());

        let signed_out = build_catalog(super::super::runtimes::catalog(), &models, &CloudAccess::SignedOut, "x".into());
        assert_eq!(find(&signed_out, "allternit/open-model"), Access::locked("Sign in", "sign_in"));
    }

    #[test]
    fn funded_account_is_open_and_etag_ignores_generated_at() {
        let models = models_like(&fixture(SIGNED_OUT));
        let cloud = CloudAccess::SignedIn { allowed: true, byok_providers: vec![], open: OpenAccess::Allowed };
        let a = build_catalog(super::super::runtimes::catalog(), &models, &cloud, "2026-01-01T00:00:00Z".into());
        let b = build_catalog(super::super::runtimes::catalog(), &models, &cloud, "2026-02-01T00:00:00Z".into());
        assert!(a.cloud_models.iter().filter(|m| m.access.state != AccessState::Hidden).all(|m| m.access == Access::ok()));
        assert_eq!(etag_for(&a), etag_for(&b));
        let locked = build_catalog(super::super::runtimes::catalog(), &models, &CloudAccess::SignedOut, "x".into());
        assert_ne!(etag_for(&a), etag_for(&locked));
    }
}
