//! Guests on the runtime (`contracts/files-drive/SPEC.md` sections 7, 8, 14):
//! `Authorization: Files-Guest <fgt1 token>`, sent by the cloud-api gateway.
//!
//! This is the second of the two guest checks. cloud-api already checked the
//! token and the share's revocation state; the runtime checks the token again
//! offline and then the scope on every request:
//!
//! - **Token.** `fgt1.<b64url claims>.<b64url Ed25519 sig over "fgt1." + claims>`,
//!   signed by cloud-api with the key named by `kid`. Public keys come from
//!   `GET /api/v1/computer-files/token-keys` (env
//!   `ALLTERNIT_FILES_TOKEN_KEYS_URL`), cached for an hour and refetched when a
//!   token names a `kid` the cache does not have (at most once every 30 s, so a
//!   forged `kid` cannot make the runtime hammer cloud-api). `exp` must be in
//!   the future and at most 1 h after `iat`.
//! - **Scope.** `computer_id` must be this runtime, `root_id` the item's root,
//!   and the item must be the shared item or below it. The root must be
//!   `shareable` and not protected.
//! - **Role.** [`gate`] admits only the guest subset of routes below and only
//!   the operations the role allows; everything else is `forbidden`.
//!   `upload_only` cannot list, stat or read (`forbidden`) and can only add new
//!   files (no `base_rev`, always keep-both).
//! - **Identity.** A request with the `Files-Guest` scheme is always a guest,
//!   even when the relay client also signed it in as the owner (the replay
//!   adds the owner's desktop token). Guests never see local paths, pins or
//!   owner permissions, and never send the protected-root unlock.
//! - **Audit.** Every guest request with a verified token emits
//!   `files.share.accessed` (allowed or denied) on [`super::FilesV2::events`].
//!
//! Guest routes: `GET /ping`, `GET /whoami`, `GET /roots/:root_id/list`,
//! `GET /items/:item_id`, `GET /items/:item_id/content`,
//! `GET /items/:item_id/comments`, `POST /comments`,
//! `PUT /roots/:root_id/content`, `POST /mkdir`, and the upload session routes
//! (a session is usable only by the guest that opened it).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use allternit_files_core::{
    ErrorBody, ErrorCode, FilesEvent, GuestCapabilityClaims, ItemKind, Permissions, PrincipalKind, PrincipalRef,
    RootGrant, ShareAccess, ShareAccessAction, ShareAccessResult, ShareAccessed, ShareRole, WhoAmIResponse,
};
use allternit_files_engine::{ItemRecord, RootRecord};
use axum::body::Body;
use axum::extract::Request;
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::Deserialize;
use tokio::sync::RwLock;

use super::{live, ApiError, Fail, FilesV2, Inner, Need, Req};

/// The `Authorization` scheme the gateway sends.
pub const GUEST_SCHEME: &str = "Files-Guest";
/// Guest token prefix (SPEC section 8; `shares.json` pins it).
pub const TOKEN_PREFIX: &str = "fgt1";
/// Where the runtime fetches cloud-api's public token keys.
pub const KEYS_URL_ENV: &str = "ALLTERNIT_FILES_TOKEN_KEYS_URL";
const DEFAULT_KEYS_URL: &str = "https://api.allternit.com/api/v1/computer-files/token-keys";
/// A guest session lasts at most an hour (SPEC section 8).
pub const MAX_TOKEN_SECS: u64 = 3600;
/// Clock skew allowed on `iat`.
const SKEW_SECS: u64 = 60;
const KEYS_TTL: Duration = Duration::from_secs(3600);
/// Lower bound between two key fetches (unknown `kid`s, fetch failures).
const MIN_REFETCH: Duration = Duration::from_secs(30);
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
/// Error bodies are read back to name the code in the audit event.
const ERROR_BODY_LIMIT: usize = 64 * 1024;
/// Walking up from an item to the shared item stops here (a loop guard).
const MAX_DEPTH: usize = 4096;

// ---- token keys ----

#[derive(Deserialize)]
struct KeysResponse {
    keys: Vec<KeyEntry>,
}

#[derive(Deserialize)]
struct KeyEntry {
    kid: String,
    alg: String,
    public_key: String,
}

struct KeyCache {
    keys: HashMap<String, VerifyingKey>,
    fetched_at: Option<Instant>,
}

/// cloud-api's guest-token public keys, verified offline.
pub struct GuestAuth {
    url: Option<String>,
    client: reqwest::Client,
    /// Fixed keys (tests); never fetched.
    pinned: bool,
    cache: RwLock<KeyCache>,
    /// Upload sessions a guest opened: session id -> guest id.
    sessions: Mutex<HashMap<String, String>>,
}

impl GuestAuth {
    pub fn from_env() -> Self {
        // The explicit URL, else the paired cloud-api's, else production.
        let url = std::env::var(KEYS_URL_ENV)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .or_else(|| {
                std::env::var("ALLTERNIT_CLOUD_API_URL")
                    .ok()
                    .filter(|v| !v.trim().is_empty())
                    .map(|base| format!("{}/api/v1/computer-files/token-keys", base.trim().trim_end_matches('/')))
            })
            .unwrap_or_else(|| DEFAULT_KEYS_URL.to_string());
        Self::build(Some(url), HashMap::new(), false)
    }

    /// Fixed `(kid, 32-byte public key)` pairs, never refreshed.
    pub fn with_keys(keys: Vec<(String, [u8; 32])>) -> Self {
        let keys = keys
            .into_iter()
            .filter_map(|(kid, raw)| VerifyingKey::from_bytes(&raw).ok().map(|k| (kid, k)))
            .collect();
        Self::build(None, keys, true)
    }

    fn build(url: Option<String>, keys: HashMap<String, VerifyingKey>, pinned: bool) -> Self {
        Self {
            url,
            client: reqwest::Client::builder().timeout(FETCH_TIMEOUT).build().unwrap_or_else(|_| reqwest::Client::new()),
            pinned,
            cache: RwLock::new(KeyCache { keys, fetched_at: None }),
            sessions: Mutex::new(HashMap::new()),
        }
    }

    async fn key(&self, kid: &str) -> Option<VerifyingKey> {
        let (hit, stale, may_fetch) = {
            let c = self.cache.read().await;
            let hit = c.keys.get(kid).copied();
            let stale = c.fetched_at.map_or(true, |t| t.elapsed() >= KEYS_TTL);
            let may_fetch = !self.pinned && c.fetched_at.map_or(true, |t| t.elapsed() >= MIN_REFETCH);
            (hit, stale, may_fetch)
        };
        if self.pinned || !may_fetch || (hit.is_some() && !stale) {
            return hit;
        }
        let url = self.url.clone()?;
        let fetched = self.fetch(&url).await;
        let mut c = self.cache.write().await;
        c.fetched_at = Some(Instant::now());
        match fetched {
            Ok(keys) => c.keys = keys,
            Err(e) => tracing::warn!(error = %e, "files v2: guest token keys not refreshed"),
        }
        c.keys.get(kid).copied()
    }

    async fn fetch(&self, url: &str) -> Result<HashMap<String, VerifyingKey>, String> {
        let res = self.client.get(url).send().await.map_err(|e| e.to_string())?;
        if !res.status().is_success() {
            return Err(format!("HTTP {}", res.status()));
        }
        let body: KeysResponse = res.json().await.map_err(|e| e.to_string())?;
        Ok(body
            .keys
            .into_iter()
            .filter(|k| k.alg == "Ed25519")
            .filter_map(|k| {
                let raw: [u8; 32] = URL_SAFE_NO_PAD.decode(k.public_key.trim()).ok()?.try_into().ok()?;
                VerifyingKey::from_bytes(&raw).ok().map(|key| (k.kid, key))
            })
            .collect())
    }

    /// Signature, `typ`, `v`, `iat` and `exp`. Scope is checked by the caller.
    pub async fn verify(&self, token: &str, now: u64) -> Result<GuestCapabilityClaims, ApiError> {
        let bad = || ApiError::new(ErrorCode::Unauthorized, "This share link session is not valid. Open the link again.");
        let mut parts = token.trim().split('.');
        let (Some(prefix), Some(payload), Some(sig), None) = (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(bad());
        };
        if prefix != TOKEN_PREFIX {
            return Err(bad());
        }
        let claims: GuestCapabilityClaims = URL_SAFE_NO_PAD
            .decode(payload)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .ok_or_else(bad)?;
        let sig = URL_SAFE_NO_PAD.decode(sig).ok().and_then(|b| Signature::from_slice(&b).ok()).ok_or_else(bad)?;
        let key = self.key(&claims.kid).await.ok_or_else(bad)?;
        key.verify_strict(format!("{prefix}.{payload}").as_bytes(), &sig).map_err(|_| bad())?;
        if claims.typ != "files_guest" || claims.v != 1 {
            return Err(bad());
        }
        if claims.exp <= now {
            return Err(ApiError::new(ErrorCode::Unauthorized, "This share link session has expired. Open the link again."));
        }
        if claims.iat > now + SKEW_SECS || claims.exp > claims.iat + MAX_TOKEN_SECS {
            return Err(bad());
        }
        Ok(claims)
    }

    pub(crate) fn remember_session(&self, session_id: &str, guest_id: &str) {
        self.sessions.lock().expect("sessions lock").insert(session_id.to_string(), guest_id.to_string());
    }

    fn owns_session(&self, session_id: &str, guest_id: &str) -> bool {
        self.sessions.lock().expect("sessions lock").get(session_id).is_some_and(|g| g == guest_id)
    }

    pub(crate) fn forget_session(&self, session_id: &str) {
        self.sessions.lock().expect("sessions lock").remove(session_id);
    }
}

// ---- the guest and what it may do ----

/// What a guest request is about to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Op {
    WhoAmI,
    List,
    Stat,
    Download,
    ReadComments,
    Comment,
    Write,
    Mkdir,
    UploadCreate,
    UploadSession,
}

impl Op {
    fn action(self) -> ShareAccessAction {
        match self {
            Op::WhoAmI | Op::List | Op::Stat => ShareAccessAction::List,
            Op::Download => ShareAccessAction::Download,
            Op::ReadComments | Op::Comment => ShareAccessAction::Comment,
            Op::Write | Op::Mkdir | Op::UploadCreate | Op::UploadSession => ShareAccessAction::Upload,
        }
    }
}

/// A verified guest, put in the request extensions by [`gate`].
#[derive(Clone, Debug)]
pub(crate) struct Guest {
    pub claims: GuestCapabilityClaims,
}

impl Guest {
    pub fn principal(&self) -> PrincipalRef {
        PrincipalRef { kind: PrincipalKind::Guest, id: self.claims.guest_id.clone(), display_name: None }
    }
    pub fn upload_only(&self) -> bool {
        self.claims.role == ShareRole::UploadOnly
    }
}

fn views(role: ShareRole) -> bool {
    matches!(role, ShareRole::ViewDownload | ShareRole::Comment | ShareRole::Edit)
}

/// Role -> operation (SPEC section 8).
fn role_allows(role: ShareRole, op: Op) -> Result<(), ApiError> {
    let ok = match op {
        Op::WhoAmI => true,
        Op::List | Op::Stat | Op::Download => views(role),
        Op::ReadComments | Op::Comment => matches!(role, ShareRole::Comment | ShareRole::Edit),
        Op::Write | Op::UploadCreate | Op::UploadSession => matches!(role, ShareRole::Edit | ShareRole::UploadOnly),
        Op::Mkdir => role == ShareRole::Edit,
    };
    if ok {
        Ok(())
    } else if role == ShareRole::UploadOnly {
        Err(ApiError::forbidden("This link only allows uploads."))
    } else {
        Err(ApiError::forbidden("This link doesn't allow that."))
    }
}

/// The guest subset of the v2 routes. `None`: not open to guests.
pub(crate) fn classify(method: &str, path: &str) -> Option<Op> {
    let segs: Vec<&str> = path.trim_matches('/').split('/').collect();
    Some(match (method, segs.as_slice()) {
        ("GET", ["ping"]) | ("GET", ["whoami"]) => Op::WhoAmI,
        ("GET", ["roots", _, "list"]) => Op::List,
        ("PUT", ["roots", _, "content"]) => Op::Write,
        ("GET", ["items", _]) => Op::Stat,
        ("GET", ["items", _, "content"]) => Op::Download,
        ("GET", ["items", _, "comments"]) => Op::ReadComments,
        ("POST", ["comments"]) => Op::Comment,
        ("POST", ["mkdir"]) => Op::Mkdir,
        ("POST", ["uploads"]) => Op::UploadCreate,
        ("GET", ["uploads", _])
        | ("DELETE", ["uploads", _])
        | ("POST", ["uploads", _, "chunks"])
        | ("PUT", ["uploads", _, "chunks", _])
        | ("POST", ["uploads", _, "commit"]) => Op::UploadSession,
        _ => return None,
    })
}

/// The path below `/files/v2`, whether or not the router stripped the nest.
fn v2_path(path: &str) -> &str {
    path.strip_prefix("/api/v1/files/v2").or_else(|| path.strip_prefix("/files/v2")).unwrap_or(path)
}

fn upload_session_id(path: &str) -> Option<&str> {
    let mut segs = path.trim_matches('/').split('/');
    (segs.next() == Some("uploads")).then(|| segs.next()).flatten()
}

/// The item a request names, for the audit row.
fn named_item(path: &str, query: Option<&str>) -> Option<String> {
    let segs: Vec<&str> = path.trim_matches('/').split('/').collect();
    if let ["items", id, ..] = segs.as_slice() {
        return Some((*id).to_string());
    }
    query?
        .split('&')
        .filter_map(|p| p.split_once('='))
        .find(|(k, _)| *k == "parent_id")
        .map(|(_, v)| v.to_string())
}

/// The token from `Authorization: Files-Guest <token>` (scheme case-insensitive).
pub(crate) fn guest_token(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(axum::http::header::AUTHORIZATION)?.to_str().ok()?.trim();
    let (scheme, token) = raw.split_once(' ')?;
    scheme.eq_ignore_ascii_case(GUEST_SCHEME).then(|| token.trim().to_string())
}

/// Any `Files-Guest` credential, valid or not. Such a request is never the owner.
pub(crate) fn has_guest_scheme(headers: &HeaderMap) -> bool {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().split(' ').next())
        .is_some_and(|s| s.eq_ignore_ascii_case(GUEST_SCHEME))
}

fn now_secs() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}

fn stamp() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Middleware on the v2 router: admits a guest request only after the token,
/// computer, route and role checks, and audits it. Owner requests pass through.
pub(crate) async fn gate(mut req: Request, next: Next) -> Response {
    if !has_guest_scheme(req.headers()) {
        return next.run(req).await;
    }
    let Some(files) = req.extensions().get::<Arc<FilesV2>>().cloned() else {
        return ApiError::internal().into_response();
    };
    if !files.cfg.enabled {
        return ApiError::new(ErrorCode::FeatureDisabled, "The Files drive is not turned on for this computer.").into_response();
    }
    let Some(token) = guest_token(req.headers()) else {
        return ApiError::new(ErrorCode::Unauthorized, "This share link session is not valid. Open the link again.").into_response();
    };
    // An unverified token names nobody, so it gets no audit event.
    let claims = match files.guests.verify(&token, now_secs()).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    let path = v2_path(req.uri().path()).to_string();
    let item_id = named_item(&path, req.uri().query());
    let op = classify(req.method().as_str(), &path);
    let action = op.map(Op::action).unwrap_or(ShareAccessAction::List);
    let denial = if claims.computer_id != files.cfg.computer_id {
        Some(ApiError::new(ErrorCode::ScopeViolation, "This link is for another computer."))
    } else {
        match op {
            None => Some(ApiError::forbidden("Share links can't do that.")),
            Some(op) => role_allows(claims.role, op).err().or_else(|| {
                let session = (op == Op::UploadSession).then(|| upload_session_id(&path)).flatten();
                session
                    .filter(|s| !files.guests.owns_session(s, &claims.guest_id))
                    .map(|_| ApiError::new(ErrorCode::UploadSessionNotFound, "No such upload."))
            }),
        }
    };
    if let Some(e) = denial {
        files.share_accessed(&claims, action, item_id, Some(e.body.code.clone()));
        return e.into_response();
    }
    let guest = Guest { claims: claims.clone() };
    req.extensions_mut().insert(Arc::new(guest));
    let res = next.run(req).await;
    let (res, error) = error_code(res).await;
    files.share_accessed(&claims, action, item_id, error);
    res
}

/// For a 4xx/5xx, the `ErrorBody` code, with the response rebuilt.
async fn error_code(res: Response) -> (Response, Option<ErrorCode>) {
    if res.status().as_u16() < 400 {
        return (res, None);
    }
    let (parts, body) = res.into_parts();
    match axum::body::to_bytes(body, ERROR_BODY_LIMIT).await {
        Ok(bytes) => {
            let code = serde_json::from_slice::<ErrorBody>(&bytes).map(|b| b.code).unwrap_or(ErrorCode::Internal);
            (Response::from_parts(parts, Body::from(bytes)), Some(code))
        }
        Err(_) => (ApiError::internal().into_response(), Some(ErrorCode::Internal)),
    }
}

impl FilesV2 {
    /// `files.share.accessed` (SPEC section 14): one per verified guest request.
    pub(crate) fn share_accessed(
        &self,
        claims: &GuestCapabilityClaims,
        action: ShareAccessAction,
        item_id: Option<String>,
        error: Option<ErrorCode>,
    ) {
        let at = stamp();
        let result = if error.is_some() { ShareAccessResult::Denied } else { ShareAccessResult::Allowed };
        tracing::info!(
            target: "files.share.accessed",
            share_id = %claims.share_id,
            guest_id = %claims.guest_id,
            action = action.as_str(),
            result = result.as_str(),
            error = error.as_ref().map(|e| e.as_str()).unwrap_or(""),
            "guest access"
        );
        let event = FilesEvent::ShareAccessed(ShareAccessed {
            event_id: format!("ev_{}", hex::encode(rand::random::<[u8; 12]>())),
            at: at.clone(),
            computer_id: self.cfg.computer_id.clone(),
            access: ShareAccess {
                share_id: claims.share_id.clone(),
                guest_id: claims.guest_id.clone(),
                action,
                item_id,
                result,
                error,
                at,
            },
        });
        // No subscriber is fine: the event is also in the log above.
        let _ = self.events.send(event);
    }
}

// ---- scope checks, called by `Inner::guard` and the guest-reachable handlers ----

/// The root half of the second check.
pub(crate) fn check_root(g: &Guest, root: &RootRecord, need: Need) -> Result<(), Fail> {
    if root.root_id != g.claims.root_id {
        return Err(ApiError::new(ErrorCode::ScopeViolation, "That item is not in the shared folder.").into());
    }
    if !root.policy.shareable || root.policy.protected {
        return Err(ApiError::forbidden("This folder is not shared.").into());
    }
    let writer = matches!(g.claims.role, ShareRole::Edit | ShareRole::UploadOnly);
    if need == Need::Write && !writer {
        return Err(ApiError::forbidden("This link doesn't allow changes.").into());
    }
    Ok(())
}

/// The item is the shared item or below it.
pub(crate) fn check_subtree(v: &Inner, g: &Guest, item: &ItemRecord) -> Result<(), Fail> {
    if item.root_id != g.claims.root_id {
        return Err(ApiError::new(ErrorCode::ScopeViolation, "That item is not in the shared folder.").into());
    }
    let mut cur = item.clone();
    for _ in 0..MAX_DEPTH {
        if cur.id == g.claims.item_id {
            return Ok(());
        }
        match cur.parent_id.as_deref() {
            Some(parent) => cur = v.drive.stat(parent)?,
            None => break,
        }
    }
    Err(ApiError::new(ErrorCode::ScopeViolation, "That item is not in the shared folder.").into())
}

/// `upload_only` adds new files only: no `base_rev`, no target item.
pub(crate) fn check_new_only(g: &Guest, base_rev: bool, item_id: bool) -> Result<(), Fail> {
    if g.upload_only() && (base_rev || item_id) {
        return Err(ApiError::forbidden("This link can only add new files.").into());
    }
    Ok(())
}

/// What a guest may do with one item (SPEC section 7), from the role and the
/// root policy. Never `delete`, `share`, `agent_process` or `execute`.
pub(crate) fn permissions(role: ShareRole, root: &RootRecord, item: &ItemRecord) -> Permissions {
    let p = &root.policy;
    let folder = item.kind == ItemKind::Folder;
    let readable = views(role) && p.read && !item.link_escapes;
    let write = match role {
        ShareRole::Edit => p.write,
        ShareRole::UploadOnly => p.write && folder,
        _ => false,
    };
    Permissions {
        list: views(role) && p.read && folder,
        read: readable,
        write,
        delete: false,
        share: false,
        comment: matches!(role, ShareRole::Comment | ShareRole::Edit) && p.read,
        agent_process: false,
        transfer: readable,
        execute: false,
    }
}

/// `GET /whoami` for a guest: one grant, scoped to the shared item.
pub(crate) fn whoami(v: &Inner, req: &Req, g: &Guest) -> Result<WhoAmIResponse, Fail> {
    let (root, item) = v.guard_item(req, &g.claims.item_id, Need::Read)?;
    Ok(WhoAmIResponse {
        principal: g.principal(),
        computer_id: v.cfg.computer_id.clone(),
        grants: vec![RootGrant {
            root_id: root.root_id.clone(),
            permissions: permissions(g.claims.role, &root, &item),
            item_scope: Some(g.claims.item_id.clone()),
        }],
        freshness: live(),
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use allternit_files_core::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::{Extension, Router};
    use ed25519_dalek::{Signer, SigningKey};
    use http_body_util::BodyExt;
    use serde_json::{json, Value};
    use tower::ServiceExt;

    use super::*;
    use crate::auth::AuthUser;
    use crate::files_v2::{routes, Config};

    const KID: &str = "ft-test";
    const COMPUTER: &str = "rd_guest_test";

    fn signer() -> SigningKey {
        SigningKey::from_bytes(&[9u8; 32])
    }

    struct T {
        _tmp: tempfile::TempDir,
        app: Router,
        files: Arc<FilesV2>,
        root: Root,
        shared: String,
        private: String,
        shared_file: String,
        private_file: String,
    }

    fn owner() -> AuthUser {
        AuthUser {
            user_id: "u1".into(),
            email: None,
            name: Some("Eoj".into()),
            avatar_url: None,
            tenant_id: None,
            organization_id: None,
            organization_role: None,
            organization_slug: None,
        }
    }

    fn config(base: &Path) -> Config {
        Config {
            enabled: true,
            data_dir: base.join("data"),
            home: Some(base.join("home")),
            computer_id: COMPUTER.into(),
            memory_db: None,
            models_dir: None,
            code_dir: None,
            watch: false,
            clock: None,
        }
    }

    struct Resp {
        status: StatusCode,
        body: Vec<u8>,
    }

    impl Resp {
        fn json(&self) -> Value {
            serde_json::from_slice(&self.body).unwrap_or_else(|_| panic!("not json: {}", String::from_utf8_lossy(&self.body)))
        }
        fn code(&self) -> ErrorCode {
            let e: ErrorBody = serde_json::from_value(self.json()).unwrap();
            assert_eq!(self.status, crate::files_v2::status_for(e.code.clone()), "status follows SPEC section 16");
            e.code
        }
    }

    async fn call(app: &Router, method: &str, uri: &str, auth: Option<&str>, body: Body) -> Resp {
        let mut b = Request::builder().method(method).uri(format!("/files/v2{uri}"));
        if let Some(token) = auth {
            b = b.header("authorization", format!("Files-Guest {token}"));
            // What the relay replay adds: the owner's desktop sign-in and the relayed marker.
            b = b.header("x-allternit-relayed", "1");
        }
        if method == "POST" {
            b = b.header("content-type", "application/json");
        }
        let res = app.clone().oneshot(b.body(body).unwrap()).await.unwrap();
        let status = res.status();
        let body = res.into_body().collect().await.unwrap().to_bytes().to_vec();
        Resp { status, body }
    }

    async fn owner_post(app: &Router, uri: &str, body: Value) -> Value {
        let r = call(app, "POST", uri, None, Body::from(body.to_string())).await;
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
        r.json()
    }

    async fn owner_put(app: &Router, root: &str, parent: &str, name: &str, data: &[u8]) -> String {
        let r = call(app, "PUT", &format!("/roots/{root}/content?parent_id={parent}&name={name}"), None, Body::from(data.to_vec())).await;
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
        r.json()["item"]["id"].as_str().unwrap().to_string()
    }

    /// A shareable root with `Shared/a.txt` and `Private/secret.txt`.
    async fn setup(shareable: bool) -> T {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        std::fs::create_dir_all(base.join("home/Docs")).unwrap();
        let pk = signer().verifying_key().to_bytes();
        let files = Arc::new(FilesV2::with_guests(config(&base), GuestAuth::with_keys(vec![(KID.into(), pk)])));
        let app = routes::<()>(files.clone()).layer(Extension(owner()));
        let policy = json!({"read": true, "write": true, "shareable": shareable, "agent_process": false, "protected": false,
            "offline": true, "replica": true, "versions": {"keep_count": 5, "keep_days": 0}, "trash_days": 30});
        let root: Root = serde_json::from_value(
            owner_post(&app, "/roots", json!({"name": "Docs", "local_path": base.join("home/Docs").to_string_lossy(), "policy": policy}))
                .await["root"]
                .clone(),
        )
        .unwrap();
        let mk = |name: &str| json!({"parent_id": root.root_item_id, "name": name, "on_conflict": "fail"});
        let shared = owner_post(&app, "/mkdir", mk("Shared")).await["item"]["id"].as_str().unwrap().to_string();
        let private = owner_post(&app, "/mkdir", mk("Private")).await["item"]["id"].as_str().unwrap().to_string();
        let shared_file = owner_put(&app, &root.id, &shared, "a.txt", b"hello guest").await;
        let private_file = owner_put(&app, &root.id, &private, "secret.txt", b"owner only").await;
        T { _tmp: tmp, app, files, root, shared, private, shared_file, private_file }
    }

    fn claims(t: &T, role: ShareRole) -> GuestCapabilityClaims {
        let now = now_secs();
        GuestCapabilityClaims {
            v: 1,
            typ: "files_guest".into(),
            share_id: "sh_test".into(),
            guest_id: "gst_test".into(),
            owner_user_id: "u1".into(),
            computer_id: COMPUTER.into(),
            root_id: t.root.id.clone(),
            item_id: t.shared.clone(),
            role,
            guest_wake: false,
            passcode_ok: true,
            iat: now,
            exp: now + 600,
            kid: KID.into(),
        }
    }

    fn mint(c: &GuestCapabilityClaims) -> String {
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(c).unwrap());
        let signed = format!("fgt1.{payload}");
        let sig = URL_SAFE_NO_PAD.encode(signer().sign(signed.as_bytes()).to_bytes());
        format!("{signed}.{sig}")
    }

    impl T {
        async fn guest_get(&self, token: &str, uri: &str) -> Resp {
            call(&self.app, "GET", uri, Some(token), Body::empty()).await
        }
        fn list_uri(&self, parent: &str) -> String {
            format!("/roots/{}/list?parent_id={parent}", self.root.id)
        }
    }

    #[tokio::test]
    async fn guest_reads_only_its_folder() {
        let t = setup(true).await;
        let token = mint(&claims(&t, ShareRole::ViewDownload));

        let r = t.guest_get(&token, &t.list_uri(&t.shared)).await;
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
        let list: ListResponse = serde_json::from_value(r.json()).unwrap();
        assert_eq!(list.items.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), vec!["a.txt"]);
        let p = &list.items[0].permissions;
        assert!(p.read && !p.write && !p.delete && !p.share && !p.agent_process, "view_download permissions: {p:?}");
        let r = t.guest_get(&token, &format!("/items/{}/content", t.shared_file)).await;
        assert_eq!((r.status, r.body.as_slice()), (StatusCode::OK, b"hello guest".as_slice()));

        // The sibling folder, its file and the root above the share are out of scope.
        assert_eq!(t.guest_get(&token, &t.list_uri(&t.private)).await.code(), ErrorCode::ScopeViolation);
        assert_eq!(t.guest_get(&token, &t.list_uri(&t.root.root_item_id)).await.code(), ErrorCode::ScopeViolation);
        assert_eq!(t.guest_get(&token, &format!("/items/{}", t.private_file)).await.code(), ErrorCode::ScopeViolation);
        assert_eq!(t.guest_get(&token, &format!("/items/{}/content", t.private_file)).await.code(), ErrorCode::ScopeViolation);
        // A view link cannot write, even inside its folder.
        let w = call(&t.app, "PUT", &format!("/roots/{}/content?parent_id={}&name=b.txt", t.root.id, t.shared), Some(&token), Body::from("x")).await;
        assert_eq!(w.code(), ErrorCode::Forbidden);
    }

    #[tokio::test]
    async fn guest_cannot_reach_owner_routes() {
        let t = setup(true).await;
        let token = mint(&claims(&t, ShareRole::Edit));
        assert_eq!(t.guest_get(&token, "/roots").await.code(), ErrorCode::Forbidden);
        assert_eq!(t.guest_get(&token, "/storage").await.code(), ErrorCode::Forbidden);
        assert_eq!(t.guest_get(&token, &format!("/roots/{}/changes", t.root.id)).await.code(), ErrorCode::Forbidden);
        let del = call(&t.app, "POST", "/delete", Some(&token), Body::from(json!({"item_id": t.shared_file}).to_string())).await;
        assert_eq!(del.code(), ErrorCode::Forbidden);
        // whoami says guest, with one grant scoped to the shared folder.
        let who: WhoAmIResponse = serde_json::from_value(t.guest_get(&token, "/whoami").await.json()).unwrap();
        assert_eq!(who.principal.kind, PrincipalKind::Guest);
        assert_eq!(who.grants.len(), 1);
        assert_eq!(who.grants[0].item_scope.as_deref(), Some(t.shared.as_str()));
    }

    #[tokio::test]
    async fn expired_token_fails() {
        let t = setup(true).await;
        let mut c = claims(&t, ShareRole::ViewDownload);
        c.iat = now_secs() - 700;
        c.exp = now_secs() - 100;
        assert_eq!(t.guest_get(&mint(&c), &t.list_uri(&t.shared)).await.code(), ErrorCode::Unauthorized);
        // A token that claims to live longer than an hour is refused too.
        let mut c = claims(&t, ShareRole::ViewDownload);
        c.exp = c.iat + MAX_TOKEN_SECS + 1;
        assert_eq!(t.guest_get(&mint(&c), &t.list_uri(&t.shared)).await.code(), ErrorCode::Unauthorized);
    }

    #[tokio::test]
    async fn tampered_token_fails() {
        let t = setup(true).await;
        let good = mint(&claims(&t, ShareRole::ViewDownload));
        // Swap in an edit-role payload but keep the view token's signature.
        let edit = mint(&claims(&t, ShareRole::Edit));
        let mut g = good.split('.');
        let mut e = edit.split('.');
        let forged = format!("fgt1.{}.{}", e.nth(1).unwrap(), g.nth(2).unwrap());
        assert_eq!(t.guest_get(&forged, &t.list_uri(&t.shared)).await.code(), ErrorCode::Unauthorized);
        // A token signed by another key under our kid.
        let c = claims(&t, ShareRole::ViewDownload);
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&c).unwrap());
        let other = SigningKey::from_bytes(&[3u8; 32]).sign(format!("fgt1.{payload}").as_bytes());
        let foreign = format!("fgt1.{payload}.{}", URL_SAFE_NO_PAD.encode(other.to_bytes()));
        assert_eq!(t.guest_get(&foreign, &t.list_uri(&t.shared)).await.code(), ErrorCode::Unauthorized);
        // A wake ticket prefix is not a guest token.
        let wrong_prefix = good.replacen("fgt1.", "awt1.", 1);
        assert_eq!(t.guest_get(&wrong_prefix, &t.list_uri(&t.shared)).await.code(), ErrorCode::Unauthorized);
    }

    #[tokio::test]
    async fn token_for_another_computer_fails() {
        let t = setup(true).await;
        let mut c = claims(&t, ShareRole::ViewDownload);
        c.computer_id = "rd_someone_else".into();
        assert_eq!(t.guest_get(&mint(&c), &t.list_uri(&t.shared)).await.code(), ErrorCode::ScopeViolation);
    }

    #[tokio::test]
    async fn upload_only_cannot_list_but_can_add_new_files() {
        let t = setup(true).await;
        let token = mint(&claims(&t, ShareRole::UploadOnly));
        let r = t.guest_get(&token, &t.list_uri(&t.shared)).await;
        assert_eq!(r.code(), ErrorCode::Forbidden);
        assert_eq!(r.json()["message"], "This link only allows uploads.");
        assert_eq!(t.guest_get(&token, &format!("/items/{}", t.shared_file)).await.code(), ErrorCode::Forbidden);
        assert_eq!(t.guest_get(&token, &format!("/items/{}/content", t.shared_file)).await.code(), ErrorCode::Forbidden);

        let (app, tok) = (&t.app, token.as_str());
        let put = |q: String| {
            let uri = format!("/roots/{}/content?parent_id={}&{q}", t.root.id, t.shared);
            async move { call(app, "PUT", &uri, Some(tok), Body::from("drop")).await }
        };
        let ok = put("name=W-9.pdf".into()).await;
        assert_eq!(ok.status, 200, "{}", String::from_utf8_lossy(&ok.body));
        assert_eq!(ok.json()["item"]["modified_by"]["kind"], "guest");
        // Replacing an existing file is not an upload-only action.
        let rev = ok.json()["item"]["rev"].as_str().unwrap().to_string();
        assert_eq!(put(format!("name=W-9.pdf&base_rev={rev}")).await.code(), ErrorCode::Forbidden);
        // Nor is dropping outside the shared folder.
        let out = call(&t.app, "PUT", &format!("/roots/{}/content?parent_id={}&name=x.txt", t.root.id, t.private), Some(&token), Body::from("x")).await;
        assert_eq!(out.code(), ErrorCode::ScopeViolation);
    }

    #[tokio::test]
    async fn unshareable_root_refuses_guests() {
        let t = setup(false).await;
        let token = mint(&claims(&t, ShareRole::ViewDownload));
        assert_eq!(t.guest_get(&token, &t.list_uri(&t.shared)).await.code(), ErrorCode::Forbidden);
    }

    #[tokio::test]
    async fn every_guest_access_emits_share_accessed() {
        let t = setup(true).await;
        let mut events = t.files.events.subscribe();
        let token = mint(&claims(&t, ShareRole::ViewDownload));
        assert_eq!(t.guest_get(&token, &t.list_uri(&t.shared)).await.status, 200);
        assert_eq!(t.guest_get(&token, &format!("/items/{}", t.private_file)).await.code(), ErrorCode::ScopeViolation);
        assert_eq!(t.guest_get(&token, "/roots").await.code(), ErrorCode::Forbidden);

        let mut got = Vec::new();
        while let Ok(FilesEvent::ShareAccessed(e)) = events.try_recv() {
            got.push(e);
        }
        assert_eq!(got.len(), 3, "one event per guest request: {got:?}");
        assert_eq!((got[0].access.result, got[0].access.error.clone()), (ShareAccessResult::Allowed, None));
        assert_eq!(got[0].access.item_id.as_deref(), Some(t.shared.as_str()));
        assert_eq!((got[1].access.result, got[1].access.error.clone()), (ShareAccessResult::Denied, Some(ErrorCode::ScopeViolation)));
        assert_eq!(got[1].access.item_id.as_deref(), Some(t.private_file.as_str()));
        assert_eq!((got[2].access.result, got[2].access.error.clone()), (ShareAccessResult::Denied, Some(ErrorCode::Forbidden)));
        for e in &got {
            assert_eq!((e.computer_id.as_str(), e.access.share_id.as_str(), e.access.guest_id.as_str()), (COMPUTER, "sh_test", "gst_test"));
            // Each event is a valid contract message.
            let v = serde_json::to_value(FilesEvent::ShareAccessed(e.clone())).unwrap();
            assert_eq!(v["type"], "files.share.accessed");
        }
        // The owner's own requests are not guest accesses.
        assert_eq!(call(&t.app, "GET", &t.list_uri(&t.shared), None, Body::empty()).await.status, 200);
        assert!(events.try_recv().is_err());
    }

    #[tokio::test]
    async fn guest_upload_session_belongs_to_its_guest() {
        let t = setup(true).await;
        let a = mint(&claims(&t, ShareRole::Edit));
        let mut other = claims(&t, ShareRole::Edit);
        other.guest_id = "gst_other".into();
        let b = mint(&other);
        let data = b"session bytes".to_vec();
        let hash = allternit_files_engine::chunker::hash_bytes(&data);
        let create = json!({
            "root_id": t.root.id, "parent_id": t.shared, "name": "big.bin", "size": data.len(),
            "content_hash": hash, "chunker": {"algo": "fastcdc-v2020", "min": 262144, "avg": 1048576, "max": 4194304},
            "chunks": [{"hash": hash, "offset": 0, "size": data.len()}], "total_chunks": 1, "on_conflict": "keep_both"
        });
        let r = call(&t.app, "POST", "/uploads", Some(&a), Body::from(create.to_string())).await;
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
        let session = r.json()["session_id"].as_str().unwrap().to_string();
        assert_eq!(t.guest_get(&a, &format!("/uploads/{session}")).await.status, 200);
        assert_eq!(t.guest_get(&b, &format!("/uploads/{session}")).await.code(), ErrorCode::UploadSessionNotFound);
    }
}
