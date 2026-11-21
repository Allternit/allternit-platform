//! `GET /api/v1/runtimes`: the runtime registry (Harness & Bots 2.0, contract
//! `CONTRACT-harness-bots-2.md`, "Routes"). Web and phones read it here, so
//! the picker works with no computer. It is the `runtimes[]` array of the same
//! `cli-catalog.json` allternit-api embeds (`cmd/allternit-api/src/cli_catalog.rs`),
//! included at build time the way `gateway_catalog.rs` includes its JSON.
//!
//! Public (no sign-in) and static: `{version, runtimes}` with a strong ETag
//! (SHA-256 of the file, the same value allternit-api sends) and
//! `Cache-Control: public, max-age=3600`; `If-None-Match` answers 304. There
//! is no `installed` here: what is installed is known only on the user's
//! computer, which adds it on its own gateway for its owner.

use std::sync::{Arc, OnceLock};

use axum::{
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::ApiState;

const CATALOG_JSON: &str =
    include_str!("../../../gizzi-code/src/runtime/providers/discovery/cli-catalog.json");

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new().route("/api/v1/runtimes", get(get_runtimes))
}

/// The whole embedded catalog (`tools[]` and `runtimes[]`), parsed once.
pub(crate) fn catalog() -> &'static Value {
    static RAW: OnceLock<Value> = OnceLock::new();
    RAW.get_or_init(|| serde_json::from_str(CATALOG_JSON).unwrap_or(Value::Null))
}

fn body() -> &'static Value {
    static BODY: OnceLock<Value> = OnceLock::new();
    BODY.get_or_init(|| {
        let raw = catalog();
        json!({
            "version": raw.get("version").cloned().unwrap_or(Value::Null),
            "runtimes": raw.get("runtimes").cloned().unwrap_or_else(|| json!([])),
        })
    })
}

fn etag_for(catalog_json: &str) -> String {
    let digest = Sha256::digest(catalog_json.as_bytes());
    format!("\"rt-{}\"", hex::encode(&digest[..16]))
}

fn etag() -> &'static str {
    static ETAG: OnceLock<String> = OnceLock::new();
    ETAG.get_or_init(|| etag_for(CATALOG_JSON))
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

async fn get_runtimes(headers: HeaderMap) -> Response {
    let etag = etag();
    let cache = [
        (header::ETAG, etag),
        (header::CACHE_CONTROL, "public, max-age=3600"),
    ];
    if etag_matches(&headers, etag) {
        return (StatusCode::NOT_MODIFIED, cache).into_response();
    }
    (cache, Json(body().clone())).into_response()
}

/// The shared runtime rules on the server side (contract "Backend gaps 1-3"):
/// the runtime normalizer, `fits(runtime, provider)` and
/// `allowed(runtime, brain)`, read from the same embedded registry. gizzi's
/// `runtime-registry.ts` (`fits`) and allternit-api's `bot_brain.rs` are the
/// references; this mirrors them for cloud-api agent saves.
///
/// `allowed` = `fits` AND, when the brain is paid by a plan (a provider id
/// starting `subs-`, or a CLI plan sign-in such as `claude-cli`), the plan's
/// provider is in the runtime's `plans`. A runtime's own CLI sign-in always
/// passes. A registry without `plans` (an older catalog) reads as the
/// contract's defaults: `allternit` takes every plan, a runtime takes its own
/// provider's plan (`claude` → claude, `codex` → chatgpt, `kimi` → kimi,
/// `antigravity` → gemini, `copilot` → copilot), every other runtime none.
pub(crate) mod rules {
    use serde_json::Value;

    pub(crate) const RUNTIME_ALLTERNIT: &str = "allternit";

    /// How a brain is paid for (contract "Bot record": `access`).
    pub(crate) const ACCESS: [&str; 4] = ["plan", "key", "local", "credits"];

    /// Plan providers (ids without `subs-`), each with its display name and
    /// the runtime that is its own app.
    const PLANS: [(&str, &str, &str); 5] = [
        ("claude", "Claude", "claude"),
        ("chatgpt", "ChatGPT", "codex"),
        ("kimi", "Kimi", "kimi"),
        ("gemini", "Gemini", "antigravity"),
        ("copilot", "GitHub Copilot", "copilot"),
    ];

    /// A rule a save broke: `code()` and `message()` are the contract's.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) enum RuleError {
        RuntimeUnknown { runtime: String },
        BrainNotFit { runtime: String, vendor: String },
        PlanNotAllowed { plan: String, runtime: String, own: String },
    }

    impl RuleError {
        pub(crate) fn code(&self) -> &'static str {
            match self {
                RuleError::RuntimeUnknown { .. } => "runtime_unknown",
                RuleError::BrainNotFit { .. } => "brain_not_fit",
                RuleError::PlanNotAllowed { .. } => "plan_not_allowed_on_runtime",
            }
        }

        pub(crate) fn message(&self) -> String {
            match self {
                RuleError::RuntimeUnknown { runtime } => format!("No runtime called {runtime}."),
                RuleError::BrainNotFit { runtime, vendor } => format!("{runtime} runs {vendor} models only."),
                RuleError::PlanNotAllowed { plan, runtime, own } => format!(
                    "{plan}'s plan can't be used inside {runtime}. Use an API key, or pick {own}."
                ),
            }
        }
    }

    fn strs(v: &Value) -> impl Iterator<Item = &str> {
        v.as_array().into_iter().flatten().filter_map(Value::as_str)
    }

    fn list<'a>(catalog: &'a Value, key: &str) -> &'a [Value] {
        catalog.get(key).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
    }

    /// The registry the rules read: `runtimes[]` and `tools[]` of a catalog.
    pub(crate) struct Registry<'a> {
        runtimes: &'a [Value],
        tools: &'a [Value],
    }

    impl<'a> Registry<'a> {
        pub(crate) fn new(catalog: &'a Value) -> Self {
            Registry { runtimes: list(catalog, "runtimes"), tools: list(catalog, "tools") }
        }

        /// A runtime by id, else by alias (case-insensitive).
        pub(crate) fn get(&self, id_or_alias: &str) -> Option<&'a Value> {
            let key = id_or_alias.trim().to_ascii_lowercase();
            if key.is_empty() {
                return None;
            }
            self.runtimes
                .iter()
                .find(|r| r["id"].as_str().is_some_and(|id| id.eq_ignore_ascii_case(&key)))
                .or_else(|| self.runtimes.iter().find(|r| strs(&r["aliases"]).any(|a| a.eq_ignore_ascii_case(&key))))
        }

        fn name(&self, id: &str) -> String {
            self.get(id).and_then(|r| r["name"].as_str()).unwrap_or(id).to_string()
        }

        /// The stored runtime: absent or empty is `allternit`; a known id or
        /// alias becomes the canonical id; anything else is `runtime_unknown`.
        pub(crate) fn normalize_runtime(&self, raw: Option<&str>) -> Result<String, RuleError> {
            match raw.map(str::trim).filter(|s| !s.is_empty()) {
                None => Ok(RUNTIME_ALLTERNIT.to_string()),
                Some(id) => self
                    .get(id)
                    .and_then(|r| r["id"].as_str())
                    .map(str::to_string)
                    .ok_or_else(|| RuleError::RuntimeUnknown { runtime: id.to_string() }),
            }
        }

        /// Is `provider` this runtime's own CLI (its tool, id, aliases or the
        /// tool's aliases)? `subs-` is stripped first.
        fn own_cli(&self, rt: &Value, provider: &str) -> bool {
            let p = provider.strip_prefix("subs-").unwrap_or(provider);
            let Some(tool) = rt["tool"].as_str() else { return false };
            if p == tool || rt["id"].as_str() == Some(p) || strs(&rt["aliases"]).any(|a| a == p) {
                return true;
            }
            self.tools
                .iter()
                .find(|t| t["id"].as_str() == Some(tool))
                .is_some_and(|t| strs(&t["aliases"]).any(|a| a == p))
        }

        /// `fits(runtime, provider)`: `brains == "any"`, the provider (with
        /// `subs-` stripped) is listed, or it is the runtime's own CLI.
        pub(crate) fn fits(&self, rt: &Value, provider: &str) -> bool {
            if rt["brains"].as_str() == Some("any") {
                return true;
            }
            let p = provider.strip_prefix("subs-").unwrap_or(provider);
            strs(&rt["brains"]).any(|b| b == p) || self.own_cli(rt, provider)
        }

        /// The plan providers this runtime may use (`plans`, or the defaults).
        fn plans(&self, rt: &Value) -> Vec<String> {
            if let Some(list) = rt.get("plans").and_then(Value::as_array) {
                return list.iter().filter_map(Value::as_str).map(str::to_string).collect();
            }
            let id = rt["id"].as_str().unwrap_or_default();
            PLANS
                .iter()
                .filter(|(_, _, own)| id == RUNTIME_ALLTERNIT || id == *own)
                .map(|(plan, _, _)| plan.to_string())
                .collect()
        }

        /// `allowed(runtime, brain)` for a brain whose provider is `provider`.
        pub(crate) fn check(&self, runtime: &str, provider: &str) -> Result<(), RuleError> {
            let rt = self.get(runtime).ok_or_else(|| RuleError::RuntimeUnknown { runtime: runtime.to_string() })?;
            let rt_name = rt["name"].as_str().unwrap_or(runtime).to_string();
            if !self.fits(rt, provider) {
                let vendor = rt["vendor"].as_str().unwrap_or(&rt_name).to_string();
                return Err(RuleError::BrainNotFit { runtime: rt_name, vendor });
            }
            let Some(plan) = plan_of(provider) else { return Ok(()) };
            if self.own_cli(rt, provider) || self.plans(rt).iter().any(|p| p == plan) {
                return Ok(());
            }
            let (_, display, own) = PLANS.iter().find(|(p, _, _)| *p == plan).copied().unwrap_or((plan, plan, RUNTIME_ALLTERNIT));
            Err(RuleError::PlanNotAllowed { plan: display.to_string(), runtime: rt_name, own: self.name(own) })
        }
    }

    /// The registry embedded in this build.
    pub(crate) fn registry() -> Registry<'static> {
        Registry::new(super::catalog())
    }

    /// The plan a brain is paid by, or `None` for a key / local / credits
    /// brain: `subs-<provider>` (vendor or plan id), or a CLI plan sign-in.
    pub(crate) fn plan_of(provider: &str) -> Option<&'static str> {
        let p = provider.trim().to_ascii_lowercase();
        let plan = match p.strip_prefix("subs-") {
            Some(sub) => match sub {
                "claude" | "anthropic" | "claude-cli" => "claude",
                "chatgpt" | "openai" | "codex" | "codex-cli" => "chatgpt",
                "kimi" | "moonshot" | "moonshotai" | "kimi-cli" => "kimi",
                "gemini" | "google" | "gemini-cli" | "antigravity" => "gemini",
                "copilot" | "github" | "github-copilot" | "copilot-cli" => "copilot",
                _ => return None,
            },
            None => match p.as_str() {
                "claude-cli" => "claude",
                "codex-cli" => "chatgpt",
                "kimi-cli" => "kimi",
                "gemini-cli" => "gemini",
                "copilot-cli" => "copilot",
                _ => return None,
            },
        };
        Some(plan)
    }

    /// The provider part of a model id: `provider/model` → `provider`, a bare
    /// id (`allternit`) is its own provider.
    pub(crate) fn provider_of(model: &str) -> &str {
        let m = model.trim();
        m.split_once('/').map(|(p, _)| p).unwrap_or(m)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use serde_json::json;

        #[test]
        fn runtime_rules_normalize_defaults_and_aliases() {
            let reg = registry();
            assert_eq!(reg.normalize_runtime(None).unwrap(), "allternit");
            assert_eq!(reg.normalize_runtime(Some("  ")).unwrap(), "allternit");
            assert_eq!(reg.normalize_runtime(Some("Hermes")).unwrap(), "hermes");
            assert_eq!(reg.normalize_runtime(Some("claude-cli")).unwrap(), "claude");
            let err = reg.normalize_runtime(Some("nope")).unwrap_err();
            assert_eq!((err.code(), err.message()), ("runtime_unknown", "No runtime called nope.".to_string()));
        }

        #[test]
        fn runtime_rules_allternit_takes_every_plan() {
            let reg = registry();
            for brain in ["subs-claude", "subs-chatgpt", "subs-kimi", "subs-gemini", "subs-copilot", "claude-cli", "anthropic", "allternit"] {
                assert_eq!(reg.check("allternit", brain), Ok(()), "{brain}");
            }
        }

        #[test]
        fn runtime_rules_hermes_refuses_a_claude_plan() {
            let err = registry().check("hermes", provider_of("subs-claude/claude-sonnet-5-5")).unwrap_err();
            assert_eq!(err.code(), "plan_not_allowed_on_runtime");
            assert_eq!(err.message(), "Claude's plan can't be used inside Hermes. Use an API key, or pick Claude Code.");
            // The same brain on a key is fine.
            assert_eq!(registry().check("hermes", "anthropic"), Ok(()));
        }

        #[test]
        fn runtime_rules_own_cli_plan_passes_and_foreign_brain_does_not_fit() {
            let reg = registry();
            assert_eq!(reg.check("claude", "subs-claude"), Ok(()));
            assert_eq!(reg.check("claude", "claude-cli"), Ok(()));
            assert_eq!(reg.check("codex", "subs-chatgpt"), Ok(()));
            let err = reg.check("claude", "openai").unwrap_err();
            assert_eq!((err.code(), err.message()), ("brain_not_fit", "Claude Code runs Anthropic models only.".to_string()));
            let err = reg.check("codex", "subs-claude").unwrap_err();
            assert_eq!(err.code(), "brain_not_fit");
            let err = reg.check("kimi", "subs-chatgpt").unwrap_err();
            assert_eq!(err.message(), "ChatGPT's plan can't be used inside Kimi Code. Use an API key, or pick Codex.");
        }

        #[test]
        fn runtime_rules_read_registry_plans_when_present() {
            let catalog = json!({
                "tools": [],
                "runtimes": [
                    { "id": "allternit", "name": "Allternit", "brains": "any", "tool": null },
                    { "id": "hermes", "name": "Hermes", "brains": "any", "tool": "hermes", "plans": ["claude"] },
                    { "id": "claude", "name": "Claude Code", "brains": ["anthropic"], "tool": "claude-cli", "plans": [] }
                ]
            });
            let reg = Registry::new(&catalog);
            assert_eq!(reg.check("hermes", "subs-claude"), Ok(()));
            // `plans: []` still lets a runtime's own CLI sign-in through.
            assert_eq!(reg.check("claude", "subs-claude"), Ok(()));
            assert_eq!(reg.check("hermes", "subs-kimi").unwrap_err().code(), "plan_not_allowed_on_runtime");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn router() -> Router {
        Router::new().route("/api/v1/runtimes", get(get_runtimes))
    }

    #[test]
    fn registry_is_v3_with_runtimes() {
        assert!(body()["version"].as_u64().is_some_and(|v| v >= 3), "{}", body()["version"]);
        let runtimes = body()["runtimes"].as_array().expect("runtimes[]");
        assert!(runtimes.len() >= 30, "{} runtimes", runtimes.len());
        assert!(runtimes.iter().any(|r| r["id"] == "allternit"));
    }

    #[tokio::test]
    async fn runtimes_route_is_public_cached_and_has_no_installed() {
        let response = router()
            .oneshot(Request::get("/api/v1/runtimes").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "public, max-age=3600");
        let tag = response.headers()[header::ETAG].to_str().unwrap().to_string();
        assert_eq!(tag, etag_for(CATALOG_JSON));
        let payload: Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert!(payload.get("installed").is_none());
        assert!(payload["runtimes"].as_array().is_some_and(|r| r.len() >= 30));

        let response = router()
            .oneshot(
                Request::get("/api/v1/runtimes")
                    .header(header::IF_NONE_MATCH, tag)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    }
}
