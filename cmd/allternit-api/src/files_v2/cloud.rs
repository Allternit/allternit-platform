//! Runtime → cloud-api calls the Files drive makes on its own: the asleep
//! snapshot upload (SPEC section 11) and the queued guest drops (section 8,
//! `QueuedDrop`). Signed exactly like the presence heartbeat
//! (`relay_auth::signed_headers` + `x-allternit-runtime-id`), so cloud-api
//! knows which computer is calling without a user session. None of these
//! calls is activity: cloud-api never touches the idle clock for them.

use std::time::Duration;

use async_trait::async_trait;

/// Largest answer read back (a queued drop is at most 4 MiB + 64 KiB).
const MAX_ANSWER_BYTES: usize = 8 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(30);

/// One signed request to cloud-api: `(status, body)`. `Err` = not sent
/// (not paired, network down). Tests replace it (the network is the edge).
#[async_trait]
pub(crate) trait CloudCalls: Send + Sync {
    async fn send(&self, method: &str, path: &str, body: Vec<u8>) -> Result<(u16, Vec<u8>), String>;
}

/// Production: `ALLTERNIT_CLOUD_API_URL` (default api.allternit.com) with this
/// runtime's device token.
pub(crate) struct SignedCloud;

pub(crate) fn paired() -> bool {
    let s = crate::relay_auth::process_secret();
    s.device_token().is_some() && s.paired_owner().is_some() && s.runtime_id().is_some()
}

#[async_trait]
impl CloudCalls for SignedCloud {
    async fn send(&self, method: &str, path: &str, body: Vec<u8>) -> Result<(u16, Vec<u8>), String> {
        let secret = crate::relay_auth::process_secret();
        let (Some(token), Some(owner), Some(runtime_id)) = (secret.device_token(), secret.paired_owner(), secret.runtime_id()) else {
            return Err("this computer is not paired".into());
        };
        // The signature covers the path without its query (cloud-api verifies it that way).
        let signed_path = path.split('?').next().unwrap_or(path);
        let method = method.to_ascii_uppercase();
        let m = reqwest::Method::from_bytes(method.as_bytes()).map_err(|e| e.to_string())?;
        let mut req = crate::net_client::client()
            .request(m, format!("{}{path}", crate::phone_sync::cloud_base()))
            .timeout(TIMEOUT)
            .header("content-type", "application/json");
        for (k, v) in crate::relay_auth::signed_headers(&token, &owner, &method, signed_path, &body) {
            req = req.header(k, v);
        }
        req = req.header(crate::relay_auth::RUNTIME_ID_HEADER, runtime_id);
        if !body.is_empty() {
            req = req.body(body);
        }
        let res = req.send().await.map_err(|e| e.to_string())?;
        let status = res.status().as_u16();
        let bytes = res.bytes().await.map_err(|e| e.to_string())?;
        if bytes.len() > MAX_ANSWER_BYTES {
            return Err("cloud-api answered with too much data".into());
        }
        Ok((status, bytes.to_vec()))
    }
}
