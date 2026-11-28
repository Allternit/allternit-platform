//! `POST /api/v1/kernel/approvals/{request,resolve,redeem}` (CONTRACT.md 2.4, 4.3).
//!
//! Mounted on the protected router, so `auth_middleware` has already put an [`AuthUser`] on the
//! request. That only says *which account*; every credential kind (Clerk session, runtime-device
//! token, access token, API key, service token) produces one. Whether a *person* is acting is
//! decided here:
//!
//! 1. [`machine_marker`] refuses anything that carries a run, agent, API-key, MCP-client or
//!    internal-service credential, whatever else it carries.
//! 2. [`crate::subscription_routes::person_acted`] (the D16 human-proof check) must pass: the
//!    Desktop window's per-launch `x-allternit-human-proof` secret, or a Clerk session token for
//!    this same user. Agent runtimes, bots, MCP and the CLI hold only machine credentials.
//!
//! Only then is the transport caller `human`. The caller kind is set here, never read from the
//! body, and [`super::approvals::SqlAuthority::resolve`] refuses every other kind with a receipt.
//!
//! `GET /api/v1/kernel/approvals?state=pending` lists the caller's own open requests (person's session only).
//!
//! 3. CONTRACT.md 12.3: when the person proof was Desktop's window proof, an approve also needs
//!    Electron main's user-presence proof for that request ([`super::presence`]), or it is
//!    refused with `forbidden_caller` and a receipt. Deny needs none.

use std::sync::Arc;

use allternit_authority::{ApprovalRedeemBody, ApprovalRequestV1, AuthorityError, Caller, CallerKind, ErrorCode};
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use super::approvals::{Decision, ResolveBody, SqlAuthority};
use super::audit::{self, Scope};
use super::{presence, tokens};
use crate::auth::{AuthUser, INTERNAL_SERVICE_USER_ID};
use crate::enterprise_auth::CredentialContext;
use crate::subscription_routes::PersonProof;
use crate::AppState;

/// Paths live under `/api/v1/kernel/approvals/`, next to the kernel UI routes
/// (`kernel_ui::api_router`), which use none of them.
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/kernel/approvals", get(list_approvals))
        .route("/api/v1/kernel/approvals/request", post(request))
        .route("/api/v1/kernel/approvals/resolve", post(resolve))
        .route("/api/v1/kernel/approvals/redeem", post(redeem))
        .route("/api/v1/kernel/receipts", get(list_receipts))
        .route("/api/v1/kernel/receipts/verify", get(verify_receipts))
}

/// Headers only machine callers send (the server sets them when it hands work to gizzi or an
/// agent, or the MCP edge relay names its OAuth client).
const RUN_HEADER: &str = "x-allternit-run-id";
const AGENT_HEADER: &str = "x-allternit-agent-id";
const INTERNAL_TOKEN_HEADER: &str = "x-allternit-internal-token";

fn bearer(headers: &HeaderMap) -> Option<&str> {
    let v = headers.get(header::AUTHORIZATION)?.to_str().ok()?.trim();
    v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer ")).map(str::trim)
}

/// `Some((kind, reason))` when the request carries a credential that is never a person's
/// session: run or agent markers, an API key (`allternit_admin_` / `allternit_access_`, seen as
/// a [`CredentialContext`]), an access token (`at-`), a fabric worker token (`atok_`), the
/// internal service token, or an MCP client. Checked before the human proof, so a machine
/// credential cannot borrow a person's proof header.
pub(crate) fn machine_marker(headers: &HeaderMap, api_credential: bool, user_id: &str) -> Option<(CallerKind, &'static str)> {
    if user_id == INTERNAL_SERVICE_USER_ID || headers.contains_key(INTERNAL_TOKEN_HEADER) {
        return Some((CallerKind::Service, "internal service credential"));
    }
    if api_credential {
        return Some((CallerKind::Service, "API key credential"));
    }
    if headers.contains_key(crate::mcp_edge_relay::CLIENT_HEADER) {
        return Some((CallerKind::Service, "MCP client credential"));
    }
    if headers.contains_key(RUN_HEADER) {
        return Some((CallerKind::Run, "run credential"));
    }
    if headers.contains_key(AGENT_HEADER) {
        return Some((CallerKind::Run, "agent credential"));
    }
    match bearer(headers) {
        Some(t) if t.starts_with("at-") => Some((CallerKind::Service, "access token")),
        Some(t) if t.starts_with("atok_") => Some((CallerKind::Run, "worker token")),
        _ => None,
    }
}

/// The transport caller for this request. `principal` is always the authenticated account.
async fn caller_for(state: &AppState, headers: &HeaderMap, user: &AuthUser, api_credential: bool) -> Caller {
    caller_and_proof(state, headers, user, api_credential).await.0
}

/// [`caller_for`], plus which person proof passed (`None` for a machine caller).
async fn caller_and_proof(
    state: &AppState,
    headers: &HeaderMap,
    user: &AuthUser,
    api_credential: bool,
) -> (Caller, Option<PersonProof>) {
    let principal = user.user_id.clone();
    if let Some((kind, reason)) = machine_marker(headers, api_credential, &user.user_id) {
        tracing::debug!(reason, "kernel approvals: machine caller");
        return (Caller { kind, principal }, None);
    }
    match crate::subscription_routes::person_proof(state, headers, &user.user_id).await {
        Ok(proof) => (Caller { kind: CallerKind::Human, principal }, Some(proof)),
        Err(reason) => {
            tracing::debug!(reason, "kernel approvals: no person's session");
            (Caller { kind: CallerKind::Run, principal }, None)
        }
    }
}

/// CONTRACT.md 12.3: an approve made through the Desktop window needs Electron main's
/// user-presence proof for this exact request. Deny, Clerk-session (web, phone) and local-dev
/// approvals are not gated here.
pub(crate) fn presence_missing(proof: Option<PersonProof>, body: &ResolveBody, headers: &HeaderMap, now: i64) -> bool {
    if proof != Some(PersonProof::Desktop) || body.decision != Decision::Approve {
        return false;
    }
    let Some(secret) = crate::subscription_routes::desktop_proof_secret() else {
        return true;
    };
    let given = headers.get(presence::PRESENCE_PROOF_HEADER).and_then(|v| v.to_str().ok()).unwrap_or("");
    !presence::verify(secret, given, &body.request_id, now)
}

fn status_for(code: ErrorCode) -> StatusCode {
    match code {
        ErrorCode::BadRequest | ErrorCode::UnsupportedVersion | ErrorCode::FrameTooLarge => StatusCode::BAD_REQUEST,
        ErrorCode::Unauthenticated => StatusCode::UNAUTHORIZED,
        ErrorCode::NotFound => StatusCode::NOT_FOUND,
        ErrorCode::Conflict => StatusCode::CONFLICT,
        ErrorCode::KernelUnavailable | ErrorCode::ChainBroken => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::FORBIDDEN,
    }
}

fn error_response(e: AuthorityError) -> Response {
    (status_for(e.code), Json(json!({ "ok": false, "error": e }))).into_response()
}

fn parse<T: DeserializeOwned>(body: Value) -> Result<T, Response> {
    serde_json::from_value(body)
        .map_err(|e| error_response(AuthorityError::without_receipt(ErrorCode::BadRequest, format!("invalid body: {e}"))))
}

/// Load the signer, then run `f` on a blocking thread with a fresh [`SqlAuthority`].
async fn with_kernel<T, F>(state: &AppState, tenant: Option<String>, f: F) -> Response
where
    T: serde::Serialize + Send + 'static,
    F: FnOnce(SqlAuthority) -> Result<T, AuthorityError> + Send + 'static,
{
    let data_dir = state.db.path().parent().map(|p| p.to_path_buf()).unwrap_or_else(|| std::path::PathBuf::from("."));
    let signer = match tokens::kernel_signer(&data_dir) {
        Ok(s) => s,
        Err(e) => return error_response(e),
    };
    let db = state.db.clone();
    match tokio::task::spawn_blocking(move || f(SqlAuthority::new(db, signer).with_tenant(tenant))).await {
        Ok(Ok(body)) => (StatusCode::OK, Json(json!({ "ok": true, "body": body }))).into_response(),
        Ok(Err(e)) => error_response(e),
        Err(e) => error_response(AuthorityError::kernel_unavailable(format!("kernel task: {e}"))),
    }
}

/// `approval.request`. Callers: runs and services (a person may raise one too).
async fn request(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    credential: Option<Extension<CredentialContext>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let body: ApprovalRequestV1 = match parse(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let caller = caller_for(&state, &headers, &user, credential.is_some()).await;
    with_kernel(&state, user.tenant_id.clone(), move |k| k.request(&caller, body)).await
}

/// `approval.resolve`: HUMAN ONLY. A machine caller reaches the kernel as `run`/`service` and is
/// refused there with `forbidden_caller` and a receipt.
async fn resolve(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    credential: Option<Extension<CredentialContext>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let body: ResolveBody = match parse(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let (caller, proof) = caller_and_proof(&state, &headers, &user, credential.is_some()).await;
    if caller.kind == CallerKind::Human && presence_missing(proof, &body, &headers, chrono::Utc::now().timestamp()) {
        tracing::info!("kernel approvals: Desktop approve without a user-presence proof, refused");
        return with_kernel(&state, user.tenant_id.clone(), move |k| Err::<Value, _>(k.refuse_without_presence(&caller, &body))).await;
    }
    with_kernel(&state, user.tenant_id.clone(), move |k| k.resolve(&caller, body)).await
}

/// `approval.redeem`: spend a token before the action runs.
async fn redeem(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    credential: Option<Extension<CredentialContext>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let body: ApprovalRedeemBody = match parse(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let caller = caller_for(&state, &headers, &user, credential.is_some()).await;
    with_kernel(&state, user.tenant_id.clone(), move |k| k.redeem(&caller, body)).await
}

/// Close the caller's requests that are past their deadline (an `expired` receipt each), so the
/// inbox feed and the pending list never show them and the log has the receipt. Best effort:
/// when the kernel key is unavailable nothing is written and expired rows are still filtered
/// out of every read by deadline.
pub async fn sweep_expired(state: &AppState, user: &AuthUser) {
    let data_dir = state.db.path().parent().map(|p| p.to_path_buf()).unwrap_or_else(|| std::path::PathBuf::from("."));
    let Ok(signer) = tokens::kernel_signer(&data_dir) else { return };
    let (db, owner, tenant) = (state.db.clone(), user.user_id.clone(), user.tenant_id.clone());
    let done = tokio::task::spawn_blocking(move || SqlAuthority::new(db, signer).with_tenant(tenant).expire_due(&owner)).await;
    if let Ok(Err(e)) = done {
        tracing::warn!("kernel approvals: expiry sweep failed: {}", e.message);
    }
}

#[derive(serde::Deserialize)]
struct PendingQuery {
    #[serde(default)]
    state: Option<String>,
}

/// `GET /api/v1/kernel/approvals?state=pending`: the caller's own open, unexpired requests,
/// newest first, in the inbox `kernel` shape. Person's session only. `pending` is the only state.
async fn list_approvals(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    credential: Option<Extension<CredentialContext>>,
    headers: HeaderMap,
    Query(q): Query<PendingQuery>,
) -> Response {
    if q.state.as_deref().unwrap_or("pending") != "pending" {
        return error_response(AuthorityError::without_receipt(ErrorCode::BadRequest, "state must be pending"));
    }
    if caller_for(&state, &headers, &user, credential.is_some()).await.kind != CallerKind::Human {
        return error_response(AuthorityError::without_receipt(ErrorCode::ForbiddenCaller, "only a person's session can list approvals"));
    }
    sweep_expired(&state, &user).await;
    let (db, owner) = (state.db.clone(), user.user_id.clone());
    let out = tokio::task::spawn_blocking(move || {
        let conn = db.connect().map_err(|e| AuthorityError::kernel_unavailable(format!("kernel store: {e}")))?;
        super::approvals::pending_requests(&conn, &owner, chrono::Utc::now()).map_err(|e| AuthorityError::kernel_unavailable(format!("kernel store: {e}")))
    })
    .await;
    match out {
        Ok(Ok(rows)) => (StatusCode::OK, Json(json!({ "ok": true, "body": { "approvals": rows } }))).into_response(),
        Ok(Err(e)) => error_response(e),
        Err(e) => error_response(AuthorityError::kernel_unavailable(format!("kernel task: {e}"))),
    }
}

/// The caller's own chain. Reads need a person's session, the same check as resolve: a run,
/// agent, bot or API key never reads or verifies the receipt log. Read-only, writes no receipt.
async fn own_scope(
    state: &AppState,
    headers: &HeaderMap,
    user: &AuthUser,
    api_credential: bool,
) -> Result<Scope, Response> {
    let caller = caller_for(state, headers, user, api_credential).await;
    if caller.kind != CallerKind::Human {
        return Err(error_response(AuthorityError::without_receipt(
            ErrorCode::ForbiddenCaller,
            "only a person's session can read the receipt log",
        )));
    }
    let tenant = user.tenant_id.clone().filter(|t| !t.is_empty()).unwrap_or_else(|| audit::DEFAULT_TENANT.to_string());
    Ok(Scope { principal: caller.principal, tenant })
}

#[derive(serde::Deserialize)]
struct ListQuery {
    #[serde(default)]
    limit: Option<u32>,
    /// Return rows older than this `idx` (paging, newest first).
    #[serde(default)]
    before: Option<i64>,
}

/// `GET /api/v1/kernel/receipts`: the caller's own receipts, newest first.
async fn list_receipts(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    credential: Option<Extension<CredentialContext>>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> Response {
    let scope = match own_scope(&state, &headers, &user, credential.is_some()).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    let db = state.db.clone();
    let out = tokio::task::spawn_blocking(move || {
        let conn = db.connect().map_err(|e| AuthorityError::kernel_unavailable(format!("receipt log: {e}")))?;
        audit::list(&conn, &scope, q.limit.unwrap_or(100), q.before)
    })
    .await;
    match out {
        Ok(Ok(rows)) => (StatusCode::OK, Json(json!({ "ok": true, "body": { "receipts": rows } }))).into_response(),
        Ok(Err(e)) => error_response(e),
        Err(e) => error_response(AuthorityError::kernel_unavailable(format!("kernel task: {e}"))),
    }
}

/// `GET /api/v1/kernel/receipts/verify`: recompute every hash, link and signature of the
/// caller's chain. Returns the first bad row, and how many rows were signed by a previous key
/// (reported as such, not as tampering).
async fn verify_receipts(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    credential: Option<Extension<CredentialContext>>,
    headers: HeaderMap,
) -> Response {
    let scope = match own_scope(&state, &headers, &user, credential.is_some()).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    let data_dir = state.db.path().parent().map(|p| p.to_path_buf()).unwrap_or_else(|| std::path::PathBuf::from("."));
    // No seed (Desktop keystore locked) is fine for verifying: old rows carry their own public key.
    let current_kid = tokens::kernel_signer(&data_dir).ok().map(|s| s.kid().to_string());
    let db = state.db.clone();
    let out = tokio::task::spawn_blocking(move || {
        let conn = db.connect().map_err(|e| AuthorityError::kernel_unavailable(format!("receipt log: {e}")))?;
        audit::verify_chain(&conn, &scope, current_kid.as_deref())
    })
    .await;
    match out {
        Ok(Ok(report)) => (StatusCode::OK, Json(json!({ "ok": true, "body": report }))).into_response(),
        Ok(Err(e)) => error_response(e),
        Err(e) => error_response(AuthorityError::kernel_unavailable(format!("kernel task: {e}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, HeaderValue::from_static(v));
        }
        h
    }

    #[test]
    fn desktop_approve_without_presence_proof_is_refused_deny_and_sessions_are_not_gated() {
        let approve = ResolveBody { request_id: "apr_1".into(), decision: Decision::Approve, note: None };
        let deny = ResolveBody { request_id: "apr_1".into(), decision: Decision::Deny, note: None };
        let none = HeaderMap::new();
        let forged = headers(&[(presence::PRESENCE_PROOF_HEADER, "v1.9999999999.00")]);
        let now = 1_800_000_000;
        assert!(presence_missing(Some(PersonProof::Desktop), &approve, &none, now));
        assert!(presence_missing(Some(PersonProof::Desktop), &approve, &forged, now));
        assert!(!presence_missing(Some(PersonProof::Desktop), &deny, &none, now));
        assert!(!presence_missing(Some(PersonProof::Session), &approve, &none, now));
        assert!(!presence_missing(Some(PersonProof::DevBypass), &approve, &none, now));
    }

    #[test]
    fn agent_run_and_api_key_credentials_are_never_a_person() {
        let cases: [(HeaderMap, bool, &str); 7] = [
            (headers(&[(AGENT_HEADER, "agent_1")]), false, "user_1"),
            (headers(&[(RUN_HEADER, "run_1")]), false, "user_1"),
            (headers(&[]), true, "user_1"),
            (headers(&[("authorization", "Bearer at-abc")]), false, "user_1"),
            (headers(&[("authorization", "Bearer atok_abc")]), false, "user_1"),
            (headers(&[(INTERNAL_TOKEN_HEADER, "x")]), false, "user_1"),
            (headers(&[]), false, INTERNAL_SERVICE_USER_ID),
        ];
        for (h, api, uid) in cases {
            let marked = machine_marker(&h, api, uid);
            assert!(marked.is_some_and(|(k, _)| k != CallerKind::Human), "{h:?} api={api} uid={uid}");
        }
        // A person's proof header does not rescue an agent credential.
        let mixed = headers(&[(AGENT_HEADER, "agent_1"), ("x-allternit-human-proof", "desktop:secret")]);
        assert!(machine_marker(&mixed, false, "user_1").is_some());
        // A plain Clerk bearer is not marked here; `person_acted` then decides.
        assert!(machine_marker(&headers(&[("authorization", "Bearer eyJhbGciOi")]), false, "user_1").is_none());
    }
}
