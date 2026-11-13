//! Secret references and stored credentials.
//!
//! A secret field in the settings file holds a reference, never the value:
//!
//! - `env:NAME`: the environment variable ([`EnvBackend`]).
//! - `file:/path`: the file, one trailing newline trimmed ([`FileBackend`]).
//! - `vault:path#key`: HashiCorp Vault KV v2 ([`VaultBackend`]). It reads
//!   `GET {address}/v1/{mount}/data/{secrets.vault.path}/{path}` and returns
//!   field `key`. An empty path (`vault:#key`) reads `secrets.vault.path`
//!   itself. Requests go through the egress client, so the Vault host must be
//!   `secrets.vault.address` (it is added to the allowlist automatically).
//!
//! Vault auth (`secrets.vault.auth.method`):
//!
//! - `token`: `auth.token` is an `env:` or `file:` reference to the token.
//!   With no `auth` block, `VAULT_TOKEN` is used.
//! - `approle`: `auth.role_id` plus `auth.secret_id` (a reference), logged in
//!   at `/v1/auth/approle/login`.
//! - `kubernetes`: `auth.role` plus the pod's service account token, logged
//!   in at `/v1/auth/kubernetes/login`.
//!
//! A login token is cached and renewed once on a 403.
//!
//! Stored credentials are sealed with the current key of a
//! [`Keyring`] built from `secrets.encryption_key_ref` and
//! `secrets.previous_key_refs`. [`rotate_credentials`] re-seals every row that
//! is not under the current key (`admin rotate-key`).

use std::path::Path;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::Mutex;

use super::egress_guard::{EgressClient, EgressGuard};
use super::settings::{resolve_secret, Settings, SettingsError, VaultAuthMethod, VaultSettings};
use super::store::{CredentialRecord, DynStore, StoreError};
use crate::token_crypto::{Keyring, KeyringError};

/// Where a Kubernetes pod finds its service account token.
pub const K8S_SA_TOKEN_PATH: &str = "/var/run/secrets/kubernetes.io/serviceaccount/token";

/// One secret source. `body` is the reference without its `scheme:`.
#[async_trait]
pub trait SecretBackend: Send + Sync {
    /// `env`, `file` or `vault`.
    fn scheme(&self) -> &'static str;
    async fn get(&self, reference: &str, body: &str) -> Result<String, SettingsError>;
}

pub struct EnvBackend;

#[async_trait]
impl SecretBackend for EnvBackend {
    fn scheme(&self) -> &'static str {
        "env"
    }
    async fn get(&self, reference: &str, _body: &str) -> Result<String, SettingsError> {
        resolve_secret(reference)
    }
}

pub struct FileBackend;

#[async_trait]
impl SecretBackend for FileBackend {
    fn scheme(&self) -> &'static str {
        "file"
    }
    async fn get(&self, reference: &str, _body: &str) -> Result<String, SettingsError> {
        resolve_secret(reference)
    }
}

/// Vault KV v2 over the egress client.
pub struct VaultBackend {
    settings: VaultSettings,
    http: EgressClient,
    /// A token from a login (approle, kubernetes), reused until a 403.
    login_token: Mutex<Option<String>>,
}

fn vault_err(reference: &str, message: impl Into<String>) -> SettingsError {
    SettingsError::Vault {
        reference: reference.to_string(),
        message: message.into(),
    }
}

/// `path#key` → (path, key). The key is required.
fn split_vault_ref(body: &str) -> Option<(&str, &str)> {
    let (path, key) = body.rsplit_once('#')?;
    if key.is_empty() || path.contains('?') || path.chars().any(char::is_whitespace) {
        return None;
    }
    Some((path, key))
}

fn join_path(base: &str, rel: &str) -> String {
    let base = base.trim_matches('/');
    let rel = rel.trim_matches('/');
    match (base.is_empty(), rel.is_empty()) {
        (true, _) => rel.to_string(),
        (false, true) => base.to_string(),
        (false, false) => format!("{base}/{rel}"),
    }
}

impl VaultBackend {
    pub fn new(settings: VaultSettings, http: EgressClient) -> Self {
        Self {
            settings,
            http,
            login_token: Mutex::new(None),
        }
    }

    fn address(&self) -> &str {
        self.settings.address.trim_end_matches('/')
    }

    /// The KV v2 read URL for a reference path.
    pub fn data_url(&self, rel_path: &str) -> String {
        format!(
            "{}/v1/{}/data/{}",
            self.address(),
            self.settings.mount.trim_matches('/'),
            join_path(&self.settings.path, rel_path)
        )
    }

    fn uses_login(&self) -> bool {
        matches!(
            self.settings.auth.as_ref().map(|a| a.method),
            Some(VaultAuthMethod::Approle | VaultAuthMethod::Kubernetes)
        )
    }

    /// A static token (method token, or no auth block).
    fn static_token(&self, reference: &str) -> Result<String, SettingsError> {
        let configured = self.settings.auth.as_ref().and_then(|a| a.token.clone());
        match configured {
            Some(r) if r.starts_with("vault:") => Err(vault_err(
                reference,
                "secrets.vault.auth.token cannot itself be a vault: reference",
            )),
            Some(r) => resolve_secret(&r),
            None => std::env::var("VAULT_TOKEN")
                .ok()
                .filter(|t| !t.is_empty())
                .ok_or_else(|| {
                    vault_err(
                        reference,
                        "no token: set secrets.vault.auth.token (env: or file:) or VAULT_TOKEN",
                    )
                }),
        }
    }

    async fn login(&self, reference: &str) -> Result<String, SettingsError> {
        let auth = self
            .settings
            .auth
            .as_ref()
            .ok_or_else(|| vault_err(reference, "no auth block"))?;
        let (url, body) = match auth.method {
            VaultAuthMethod::Approle => {
                let role_id = auth
                    .role_id
                    .clone()
                    .filter(|r| !r.is_empty())
                    .ok_or_else(|| vault_err(reference, "approle needs secrets.vault.auth.role_id"))?;
                let secret_ref = auth.secret_id.clone().ok_or_else(|| {
                    vault_err(reference, "approle needs secrets.vault.auth.secret_id")
                })?;
                if secret_ref.starts_with("vault:") {
                    return Err(vault_err(reference, "secret_id cannot be a vault: reference"));
                }
                let secret_id = resolve_secret(&secret_ref)?;
                (
                    format!("{}/v1/auth/approle/login", self.address()),
                    serde_json::json!({ "role_id": role_id, "secret_id": secret_id }),
                )
            }
            VaultAuthMethod::Kubernetes => {
                let role = auth
                    .role
                    .clone()
                    .filter(|r| !r.is_empty())
                    .ok_or_else(|| vault_err(reference, "kubernetes auth needs secrets.vault.auth.role"))?;
                let jwt = std::fs::read_to_string(K8S_SA_TOKEN_PATH)
                    .map_err(|e| vault_err(reference, format!("cannot read {K8S_SA_TOKEN_PATH}: {e}")))?;
                (
                    format!("{}/v1/auth/kubernetes/login", self.address()),
                    serde_json::json!({ "role": role, "jwt": jwt.trim() }),
                )
            }
            VaultAuthMethod::Token => return self.static_token(reference),
        };
        let res = self
            .http
            .post(&url)
            .map_err(|e| vault_err(reference, e.to_string()))?
            .json(&body)
            .send()
            .await
            .map_err(|e| vault_err(reference, format!("login request failed: {e}")))?;
        let status = res.status();
        if !status.is_success() {
            return Err(vault_err(reference, format!("login returned HTTP {status}")));
        }
        let v: Value = res
            .json()
            .await
            .map_err(|e| vault_err(reference, format!("login response is not JSON: {e}")))?;
        v["auth"]["client_token"]
            .as_str()
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .ok_or_else(|| vault_err(reference, "login response has no auth.client_token"))
    }

    async fn token(&self, reference: &str, fresh: bool) -> Result<String, SettingsError> {
        if !self.uses_login() {
            return self.static_token(reference);
        }
        let mut cached = self.login_token.lock().await;
        if !fresh {
            if let Some(t) = cached.as_ref() {
                return Ok(t.clone());
            }
        }
        let t = self.login(reference).await?;
        *cached = Some(t.clone());
        Ok(t)
    }

    async fn read(&self, reference: &str, rel_path: &str, key: &str) -> Result<String, SettingsError> {
        let url = self.data_url(rel_path);
        let mut fresh = false;
        loop {
            let token = self.token(reference, fresh).await?;
            let res = self
                .http
                .get(&url)
                .map_err(|e| vault_err(reference, e.to_string()))?
                .header("X-Vault-Token", token)
                .send()
                .await
                .map_err(|e| vault_err(reference, format!("request failed: {e}")))?;
            let status = res.status();
            if status == reqwest::StatusCode::FORBIDDEN && self.uses_login() && !fresh {
                // The cached login token may have expired: log in once more.
                fresh = true;
                continue;
            }
            if status == reqwest::StatusCode::NOT_FOUND {
                return Err(vault_err(reference, "no secret at that path"));
            }
            if !status.is_success() {
                return Err(vault_err(reference, format!("HTTP {status}")));
            }
            let v: Value = res
                .json()
                .await
                .map_err(|e| vault_err(reference, format!("response is not JSON: {e}")))?;
            return match &v["data"]["data"][key] {
                Value::Null => Err(vault_err(reference, format!("the secret has no field {key:?}"))),
                Value::String(s) => Ok(s.clone()),
                other => Ok(other.to_string()),
            };
        }
    }
}

#[async_trait]
impl SecretBackend for VaultBackend {
    fn scheme(&self) -> &'static str {
        "vault"
    }
    async fn get(&self, reference: &str, body: &str) -> Result<String, SettingsError> {
        let (path, key) =
            split_vault_ref(body).ok_or_else(|| SettingsError::BadSecretRef(reference.to_string()))?;
        self.read(reference, path, key).await
    }
}

/// Resolves any secret reference with the backend its scheme names.
pub struct SecretResolver {
    env: EnvBackend,
    file: FileBackend,
    vault: Option<VaultBackend>,
}

impl SecretResolver {
    /// Vault reads go through `egress` (pass the server's client so they
    /// are counted in the egress metrics).
    pub fn new(settings: &Settings, egress: EgressClient) -> Self {
        Self {
            env: EnvBackend,
            file: FileBackend,
            vault: settings
                .secrets
                .vault
                .clone()
                .map(|v| VaultBackend::new(v, egress)),
        }
    }

    /// With an egress client of its own, built from the settings. Used by
    /// the admin CLI and before the server state exists.
    pub fn from_settings(settings: &Settings) -> Result<Self, SettingsError> {
        let guard = std::sync::Arc::new(EgressGuard::from_settings(settings));
        let egress = guard.client().map_err(|e| SettingsError::Invalid {
            field: "egress".into(),
            message: e.to_string(),
        })?;
        Ok(Self::new(settings, egress))
    }

    pub fn backend(&self, scheme: &str) -> Option<&dyn SecretBackend> {
        match scheme {
            "env" => Some(&self.env),
            "file" => Some(&self.file),
            "vault" => self.vault.as_ref().map(|v| v as &dyn SecretBackend),
            _ => None,
        }
    }

    pub async fn resolve(&self, reference: &str) -> Result<String, SettingsError> {
        let (scheme, body) = reference
            .split_once(':')
            .filter(|(_, b)| !b.is_empty())
            .ok_or_else(|| SettingsError::BadSecretRef(reference.to_string()))?;
        match self.backend(scheme) {
            Some(b) => b.get(reference, body).await,
            None if scheme == "vault" => Err(SettingsError::VaultNotConfigured(reference.to_string())),
            None => Err(SettingsError::BadSecretRef(reference.to_string())),
        }
    }
}

// ─── Keyring and stored credentials ─────────────────────────────────────────

/// The keyring from `secrets.encryption_key_ref` (current) and
/// `secrets.previous_key_refs`. None when no key is configured: the server
/// then refuses to store credentials.
pub async fn load_keyring(
    settings: &Settings,
    resolver: &SecretResolver,
) -> Result<Option<Keyring>, SettingsError> {
    let Some(cur_ref) = &settings.secrets.encryption_key_ref else {
        return Ok(None);
    };
    let current = resolver.resolve(cur_ref).await?;
    let mut previous = Vec::with_capacity(settings.secrets.previous_key_refs.len());
    for r in &settings.secrets.previous_key_refs {
        previous.push(resolver.resolve(r).await?);
    }
    Keyring::new(&current, &previous)
        .map(Some)
        .map_err(|e| SettingsError::Invalid {
            field: "secrets.encryption_key_ref".into(),
            message: e.to_string(),
        })
}

#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("credential {name}: {source}")]
    Crypto { name: String, source: KeyringError },
    #[error("credential {0}: stored value is not text")]
    NotText(String),
}

/// Seal `plain` with the current key and store it under `name`.
pub async fn put_sealed_credential(
    store: &DynStore,
    ring: &Keyring,
    name: &str,
    plain: &str,
) -> Result<CredentialRecord, CredentialError> {
    let sealed = ring.seal(plain).map_err(|source| CredentialError::Crypto {
        name: name.to_string(),
        source,
    })?;
    Ok(store
        .put_credential(name, sealed.as_bytes(), ring.current_key_id())
        .await?)
}

/// The plaintext of a stored credential, opened with whichever key sealed it.
pub async fn open_credential(
    store: &DynStore,
    ring: &Keyring,
    name: &str,
) -> Result<String, CredentialError> {
    let rec = store.get_credential(name).await?;
    open_record(ring, &rec)
}

fn open_record(ring: &Keyring, rec: &CredentialRecord) -> Result<String, CredentialError> {
    let sealed =
        std::str::from_utf8(&rec.sealed).map_err(|_| CredentialError::NotText(rec.name.clone()))?;
    ring.open(sealed).map_err(|source| CredentialError::Crypto {
        name: rec.name.clone(),
        source,
    })
}

/// What [`rotate_credentials`] did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RotationReport {
    /// Re-sealed with the current key.
    pub rotated: Vec<String>,
    /// Already under the current key.
    pub unchanged: usize,
    /// Could not be opened with any key in the ring: (name, reason). Left as
    /// they were.
    pub failed: Vec<(String, String)>,
}

/// Re-seal every credential that is not under the current key. Rows that
/// cannot be opened are reported and left alone, so a missing previous key
/// never destroys data. Safe to run again.
pub async fn rotate_credentials(
    store: &DynStore,
    ring: &Keyring,
) -> Result<RotationReport, StoreError> {
    let mut report = RotationReport::default();
    for rec in store.list_credentials().await? {
        let sealed = match std::str::from_utf8(&rec.sealed) {
            Ok(s) => s,
            Err(_) => {
                report.failed.push((rec.name.clone(), "stored value is not text".into()));
                continue;
            }
        };
        if !ring.needs_rotation(sealed) && rec.key_id == ring.current_key_id() {
            report.unchanged += 1;
            continue;
        }
        match ring.reseal(sealed) {
            Ok(new) => {
                store
                    .put_credential(&rec.name, new.as_bytes(), ring.current_key_id())
                    .await?;
                report.rotated.push(rec.name.clone());
            }
            Err(e) => report.failed.push((rec.name.clone(), e.to_string())),
        }
    }
    Ok(report)
}

/// For `admin doctor`: does a path look like a readable file?
pub fn readable_file(path: &Path) -> bool {
    std::fs::metadata(path).map(|m| m.is_file()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vault_refs_split() {
        assert_eq!(split_vault_ref("db#password"), Some(("db", "password")));
        assert_eq!(split_vault_ref("#password"), Some(("", "password")));
        assert_eq!(split_vault_ref("a/b#c#d"), Some(("a/b#c", "d")));
        assert_eq!(split_vault_ref("db"), None);
        assert_eq!(split_vault_ref("db#"), None);
    }

    #[test]
    fn data_url_joins_mount_and_base_path() {
        let s = Settings::from_str(
            "inference: {provider: anthropic}\nsecrets:\n  backend: vault\n  vault: {address: 'https://vault.corp:8200/', mount: kv, path: agents/prod}\n",
        )
        .unwrap();
        let guard = std::sync::Arc::new(EgressGuard::from_settings(&s));
        let v = VaultBackend::new(s.secrets.vault.clone().unwrap(), guard.client().unwrap());
        assert_eq!(v.data_url("db"), "https://vault.corp:8200/v1/kv/data/agents/prod/db");
        assert_eq!(v.data_url(""), "https://vault.corp:8200/v1/kv/data/agents/prod");
    }
}
