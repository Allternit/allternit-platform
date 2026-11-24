//! A blocking `files/2` client (SPEC section 2) over loopback, mesh or the relay.
//!
//! The cfapi callbacks run on worker threads, so a blocking client is the simple
//! fit. Every error body is the contract's `ErrorBody`.

use std::io::Read;
use std::sync::RwLock;
use std::time::Duration;

use allternit_files_core::*;
use base64::Engine;
use serde::de::DeserializeOwned;
use serde::Serialize;

/// Prefix of every runtime route.
pub const V2: &str = "/api/v1/files/v2";
/// Header that carries a wake ticket (SPEC section 9).
pub const WAKE_TICKET_HEADER: &str = "x-allternit-wake-ticket";
/// Largest body the helper reads into memory (a 4 MiB chunk plus room).
const MAX_BODY: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub enum Transport {
    /// loopback or mesh: `base` ends in `/api/v1/files/v2`.
    Direct { base: String },
    /// cloud-api relay: `proxy_url` is `.../runtime-devices/:id/proxy`.
    Relay { proxy_url: String },
}

impl Transport {
    pub fn from_config(kind: &str, base_url: &str) -> Result<Self, ApiError> {
        match kind {
            "loopback" | "mesh" => Ok(Transport::Direct { base: base_url.trim_end_matches('/').to_string() }),
            "relay" => Ok(Transport::Relay { proxy_url: base_url.to_string() }),
            other => Err(ApiError::Transport(format!("transport {other} is not for the helper"))),
        }
    }
}

#[derive(Debug)]
pub enum ApiError {
    /// The server answered with a contract error.
    Files { status: u16, body: ErrorBody },
    /// Non-contract HTTP error.
    Http { status: u16, text: String },
    /// Network or TLS failure.
    Transport(String),
    /// The body did not match the contract.
    Decode(String),
}

impl ApiError {
    pub fn code(&self) -> Option<&ErrorCode> {
        match self {
            ApiError::Files { body, .. } => Some(&body.code),
            _ => None,
        }
    }
    pub fn is(&self, code: ErrorCode) -> bool {
        self.code() == Some(&code)
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Files { status, body } => write!(f, "{status} {}: {}", body.code.as_str(), body.message),
            ApiError::Http { status, text } => write!(f, "HTTP {status}: {text}"),
            ApiError::Transport(s) => write!(f, "network: {s}"),
            ApiError::Decode(s) => write!(f, "bad response: {s}"),
        }
    }
}

pub struct Response {
    pub status: u16,
    pub etag: Option<String>,
    pub body: Vec<u8>,
}

pub struct FilesClient {
    agent: ureq::Agent,
    transport: Transport,
    cloud_api_base: String,
    computer_id: String,
    pub device_id: String,
    pub device_name: String,
    token: RwLock<Option<String>>,
}

fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

impl FilesClient {
    pub fn new(transport: Transport, cloud_api_base: &str, computer_id: &str, device_id: &str, device_name: &str) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            // Long polls hold 25 s (SPEC section 6); the relay may wait 30 s for a wake.
            .timeout_read(Duration::from_secs(75))
            .timeout_write(Duration::from_secs(60))
            .build();
        FilesClient {
            agent,
            transport,
            cloud_api_base: cloud_api_base.trim_end_matches('/').to_string(),
            computer_id: computer_id.to_string(),
            device_id: device_id.to_string(),
            device_name: device_name.to_string(),
            token: RwLock::new(None),
        }
    }

    pub fn set_token(&self, token: Option<String>) {
        *self.token.write().unwrap() = token.filter(|t| !t.is_empty());
    }

    pub fn computer_id(&self) -> &str {
        &self.computer_id
    }

    fn auth(&self) -> Option<String> {
        self.token.read().unwrap().as_ref().map(|t| format!("Bearer {t}"))
    }

    fn read_body(r: ureq::Response) -> Result<(u16, Option<String>, Vec<u8>), ApiError> {
        let status = r.status();
        let etag = r.header("etag").map(|s| s.trim_matches('"').to_string());
        let mut body = Vec::new();
        r.into_reader().take(MAX_BODY).read_to_end(&mut body).map_err(|e| ApiError::Transport(e.to_string()))?;
        Ok((status, etag, body))
    }

    fn finish(status: u16, etag: Option<String>, body: Vec<u8>) -> Result<Response, ApiError> {
        if status < 400 {
            return Ok(Response { status, etag, body });
        }
        match serde_json::from_slice::<ErrorBody>(&body) {
            Ok(b) => Err(ApiError::Files { status, body: b }),
            Err(_) => Err(ApiError::Http { status, text: String::from_utf8_lossy(&body).chars().take(300).collect() }),
        }
    }

    /// Sends one runtime request. `path` is after `/api/v1/files/v2`, with its query.
    pub fn send(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, String)],
        body: Option<(&[u8], &str)>,
    ) -> Result<Response, ApiError> {
        let mut hs: Vec<(String, String)> = headers.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
        if let Some(a) = self.auth() {
            hs.push(("authorization".into(), a));
        }
        hs.push(("x-allternit-device-id".into(), self.device_id.clone()));
        hs.push(("x-allternit-device-name".into(), self.device_name.clone()));
        hs.push(("x-allternit-device-platform".into(), "windows".into()));
        if let Some((_, ct)) = body {
            hs.push(("content-type".into(), ct.to_string()));
        }
        let result = match &self.transport {
            Transport::Direct { base } => {
                let mut req = self.agent.request(method, &format!("{base}{path}"));
                for (k, v) in &hs {
                    req = req.set(k, v);
                }
                match body {
                    Some((b, _)) => req.send_bytes(b),
                    None => req.call(),
                }
            }
            Transport::Relay { proxy_url } => {
                // RelayRequest (runtime_relay.rs). The proxy streams the raw response.
                let map: serde_json::Map<String, serde_json::Value> =
                    hs.iter().filter(|(k, _)| k != "authorization").map(|(k, v)| (k.clone(), v.clone().into())).collect();
                let env = serde_json::json!({
                    "method": method,
                    "path": format!("{V2}{path}"),
                    "headers": map,
                    "body": body.map(|(b, _)| base64::engine::general_purpose::STANDARD.encode(b)).unwrap_or_default(),
                    "body_encoding": "base64",
                });
                let mut req = self.agent.post(proxy_url).set("content-type", "application/json");
                if let Some(a) = self.auth() {
                    req = req.set("authorization", &a);
                }
                req.send_string(&env.to_string())
            }
        };
        match result {
            Ok(r) => {
                let (s, e, b) = Self::read_body(r)?;
                Self::finish(s, e, b)
            }
            Err(ureq::Error::Status(_, r)) => {
                let (s, e, b) = Self::read_body(r)?;
                Self::finish(s, e, b)
            }
            Err(e) => Err(ApiError::Transport(e.to_string())),
        }
    }

    fn json<T: DeserializeOwned>(r: Response) -> Result<T, ApiError> {
        serde_json::from_slice(&r.body).map_err(|e| ApiError::Decode(e.to_string()))
    }

    fn ticket_header(ticket: Option<&str>) -> Vec<(&'static str, String)> {
        ticket.map(|t| vec![(WAKE_TICKET_HEADER, t.to_string())]).unwrap_or_default()
    }

    fn post<B: Serialize, T: DeserializeOwned>(&self, path: &str, body: &B, ticket: Option<&str>) -> Result<T, ApiError> {
        let b = serde_json::to_vec(body).map_err(|e| ApiError::Decode(e.to_string()))?;
        Self::json(self.send("POST", path, &Self::ticket_header(ticket), Some((&b, "application/json")))?)
    }

    // ---- metadata (never needs a wake ticket; answered from the snapshot when asleep)

    pub fn roots(&self) -> Result<RootsResponse, ApiError> {
        Self::json(self.send("GET", "/roots", &[], None)?)
    }

    pub fn list(&self, root_id: &str, parent_id: &str, page_token: Option<&str>) -> Result<ListResponse, ApiError> {
        let mut q = format!("/roots/{}/list?parent_id={}&limit=1000", enc(root_id), enc(parent_id));
        if let Some(t) = page_token {
            q.push_str(&format!("&page_token={}", enc(t)));
        }
        Self::json(self.send("GET", &q, &[], None)?)
    }

    pub fn stat(&self, item_id: &str) -> Result<StatResponse, ApiError> {
        Self::json(self.send("GET", &format!("/items/{}", enc(item_id)), &[], None)?)
    }

    pub fn changes(&self, root_id: &str, cursor: &str, wait_s: u64) -> Result<ChangesResponse, ApiError> {
        let q = format!("/roots/{}/changes?cursor={}&limit=500&wait_s={}", enc(root_id), enc(cursor), wait_s.min(25));
        Self::json(self.send("GET", &q, &[], None)?)
    }

    // ---- content (needs a wake ticket while the computer sleeps)

    /// Reads `len` bytes at `off`. Returns the bytes and the rev the server served.
    pub fn read_range(&self, item_id: &str, off: u64, len: u64, ticket: Option<&str>) -> Result<(Vec<u8>, Option<String>), ApiError> {
        let mut h = Self::ticket_header(ticket);
        h.push(("range", crate::ranges::range_header(off, len)));
        let r = self.send("GET", &format!("/items/{}/content", enc(item_id)), &h, None)?;
        Ok((r.body, r.etag))
    }

    pub fn upload_create(&self, req: &UploadCreateRequest, ticket: Option<&str>) -> Result<UploadStatus, ApiError> {
        self.post("/uploads", req, ticket)
    }

    pub fn upload_append(&self, session_id: &str, req: &UploadChunksAppend, ticket: Option<&str>) -> Result<UploadStatus, ApiError> {
        self.post(&format!("/uploads/{}/chunks", enc(session_id)), req, ticket)
    }

    pub fn upload_status(&self, session_id: &str, ticket: Option<&str>) -> Result<UploadStatus, ApiError> {
        Self::json(self.send("GET", &format!("/uploads/{}", enc(session_id)), &Self::ticket_header(ticket), None)?)
    }

    pub fn put_chunk(&self, session_id: &str, hash: &str, bytes: &[u8], ticket: Option<&str>) -> Result<ChunkPutResponse, ApiError> {
        let p = format!("/uploads/{}/chunks/{}", enc(session_id), enc(hash));
        Self::json(self.send("PUT", &p, &Self::ticket_header(ticket), Some((bytes, "application/octet-stream")))?)
    }

    pub fn upload_commit(&self, session_id: &str, ticket: Option<&str>) -> Result<UploadCommitResponse, ApiError> {
        let p = format!("/uploads/{}/commit", enc(session_id));
        Self::json(self.send("POST", &p, &Self::ticket_header(ticket), Some((b"{}", "application/json")))?)
    }

    pub fn mkdir(&self, req: &MkdirRequest, ticket: Option<&str>) -> Result<MutationResponse, ApiError> {
        self.post("/mkdir", req, ticket)
    }

    pub fn move_item(&self, req: &MoveRequest, ticket: Option<&str>) -> Result<MutationResponse, ApiError> {
        self.post("/move", req, ticket)
    }

    pub fn delete(&self, req: &DeleteRequest, ticket: Option<&str>) -> Result<DeleteResponse, ApiError> {
        self.post("/delete", req, ticket)
    }

    pub fn pin(&self, item_id: &str, pinned: bool) -> Result<PinResponse, ApiError> {
        let req = PinRequest { item_id: item_id.to_string(), device_id: self.device_id.clone(), pinned };
        self.post("/pin", &req, None)
    }

    // ---- cloud-api

    /// The only call that wakes a computer (SPEC section 9). Called only after the
    /// user said yes, or when the computer's setting is `auto`.
    pub fn wake_consent(&self, reason: WakeReason, item_id: Option<&str>) -> Result<WakeConsentResponse, ApiError> {
        let body = WakeConsentRequest { reason, item_id: item_id.map(str::to_string), device_id: self.device_id.clone() };
        let b = serde_json::to_vec(&body).map_err(|e| ApiError::Decode(e.to_string()))?;
        let url = format!("{}/api/v1/computer-files/{}/wake-consent", self.cloud_api_base, enc(&self.computer_id));
        let mut req = self.agent.post(&url).set("content-type", "application/json");
        if let Some(a) = self.auth() {
            req = req.set("authorization", &a);
        }
        let r = match req.send_bytes(&b) {
            Ok(r) | Err(ureq::Error::Status(_, r)) => r,
            Err(e) => return Err(ApiError::Transport(e.to_string())),
        };
        let (s, e, body) = Self::read_body(r)?;
        Self::json(Self::finish(s, e, body)?)
    }
}

#[cfg(test)]
mod tests {
    use super::enc;

    // Item IDs and cursors are opaque and may hold any character; an unescaped
    // '&' or '#' would silently change the query.
    #[test]
    fn query_values_are_percent_encoded() {
        assert_eq!(enc("c1.a&b#c d"), "c1.a%26b%23c%20d");
        assert_eq!(enc("it_AZaz09-_.~"), "it_AZaz09-_.~");
    }
}
