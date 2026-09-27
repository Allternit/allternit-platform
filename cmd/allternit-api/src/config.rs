//! Unified configuration loader for the packaged Allternit Platform app.
//!
//! Configuration is layered so that company standards are baked in and users
//! only ever touch brain/provider settings:
//!
//!   1. Company config (`resources/company.json` in the app bundle, or
//!      `~/.allternit/company.json` for dev overrides).
//!   2. User config (`~/.allternit/config.json`) — created by the onboarding
//!      wizard and editable in settings.
//!   3. Environment variables — power-user override, kept for CI/dev.
//!
//! Any value present in a higher layer wins.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::{info, warn};

/// Company-level configuration. These values are part of the packaged app and
/// should not be edited by end users. They standardize auth, endpoints, and
/// encryption across every host computer.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct CompanyConfig {
    /// Clerk publishable key rendered by the frontend.
    #[serde(rename = "clerkPublishableKey")]
    pub clerk_publishable_key: Option<String>,

    /// Clerk JWKS URL used by the API to verify JWT signatures.
    #[serde(rename = "clerkJwksUrl")]
    pub clerk_jwks_url: Option<String>,

    /// Expected Clerk JWT issuer (`iss`).
    #[serde(rename = "clerkIssuer")]
    pub clerk_issuer: Option<String>,

    /// Secret for verifying Clerk webhook signatures.
    #[serde(rename = "clerkWebhookSecret")]
    pub clerk_webhook_secret: Option<String>,

    /// Default URL the frontend should use to reach the API.
    #[serde(rename = "gatewayUrl")]
    pub gateway_url: Option<String>,

    /// Default URL the API should use to reach the Gizzi runtime.
    #[serde(rename = "terminalServerUrl")]
    pub terminal_server_url: Option<String>,

    /// Base URL of the cloud API (usage metering / entitlements proxy for
    /// GET /api/v1/me/usage). Unset means usage metering is unavailable and
    /// the route answers 503.
    #[serde(rename = "cloudApiUrl")]
    pub cloud_api_url: Option<String>,

    /// Default encryption key for local-at-rest data.
    #[serde(rename = "encryptionKey")]
    pub encryption_key: Option<String>,

    /// Shared secret the ACU (computer-use) Python gateway presents on
    /// internal-only routes (e.g. cloud-credential resolution, usage-event
    /// ingestion) since that service has no Clerk JWT to send.
    #[serde(rename = "internalServiceToken")]
    pub internal_service_token: Option<String>,

    /// Whether the Stripe-backed credit-pack checkout is live in this
    /// deployment. `POST /credits/purchase` refuses with 409 ("not enabled")
    /// unless this is true AND a cloud API (billing checkout backend) is
    /// configured — the endpoint never self-credits. Env
    /// `ALLTERNIT_CREDITS_CHECKOUT_ENABLED` overrides this file value.
    #[serde(rename = "creditsCheckoutEnabled")]
    pub credits_checkout_enabled: Option<bool>,

    /// Cloudflare Remote Control push worker URL. Optional.
    #[serde(rename = "pushWorkerUrl")]
    pub push_worker_url: Option<String>,

    /// Company branding / tenant marker.
    #[serde(rename = "tenantId")]
    pub tenant_id: Option<String>,

    /// URL of the Rails service (ledger/gate/leases). Baked into packaged apps.
    #[serde(rename = "railsUrl")]
    pub rails_url: Option<String>,

    /// Canonical AllternitOS lease authority. Unset means fabric lease
    /// issuance returns 503 until production configures it.
    /// URL of the canonical AllternitOS lease authority. When set, the gateway
    /// proxies `/api/v1/fabric/leases` there instead of relying on a runtime-local
    /// dev issuer. Unset means lease issuance returns 503 in production.
    #[serde(rename = "allternitOSLeaseAuthorityUrl")]
    pub allternitos_lease_authority_url: Option<String>,

    /// Rails workspace ID for this packaged deployment.
    #[serde(rename = "railsWorkspaceId")]
    pub rails_workspace_id: Option<String>,

    /// Default directory for VM storage. Baked into packaged apps.
    #[serde(rename = "vmDir")]
    pub vm_dir: Option<String>,

    /// URL of the cron daemon. Usually baked in; users can override.
    #[serde(rename = "cronDaemonUrl")]
    pub cron_daemon_url: Option<String>,

    /// URL of the Etrid native agent wallet service.
    #[serde(rename = "etridUrl")]
    pub etrid_url: Option<String>,

    /// When true, the packaged app runs in self-hosted mode. Clerk JWT
    /// verification is skipped and the desktop bootstrap headers are trusted
    /// instead. This is for deployments where the app bundle is the authority
    /// and there is no external Clerk tenant.
    #[serde(rename = "selfHosted")]
    pub self_hosted: Option<bool>,

    /// Named agent-level permission policies baked into the deployment.
    #[serde(rename = "permissionPolicies", default)]
    pub permission_policies: Option<Vec<crate::permission_policy::PermissionPolicy>>,

    /// Name of the company-level permission policy that is active by default.
    #[serde(rename = "activePermissionPolicy", default)]
    pub active_permission_policy: Option<String>,

    /// When true, tools that can execute code or mutate the host filesystem
    /// (shell.exec, file.write, system.env, etc.) are allowed. Defaults to
    /// false so local-computer access is opt-in.
    #[serde(rename = "hostControlEnabled", default)]
    pub host_control_enabled: Option<bool>,
}

/// User-level configuration. Written by the onboarding wizard and the settings
/// UI. Contains only things an end user is expected to change: their brain,
/// their provider credentials, and optional local overrides.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct UserConfig {
    /// Default LLM provider/model, e.g. `kimi-cli/kimi-k3`.
    #[serde(rename = "defaultModel")]
    pub default_model: Option<String>,

    /// Optional override for the Gizzi runtime URL.
    #[serde(rename = "terminalServerUrl")]
    pub terminal_server_url: Option<String>,

    /// Optional override for the API gateway URL (frontend use).
    #[serde(rename = "gatewayUrl")]
    pub gateway_url: Option<String>,

    /// Legacy provider secret field. Loaded only so an old config can be
    /// migrated; never returned or written again. Gizzi owns credentials.
    #[serde(rename = "providerApiKeys", default, skip_serializing)]
    pub provider_api_keys: Option<serde_json::Map<String, serde_json::Value>>,

    /// Whether onboarding has been completed.
    #[serde(rename = "onboardingComplete")]
    pub onboarding_complete: Option<bool>,

    /// Local Ollama base URL (e.g. http://localhost:11434).
    #[serde(rename = "ollamaUrl")]
    pub ollama_url: Option<String>,

    /// Allternit memory service URL.
    #[serde(rename = "memoryUrl")]
    pub memory_url: Option<String>,

    /// Voice service URL (services/voice — TTS/STT). Optional off-gateway
    /// service; the /api/v1/voice routes degrade gracefully when it's
    /// unreachable.
    #[serde(rename = "voiceUrl")]
    pub voice_url: Option<String>,

    /// Embedding service URL.
    #[serde(rename = "embeddingUrl")]
    pub embedding_url: Option<String>,

    /// Agent working directory.
    #[serde(rename = "agentWorkdir")]
    pub agent_workdir: Option<String>,

    /// Cron daemon URL.
    #[serde(rename = "cronDaemonUrl")]
    pub cron_daemon_url: Option<String>,

    /// Etrid native agent wallet service URL.
    #[serde(rename = "etridUrl")]
    pub etrid_url: Option<String>,

    /// Cloudflare Remote Control push worker URL. Optional: when unset,
    /// runtime-triggered push notifications are unavailable.
    #[serde(rename = "pushWorkerUrl")]
    pub push_worker_url: Option<String>,

    /// First-start wizard tracking (OpenClaw-style versioning).
    #[serde(rename = "wizard")]
    pub wizard: Option<WizardState>,

    /// User-defined agent-level permission policies.
    #[serde(rename = "permissionPolicies", default)]
    pub permission_policies: Option<Vec<crate::permission_policy::PermissionPolicy>>,

    /// Name of the active permission policy. Overrides the company-level default.
    #[serde(rename = "activePermissionPolicy", default)]
    pub active_permission_policy: Option<String>,

    /// Per-user override for enabling host-control tools. The company default
    /// still applies when this is unset.
    #[serde(rename = "hostControlEnabled", default)]
    pub host_control_enabled: Option<bool>,
    /// Inference router configuration: which local CLI provider handles turns.
    #[serde(rename = "inferenceRouter", default)]
    pub inference_router: Option<InferenceRouterConfig>,
}

/// Local CLI provider routing preferences.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct InferenceRouterConfig {
    /// Active provider: codex, claude-code, cursor, openrouter.
    #[serde(rename = "provider", default)]
    pub provider: Option<String>,

    /// Default model passed to the provider (e.g. "claude-sonnet-4-6").
    #[serde(rename = "defaultModel", default)]
    pub default_model: Option<String>,

    /// Whether to route turns through a local sandbox instead of the provider.
    #[serde(rename = "localSandbox", default)]
    pub local_sandbox: Option<bool>,

    /// Provider-specific extra options (e.g. OpenRouter API key reference).
    #[serde(rename = "options", default)]
    pub options: Option<serde_json::Map<String, serde_json::Value>>,
}

/// Tracks when the first-start / env wizard last ran so the app can prompt
/// again after updates or when the wizard schema changes.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct WizardState {
    #[serde(rename = "lastRunAt")]
    pub last_run_at: Option<String>,
    #[serde(rename = "lastRunVersion")]
    pub last_run_version: Option<String>,
    #[serde(rename = "lastRunCommand")]
    pub last_run_command: Option<String>,
    #[serde(rename = "lastRunMode")]
    pub last_run_mode: Option<String>,
}

/// Merged runtime configuration. Code reads from this struct instead of calling
/// `std::env::var` directly.
#[derive(Debug, Clone)]
pub struct AppConfig {
    pub company: CompanyConfig,
    pub user: UserConfig,
}

impl AppConfig {
    /// Load company + user config from disk and apply env overrides.
    /// If the user config has no default model but the Gizzi runtime config
    /// does, mirror it so the UI and API agree on which brain to use.
    pub fn load() -> Self {
        // One-time migration: if a brain was saved to the legacy (wrong) gizzi
        // config path, copy it to the file the gizzi runtime actually reads so
        // a previously configured provider starts taking effect immediately.
        migrate_legacy_gizzi_config();

        let company = load_company_config();
        let mut user = load_user_config();

        if user.default_model.is_none() {
            if let Some(model) = read_gizzi_default_model() {
                info!(model = %model, "Mirroring Gizzi default model into user config");
                user.default_model = Some(model);
            }
        }

        let mut config = Self { company, user };
        config.apply_env_overrides();
        config
    }

    /// Port the API server listens on.
    ///
    /// The production port (8013) is owned, not defaulted: production
    /// launchers (packaged Desktop spawn, server systemd units) pin it
    /// explicitly via `ALLTERNIT_API_PORT`. The unset default is the dev
    /// port so worktree/debug builds never squat the production gateway.
    pub fn api_port(&self) -> u16 {
        std::env::var("ALLTERNIT_API_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(18013)
    }

    /// Port the dedicated inbound webhook receiver listens on.
    pub fn webhook_receiver_port(&self) -> u16 {
        std::env::var("ALLTERNIT_WEBHOOK_RECEIVER_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(8080)
    }

    /// URL the API uses to reach the Gizzi runtime.
    pub fn terminal_server_url(&self) -> String {
        std::env::var("TERMINAL_SERVER_URL")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| self.user.terminal_server_url.clone())
            .or_else(|| self.company.terminal_server_url.clone())
            .unwrap_or_else(|| "http://127.0.0.1:4096".to_string())
    }

    /// Base URL of the cloud API used by the usage-metering proxy
    /// (me_routes.rs GET /me/usage → the cloud quota/entitlements service).
    /// `None` disables usage metering — the route answers 503 rather than
    /// inventing numbers.
    pub fn cloud_api_url(&self) -> Option<String> {
        std::env::var("ALLTERNIT_CLOUD_API_URL")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| self.company.cloud_api_url.clone())
            .filter(|s| !s.is_empty())
    }

    /// Whether `POST /credits/purchase` may delegate to the platform billing
    /// checkout. False by default: the endpoint then answers 409 "not
    /// enabled" instead of self-crediting, and only an explicit opt-in
    /// (`ALLTERNIT_CREDITS_CHECKOUT_ENABLED=true`/`1` or the company config
    /// flag) turns the delegation path on — and even then only when a cloud
    /// API checkout backend is configured.
    pub fn credits_checkout_enabled(&self) -> bool {
        std::env::var("ALLTERNIT_CREDITS_CHECKOUT_ENABLED")
            .ok()
            .filter(|s| !s.is_empty())
            .map(|value| value == "true" || value == "1")
            .unwrap_or(self.company.credits_checkout_enabled.unwrap_or(false))
    }

    /// Default LLM provider/model when a request does not specify one.
    /// When nothing is configured, falls back to the first entry of the model
    /// catalog so the agent-chat bridge never forwards an empty provider/model
    /// to the runtime (which rejects it with ProviderModelNotFoundError).
    pub fn default_model(&self) -> (String, String) {
        let raw = std::env::var("ALLTERNIT_DEFAULT_MODEL")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| self.user.default_model.clone())
            .or_else(|| {
                crate::provider_routes::available_model_catalog()
                    .first()
                    .and_then(|m| m.get("id").and_then(|v| v.as_str()))
                    .map(str::to_string)
            })
            .unwrap_or_default();

        if let Some((provider, model)) = raw.split_once('/') {
            (provider.trim().to_string(), model.trim().to_string())
        } else {
            (String::new(), raw.trim().to_string())
        }
    }

    /// Clerk JWKS URL used to verify JWT signatures.
    pub fn clerk_jwks_url(&self) -> Option<String> {
        std::env::var("CLERK_JWKS_URL")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| self.company.clerk_jwks_url.clone())
    }

    /// Expected Clerk JWT issuer.
    pub fn clerk_issuer(&self) -> Option<String> {
        std::env::var("CLERK_ISSUER")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| self.company.clerk_issuer.clone())
    }

    /// Clerk webhook secret.
    pub fn clerk_webhook_secret(&self) -> Option<String> {
        std::env::var("CLERK_WEBHOOK_SECRET")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| self.company.clerk_webhook_secret.clone())
    }

    /// Clerk publishable key rendered by the frontend.
    pub fn clerk_publishable_key(&self) -> Option<String> {
        std::env::var("NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| self.company.clerk_publishable_key.clone())
    }

    /// Default encryption key for local data.
    ///
    /// The signed desktop process injects this through the environment. A
    /// headless runtime may use its mode-0600 local key managed by
    /// `token_crypto`; this config accessor never touches OS Keychain.
    pub fn encryption_key(&self) -> Option<String> {
        std::env::var("ENCRYPTION_KEY")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| {
                self.company
                    .encryption_key
                    .clone()
                    .filter(|s| !s.is_empty())
            })
    }

    /// Shared secret for internal-only routes the ACU Python gateway calls
    /// (cloud-credential resolution, usage-event ingestion). `None` means
    /// those routes are unreachable except via the local-dev bypass.
    pub fn internal_service_token(&self) -> Option<String> {
        std::env::var("ALLTERNIT_INTERNAL_SERVICE_TOKEN")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| {
                self.company
                    .internal_service_token
                    .clone()
                    .filter(|s| !s.is_empty())
            })
    }

    /// Slack app signing secret, used by `slack_webhook_routes.rs` to verify
    /// inbound Events API requests are really from Slack. `None` means the
    /// webhook rejects everything rather than trusting unsigned requests.
    pub fn slack_signing_secret(&self) -> Option<String> {
        std::env::var("ALLTERNIT_SLACK_SIGNING_SECRET")
            .ok()
            .filter(|s| !s.is_empty())
    }

    /// Slack bot token (`xoxb-...`) used to post replies via
    /// `chat.postMessage`. `None` means inbound events can be received and
    /// routed to an agent, but the reply can't be posted back.
    pub fn slack_bot_token(&self) -> Option<String> {
        std::env::var("ALLTERNIT_SLACK_BOT_TOKEN")
            .ok()
            .filter(|s| !s.is_empty())
    }

    /// Shared secret the signed desktop process must present via
    /// `x-allternit-desktop-access-token` to use the header-trust bootstrap
    /// auth path (`auth::extract_desktop_bootstrap_user`). `None` disables
    /// that path entirely — same fail-closed posture as
    /// `internal_service_token` above. The signed desktop process injects
    /// this through the environment, same as `encryption_key`.
    pub fn desktop_access_token(&self) -> Option<String> {
        std::env::var("ALLTERNIT_DESKTOP_ACCESS_TOKEN")
            .ok()
            .filter(|s| !s.is_empty())
    }

    /// Always-on allternit-api that receives cloud-continuation ingest.
    /// Without this, enabling cloud continuation must fail closed.
    pub fn continuation_api_url(&self) -> Option<String> {
        std::env::var("ALLTERNIT_CONTINUATION_API_URL")
            .ok()
            .map(|s| s.trim().trim_end_matches('/').to_string())
            .filter(|s| !s.is_empty())
    }

    /// Shared secret for laptop → always-on ingest (`x-allternit-continuation-token`).
    pub fn continuation_token(&self) -> Option<String> {
        std::env::var("ALLTERNIT_CONTINUATION_TOKEN")
            .ok()
            .filter(|s| !s.is_empty())
    }

    /// Workspace directory the always-on cloud worker may write (folder handoff).
    pub fn cloud_workspace_dir(&self) -> PathBuf {
        std::env::var("ALLTERNIT_CLOUD_WORKSPACE")
            .ok()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let root = std::env::var("ALLTERNIT_DATA_DIR")
                    .ok()
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| {
                        dirs::home_dir()
                            .unwrap_or_else(|| PathBuf::from("."))
                            .join(".allternit")
                    });
                root.join("cloud-workspace")
            })
    }

    /// One-time setup token for self-hosted deployments. When configured, the
    /// onboarding wizard can authenticate its save-config call by sending this
    /// value in the `X-Allternit-Self-Hosted-Token` header, bypassing Clerk JWT
    /// verification. `None` disables the path.
    pub fn self_hosted_setup_token(&self) -> Option<String> {
        std::env::var("ALLTERNIT_SELF_HOSTED_SETUP_TOKEN")
            .ok()
            .filter(|s| !s.is_empty())
    }

    /// Secret used to sign enrollment tokens for user-profile consent URLs.
    /// Falls back to the platform encryption key so a packaged deployment has
    /// a stable secret without extra configuration; explicit value preferred.
    pub fn enrollment_secret(&self) -> Option<String> {
        std::env::var("ALLTERNIT_ENROLLMENT_SECRET")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| self.encryption_key())
    }

    /// Tenant identifier for this packaged deployment.
    pub fn tenant_id(&self) -> String {
        self.company
            .tenant_id
            .clone()
            .unwrap_or_else(|| "default".to_string())
    }

    /// URL of the canonical AllternitOS lease authority.
    pub fn allternitos_lease_authority_url(&self) -> Option<String> {
        std::env::var("ALLTERNITOS_LEASE_AUTHORITY_URL")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| {
                self.company
                    .allternitos_lease_authority_url
                    .clone()
                    .filter(|s| !s.is_empty())
            })
    }

    /// URL the frontend should use to reach the API.
    pub fn gateway_url(&self) -> String {
        std::env::var("NEXT_PUBLIC_ALLTERNIT_GATEWAY_URL")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| self.user.gateway_url.clone())
            .or_else(|| self.company.gateway_url.clone())
            .unwrap_or_else(|| "http://localhost:8013".to_string())
    }

    /// Whether onboarding has been completed.
    pub fn onboarding_complete(&self) -> bool {
        self.user.onboarding_complete.unwrap_or(false)
    }

    /// When true, requests originating from localhost without a Clerk token are
    /// accepted as a default local user. This is intended only for local
    /// development and packaged-app smoke tests; it is disabled by default.
    pub fn local_dev_bypass(&self) -> bool {
        std::env::var("ALLTERNIT_LOCAL_DEV_BYPASS")
            .ok()
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(false)
    }

    /// Origins allowed to make cross-origin browser calls, from
    /// `ALLTERNIT_CORS_ORIGINS` (comma-separated). When unset or empty, the
    /// [`crate::cors::DEFAULT_ALLOWED_ORIGINS`] list is used. Requests without
    /// an `Origin` header (non-browser clients) are never CORS-gated.
    pub fn cors_origins(&self) -> Vec<axum::http::HeaderValue> {
        let raw = std::env::var("ALLTERNIT_CORS_ORIGINS").ok();
        crate::cors::parse_allowed_origins(raw.as_deref())
    }

    /// When true, the app is running in self-hosted mode. Clerk is not required
    /// and the desktop bootstrap headers are trusted. Packaged apps can set this
    /// in company.json to avoid distributing Clerk keys.
    pub fn self_hosted(&self) -> bool {
        std::env::var("ALLTERNIT_SELF_HOSTED")
            .ok()
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or_else(|| self.company.self_hosted.unwrap_or(false))
    }

    /// URL of the Rails service.
    pub fn rails_url(&self) -> String {
        std::env::var("ALLTERNIT_RAILS_URL")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| self.company.rails_url.clone())
            .unwrap_or_else(|| "http://127.0.0.1:8080".to_string())
    }

    /// URL of the Etrid native agent wallet service.
    pub fn etrid_url(&self) -> String {
        std::env::var("ALLTERNIT_ETRID_URL")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| self.user.etrid_url.clone())
            .or_else(|| self.company.etrid_url.clone())
            .unwrap_or_else(|| "http://127.0.0.1:8723".to_string())
    }

    /// URL of the Cloudflare Remote Control push worker. Optional.
    pub fn push_worker_url(&self) -> Option<String> {
        std::env::var("ALLTERNIT_PUSH_WORKER_URL")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| self.user.push_worker_url.clone())
            .or_else(|| self.company.push_worker_url.clone())
    }

    /// Rails workspace ID for this deployment.
    pub fn rails_workspace_id(&self) -> String {
        std::env::var("ALLTERNIT_RAILS_WORKSPACE_ID")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| self.company.rails_workspace_id.clone())
            .unwrap_or_else(|| "default".to_string())
    }

    /// Directory used for VM storage.
    pub fn vm_dir(&self) -> Option<PathBuf> {
        std::env::var("ALLTERNIT_VM_DIR")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| self.company.vm_dir.clone())
            .map(PathBuf::from)
    }

    /// Path to the OfficeCLI binary used by the Office add-in gateway routes.
    /// Resolution order: `OFFICECLI_BIN` env → `officecli` on PATH → the
    /// default per-user install location (`~/.officecli/bin/officecli`).
    pub fn officecli_bin(&self) -> PathBuf {
        if let Ok(value) = std::env::var("OFFICECLI_BIN") {
            if !value.is_empty() {
                return PathBuf::from(value);
            }
        }
        if let Some(found) = find_on_path("officecli") {
            return found;
        }
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".officecli")
            .join("bin")
            .join("officecli")
    }

    /// Directory where OfficeCLI document snapshots and artifacts live.
    /// Falls back to `<data_dir>/office-cli` using the same data-dir
    /// convention as `main.rs` (env → platform data dir → /var/lib).
    pub fn office_cli_dir(&self) -> PathBuf {
        std::env::var("ALLTERNIT_OFFICE_CLI_DIR")
            .ok()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::var("ALLTERNIT_DATA_DIR")
                    .ok()
                    .filter(|value| !value.is_empty())
                    .map(PathBuf::from)
                    .or_else(|| dirs::data_dir().map(|d| d.join("allternit")))
                    .unwrap_or_else(|| PathBuf::from("/var/lib/allternit"))
                    .join("office-cli")
            })
    }

    /// Directory where hosted brain remotes (per-user bare git repos) live.
    /// Falls back to `<data_dir>/brains` using the same data-dir convention as
    /// `office_cli_dir()` (env → platform data dir → /var/lib).
    pub fn brains_dir(&self) -> PathBuf {
        std::env::var("ALLTERNIT_BRAINS_DIR")
            .ok()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::var("ALLTERNIT_DATA_DIR")
                    .ok()
                    .filter(|value| !value.is_empty())
                    .map(PathBuf::from)
                    .or_else(|| dirs::data_dir().map(|d| d.join("allternit")))
                    .unwrap_or_else(|| PathBuf::from("/var/lib/allternit"))
                    .join("brains")
            })
    }

    /// Arguments used to spawn the OfficeCLI MCP stdio server
    /// (comma-or-space separated). Verified against officecli 1.0.138: the
    /// stdio server starts on bare `officecli mcp` (targets like `claude` are
    /// for registration, `serve` is not a valid subcommand). The
    /// `OFFICECLI_MCP_ARGS` env override absorbs any future change.
    pub fn officecli_mcp_args(&self) -> Vec<String> {
        let raw = std::env::var("OFFICECLI_MCP_ARGS").unwrap_or_else(|_| "mcp".to_string());
        raw.split(|c: char| c == ',' || c.is_whitespace())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect()
    }

    /// Whether OfficeCLI may run directly against on-disk file paths
    /// (transport model 3). Only meaningful when the gateway shares the user's
    /// filesystem, so it defaults to true for self-hosted / local-dev
    /// deployments and false otherwise.
    pub fn officecli_live_fs(&self) -> bool {
        std::env::var("ALLTERNIT_OFFICECLI_LIVE_FS")
            .ok()
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or_else(|| self.self_hosted() || self.local_dev_bypass())
    }

    /// Port range available to `officecli watch` preview servers.
    pub fn officecli_watch_ports(&self) -> std::ops::RangeInclusive<u16> {
        let raw = std::env::var("ALLTERNIT_OFFICECLI_WATCH_PORTS")
            .unwrap_or_else(|_| "26400-26419".to_string());
        let parsed = raw.split_once('-').and_then(|(start, end)| {
            Some(start.trim().parse::<u16>().ok()?..=end.trim().parse::<u16>().ok()?)
        });
        parsed.unwrap_or(26400..=26419)
    }

    /// Cloud desktop control-plane URL. Used by the `cloud-desktop` vmOperator
    /// provider to provision a remote virtual computer without changing the
    /// agent's selected brain.
    pub fn cloud_desktop_url(&self) -> String {
        std::env::var("ALLTERNIT_CLOUD_DESKTOP_URL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "http://localhost:57110".to_string())
    }

    /// Ollama base URL.
    pub fn ollama_url(&self) -> String {
        std::env::var("OLLAMA_URL")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| self.user.ollama_url.clone())
            .unwrap_or_else(|| "http://localhost:11434".to_string())
    }

    /// Memory service URL.
    pub fn memory_url(&self) -> String {
        std::env::var("ALLTERNIT_MEMORY_URL")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| self.user.memory_url.clone())
            .unwrap_or_else(|| "http://127.0.0.1:4096".to_string())
    }

    /// Voice service URL (services/voice — TTS/STT). Optional off-gateway
    /// service; the /api/v1/voice routes answer their graceful-degradation
    /// fallbacks when it's unreachable.
    pub fn voice_url(&self) -> String {
        std::env::var("ALLTERNIT_VOICE_URL")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| self.user.voice_url.clone())
            .unwrap_or_else(|| "http://127.0.0.1:8001".to_string())
    }

    /// Embedding service URL.
    pub fn embedding_url(&self) -> String {
        std::env::var("ALLTERNIT_EMBEDDING_URL")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| self.user.embedding_url.clone())
            .unwrap_or_else(|| self.ollama_url())
    }

    /// Agent working directory.
    pub fn agent_workdir(&self) -> Option<String> {
        std::env::var("ALLTERNIT_AGENT_WORKDIR")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| self.user.agent_workdir.clone())
    }

    /// Cron daemon URL.
    ///
    /// The gizzi-code terminal server exposes the cron API under `/cron`, so the
    /// default points at the local gizzi server (`http://127.0.0.1:4096/cron`).
    pub fn cron_daemon_url(&self) -> String {
        std::env::var("ALLTERNIT_CRON_DAEMON_URL")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| self.user.cron_daemon_url.clone())
            .or_else(|| self.company.cron_daemon_url.clone())
            .unwrap_or_else(|| "http://127.0.0.1:4096/cron".to_string())
    }

    /// ACU (computer-use) gateway base URL. The Python/FastAPI gateway in
    /// `domains/computer-use/core` serves the computer-use API under
    /// `/v1/computer-use` on port 8760 by default; `/api/aci/*` proxies to it.
    pub fn acu_url(&self) -> String {
        std::env::var("ALLTERNIT_ACU_URL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "http://127.0.0.1:8760".to_string())
    }

    /// Whether tools that can execute arbitrary code or read sensitive host
    /// state (shell.exec, file.write, system.env, local VM fallback, etc.) are
    /// enabled. Local-computer / host-control access is opt-in and defaults to
    /// false. Resolution order: env > user config > company config > false.
    pub fn host_control_enabled(&self) -> bool {
        std::env::var("ALLTERNIT_HOST_CONTROL_ENABLED")
            .ok()
            .and_then(|v| v.parse().ok())
            .or(self.user.host_control_enabled)
            .or(self.company.host_control_enabled)
            .unwrap_or(false)
    }

    /// Office engine (services/office-engine) base URL. The TypeScript Hono
    /// service exposes `POST /parse` and `POST /docx/roundtrip`; the
    /// `/api/office/*` gateway routes proxy to it.
    pub fn office_engine_url(&self) -> String {
        std::env::var("OFFICE_ENGINE_URL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "http://127.0.0.1:8099".to_string())
    }

    /// Gizzi runtime port. Used when a full URL is not supplied.
    pub fn gizzi_port(&self) -> u16 {
        std::env::var("GIZZI_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(4096)
    }

    /// Return the currently active permission policy, if one is configured.
    /// User config selects the active policy name and overrides company policy
    /// definitions with the same name.
    pub fn active_permission_policy(&self) -> Option<crate::permission_policy::PermissionPolicy> {
        let mut by_name: std::collections::HashMap<String, crate::permission_policy::PermissionPolicy> =
            std::collections::HashMap::new();
        if let Some(policies) = self.company.permission_policies.clone() {
            for policy in policies {
                by_name.insert(policy.name.clone(), policy);
            }
        }
        if let Some(policies) = self.user.permission_policies.clone() {
            for policy in policies {
                by_name.insert(policy.name.clone(), policy);
            }
        }
        let active_name = self
            .user
            .active_permission_policy
            .as_ref()
            .or_else(|| self.company.active_permission_policy.as_ref())?;
        by_name.remove(active_name)
    }

    /// Apply env-variable overrides to the in-memory config. This keeps the
    /// existing dev/CI workflow working while the file-based config becomes
    /// the primary path for packaged users.
    fn apply_env_overrides(&mut self) {
        if let Ok(v) = std::env::var("ALLTERNIT_DEFAULT_MODEL") {
            if !v.is_empty() {
                self.user.default_model = Some(v);
            }
        }
        if let Ok(v) = std::env::var("TERMINAL_SERVER_URL") {
            if !v.is_empty() {
                self.user.terminal_server_url = Some(v);
            }
        }
        if let Ok(v) = std::env::var("OLLAMA_URL") {
            if !v.is_empty() {
                self.user.ollama_url = Some(v);
            }
        }
        if let Ok(v) = std::env::var("ALLTERNIT_MEMORY_URL") {
            if !v.is_empty() {
                self.user.memory_url = Some(v);
            }
        }
        if let Ok(v) = std::env::var("ALLTERNIT_VOICE_URL") {
            if !v.is_empty() {
                self.user.voice_url = Some(v);
            }
        }
        if let Ok(v) = std::env::var("ALLTERNIT_EMBEDDING_URL") {
            if !v.is_empty() {
                self.user.embedding_url = Some(v);
            }
        }
        if let Ok(v) = std::env::var("ALLTERNIT_AGENT_WORKDIR") {
            if !v.is_empty() {
                self.user.agent_workdir = Some(v);
            }
        }
        if let Ok(v) = std::env::var("ALLTERNIT_CRON_DAEMON_URL") {
            if !v.is_empty() {
                self.user.cron_daemon_url = Some(v);
            }
        }
        if let Ok(v) = std::env::var("ALLTERNIT_ETRID_URL") {
            if !v.is_empty() {
                self.user.etrid_url = Some(v);
            }
        }
        if let Ok(v) = std::env::var("ALLTERNIT_PUSH_WORKER_URL") {
            if !v.is_empty() {
                self.user.push_worker_url = Some(v);
            }
        }
        if let Ok(v) = std::env::var("ALLTERNIT_HOST_CONTROL_ENABLED") {
            if !v.is_empty() {
                if let Ok(b) = v.parse::<bool>() {
                    self.user.host_control_enabled = Some(b);
                }
            }
        }
    }
}

fn load_company_config() -> CompanyConfig {
    let paths = company_config_paths();
    for path in &paths {
        if let Ok(text) = std::fs::read_to_string(path) {
            match serde_json::from_str::<CompanyConfig>(&text) {
                Ok(config) => {
                    info!(path = %path.display(), ?config, "Loaded company config");
                    return config;
                }
                Err(err) => {
                    warn!(path = %path.display(), error = %err, "Failed to parse company config");
                }
            }
        }
    }
    info!("No company config found; using defaults");
    CompanyConfig::default()
}

fn load_user_config() -> UserConfig {
    let path = user_config_path();
    if let Ok(text) = std::fs::read_to_string(&path) {
        match serde_json::from_str::<UserConfig>(&text) {
            Ok(config) => {
                info!(path = %path.display(), "Loaded user config");
                return config;
            }
            Err(err) => {
                warn!(path = %path.display(), error = %err, "Failed to parse user config");
            }
        }
    }
    info!(path = %path.display(), "No user config found; using defaults");
    UserConfig::default()
}

fn company_config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    // Packaged app bundle paths.
    // macOS .app: Contents/MacOS/<binary> -> Contents/Resources/company.json
    // Generic layout: <binary_dir>/resources/company.json or <binary_dir>/company.json
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            paths.push(exe_dir.join("resources").join("company.json"));
            paths.push(exe_dir.join("company.json"));
            if let Some(bundle_dir) = exe_dir.parent() {
                paths.push(bundle_dir.join("Resources").join("company.json"));
                paths.push(bundle_dir.join("resources").join("company.json"));
            }
        }
    }

    // Dev override in the user's home directory.
    if let Some(home) = dirs::home_dir() {
        paths.push(home.join(".allternit").join("company.json"));
    }

    paths
}

/// Locate an executable on PATH (first match wins).
fn find_on_path(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn user_config_path() -> PathBuf {
    dirs::data_dir()
        .or_else(dirs::home_dir)
        .map(|p| p.join("allternit"))
        .unwrap_or_else(|| PathBuf::from(".allternit"))
        .join("config.json")
}

/// Persist user config to disk. Called by the onboarding wizard and settings UI.
pub fn save_user_config(config: &UserConfig) -> std::io::Result<()> {
    let path = user_config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(config)?;
    std::fs::write(&path, text)?;
    info!(path = %path.display(), "Saved user config");
    Ok(())
}

/// API endpoint payload used by the onboarding wizard to save user config.
#[derive(Debug, Deserialize)]
pub struct SaveUserConfigPayload {
    #[serde(rename = "defaultModel")]
    pub default_model: Option<String>,
    #[serde(rename = "terminalServerUrl")]
    pub terminal_server_url: Option<String>,
    #[serde(rename = "gatewayUrl")]
    pub gateway_url: Option<String>,
    #[serde(rename = "providerApiKeys")]
    pub provider_api_keys: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(rename = "onboardingComplete")]
    pub onboarding_complete: Option<bool>,
    #[serde(rename = "ollamaUrl")]
    pub ollama_url: Option<String>,
    #[serde(rename = "memoryUrl")]
    pub memory_url: Option<String>,
    #[serde(rename = "embeddingUrl")]
    pub embedding_url: Option<String>,
    #[serde(rename = "agentWorkdir")]
    pub agent_workdir: Option<String>,
    #[serde(rename = "cronDaemonUrl")]
    pub cron_daemon_url: Option<String>,
    #[serde(rename = "etridUrl")]
    pub etrid_url: Option<String>,
    #[serde(rename = "hostControlEnabled")]
    pub host_control_enabled: Option<bool>,
    #[serde(rename = "wizard")]
    pub wizard: Option<WizardState>,
}

impl From<SaveUserConfigPayload> for UserConfig {
    fn from(payload: SaveUserConfigPayload) -> Self {
        Self {
            default_model: payload.default_model,
            terminal_server_url: payload.terminal_server_url,
            gateway_url: payload.gateway_url,
            // Never copy provider credentials into Allternit's user config.
            provider_api_keys: None,
            onboarding_complete: payload.onboarding_complete,
            ollama_url: payload.ollama_url,
            memory_url: payload.memory_url,
            embedding_url: payload.embedding_url,
            // Voice service is env/config-file only for now — the wizard has
            // no voice URL field yet.
            voice_url: None,
            agent_workdir: payload.agent_workdir,
            cron_daemon_url: payload.cron_daemon_url,
            etrid_url: payload.etrid_url,
            host_control_enabled: payload.host_control_enabled,
            push_worker_url: None,
            wizard: payload.wizard,
            permission_policies: None,
            active_permission_policy: None,
            inference_router: None,
        }
    }
}

/// Best-effort read of the default model from the Gizzi runtime user config.
/// This lets the Allternit UI reflect a brain that was already configured
/// directly in Gizzi without requiring the user to re-run the wizard.
/// Falls back to the first provider/model entry when no explicit default is set.
fn read_gizzi_default_model() -> Option<String> {
    let path = gizzi_user_config_path();
    let text = std::fs::read_to_string(&path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;

    if let Some(model) = value.get("model").and_then(|v| v.as_str()) {
        return Some(model.to_string());
    }

    // Infer default from the first provider's first model.
    value
        .get("provider")?
        .as_object()?
        .iter()
        .next()
        .and_then(|(provider_id, provider)| {
            let model_id = provider
                .get("models")
                .and_then(|m| m.as_object())
                .and_then(|m| m.keys().next())
                .cloned()?;
            Some(format!("{}/{}", provider_id, model_id))
        })
}

/// Convenience builder that looks up a provider by id from the Gizzi runtime
/// config and builds its harness (injecting any stored API key for subprocess
/// providers).
pub fn build_gizzi_harness_for_provider(provider_id: &str) -> Option<serde_json::Value> {
    let path = gizzi_user_config_path();
    let text = std::fs::read_to_string(&path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let provider = value.get("provider")?.as_object()?.get(provider_id)?;
    build_gizzi_harness(provider_id, provider)
}

/// Best-effort build of a Gizzi harness for a provider configured in the Gizzi
/// runtime user config. Credentials are resolved exclusively by Gizzi and are
/// never inserted into a session payload by allternit-api.
pub fn build_gizzi_harness(
    _provider_id: &str,
    provider: &serde_json::Value,
) -> Option<serde_json::Value> {
    let provider = provider.as_object()?;

    // Subprocess provider (e.g. claude-cli, custom local scripts).
    if let Some(cmd) = provider.get("subprocess_cmd").and_then(|v| v.as_str()) {
        return Some(serde_json::json!({
            "mode": "subprocess",
            "subprocess": { "command": cmd, "env": {} }
        }));
    }

    // API-key providers are already configured in Gizzi and Gizzi owns their
    // credentials. Omitting a BYOK harness prevents a secret-less override from
    // shadowing the runtime's authenticated provider.
    if provider.get("auth_type").and_then(|v| v.as_str()) == Some("api_key") {
        return None;
    }

    // Local/OpenAI-compatible provider (e.g. Ollama).
    if let Some(base_url) = provider
        .get("options")
        .and_then(|o| o.get("baseURL"))
        .and_then(|v| v.as_str())
    {
        return Some(serde_json::json!({
            "mode": "local",
            "local": { "baseURL": base_url }
        }));
    }

    None
}

/// Best-effort build of a Gizzi harness for the default model configured in
/// the Gizzi runtime user config. This lets the Allternit API create sessions
/// that route through the user's chosen brain (subprocess, local, or BYOK)
/// without requiring an agent record in the platform DB.
pub fn read_gizzi_default_harness() -> Option<serde_json::Value> {
    let path = gizzi_user_config_path();
    let text = std::fs::read_to_string(&path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;

    let provider_id = value
        .get("model")
        .and_then(|v| v.as_str())
        .and_then(|m| m.split_once('/').map(|(p, _)| p.to_string()))?;

    let provider = value
        .get("provider")?
        .as_object()?
        .get(&provider_id)?
        .clone();
    build_gizzi_harness(&provider_id, &provider)
}

/// Path to the gizzi-code global config file that the runtime actually reads.
///
/// This MUST mirror gizzi-code's own resolution:
///   - config dir = `Global.Path.config` = xdg-basedir config + `"gizzi-code"`
///     (`src/runtime/context/global/paths.ts`)
///   - filename   = `config.json` (`src/cli/commands/provider.ts`)
///
/// Previously this resolved to `<dirs::config_dir>/gizzi/gizzi.json`, which on
/// macOS is `~/Library/Application Support/gizzi/gizzi.json` — a file the gizzi
/// runtime never opens. That is why "add a brain instead of kimi" silently did
/// nothing in packaged builds: the wizard wrote the provider to the wrong file
/// and `read_gizzi_default_harness` read it back from the same wrong place, so
/// every session kept falling back to the built-in default.
///
/// Precedence: `GIZZI_CONFIG_HOME` (dir) > `XDG_CONFIG_HOME` > platform default
/// (`~/.config` on macOS/Linux to match xdg-basedir, `%APPDATA%` on Windows).
pub(crate) fn gizzi_user_config_path() -> PathBuf {
    if let Ok(dir) = std::env::var("GIZZI_CONFIG_HOME") {
        if !dir.is_empty() {
            return PathBuf::from(dir).join("config.json");
        }
    }
    let base = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            if cfg!(windows) {
                std::env::var("APPDATA").ok().map(PathBuf::from)
            } else {
                dirs::home_dir().map(|h| h.join(".config"))
            }
        })
        .unwrap_or_else(|| PathBuf::from(".config"));
    base.join("gizzi-code").join("config.json")
}

/// One-time migration from the legacy gizzi config paths the API used to write
/// (`<dirs::config_dir>/gizzi/gizzi.json`, and the xdg-style
/// `~/.config/gizzi/{gizzi,config}.json` leftovers) into the file the gizzi-code
/// runtime actually reads: `<xdg-config>/gizzi-code/config.json`.
///
/// Best-effort and idempotent: if the target already exists, or no legacy file
/// is present, it does nothing. When several legacy files exist, the richest
/// (most configured providers) wins so a thin auto-generated file never shadows
/// the user's real brain set. Legacy files are copied (never deleted) so the
/// operation is reversible.
fn migrate_legacy_gizzi_config() {
    let target = gizzi_user_config_path();
    if target.exists() {
        return;
    }

    let mut candidates: Vec<PathBuf> = Vec::new();
    // Where the old API wrote on each platform (macOS: ~/Library/Application Support).
    if let Some(dir) = dirs::config_dir() {
        candidates.push(dir.join("gizzi").join("gizzi.json"));
    }
    // xdg-style leftovers observed in the wild.
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".config").join("gizzi").join("gizzi.json"));
        candidates.push(home.join(".config").join("gizzi").join("config.json"));
    }

    let provider_count = |path: &PathBuf| -> usize {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| {
                v.get("provider")
                    .and_then(|p| p.as_object())
                    .map(|o| o.len())
            })
            .unwrap_or(0)
    };

    let Some(source) = candidates
        .into_iter()
        .filter(|p| p.is_file())
        .max_by_key(|p| provider_count(p))
    else {
        return;
    };

    if provider_count(&source) == 0 {
        return;
    }

    if let Some(parent) = target.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            warn!(error = %err, path = %parent.display(), "Failed to create gizzi config dir for migration");
            return;
        }
    }

    match std::fs::copy(&source, &target) {
        Ok(bytes) => {
            info!(from = %source.display(), to = %target.display(), bytes, "Migrated legacy gizzi brain config")
        }
        Err(err) => {
            warn!(error = %err, from = %source.display(), to = %target.display(), "Failed to migrate legacy gizzi brain config")
        }
    }
}
