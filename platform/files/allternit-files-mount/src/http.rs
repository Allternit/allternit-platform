//! `Backend` over a computer's `/api/v1/files/v2` (SPEC section 2), through
//! one of two transports (section 10):
//!
//! - **loopback**: plain HTTP to this machine's runtime (`http://127.0.0.1:8013`).
//! - **relay**: cloud-api's proxy, `POST /api/v1/runtime-devices/:id/proxy`
//!   with a `RelayRequest` (`body_encoding: base64` for bytes). The relay
//!   streams the runtime's response back as-is.
//!
//! Wake (section 9): the mount never wakes a computer on its own. Metadata is
//! served from the asleep snapshot. Content on a sleeping computer fails with
//! `wake_consent_required` unless the user agreed to wake it when mounting
//! (`allow_wake`, set by `gizzi files mount --wake` or its prompt). Then this
//! client asks cloud-api for a wake ticket and reuses it until it expires.
//!
//! Blocking (`reqwest::blocking`): FUSE callbacks are synchronous, and the
//! WebDAV server calls in from `spawn_blocking`. Build it outside any tokio
//! runtime: a blocking client must not be created or dropped on an async thread.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use allternit_files_core::{
    ChunkRef, Item, ListResponse, MutationResponse, OnConflict, Root, RootsResponse, UploadCommitResponse, UploadStatus,
};
use allternit_files_engine::chunker;
use base64::Engine as _;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::backend::{Backend, MountError, Result};

pub const FILES_V2_PREFIX: &str = "/api/v1/files/v2";
/// Small write limit (SPEC section 2): bigger files use an upload session.
pub const SMALL_WRITE_MAX: u64 = 4 * 1024 * 1024;
/// Chunk refs per upload request (SPEC section 5).
const REFS_PER_PAGE: usize = 10_000;
/// A wake ticket is dropped this long before it expires, so it never lapses in flight.
const TICKET_MARGIN: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub enum Transport {
    /// `base_url` ends in `/api/v1/files/v2`.
    Loopback { base_url: String },
    /// cloud-api's proxy URL for this computer.
    Relay { proxy_url: String },
}

/// How to reach one computer. `from_env` reads what `gizzi files mount` sets.
#[derive(Debug, Clone)]
pub struct HttpConfig {
    pub transport: Transport,
    pub computer_id: String,
    /// Bearer token: the runtime's (loopback) or cloud-api's (relay).
    pub auth: Option<String>,
    /// `x-allternit-user-id` on loopback (the local gateway reads it).
    pub user_id: Option<String>,
    /// cloud-api origin, for wake consent.
    pub cloud_url: String,
    pub cloud_auth: Option<String>,
    pub device_id: String,
    pub device_name: String,
    /// The user agreed to wake this computer for content reads and writes.
    pub allow_wake: bool,
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

impl HttpConfig {
    /// Reads `ALLTERNIT_FILES_*`. Tokens come by environment, never argv, so
    /// they don't show in `ps`.
    pub fn from_env() -> std::result::Result<Self, String> {
        let computer_id = env("ALLTERNIT_FILES_COMPUTER_ID").ok_or("ALLTERNIT_FILES_COMPUTER_ID is not set")?;
        let base = env("ALLTERNIT_FILES_BASE_URL").ok_or("ALLTERNIT_FILES_BASE_URL is not set")?;
        let transport = match env("ALLTERNIT_FILES_TRANSPORT").as_deref().unwrap_or("loopback") {
            "loopback" | "mesh" => Transport::Loopback { base_url: base.trim_end_matches('/').to_string() },
            "relay" => Transport::Relay { proxy_url: base },
            other => return Err(format!("ALLTERNIT_FILES_TRANSPORT={other} is not loopback, mesh or relay")),
        };
        Ok(Self {
            transport,
            computer_id,
            auth: env("ALLTERNIT_FILES_AUTH"),
            user_id: env("ALLTERNIT_FILES_USER_ID"),
            cloud_url: env("ALLTERNIT_FILES_CLOUD_URL").unwrap_or_else(|| "https://api.allternit.com".to_string()),
            cloud_auth: env("ALLTERNIT_FILES_CLOUD_AUTH"),
            device_id: env("ALLTERNIT_FILES_DEVICE_ID").unwrap_or_else(|| "dev_mount_unknown".to_string()),
            device_name: env("ALLTERNIT_FILES_DEVICE_NAME").unwrap_or_else(|| "Linux mount".to_string()),
            allow_wake: env("ALLTERNIT_FILES_ALLOW_WAKE").is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true")),
        })
    }
}

/// A request body.
enum Body {
    None,
    Json(Value),
    Bytes(Vec<u8>),
}

/// A response with its bytes read.
struct Resp {
    status: u16,
    body: Vec<u8>,
}

impl Resp {
    fn json<T: for<'de> Deserialize<'de>>(&self) -> Result<T> {
        serde_json::from_slice(&self.body)
            .map_err(|e| MountError::new("internal", self.status, format!("unexpected answer from the computer: {e}")))
    }
}

/// Loose `ErrorBody`: tolerate codes this build doesn't know.
#[derive(Deserialize, Default)]
struct LooseError {
    code: Option<String>,
    error: Option<Value>,
    message: Option<String>,
    retry_after_s: Option<u64>,
    missing_chunks: Option<Vec<String>>,
}

fn error_from(status: u16, body: &[u8]) -> MountError {
    let parsed: LooseError = serde_json::from_slice(body).unwrap_or_default();
    let code = parsed
        .code
        .or_else(|| parsed.error.as_ref().and_then(|v| v.as_str().map(str::to_string)))
        .unwrap_or_else(|| {
            match status {
                401 => "unauthorized",
                403 => "forbidden",
                404 => "not_found",
                413 => "payload_too_large",
                429 => "rate_limited",
                503 => "offline",
                _ => "internal",
            }
            .to_string()
        });
    let message = parsed.message.unwrap_or_else(|| format!("the computer answered {status}"));
    MountError { code, status, message, retry_after_s: parsed.retry_after_s, missing_chunks: parsed.missing_chunks.unwrap_or_default() }
}

pub struct HttpBackend {
    cfg: HttpConfig,
    client: reqwest::blocking::Client,
    ticket: Mutex<Option<(String, Instant)>>,
}

impl HttpBackend {
    pub fn new(cfg: HttpConfig) -> std::result::Result<Self, String> {
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            // The relay waits up to 30 s for a waking computer, 90 s for an answer.
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| format!("http client: {e}"))?;
        Ok(Self { cfg, client, ticket: Mutex::new(None) })
    }

    pub fn config(&self) -> &HttpConfig {
        &self.cfg
    }

    fn valid_ticket(&self) -> Option<String> {
        let mut t = self.ticket.lock().expect("ticket lock");
        match t.as_ref() {
            Some((v, until)) if Instant::now() + TICKET_MARGIN < *until => Some(v.clone()),
            _ => {
                *t = None;
                None
            }
        }
    }

    /// The consent call (SPEC section 9). Only reached when the user agreed.
    fn wake(&self) -> Result<()> {
        let url = format!("{}/api/v1/computer-files/{}/wake-consent", self.cfg.cloud_url.trim_end_matches('/'), urlencoding::encode(&self.cfg.computer_id));
        let mut req = self.client.post(url).json(&json!({ "reason": "open_file", "device_id": self.cfg.device_id }));
        if let Some(t) = &self.cfg.cloud_auth {
            req = req.bearer_auth(t);
        }
        let res = req.send().map_err(|e| MountError::new("offline", 0, format!("couldn't reach Allternit Cloud: {e}")))?;
        let status = res.status().as_u16();
        let body = res.bytes().map(|b| b.to_vec()).unwrap_or_default();
        if !(200..300).contains(&status) {
            return Err(error_from(status, &body));
        }
        #[derive(Deserialize)]
        struct Ticket {
            ticket: String,
            expires_at: String,
        }
        let t: Ticket = serde_json::from_slice(&body).map_err(|e| MountError::new("internal", status, format!("wake consent answer: {e}")))?;
        let left = chrono::DateTime::parse_from_rfc3339(&t.expires_at)
            .map(|at| (at.timestamp() - chrono::Utc::now().timestamp()).max(0) as u64)
            .unwrap_or(60);
        tracing::info!(computer = %self.cfg.computer_id, "woke the computer (consent given when mounting)");
        *self.ticket.lock().expect("ticket lock") = Some((t.ticket, Instant::now() + Duration::from_secs(left)));
        Ok(())
    }

    /// One HTTP exchange, no retries.
    fn send_once(&self, method: &str, path_q: &str, headers: &[(&str, String)], body: &Body) -> Result<Resp> {
        let mut hdrs: Vec<(String, String)> = vec![
            ("x-allternit-device-id".into(), self.cfg.device_id.clone()),
            ("x-allternit-device-name".into(), self.cfg.device_name.clone()),
        ];
        if let Some(t) = self.valid_ticket() {
            hdrs.push(("x-allternit-wake-ticket".into(), t));
        }
        for (k, v) in headers {
            hdrs.push((k.to_string(), v.clone()));
        }
        let offline = |e: reqwest::Error| MountError::new("offline", 0, format!("couldn't reach the computer: {e}"));
        let method_v = reqwest::Method::from_bytes(method.as_bytes()).map_err(|_| MountError::new("internal", 0, "bad method"))?;
        let res = match &self.cfg.transport {
            Transport::Loopback { base_url } => {
                let mut req = self.client.request(method_v, format!("{base_url}{path_q}"));
                if let Some(t) = &self.cfg.auth {
                    req = req.bearer_auth(t);
                }
                if let Some(u) = &self.cfg.user_id {
                    req = req.header("x-allternit-user-id", u);
                }
                for (k, v) in &hdrs {
                    req = req.header(k.as_str(), v.as_str());
                }
                req = match body {
                    Body::None => req,
                    Body::Json(v) => req.json(v),
                    Body::Bytes(b) => req.header("content-type", "application/octet-stream").body(b.clone()),
                };
                req.send().map_err(offline)?
            }
            Transport::Relay { proxy_url } => {
                let mut map: HashMap<String, String> = hdrs.into_iter().collect();
                let (text, encoding) = match body {
                    Body::None => (String::new(), "utf8"),
                    Body::Json(v) => {
                        map.entry("content-type".into()).or_insert_with(|| "application/json".into());
                        (v.to_string(), "utf8")
                    }
                    Body::Bytes(b) => {
                        map.entry("content-type".into()).or_insert_with(|| "application/octet-stream".into());
                        (base64::engine::general_purpose::STANDARD.encode(b), "base64")
                    }
                };
                // cloud-api reads `bodyEncoding`; the SPEC field is `body_encoding`. Send both.
                let envelope = json!({
                    "method": method,
                    "path": format!("{FILES_V2_PREFIX}{path_q}"),
                    "headers": map,
                    "body": text,
                    "body_encoding": encoding,
                    "bodyEncoding": encoding,
                });
                let mut req = self.client.post(proxy_url).json(&envelope);
                if let Some(t) = &self.cfg.auth {
                    req = req.bearer_auth(t);
                }
                req.send().map_err(offline)?
            }
        };
        let status = res.status().as_u16();
        let body = res.bytes().map_err(offline)?.to_vec();
        Ok(unwrap_stub_envelope(Resp { status, body }))
    }

    /// One call with the wake and `waking` retries.
    fn call(&self, method: &str, path_q: &str, headers: &[(&str, String)], body: Body) -> Result<Resp> {
        let mut attempt = 0;
        loop {
            let res = self.send_once(method, path_q, headers, &body)?;
            if (200..300).contains(&res.status) || res.status == 304 {
                return Ok(res);
            }
            let err = error_from(res.status, &res.body);
            attempt += 1;
            if attempt > 4 {
                return Err(err);
            }
            match err.code.as_str() {
                "wake_consent_required" if self.cfg.allow_wake => self.wake()?,
                "waking" => std::thread::sleep(Duration::from_secs(err.retry_after_s.unwrap_or(3).clamp(1, 10))),
                _ => return Err(err),
            }
        }
    }

    fn json_call<T: for<'de> Deserialize<'de>>(&self, method: &str, path_q: &str, body: Body) -> Result<T> {
        self.call(method, path_q, &[], body)?.json()
    }

    fn put_chunk(&self, session: &str, file: &mut File, r: &ChunkRef) -> Result<()> {
        let mut buf = vec![0u8; r.size as usize];
        file.seek(SeekFrom::Start(r.offset)).map_err(MountError::io)?;
        file.read_exact(&mut buf).map_err(MountError::io)?;
        self.call("PUT", &format!("/uploads/{}/chunks/{}", enc(session), r.hash), &[], Body::Bytes(buf))?;
        Ok(())
    }

    /// Upload session (SPEC section 5): pinned chunker, have/need, resume missing once.
    fn upload(&self, root_id: &str, parent_id: &str, name: &str, replace: Option<&Item>, on_conflict: OnConflict, src: &Path) -> Result<Item> {
        let chunked = chunker::chunk_reader(File::open(src).map_err(MountError::io)?, |_| Ok(()))
            .map_err(|e| MountError::new("internal", 0, format!("chunking: {e}")))?;
        let mut create = json!({
            "root_id": root_id,
            "parent_id": parent_id,
            "name": name,
            "size": chunked.size,
            "content_hash": chunked.content_hash,
            "chunker": chunker::params(),
            "chunks": chunked.chunks.iter().take(REFS_PER_PAGE).collect::<Vec<_>>(),
            "total_chunks": chunked.chunks.len(),
            "on_conflict": on_conflict,
            "device_name": self.cfg.device_name,
            "utc_offset_minutes": utc_offset_minutes(),
        });
        if let Some(item) = replace {
            create["item_id"] = json!(item.id);
            create["base_rev"] = json!(item.rev);
        }
        let mut status: UploadStatus = self.json_call("POST", "/uploads", Body::Json(create))?;
        for page in chunked.chunks.chunks(REFS_PER_PAGE).skip(1) {
            status = self.json_call("POST", &format!("/uploads/{}/chunks", enc(&status.session_id)), Body::Json(json!({ "chunks": page })))?;
        }
        let by_hash: HashMap<&str, &ChunkRef> = chunked.chunks.iter().map(|c| (c.hash.as_str(), c)).collect();
        let mut file = File::open(src).map_err(MountError::io)?;
        let session = status.session_id.clone();
        let send = |need: &[String], file: &mut File| -> Result<()> {
            for h in need {
                let r = by_hash.get(h.as_str()).ok_or_else(|| MountError::new("manifest_invalid", 400, "the computer asked for a piece this file doesn't have"))?;
                self.put_chunk(&session, file, r)?;
            }
            Ok(())
        };
        send(&status.need, &mut file)?;
        let commit_path = format!("/uploads/{}/commit", enc(&session));
        let commit: UploadCommitResponse = match self.json_call("POST", &commit_path, Body::None) {
            Err(e) if e.code == "chunk_missing" => {
                send(&e.missing_chunks, &mut file)?;
                self.json_call("POST", &commit_path, Body::None)?
            }
            other => other?,
        };
        if let Some(c) = &commit.conflict {
            tracing::warn!(name = %name, copy = %c.conflict_name, "changed elsewhere: saved as a conflict copy");
        }
        Ok(commit.item)
    }
}

/// The Files stub (`tools/files-stub`) wraps relayed answers in
/// `{status, headers, body, body_encoding}`; production streams them raw.
fn unwrap_stub_envelope(res: Resp) -> Resp {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Env {
        status: u16,
        #[allow(dead_code)]
        headers: Option<Value>,
        body: String,
        body_encoding: String,
    }
    if res.status != 200 || res.body.first() != Some(&b'{') {
        return res;
    }
    match serde_json::from_slice::<Env>(&res.body) {
        Ok(env) => {
            let body = if env.body_encoding == "base64" {
                base64::engine::general_purpose::STANDARD.decode(env.body.as_bytes()).unwrap_or_default()
            } else {
                env.body.into_bytes()
            };
            Resp { status: env.status, body }
        }
        Err(_) => res,
    }
}

fn enc(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}

/// This machine's UTC offset in minutes, clamped to the contract's range (conflict copy names).
pub fn utc_offset_minutes() -> i64 {
    let secs = chrono::Local::now().offset().local_minus_utc() as i64;
    (secs / 60).clamp(-840, 840)
}

/// `bytes=a-b` for `len` bytes at `offset`.
pub fn range_header(offset: u64, len: u64) -> String {
    format!("bytes={}-{}", offset, offset + len.max(1) - 1)
}

impl Backend for HttpBackend {
    fn roots(&self) -> Result<Vec<Root>> {
        Ok(self.json_call::<RootsResponse>("GET", "/roots", Body::None)?.roots)
    }

    fn list(&self, root_id: &str, parent_id: &str) -> Result<Vec<Item>> {
        let mut items = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut q = format!("/roots/{}/list?parent_id={}&limit=500", enc(root_id), enc(parent_id));
            if let Some(t) = &token {
                q.push_str(&format!("&page_token={}", enc(t)));
            }
            let page: ListResponse = self.json_call("GET", &q, Body::None)?;
            items.extend(page.items);
            match page.next_page_token {
                Some(t) if !t.is_empty() => token = Some(t),
                _ => return Ok(items),
            }
        }
    }

    fn read(&self, item: &Item, offset: u64, len: u64) -> Result<Vec<u8>> {
        if len == 0 || offset >= item.size {
            return Ok(Vec::new());
        }
        let len = len.min(item.size - offset);
        let res = self.call("GET", &format!("/items/{}/content", enc(&item.id)), &[("range", range_header(offset, len))], Body::None)?;
        if res.status == 206 {
            return Ok(res.body);
        }
        // 200: the whole file came back; take our slice.
        let start = (offset as usize).min(res.body.len());
        let end = (start + len as usize).min(res.body.len());
        Ok(res.body[start..end].to_vec())
    }

    fn write(&self, root_id: &str, parent_id: &str, name: &str, replace: Option<&Item>, on_conflict: OnConflict, src: &Path) -> Result<Item> {
        let size = std::fs::metadata(src).map_err(MountError::io)?.len();
        if size > SMALL_WRITE_MAX {
            return self.upload(root_id, parent_id, name, replace, on_conflict, src);
        }
        let bytes = std::fs::read(src).map_err(MountError::io)?;
        let conflict = serde_json::to_value(on_conflict).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_else(|| "keep_both".into());
        let mut q = format!(
            "/roots/{}/content?parent_id={}&name={}&on_conflict={}&device_name={}&utc_offset_minutes={}",
            enc(root_id),
            enc(parent_id),
            enc(name),
            conflict,
            enc(&self.cfg.device_name),
            utc_offset_minutes()
        );
        if let Some(item) = replace {
            q.push_str(&format!("&base_rev={}", enc(&item.rev)));
        }
        let m: MutationResponse = self.call("PUT", &q, &[], Body::Bytes(bytes))?.json()?;
        if let Some(c) = &m.conflict {
            tracing::warn!(name = %name, copy = %c.conflict_name, "changed elsewhere: saved as a conflict copy");
        }
        Ok(m.item)
    }

    fn mkdir(&self, parent_id: &str, name: &str) -> Result<Item> {
        let m: MutationResponse = self.json_call("POST", "/mkdir", Body::Json(json!({ "parent_id": parent_id, "name": name, "on_conflict": "fail" })))?;
        Ok(m.item)
    }

    fn rename(&self, item: &Item, new_parent_id: Option<&str>, new_name: Option<&str>, on_conflict: OnConflict) -> Result<Item> {
        let mut body = json!({ "item_id": item.id, "base_rev": item.rev, "on_conflict": on_conflict });
        if let Some(p) = new_parent_id {
            body["new_parent_id"] = json!(p);
        }
        if let Some(n) = new_name {
            body["new_name"] = json!(n);
        }
        Ok(self.json_call::<MutationResponse>("POST", "/move", Body::Json(body))?.item)
    }

    fn copy(&self, item: &Item, dest_parent_id: &str, new_name: Option<&str>, on_conflict: OnConflict) -> Result<Item> {
        let mut body = json!({ "item_id": item.id, "dest_parent_id": dest_parent_id, "on_conflict": on_conflict });
        if let Some(n) = new_name {
            body["new_name"] = json!(n);
        }
        let v: Value = self.json_call("POST", "/copy", Body::Json(body))?;
        serde_json::from_value::<Item>(v.get("item").cloned().unwrap_or(Value::Null))
            .map_err(|_| MountError::new("unsupported", 501, "this copy runs as a background job; copy it with `gizzi files cp` instead"))
    }

    fn delete(&self, item: &Item) -> Result<()> {
        self.call("POST", "/delete", &[], Body::Json(json!({ "item_id": item.id, "base_rev": item.rev, "permanent": false })))?;
        Ok(())
    }
}
