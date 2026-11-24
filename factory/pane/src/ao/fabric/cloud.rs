//! Cloud pairing lifecycle client: create → poll exchange → token, plus
//! heartbeat / rotate / revoke-self (spike Q3; agent-daemon `index.ts`).
//! The node never holds a Clerk token — the approving browser does.

use super::cloud_api_url;
use super::identity::NodeIdentity;
use super::wire::{
    cloud_error_message, CreatePairingRequest, ExchangeOutcome, ExchangePairingRequest,
    PairingStart, RuntimeSessionResponse,
};

/// The seven default capabilities (runtime_pairing.rs:39-47); requesting
/// exactly these needs no new server-side capability (spike D3).
pub(crate) const DEFAULT_CAPABILITIES: [&str; 7] = [
    "runtime:connect",
    "runtime:execute",
    "runtime:files",
    "runtime:terminal",
    "runtime:remote_control",
    "providers:connect",
    "providers:use",
];

fn http() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(format!("ao-fabric-node/{}", env!("CARGO_PKG_VERSION")))
        // The cloud closes idle keep-alive connections aggressively; a
        // reused dead pooled connection fails the next POST with an
        // "error sending request" transport error (seen live 2026-09-09:
        // pairing create succeeded, the first exchange poll failed).
        .pool_max_idle_per_host(0)
        .build()
        .map_err(|err| format!("cannot build http client: {err}"))
}

/// Format a reqwest error with its full source chain for diagnosis.
fn request_error(context: &str, err: &reqwest::Error) -> String {
    use std::error::Error as _;
    let mut message = format!("{context}: {err}");
    let mut source = err.source();
    while let Some(cause) = source {
        message.push_str(&format!(" | caused by: {cause}"));
        source = cause.source();
    }
    message
}

pub(crate) struct CloudClient {
    client: reqwest::Client,
    pub base_url: String,
}

impl CloudClient {
    pub(crate) fn new() -> Result<Self, String> {
        Ok(CloudClient {
            client: http()?,
            base_url: cloud_api_url(),
        })
    }

    /// Leg 1: `POST /api/v1/runtime-pairings` (unauthenticated).
    pub(crate) async fn create_pairing(
        &self,
        identity: &NodeIdentity,
        name: &str,
        runtime_type: &str,
    ) -> Result<PairingStart, String> {
        let hostname = std::env::var_os("HOSTNAME")
            .map(|value| value.to_string_lossy().into_owned())
            .or_else(|| {
                std::process::Command::new("hostname")
                    .output()
                    .ok()
                    .filter(|output| output.status.success())
                    .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            })
            .filter(|value| !value.is_empty());
        let platform = Some(format!(
            "{}-{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ));
        let body = CreatePairingRequest {
            name: name.to_string(),
            runtime_type: runtime_type.to_string(),
            hostname,
            platform,
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
            machine_id: shared_machine_id(),
            public_key: identity.public_key.clone(),
            capabilities: DEFAULT_CAPABILITIES.iter().map(|cap| (*cap).to_string()).collect(),
        };
        let response = self
            .client
            .post(format!("{}/api/v1/runtime-pairings", self.base_url))
            .json(&body)
            .send()
            .await
            .map_err(|err| request_error("pairing create request failed", &err))?;
        let status = response.status();
        if status != reqwest::StatusCode::CREATED {
            let text = response.text().await.unwrap_or_default();
            return Err(format!(
                "unable to begin runtime pairing: {}",
                cloud_error_message(&text, status.as_u16())
            ));
        }
        response
            .json::<PairingStart>()
            .await
            .map_err(|err| format!("pairing create returned malformed json: {err}"))
    }

    /// Leg 3: `POST /api/v1/runtime-pairings/exchange` (unauthenticated, poll).
    pub(crate) async fn exchange(
        &self,
        identity: &NodeIdentity,
        start: &PairingStart,
    ) -> Result<ExchangeOutcome, String> {
        let signature = identity.pairing_signature(&start.pairing_id, &start.challenge)?;
        let body = ExchangePairingRequest {
            pairing_id: start.pairing_id.clone(),
            device_code: start.device_code.clone(),
            signature,
        };
        let response = self
            .client
            .post(format!("{}/api/v1/runtime-pairings/exchange", self.base_url))
            .json(&body)
            .send()
            .await
            .map_err(|err| request_error("pairing exchange request failed", &err))?;
        let status = response.status();
        if status == reqwest::StatusCode::OK {
            let session = response
                .json::<RuntimeSessionResponse>()
                .await
                .map_err(|err| format!("pairing exchange returned malformed json: {err}"))?;
            return Ok(ExchangeOutcome::Issued(session));
        }
        if status.as_u16() == 428 {
            return Ok(ExchangeOutcome::Pending);
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let retry_after = response
                .headers()
                .get("retry_after")
                .or_else(|| response.headers().get("retry-after"))
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(2);
            return Ok(ExchangeOutcome::RetryAfter(retry_after));
        }
        if status.as_u16() == 410 {
            return Ok(ExchangeOutcome::Expired);
        }
        if status == reqwest::StatusCode::FORBIDDEN {
            return Ok(ExchangeOutcome::Denied);
        }
        let text = response.text().await.unwrap_or_default();
        Err(cloud_error_message(&text, status.as_u16()))
    }

    fn device_auth(&self, identity: &NodeIdentity) -> Result<reqwest::RequestBuilder, String> {
        let token = identity.device_token()?.to_string();
        let runtime_id = identity.runtime_id()?.to_string();
        Ok(self
            .client
            .post(format!(
                "{}/api/v1/runtime-devices/{}/heartbeat",
                self.base_url,
                urlencoding_encode(&runtime_id)
            ))
            .bearer_auth(token))
    }

    /// `POST /runtime-devices/:id/heartbeat` — 401/403 means revoked
    /// (agent-daemon `heartbeat`, index.ts:215-225).
    pub(crate) async fn heartbeat(&self, identity: &NodeIdentity) -> Result<(), HeartbeatError> {
        let response = self
            .device_auth(identity)
            .map_err(HeartbeatError::Transport)?
            .send()
            .await
            .map_err(|err| HeartbeatError::Transport(err.to_string()))?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(HeartbeatError::Revoked);
        }
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(HeartbeatError::Transport(cloud_error_message(
                &text,
                status.as_u16(),
            )));
        }
        Ok(())
    }

    /// `POST /runtime-devices/:id/rotate` — replaces the 90-day token; the
    /// server keeps the rotated-out hash valid for 15 minutes
    /// (`ROTATION_GRACE_MINUTES`) so components sharing the identity self-heal.
    pub(crate) async fn rotate(&self, identity: &NodeIdentity) -> Result<RuntimeSessionResponse, String> {
        let token = identity.device_token()?.to_string();
        let runtime_id = identity.runtime_id()?.to_string();
        let response = self
            .client
            .post(format!(
                "{}/api/v1/runtime-devices/{}/rotate",
                self.base_url,
                urlencoding_encode(&runtime_id)
            ))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|err| format!("credential rotation request failed: {err}"))?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!(
                "credential rotation failed: {}",
                cloud_error_message(&text, status.as_u16())
            ));
        }
        response
            .json::<RuntimeSessionResponse>()
            .await
            .map_err(|err| format!("credential rotation returned malformed json: {err}"))
    }

    /// Best-effort unpair (`revoke-self`, index.ts on unpair).
    pub(crate) async fn revoke_self(&self, identity: &NodeIdentity) {
        let (Ok(token), Ok(runtime_id)) = (identity.device_token(), identity.runtime_id()) else {
            return;
        };
        let _ = self
            .client
            .post(format!(
                "{}/api/v1/runtime-devices/{}/revoke-self",
                self.base_url,
                urlencoding_encode(runtime_id)
            ))
            .bearer_auth(token)
            .send()
            .await;
    }
}

#[derive(Debug)]
pub(crate) enum HeartbeatError {
    Revoked,
    Transport(String),
}

fn urlencoding_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// This computer's machine id: ~/.allternit/machine-id, the same file Allternit
/// Desktop and `gizzi pair` read and write (random, made once, 16-64 of
/// `[A-Za-z0-9_-]`). Made here when missing. None when the home directory is
/// unknown or unwritable; the cloud then falls back to hostname + platform.
pub(crate) fn shared_machine_id() -> Option<String> {
    let home = crate::ao::native::home_dir()?;
    machine_id_in(&home.join(".allternit").join("machine-id"))
}

fn valid_machine_id(value: &str) -> bool {
    (16..=64).contains(&value.len()) && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn machine_id_in(file: &std::path::Path) -> Option<String> {
    if let Ok(existing) = std::fs::read_to_string(file) {
        let existing = existing.trim();
        if valid_machine_id(existing) {
            return Some(existing.to_string());
        }
    }
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine as _;
    use rand::RngCore as _;
    let mut bytes = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut bytes);
    let id = URL_SAFE_NO_PAD.encode(bytes);
    std::fs::create_dir_all(file.parent()?).ok()?;
    std::fs::write(file, format!("{id}\n")).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600));
    }
    Some(id)
}

#[cfg(test)]
mod machine_id_tests {
    use super::machine_id_in;

    #[test]
    fn the_machine_id_is_made_once_and_shared() {
        let dir = std::env::temp_dir().join(format!("ao-machine-id-{}", std::process::id()));
        let file = dir.join(".allternit").join("machine-id");
        let _ = std::fs::remove_dir_all(&dir);
        let made = machine_id_in(&file).expect("made");
        assert_eq!(made.len(), 32, "24 random bytes, base64url");
        assert_eq!(machine_id_in(&file).as_deref(), Some(made.as_str()), "read back, not made again");
        // A file Desktop or gizzi wrote is used as is.
        std::fs::write(&file, "Zk3m9QpL2vX8aB4cD6eF7gH1\n").unwrap();
        assert_eq!(machine_id_in(&file).as_deref(), Some("Zk3m9QpL2vX8aB4cD6eF7gH1"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
