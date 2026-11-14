//! `Idempotency-Key` replay for the selfhost create routes.
//!
//! Contract: the `IdempotencyKey` parameter in `openapi/selfhost-v1.yaml`.
//! Mirrors the platform API's `crate::idempotency` (same header, 1-255 ASCII
//! characters, 24 hours, `content-type` the only stored header), with three
//! differences the selfhost contract asks for: the stored record lives in the
//! selfhost store so every replica sees it, the request body is part of the
//! match (a different body is 422 `idempotency_key_reuse`), and only 2xx
//! responses are stored.
//!
//! - No header, an unknown route, or a request that does not authenticate:
//!   the request goes on untouched, so the route gives its normal answer
//!   (401, 403, ...). A replay never reaches an unauthenticated caller.
//! - The scope is caller + method + path + key. The match is a SHA-256 of
//!   method, path and body bytes.
//! - The handler runs in its own task. A client that disconnects does not
//!   cancel it, so the record is finished and the client's retry gets the
//!   replay instead of a second run. A non-2xx answer or a panic drops the
//!   record, so a retry runs again.

use axum::body::{to_bytes, Body};
use axum::extract::{MatchedPath, Request, State};
use axum::http::{HeaderValue, Method};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use sha2::{Digest, Sha256};
use tracing::Instrument;

use super::auth::{authenticate_user, role_allows};
use super::store::{DynStore, IdempotencyBegin, IdempotencyScope, StoredResponse};
use super::{ApiError, SelfHostState};

pub const IDEMPOTENCY_KEY_HEADER: &str = "idempotency-key";
pub const IDEMPOTENCY_REPLAYED_HEADER: &str = "idempotency-replayed";

/// `POST` routes that honor the header (axum paths).
pub const IDEMPOTENT_ROUTES: &[&str] = &[
    "/v1/agents",
    "/v1/environments",
    "/v1/sessions",
    "/v1/sessions/:id/messages",
];

/// The largest body buffered here: axum's default body limit, which is what
/// these routes' JSON extractors already accept.
pub const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

/// Longest key, per the contract.
pub const MAX_KEY_LEN: usize = 255;

/// Response headers stored and replayed. `content-length` is recomputed.
const STORED_HEADERS: &[&str] = &["content-type"];

/// Route layer (`Router::route_layer`), so `MatchedPath` is known.
pub async fn middleware(State(st): State<SelfHostState>, req: Request, next: Next) -> Response {
    let applies = req.method() == Method::POST
        && req
            .extensions()
            .get::<MatchedPath>()
            .is_some_and(|p| IDEMPOTENT_ROUTES.contains(&p.as_str()));
    if !applies {
        return next.run(req).await;
    }
    let Some(raw) = req.headers().get(IDEMPOTENCY_KEY_HEADER).cloned() else {
        return next.run(req).await;
    };
    // Same check the route's `User` extractor makes. On failure the route
    // itself answers, with its normal error.
    let caller = match authenticate_user(&st, req.headers()).await {
        Ok(c) if role_allows(c.role, req.method()) => c,
        _ => return next.run(req).await,
    };
    let key = match raw.to_str().map(str::trim) {
        Ok("") => return next.run(req).await,
        Ok(k) if k.len() <= MAX_KEY_LEN => k.to_string(),
        _ => {
            return ApiError::invalid(
                "invalid_idempotency_key",
                "Idempotency-Key must be 1 to 255 ASCII characters.",
            )
            .into_response()
        }
    };

    let (parts, body) = req.into_parts();
    let bytes = match to_bytes(body, MAX_BODY_BYTES).await {
        Ok(b) => b,
        // The same answer the route's JSON extractor gives.
        Err(_) => {
            return ApiError::invalid(
                "invalid_body",
                "Failed to buffer the request body: length limit exceeded",
            )
            .into_response()
        }
    };
    let scope = IdempotencyScope {
        principal: caller.subject,
        method: parts.method.to_string(),
        path: parts.uri.path().to_string(),
        key,
    };
    let hash = request_hash(&scope.method, &scope.path, &bytes);

    match st.store.idempotency_begin(&scope, &hash, Utc::now()).await {
        Ok(IdempotencyBegin::New) => {}
        Ok(IdempotencyBegin::Replay(stored)) => return replay(stored),
        Ok(IdempotencyBegin::InProgress) => {
            return ApiError::conflict(
                "idempotency_in_progress",
                "The first request with this Idempotency-Key is still running. Retry shortly.",
            )
            .into_response()
        }
        Ok(IdempotencyBegin::Mismatch) => {
            return ApiError::unprocessable(
                "idempotency_key_reuse",
                "This Idempotency-Key was used with a different request body.",
            )
            .into_response()
        }
        Err(e) => return ApiError::from(e).into_response(),
    }

    let req = Request::from_parts(parts, Body::from(bytes));
    let guard = Pending {
        store: st.store.clone(),
        scope,
        hash,
        open: true,
    };
    let task = tokio::spawn(run(req, next, guard).instrument(tracing::Span::current()));
    match task.await {
        Ok(res) => res,
        // The guard dropped the record while the task unwound.
        Err(e) => {
            tracing::error!(error = %e, "idempotent request handler panicked");
            ApiError::internal("The request failed.").into_response()
        }
    }
}

async fn run(req: Request, next: Next, mut guard: Pending) -> Response {
    let res = next.run(req).await;
    if !res.status().is_success() {
        guard.abandon().await;
        return res;
    }
    let (parts, body) = res.into_parts();
    let bytes = match to_bytes(body, usize::MAX).await {
        Ok(b) => b,
        Err(e) => {
            guard.abandon().await;
            tracing::error!(error = %e, "could not read the response to store it");
            return ApiError::internal("The response could not be read.").into_response();
        }
    };
    let stored = StoredResponse {
        status: parts.status.as_u16(),
        headers: STORED_HEADERS
            .iter()
            .filter_map(|name| {
                let v = parts.headers.get(*name)?.to_str().ok()?;
                Some((name.to_string(), v.to_string()))
            })
            .collect(),
        body: bytes.to_vec(),
    };
    guard.finish(&stored).await;
    Response::from_parts(parts, Body::from(bytes))
}

fn request_hash(method: &str, path: &str, body: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(method.as_bytes());
    h.update(b"\n");
    h.update(path.as_bytes());
    h.update(b"\n");
    h.update(body);
    hex::encode(h.finalize())
}

fn replay(stored: StoredResponse) -> Response {
    let mut res = Response::new(Body::from(stored.body));
    *res.status_mut() = axum::http::StatusCode::from_u16(stored.status)
        .unwrap_or(axum::http::StatusCode::OK);
    let headers = res.headers_mut();
    for (name, value) in &stored.headers {
        if let (Ok(n), Ok(v)) = (
            axum::http::HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            headers.insert(n, v);
        }
    }
    headers.insert(IDEMPOTENCY_REPLAYED_HEADER, HeaderValue::from_static("true"));
    res
}

/// The record this request owns. Dropped without `finish` or `abandon` (a
/// panic), it abandons the record in the background.
struct Pending {
    store: DynStore,
    scope: IdempotencyScope,
    hash: String,
    open: bool,
}

impl Pending {
    async fn finish(&mut self, stored: &StoredResponse) {
        self.open = false;
        if let Err(e) = self
            .store
            .idempotency_finish(&self.scope, &self.hash, stored, Utc::now())
            .await
        {
            // Left in progress, retries would see 409 for a minute and then
            // run again anyway. Drop it so they run again now.
            tracing::error!(error = %e, "could not store the idempotent response");
            let _ = self.store.idempotency_abandon(&self.scope, &self.hash).await;
        }
    }

    async fn abandon(&mut self) {
        self.open = false;
        if let Err(e) = self.store.idempotency_abandon(&self.scope, &self.hash).await {
            tracing::warn!(error = %e, "could not drop the idempotency record");
        }
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        if !self.open {
            return;
        }
        let store = self.store.clone();
        let scope = self.scope.clone();
        let hash = std::mem::take(&mut self.hash);
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move {
                let _ = store.idempotency_abandon(&scope, &hash).await;
            });
        }
    }
}
