//! `GET /api/v1/picker-catalog` on a computer: the same shape cloud-api serves
//! (`docs/contracts/picker-catalog/schema.json`,
//! `surfaces/docs/architecture/picker-catalog.mdx`), plus `installed` and
//! `signedIn` per runtime for this computer's owner.
//!
//! - `runtimes` come from the `cli-catalog.json` this binary embeds
//!   (`cli_catalog`), joined to each runtime's tool for logo, sign-in and
//!   models. For the owner (Desktop access token or a verified session, the
//!   same test `/api/v1/runtimes` uses) each row carries the CLI detection of
//!   `cli_catalog::owner_cli_status`.
//! - `brains` and `cloudModels` (with their locks) come from cloud-api's own
//!   `/api/v1/picker-catalog`, with the caller's `Authorization` forwarded the
//!   way `/me/usage` reaches the cloud, so the lock rule stays on cloud-api.
//!   With no cloud URL, or when the cloud can't be reached, `cloudModels` is
//!   empty and the Allternit Cloud brain is locked with that reason.
//! - `onDevice` is the same fixed list cloud-api sends.
//!
//! Public; strong ETag over the body without `generatedAt`. Owner responses
//! are per machine (`private, no-cache`, revalidated by ETag); others get
//! `max-age=21600`.

use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::warn;

use crate::cli_catalog::{self, RuntimeRegistry};
use crate::AppState;

/// Mounted on the public router in `main.rs` (outside `auth_middleware`); the
/// handler checks owner auth itself before it adds `installed`.
pub fn public_router() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/picker-catalog", get(get_picker_catalog))
}

/// Six hours, the contract's cache window.
const MAX_AGE: &str = "max-age=21600";
const CLOUD_TIMEOUT: Duration = Duration::from_secs(4);

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
    fn locked(reason: &str, action: Option<&str>) -> Self {
        Access { state: AccessState::Locked, reason: Some(reason.to_string()), action: action.map(str::to_string) }
    }
}

// ─── Build ───────────────────────────────────────────────────────────────────

/// The Cloud half of the catalog: cloud-api's answer, or why there is none.
#[derive(Debug, Clone, PartialEq)]
pub enum CloudPart {
    /// cloud-api's own catalog (its `brains` and `cloudModels` are used).
    Served(PickerCatalog),
    /// No cloud API URL on this computer.
    NotConfigured,
    /// The cloud API failed or answered badly.
    Unreachable,
}

/// The on-device models the apps can download (same list as cloud-api).
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

/// The four brains with the Allternit Cloud brain set to `cloud`.
fn brains_with_cloud(cloud: Access) -> Vec<PickerBrain> {
    vec![
        PickerBrain { id: "subscription".into(), label: "My subscription".into(), kind: BrainKind::Subscription, access: Access::ok() },
        PickerBrain { id: "api_key".into(), label: "My API key".into(), kind: BrainKind::ApiKey, access: Access::ok() },
        PickerBrain { id: "allternit-cloud".into(), label: "Allternit Cloud".into(), kind: BrainKind::Cloud, access: cloud },
        PickerBrain { id: "on-device".into(), label: "On this device".into(), kind: BrainKind::OnDevice, access: Access::ok() },
    ]
}

/// One row per registry runtime, joined to its catalog tool. A model's
/// `aliases` are the retired ids that now map to it. The native runtime
/// (`tool: null`) has no logo, models or default model and signs in with
/// `none`. `status` (owner only) fills `installed` / `signedIn`.
pub(crate) fn runtimes(
    registry: &RuntimeRegistry,
    status: Option<&std::collections::HashMap<String, (bool, bool)>>,
) -> Vec<PickerRuntime> {
    registry
        .runtimes
        .iter()
        .map(|rt| {
            let tool = rt.tool.as_deref().and_then(cli_catalog::find);
            let models = tool
                .map(|t| {
                    t.models
                        .iter()
                        .map(|m| PickerRuntimeModel {
                            id: m.id.clone(),
                            name: m.name.clone().unwrap_or_else(|| m.id.clone()),
                            aliases: t
                                .retired
                                .iter()
                                .filter(|(_, to)| **to == m.id)
                                .map(|(from, _)| from.clone())
                                .collect(),
                            context: m.context.unwrap_or(0),
                            output: m.output.unwrap_or(0),
                        })
                        .collect()
                })
                .unwrap_or_default();
            let (installed, signed_in) = match status {
                Some(status) => {
                    let (installed, signed_in) = cli_catalog::runtime_install_state(rt, status);
                    (Some(installed), Some(signed_in))
                }
                None => (None, None),
            };
            PickerRuntime {
                id: rt.id.clone(),
                label: rt.name.clone(),
                vendor: rt.vendor.clone(),
                logo: tool.and_then(|t| t.logo.clone()).unwrap_or_default(),
                sign_in: tool.map(|t| t.sign_in.clone()).unwrap_or_else(|| "none".to_string()),
                default_model: tool
                    .filter(|t| !t.models.is_empty())
                    .map(|t| t.default_model().to_string())
                    .unwrap_or_default(),
                models,
                installed,
                signed_in,
            }
        })
        .collect()
}

pub(crate) fn build_catalog(
    registry: &RuntimeRegistry,
    status: Option<&std::collections::HashMap<String, (bool, bool)>>,
    cloud: CloudPart,
    generated_at: String,
) -> PickerCatalog {
    let (brains, cloud_models) = match cloud {
        CloudPart::Served(served) => (served.brains, served.cloud_models),
        CloudPart::NotConfigured => (
            brains_with_cloud(Access::locked("Allternit Cloud is not set up on this computer", None)),
            Vec::new(),
        ),
        CloudPart::Unreachable => (
            brains_with_cloud(Access::locked("Allternit Cloud can't be reached", Some("retry"))),
            Vec::new(),
        ),
    };
    PickerCatalog {
        schema: 1,
        generated_at,
        runtimes: runtimes(registry, status),
        brains,
        cloud_models,
        on_device: on_device_models(),
    }
}

/// Strong ETag over everything but `generatedAt`.
pub fn etag_for(catalog: &PickerCatalog) -> String {
    let mut stable = catalog.clone();
    stable.generated_at.clear();
    let bytes = serde_json::to_vec(&stable).unwrap_or_default();
    let digest = Sha256::digest(&bytes);
    format!("\"pc-{}\"", hex::encode(&digest[..16]))
}

/// cloud-api's catalog for this caller (its `Authorization` forwarded).
async fn fetch_cloud(state: &AppState, headers: &HeaderMap) -> CloudPart {
    let Some(base) = state.config.cloud_api_url() else {
        return CloudPart::NotConfigured;
    };
    let mut request = crate::net_client::client()
        .get(format!("{}/api/v1/picker-catalog", base.trim_end_matches('/')))
        .timeout(CLOUD_TIMEOUT);
    if let Some(authorization) = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) {
        request = request.header(header::AUTHORIZATION, authorization);
    }
    let response = match request.send().await {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => {
            warn!(status = %response.status(), "cloud picker catalog answered an error");
            return CloudPart::Unreachable;
        }
        Err(error) => {
            warn!(%error, "cloud picker catalog unreachable");
            return CloudPart::Unreachable;
        }
    };
    match response.json::<PickerCatalog>().await {
        Ok(catalog) => CloudPart::Served(catalog),
        Err(error) => {
            warn!(%error, "cloud picker catalog body did not match the contract");
            CloudPart::Unreachable
        }
    }
}

async fn get_picker_catalog(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let owner = cli_catalog::caller_is_owner(&state, &headers).await;
    let (cloud, status) = if owner {
        let (cloud, status) = tokio::join!(fetch_cloud(&state, &headers), cli_catalog::owner_cli_status());
        (cloud, Some(status))
    } else {
        (fetch_cloud(&state, &headers).await, None)
    };
    let generated_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let catalog = build_catalog(cli_catalog::runtime_registry(), status.as_ref(), cloud, generated_at);
    let etag = etag_for(&catalog);
    let cache = if owner {
        "private, no-cache".to_string()
    } else if headers.contains_key(header::AUTHORIZATION) {
        format!("private, {MAX_AGE}")
    } else {
        format!("public, {MAX_AGE}")
    };
    let headers_out = [
        (header::ETAG, etag.clone()),
        (header::CACHE_CONTROL, cache),
        (header::VARY, "Authorization, x-allternit-desktop-access-token".to_string()),
    ];
    if cli_catalog::etag_matches(&headers, &etag) {
        return (StatusCode::NOT_MODIFIED, headers_out).into_response();
    }
    (headers_out, Json(catalog)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIGNED_OUT: &str = include_str!("../../../docs/contracts/picker-catalog/fixtures/signed-out.json");
    const NO_CREDITS: &str = include_str!("../../../docs/contracts/picker-catalog/fixtures/no-credits.json");

    fn fixture(text: &str) -> PickerCatalog {
        serde_json::from_str(text).expect("fixture matches the contract structs")
    }

    fn round_trip(catalog: &PickerCatalog) -> PickerCatalog {
        let text = serde_json::to_string(catalog).unwrap();
        serde_json::from_str(&text).expect("served body parses as the contract")
    }

    #[test]
    fn signed_out_matches_the_signed_out_fixture() {
        let want = fixture(SIGNED_OUT);
        let got = round_trip(&build_catalog(
            cli_catalog::runtime_registry(),
            None,
            CloudPart::Served(want.clone()),
            "2026-11-27T15:00:00Z".into(),
        ));
        assert_eq!(got.brains, want.brains);
        assert_eq!(got.cloud_models, want.cloud_models);
        assert!(got.cloud_models.iter().all(|m| m.access.reason.as_deref() == Some("Sign in")));
        assert_eq!(got.on_device, want.on_device);
        for rt in &want.runtimes {
            let ours = got.runtimes.iter().find(|r| r.id == rt.id).expect("runtime");
            assert_eq!(
                (&ours.label, &ours.vendor, &ours.logo, &ours.sign_in, &ours.default_model),
                (&rt.label, &rt.vendor, &rt.logo, &rt.sign_in, &rt.default_model)
            );
            assert!(ours.installed.is_none() && ours.signed_in.is_none(), "only the owner gets installed");
            for m in &rt.models {
                let om = ours.models.iter().find(|x| x.id == m.id).expect("model");
                assert_eq!((&om.name, om.context, om.output), (&m.name, m.context, m.output));
            }
        }
        let native = got.runtimes.iter().find(|r| r.id == "allternit").unwrap();
        assert_eq!((native.sign_in.as_str(), native.models.len(), native.default_model.as_str()), ("none", 0, ""));
    }

    #[test]
    fn no_credits_account_gets_locked_rows() {
        let want = fixture(NO_CREDITS);
        let got = round_trip(&build_catalog(cli_catalog::runtime_registry(), None, CloudPart::Served(want.clone()), "x".into()));
        assert!(!got.cloud_models.is_empty());
        for m in &got.cloud_models {
            assert_eq!(m.access, Access::locked("No credits left", Some("add_credits")), "{}", m.id);
        }
        let cloud_brain = got.brains.iter().find(|b| b.kind == BrainKind::Cloud).unwrap();
        assert_eq!(cloud_brain.access.state, AccessState::Locked);
    }

    #[test]
    fn owner_rows_carry_install_state_and_cloud_outage_locks_the_cloud_brain() {
        let mut status = std::collections::HashMap::new();
        status.insert("claude-cli".to_string(), (true, false));
        let got = round_trip(&build_catalog(cli_catalog::runtime_registry(), Some(&status), CloudPart::Unreachable, "x".into()));
        let claude = got.runtimes.iter().find(|r| r.id == "claude").unwrap();
        assert_eq!((claude.installed, claude.signed_in), (Some(true), Some(false)));
        let codex = got.runtimes.iter().find(|r| r.id == "codex").unwrap();
        assert_eq!((codex.installed, codex.signed_in), (Some(false), Some(false)));
        let native = got.runtimes.iter().find(|r| r.id == "allternit").unwrap();
        assert_eq!((native.installed, native.signed_in), (Some(true), Some(true)));
        assert!(got.cloud_models.is_empty());
        let cloud_brain = got.brains.iter().find(|b| b.kind == BrainKind::Cloud).unwrap();
        assert_eq!(cloud_brain.access, Access::locked("Allternit Cloud can't be reached", Some("retry")));
    }
}
