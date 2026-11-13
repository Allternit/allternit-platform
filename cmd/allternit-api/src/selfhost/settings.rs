//! Settings for `allternit-agents-server`, per
//! `openapi/selfhost-settings.schema.json`.
//!
//! The file is YAML or JSON (YAML is a superset of JSON, so one parser reads
//! both). Its path comes from `--config` or `ALLTERNIT_SELFHOST_CONFIG`.
//! Unknown keys are refused, like `additionalProperties: false` in the
//! schema.
//!
//! Secrets never sit in the file. A secret field holds a reference:
//! `env:NAME`, `file:/path` or `vault:path#key`. [`Settings::secret`] reads
//! any of them through [`super::secrets::SecretResolver`] (Vault KV v2 over
//! the egress client). The sync [`resolve_secret`] reads only `env:` and
//! `file:`, since a Vault read needs the network.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::store::Role;

pub const CONFIG_ENV: &str = "ALLTERNIT_SELFHOST_CONFIG";
pub const DEFAULT_LISTEN: &str = "127.0.0.1:8400";
pub const DEFAULT_DATA_DIR: &str = "/var/lib/allternit-agents-server";

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("no settings file: pass --config <path> or set {CONFIG_ENV}")]
    NoConfig,
    #[error("cannot read settings file {path}: {source}")]
    Read {
        path: String,
        source: std::io::Error,
    },
    #[error("settings file is not valid: {0}")]
    Parse(String),
    #[error("invalid setting {field}: {message}")]
    Invalid { field: String, message: String },
    #[error("secret reference {0} is not valid: use env:NAME, file:/path or vault:path#key")]
    BadSecretRef(String),
    #[error("secret {reference}: environment variable {name} is not set")]
    EnvMissing { reference: String, name: String },
    #[error("secret {reference}: cannot read file: {message}")]
    FileUnreadable { reference: String, message: String },
    #[error("secret {0}: secrets.vault is not set in the settings file")]
    VaultNotConfigured(String),
    #[error("secret {0}: a vault: reference needs the async resolver (Settings::secret)")]
    VaultNeedsResolver(String),
    #[error("secret {reference}: Vault: {message}")]
    Vault { reference: String, message: String },
}

fn invalid(field: &str, message: impl Into<String>) -> SettingsError {
    SettingsError::Invalid {
        field: field.to_string(),
        message: message.into(),
    }
}

// ─── Shape ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,
    #[serde(default)]
    pub ephemeral: bool,
    #[serde(default)]
    pub server: ServerSettings,
    #[serde(default)]
    pub database: DatabaseSettings,
    pub inference: InferenceSettings,
    #[serde(default)]
    pub mcp: McpSettings,
    #[serde(default)]
    pub git: GitSettings,
    #[serde(default)]
    pub web_tools: WebToolsSettings,
    #[serde(default)]
    pub auth: AuthSettings,
    #[serde(default)]
    pub secrets: SecretsSettings,
    #[serde(default)]
    pub observability: ObservabilitySettings,
    #[serde(default)]
    pub health: HealthSettings,
    #[serde(default)]
    pub sessions: SessionsSettings,
}

/// For tests and tools: an ephemeral server on the anthropic provider.
impl Default for Settings {
    fn default() -> Self {
        Self {
            data_dir: default_data_dir(),
            ephemeral: true,
            server: ServerSettings::default(),
            database: DatabaseSettings::default(),
            inference: InferenceSettings::default(),
            mcp: McpSettings::default(),
            git: GitSettings::default(),
            web_tools: WebToolsSettings::default(),
            auth: AuthSettings::default(),
            secrets: SecretsSettings::default(),
            observability: ObservabilitySettings::default(),
            health: HealthSettings::default(),
            sessions: SessionsSettings::default(),
        }
    }
}

fn default_data_dir() -> PathBuf {
    PathBuf::from(DEFAULT_DATA_DIR)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerSettings {
    #[serde(default = "default_listen")]
    pub listen: String,
    #[serde(default)]
    pub public_url: Option<String>,
    /// Exact browser origins. Empty means no CORS headers. The literal
    /// `null` allows `Origin: null` (Desktop's file:/app: renderer); it is
    /// never implied.
    #[serde(default)]
    pub cors_origins: Vec<String>,
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            listen: default_listen(),
            public_url: None,
            cors_origins: vec![],
        }
    }
}

fn default_listen() -> String {
    DEFAULT_LISTEN.to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseSettings {
    /// A `postgres://` URL or a secret reference to one.
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default = "default_max_connections")]
    pub max_connections: u32,
}

impl Default for DatabaseSettings {
    fn default() -> Self {
        Self {
            url: None,
            max_connections: default_max_connections(),
        }
    }
}

fn default_max_connections() -> u32 {
    20
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    #[default]
    Anthropic,
    Bedrock,
    Vertex,
    Foundry,
    Gateway,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::Anthropic => "anthropic",
            Provider::Bedrock => "bedrock",
            Provider::Vertex => "vertex",
            Provider::Foundry => "foundry",
            Provider::Gateway => "gateway",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GatewayFormat {
    AnthropicMessages,
    BedrockInvoke,
    VertexRawPredict,
    OpenaiChat,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatewaySettings {
    pub format: GatewayFormat,
    pub base_url: String,
    /// Values may be secret references.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InferenceCredentials {
    pub api_key: Option<String>,
    pub aws_access_key_id: Option<String>,
    pub aws_secret_access_key: Option<String>,
    pub aws_session_token: Option<String>,
    pub aws_region: Option<String>,
    pub gcp_service_account_json: Option<String>,
    pub gcp_project: Option<String>,
    pub gcp_region: Option<String>,
    pub foundry_endpoint: Option<String>,
    pub foundry_api_key: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InferenceSettings {
    pub provider: Provider,
    #[serde(default)]
    pub gateway: Option<GatewaySettings>,
    /// Agent model name to provider model id. When set (and always for the
    /// gateway provider), a model not listed is refused with 422
    /// `model_not_mapped` before any outbound request.
    #[serde(default)]
    pub model_map: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub credentials: Option<InferenceCredentials>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpSettings {
    #[serde(default)]
    pub allowed_hostnames: Vec<String>,
    #[serde(default = "default_mcp_max")]
    pub max_result_bytes: u64,
}

impl Default for McpSettings {
    fn default() -> Self {
        Self {
            allowed_hostnames: vec![],
            max_result_bytes: default_mcp_max(),
        }
    }
}

fn default_mcp_max() -> u64 {
    102_400
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitSettings {
    #[serde(default)]
    pub allowed_hostnames: Vec<String>,
    #[serde(default)]
    pub allowed_push_refs: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebToolsSettings {
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthSettings {
    #[serde(default)]
    pub oidc: Option<OidcSettings>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OidcDefaultRole {
    Admin,
    Developer,
    Viewer,
    #[default]
    None,
}

impl OidcDefaultRole {
    pub fn role(self) -> Option<Role> {
        match self {
            OidcDefaultRole::Admin => Some(Role::Admin),
            OidcDefaultRole::Developer => Some(Role::Developer),
            OidcDefaultRole::Viewer => Some(Role::Viewer),
            OidcDefaultRole::None => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OidcSettings {
    pub issuer: String,
    pub audiences: Vec<String>,
    /// Where the signing keys are. When absent, it comes from the issuer's
    /// discovery document, and must then be on the issuer's own host.
    #[serde(default)]
    pub jwks_uri: Option<String>,
    #[serde(default = "default_role_claim")]
    pub role_claim: String,
    #[serde(default)]
    pub role_map: BTreeMap<String, Role>,
    #[serde(default)]
    pub default_role: OidcDefaultRole,
}

fn default_role_claim() -> String {
    "groups".to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SecretBackendKind {
    #[default]
    Env,
    File,
    Vault,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VaultAuthMethod {
    Token,
    Approle,
    Kubernetes,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VaultAuth {
    pub method: VaultAuthMethod,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub role_id: Option<String>,
    #[serde(default)]
    pub secret_id: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VaultSettings {
    pub address: String,
    #[serde(default = "default_vault_mount")]
    pub mount: String,
    pub path: String,
    #[serde(default)]
    pub auth: Option<VaultAuth>,
}

fn default_vault_mount() -> String {
    "secret".to_string()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretsSettings {
    #[serde(default)]
    pub backend: SecretBackendKind,
    #[serde(default)]
    pub vault: Option<VaultSettings>,
    #[serde(default)]
    pub encryption_key_ref: Option<String>,
    /// Earlier keys, still accepted when opening values sealed before a
    /// rotation. `admin rotate-key` re-seals those with the current key.
    #[serde(default)]
    pub previous_key_refs: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    #[default]
    Json,
    Text,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrometheusSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_prom_listen")]
    pub listen: String,
}

impl Default for PrometheusSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            listen: default_prom_listen(),
        }
    }
}

fn default_prom_listen() -> String {
    "127.0.0.1:9464".to_string()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservabilitySettings {
    #[serde(default)]
    pub log_format: LogFormat,
    #[serde(default)]
    pub prometheus: PrometheusSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthSettings {
    #[serde(default = "default_check_interval")]
    pub check_interval: String,
}

impl Default for HealthSettings {
    fn default() -> Self {
        Self {
            check_interval: default_check_interval(),
        }
    }
}

fn default_check_interval() -> String {
    "60s".to_string()
}

/// Session and work item timing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionsSettings {
    /// A session with no turn running and no pending tool call for this long
    /// ends its work item: `tool_calls/next` answers 204 with
    /// `X-Session-Closed: true`. A later turn queues a new item.
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout: String,
    /// How long a tool call waits for a worker to take it before the
    /// internal route answers 504 `no_worker`.
    #[serde(default = "default_no_worker_timeout")]
    pub no_worker_timeout: String,
}

impl Default for SessionsSettings {
    fn default() -> Self {
        Self {
            idle_timeout: default_idle_timeout(),
            no_worker_timeout: default_no_worker_timeout(),
        }
    }
}

fn default_idle_timeout() -> String {
    "1h".to_string()
}

fn default_no_worker_timeout() -> String {
    "10m".to_string()
}

// ─── Loading ────────────────────────────────────────────────────────────────

/// `--config <path>` / `--config=<path>` from args, else the env var.
pub fn config_path(args: &[String]) -> Option<PathBuf> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--config" || a == "-c" {
            return it.next().map(PathBuf::from);
        }
        if let Some(v) = a.strip_prefix("--config=") {
            return Some(PathBuf::from(v));
        }
    }
    std::env::var(CONFIG_ENV)
        .ok()
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

impl Settings {
    pub fn load(path: &Path) -> Result<Settings, SettingsError> {
        let text = std::fs::read_to_string(path).map_err(|source| SettingsError::Read {
            path: path.display().to_string(),
            source,
        })?;
        Settings::from_str(&text)
    }

    /// Parse YAML or JSON and validate.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(text: &str) -> Result<Settings, SettingsError> {
        let s: Settings =
            serde_yaml::from_str(text).map_err(|e| SettingsError::Parse(e.to_string()))?;
        s.validate()?;
        Ok(s)
    }

    /// The checks JSON Schema expresses that serde does not.
    pub fn validate(&self) -> Result<(), SettingsError> {
        if self.server.listen.trim().is_empty() {
            return Err(invalid("server.listen", "must be host:port"));
        }
        if let Some(u) = &self.server.public_url {
            parse_url("server.public_url", u)?;
        }
        for o in &self.server.cors_origins {
            if o == "*" {
                return Err(invalid(
                    "server.cors_origins",
                    "\"*\" is not allowed; list exact origins",
                ));
            }
            if o != "null" {
                let u = parse_url("server.cors_origins", o)?;
                if u.path() != "/" || u.query().is_some() {
                    return Err(invalid(
                        "server.cors_origins",
                        format!("{o} is not an origin (scheme://host[:port])"),
                    ));
                }
            }
        }
        if let Some(url) = &self.database.url {
            let is_pg = url.starts_with("postgres://") || url.starts_with("postgresql://");
            if !is_pg {
                check_secret_ref("database.url", url)?;
            }
        }
        if self.database.max_connections == 0 {
            return Err(invalid("database.max_connections", "must be at least 1"));
        }

        let inf = &self.inference;
        if inf.provider == Provider::Gateway {
            let Some(gw) = &inf.gateway else {
                return Err(invalid("inference.gateway", "required for the gateway provider"));
            };
            if inf.model_map.as_ref().map_or(true, |m| m.is_empty()) {
                return Err(invalid(
                    "inference.model_map",
                    "required (and not empty) for the gateway provider",
                ));
            }
            parse_url("inference.gateway.base_url", &gw.base_url)?;
        }
        if let Some(gw) = &inf.gateway {
            parse_url("inference.gateway.base_url", &gw.base_url)?;
        }
        if let Some(m) = &inf.model_map {
            if m.values().any(|v| v.is_empty()) {
                return Err(invalid("inference.model_map", "provider model ids cannot be empty"));
            }
        }
        if let Some(c) = &inf.credentials {
            for (field, v) in [
                ("api_key", &c.api_key),
                ("aws_access_key_id", &c.aws_access_key_id),
                ("aws_secret_access_key", &c.aws_secret_access_key),
                ("aws_session_token", &c.aws_session_token),
                ("gcp_service_account_json", &c.gcp_service_account_json),
                ("foundry_api_key", &c.foundry_api_key),
            ] {
                if let Some(v) = v {
                    check_secret_ref(&format!("inference.credentials.{field}"), v)?;
                }
            }
            if let Some(e) = &c.foundry_endpoint {
                parse_url("inference.credentials.foundry_endpoint", e)?;
            }
        }

        check_hostnames("mcp.allowed_hostnames", &self.mcp.allowed_hostnames)?;
        if self.mcp.max_result_bytes == 0 {
            return Err(invalid("mcp.max_result_bytes", "must be at least 1"));
        }
        check_hostnames("git.allowed_hostnames", &self.git.allowed_hostnames)?;

        if let Some(o) = &self.auth.oidc {
            parse_url("auth.oidc.issuer", &o.issuer)?;
            if o.audiences.is_empty() {
                return Err(invalid("auth.oidc.audiences", "needs at least one audience"));
            }
            if let Some(j) = &o.jwks_uri {
                parse_url("auth.oidc.jwks_uri", j)?;
            }
            if o.role_claim.is_empty() {
                return Err(invalid("auth.oidc.role_claim", "cannot be empty"));
            }
        }

        if self.secrets.backend == SecretBackendKind::Vault && self.secrets.vault.is_none() {
            return Err(invalid("secrets.vault", "required when secrets.backend is vault"));
        }
        if let Some(v) = &self.secrets.vault {
            parse_url("secrets.vault.address", &v.address)?;
            if let Some(a) = &v.auth {
                if let Some(t) = &a.token {
                    check_secret_ref("secrets.vault.auth.token", t)?;
                }
                if let Some(t) = &a.secret_id {
                    check_secret_ref("secrets.vault.auth.secret_id", t)?;
                }
            }
        }
        if let Some(k) = &self.secrets.encryption_key_ref {
            check_secret_ref("secrets.encryption_key_ref", k)?;
        }
        for k in &self.secrets.previous_key_refs {
            check_secret_ref("secrets.previous_key_refs", k)?;
        }
        if !self.secrets.previous_key_refs.is_empty() && self.secrets.encryption_key_ref.is_none() {
            return Err(invalid(
                "secrets.previous_key_refs",
                "needs secrets.encryption_key_ref (the current key)",
            ));
        }
        parse_duration(&self.health.check_interval)
            .ok_or_else(|| invalid("health.check_interval", "use a number and ms, s, m or h"))?;
        for (field, v) in [
            ("sessions.idle_timeout", &self.sessions.idle_timeout),
            ("sessions.no_worker_timeout", &self.sessions.no_worker_timeout),
        ] {
            match parse_duration(v) {
                Some(d) if !d.is_zero() => {}
                _ => return Err(invalid(field, "use a number above 0 and ms, s, m or h")),
            }
        }
        Ok(())
    }

    /// May an agent use this model? With a model map, only mapped names are
    /// allowed. The gateway provider always needs one.
    pub fn model_allowed(&self, model: &str) -> bool {
        match &self.inference.model_map {
            Some(m) if !m.is_empty() => m.contains_key(model),
            _ => self.inference.provider != Provider::Gateway,
        }
    }

    /// Read a secret reference of any kind (`env:`, `file:`, `vault:`).
    /// Vault reads go through an egress client built from these settings.
    pub async fn secret(&self, reference: &str) -> Result<String, SettingsError> {
        super::secrets::SecretResolver::from_settings(self)?
            .resolve(reference)
            .await
    }

    /// Like [`Settings::database_url`], but a `vault:` reference is read too.
    pub async fn database_url_resolved(&self) -> Result<Option<String>, SettingsError> {
        match &self.database.url {
            None => Ok(None),
            Some(u) if u.starts_with("postgres://") || u.starts_with("postgresql://") => {
                Ok(Some(u.clone()))
            }
            Some(r) => self.secret(r).await.map(Some),
        }
    }

    /// The Postgres URL, with an `env:`/`file:` reference resolved. None
    /// means SQLite or memory. Use [`Settings::database_url_resolved`] when
    /// the reference may be `vault:`.
    pub fn database_url(&self) -> Result<Option<String>, SettingsError> {
        match &self.database.url {
            None => Ok(None),
            Some(u) if u.starts_with("postgres://") || u.starts_with("postgresql://") => {
                Ok(Some(u.clone()))
            }
            Some(r) => resolve_secret(r).map(Some),
        }
    }

    pub fn health_check_interval(&self) -> std::time::Duration {
        parse_duration(&self.health.check_interval).unwrap_or(std::time::Duration::from_secs(60))
    }

    /// `sessions.idle_timeout`, default 1 hour.
    pub fn idle_timeout(&self) -> std::time::Duration {
        parse_duration(&self.sessions.idle_timeout)
            .filter(|d| !d.is_zero())
            .unwrap_or(std::time::Duration::from_secs(3600))
    }

    /// `sessions.no_worker_timeout`, default 10 minutes.
    pub fn no_worker_timeout(&self) -> std::time::Duration {
        parse_duration(&self.sessions.no_worker_timeout)
            .filter(|d| !d.is_zero())
            .unwrap_or(std::time::Duration::from_secs(600))
    }
}

fn parse_url(field: &str, value: &str) -> Result<url::Url, SettingsError> {
    let u = url::Url::parse(value).map_err(|e| invalid(field, format!("{value}: {e}")))?;
    if u.host_str().is_none() {
        return Err(invalid(field, format!("{value} has no host")));
    }
    Ok(u)
}

fn check_hostnames(field: &str, hosts: &[String]) -> Result<(), SettingsError> {
    let mut seen = std::collections::BTreeSet::new();
    for h in hosts {
        if h.is_empty() || h.contains('/') || h.contains('*') || h.chars().any(char::is_whitespace) {
            return Err(invalid(
                field,
                format!("{h:?} must be an exact hostname, optionally with a port (no scheme, path or wildcard)"),
            ));
        }
        if !seen.insert(h.to_ascii_lowercase()) {
            return Err(invalid(field, format!("{h} is listed twice")));
        }
    }
    Ok(())
}

fn check_secret_ref(field: &str, value: &str) -> Result<(), SettingsError> {
    let ok = ["env:", "file:", "vault:"]
        .iter()
        .any(|p| value.strip_prefix(p).is_some_and(|rest| !rest.is_empty()));
    if ok {
        Ok(())
    } else {
        Err(invalid(
            field,
            "must be a secret reference (env:NAME, file:/path or vault:path#key), never the secret itself",
        ))
    }
}

/// `250ms`, `15s`, `5m`, `1h`.
pub fn parse_duration(s: &str) -> Option<std::time::Duration> {
    let (num, unit) = if let Some(n) = s.strip_suffix("ms") {
        (n, "ms")
    } else if let Some(n) = s.strip_suffix('s') {
        (n, "s")
    } else if let Some(n) = s.strip_suffix('m') {
        (n, "m")
    } else if let Some(n) = s.strip_suffix('h') {
        (n, "h")
    } else {
        return None;
    };
    if num.is_empty() || !num.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: u64 = num.parse().ok()?;
    Some(match unit {
        "ms" => std::time::Duration::from_millis(n),
        "s" => std::time::Duration::from_secs(n),
        "m" => std::time::Duration::from_secs(n.checked_mul(60)?),
        _ => std::time::Duration::from_secs(n.checked_mul(3600)?),
    })
}

// ─── Secret references ──────────────────────────────────────────────────────

/// Read an `env:` or `file:` secret reference without the network.
/// `env:NAME` reads the variable, `file:/path` reads the file (one trailing
/// newline trimmed). A `vault:path#key` reference is refused with
/// [`SettingsError::VaultNeedsResolver`]; read it with [`Settings::secret`].
pub fn resolve_secret(reference: &str) -> Result<String, SettingsError> {
    if let Some(name) = reference.strip_prefix("env:").filter(|n| !n.is_empty()) {
        return std::env::var(name)
            .ok()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| SettingsError::EnvMissing {
                reference: reference.to_string(),
                name: name.to_string(),
            });
    }
    if let Some(path) = reference.strip_prefix("file:").filter(|p| !p.is_empty()) {
        let text = std::fs::read_to_string(path).map_err(|e| SettingsError::FileUnreadable {
            reference: reference.to_string(),
            message: e.to_string(),
        })?;
        let trimmed = text
            .strip_suffix("\r\n")
            .or_else(|| text.strip_suffix('\n'))
            .unwrap_or(&text);
        return Ok(trimmed.to_string());
    }
    if reference.strip_prefix("vault:").is_some_and(|r| !r.is_empty()) {
        return Err(SettingsError::VaultNeedsResolver(reference.to_string()));
    }
    Err(SettingsError::BadSecretRef(reference.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_needs_model_map() {
        let err = Settings::from_str(
            "inference:\n  provider: gateway\n  gateway:\n    format: openai-chat\n    base_url: https://llm.internal\n",
        )
        .unwrap_err();
        assert!(err.to_string().contains("model_map"), "{err}");
    }

    #[test]
    fn unknown_keys_are_refused() {
        let err = Settings::from_str("inference:\n  provider: anthropic\nsurprise: 1\n").unwrap_err();
        assert!(matches!(err, SettingsError::Parse(_)), "{err}");
    }

    #[test]
    fn plain_secret_in_file_is_refused() {
        let err = Settings::from_str(
            "inference:\n  provider: anthropic\n  credentials:\n    api_key: sk-ant-123\n",
        )
        .unwrap_err();
        assert!(err.to_string().contains("secret reference"), "{err}");
    }

    #[test]
    fn wildcard_cors_is_refused() {
        let err = Settings::from_str(
            "inference: {provider: anthropic}\nserver: {cors_origins: ['*']}\n",
        )
        .unwrap_err();
        assert!(err.to_string().contains("cors_origins"), "{err}");
        Settings::from_str("inference: {provider: anthropic}\nserver: {cors_origins: ['null', 'https://ai.example.com']}\n")
            .expect("null and exact origins are fine");
    }

    #[tokio::test]
    async fn vault_ref_without_vault_settings_is_a_clear_error() {
        let s = Settings::from_str("inference: {provider: anthropic}\n").unwrap();
        let err = s.secret("vault:db#password").await.unwrap_err();
        assert!(matches!(err, SettingsError::VaultNotConfigured(_)), "{err}");
    }

    #[test]
    fn previous_keys_need_a_current_key() {
        let err = Settings::from_str(
            "inference: {provider: anthropic}\nsecrets: {previous_key_refs: ['env:OLD_KEY']}\n",
        )
        .unwrap_err();
        assert!(err.to_string().contains("previous_key_refs"), "{err}");
        Settings::from_str(
            "inference: {provider: anthropic}\nsecrets: {encryption_key_ref: 'env:NEW_KEY', previous_key_refs: ['env:OLD_KEY']}\n",
        )
        .expect("current plus previous is fine");
    }

    #[test]
    fn model_map_gates_models() {
        let s = Settings::from_str(
            "inference:\n  provider: bedrock\n  model_map:\n    claude-sonnet: anthropic.claude-sonnet-v1\n",
        )
        .unwrap();
        assert!(s.model_allowed("claude-sonnet"));
        assert!(!s.model_allowed("gpt-4o"));
        let open = Settings::from_str("inference: {provider: anthropic}\n").unwrap();
        assert!(open.model_allowed("anything"));
    }

    #[test]
    fn session_timeouts_parse() {
        let s = Settings::from_str("inference: {provider: anthropic}\n").unwrap();
        assert_eq!(s.idle_timeout(), std::time::Duration::from_secs(3600));
        assert_eq!(s.no_worker_timeout(), std::time::Duration::from_secs(600));
        let s = Settings::from_str(
            "inference: {provider: anthropic}\nsessions: {idle_timeout: 90s, no_worker_timeout: 250ms}\n",
        )
        .unwrap();
        assert_eq!(s.idle_timeout(), std::time::Duration::from_secs(90));
        assert_eq!(s.no_worker_timeout(), std::time::Duration::from_millis(250));
        let err = Settings::from_str("inference: {provider: anthropic}\nsessions: {idle_timeout: 0s}\n")
            .unwrap_err();
        assert!(err.to_string().contains("sessions.idle_timeout"), "{err}");
    }

    #[test]
    fn secret_refs_resolve() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("pw");
        std::fs::write(&p, "hunter2\n").unwrap();
        assert_eq!(resolve_secret(&format!("file:{}", p.display())).unwrap(), "hunter2");
        assert!(matches!(
            resolve_secret("vault:kv/db#password"),
            Err(SettingsError::VaultNeedsResolver(_))
        ));
        assert!(matches!(
            resolve_secret("env:SELFHOST_TEST_SURELY_UNSET_VAR"),
            Err(SettingsError::EnvMissing { .. })
        ));
        assert!(matches!(resolve_secret("plain"), Err(SettingsError::BadSecretRef(_))));
    }
}
