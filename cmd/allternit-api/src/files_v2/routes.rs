//! The runtime routes of SPEC section 2, under `/files/v2` (nested at `/api/v1`).

use std::sync::Arc;

use allternit_files_core::{
    AddRootRequest, ChunkPutResponse, Comment, CommentCreateRequest, ComputerState, CopyRequest, CopyResponse,
    DeleteRequest, DeleteResponse, ErrorCode, HaveNeedRequest, HaveNeedResponse, JobRef, ListQuery, ListQuerySort,
    ListResponse, Manifest, MkdirRequest, MoveRequest, MutationResponse, OnConflict, PinRequest, PinResponse,
    PingResponse, PrincipalKind, RestoreRequest, RestoreVersionRequest, RootGrant, RootResponse, RootsResponse,
    SearchHit, SearchMode, SearchRequest, SearchResponse, SearchResponseIndexState, SmallWriteQuery, StatResponse,
    TrashListResponse, UpdateRootRequest, UploadChunksAppend, UploadCommitResponse, UploadCreateRequest,
    UploadStatus, VersionsResponse, WhoAmIResponse, PROTOCOL,
};
use allternit_files_engine::upload::utc_offset;
use allternit_files_engine::{chunker, naming, EditingDevice, Mutation, RootRecord};
use axum::body::{Body, Bytes};
use axum::extract::rejection::{BytesRejection, JsonRejection, QueryRejection};
use axum::extract::{DefaultBodyLimit, Path, Query};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post, put};
use axum::{Extension, Json, Router};
use serde::de::DeserializeOwned;
use serde::Deserialize;

use super::{
    feed, intel, live, memory, meta, permissions, roots, shares, ApiError, Fail, FilesV2, Inner, Need, Req, BIG_BODY_LIMIT,
    BODY_LIMIT,
};

type Files = Extension<Arc<FilesV2>>;
type Res<T> = Result<Json<T>, ApiError>;

pub fn routes<S: Clone + Send + Sync + 'static>(files: Arc<FilesV2>) -> Router<S> {
    let big = || DefaultBodyLimit::max(BIG_BODY_LIMIT);
    let v2 = Router::new()
        .route("/ping", get(ping))
        .route("/whoami", get(whoami))
        .route("/roots", get(list_roots).post(add_root))
        .route("/roots/:root_id", patch(update_root).delete(remove_root))
        .route("/roots/:root_id/list", get(list))
        .route("/roots/:root_id/content", put(small_write).layer(big()))
        .route("/roots/:root_id/trash", get(trash))
        .route("/roots/:root_id/changes", get(feed::changes))
        .route("/items/:item_id", get(stat))
        .route("/items/:item_id/content", get(content))
        .route("/items/:item_id/range", get(super::range::range))
        .route("/items/:item_id/manifest", get(manifest))
        .route("/items/:item_id/versions", get(versions))
        .route("/items/:item_id/versions/restore", post(restore_version))
        .route("/items/:item_id/comments", get(comments))
        .route("/comments", post(add_comment))
        .route("/chunks/have-need", post(have_need))
        .route("/chunks/:hash", get(chunk))
        .route("/uploads", post(upload_create))
        .route("/uploads/:session_id", get(upload_status).delete(upload_abort))
        .route("/uploads/:session_id/chunks", post(upload_append))
        .route("/uploads/:session_id/chunks/:hash", put(upload_chunk).layer(big()))
        .route("/uploads/:session_id/commit", post(upload_commit))
        .route("/mkdir", post(mkdir))
        .route("/move", post(move_item))
        .route("/copy", post(copy))
        .route("/delete", post(delete))
        .route("/restore", post(restore))
        .route("/search", post(search))
        .route("/pin", post(pin))
        .route("/items/:item_id/preview", get(intel::routes::preview))
        .route("/intel/status", get(intel::routes::status))
        .route("/intel/roots/:root_id", put(intel::routes::set_root))
        .route("/intel/tags", get(intel::routes::tag_counts).post(intel::routes::change_tags))
        .route("/intel/tags/:tag/items", get(intel::routes::tagged))
        .route("/intel/provenance", post(intel::routes::set_provenance))
        .route("/intel/dedup", get(intel::routes::dedup_report))
        .route("/intel/collections", get(intel::routes::collections).post(intel::routes::create_collection))
        .route(
            "/intel/collections/:id",
            get(intel::routes::collection).post(intel::routes::change_collection).delete(intel::routes::delete_collection),
        )
        .route("/intel/actions", post(intel::routes::action))
        .route("/intel/rules", get(intel::routes::rules).post(intel::routes::create_rule))
        .route("/intel/rules/:id", axum::routing::delete(intel::routes::delete_rule))
        .route("/subscribe", get(feed::subscribe))
        .route("/jobs/:job_id", get(job))
        .route("/models/handoff/:item_id", get(super::models::handoff))
        .route("/models/grant", post(super::models::grant))
        .route("/models/transfer", post(super::models::transfer))
        .route("/route", post(super::models::route_file))
        .route("/storage", get(super::storage::storage))
        .merge(super::replicas::routes())
        .merge(super::inbox::routes())
        // Guests (`Files-Guest`) pass the token, computer, route and role
        // checks here before any handler runs. Inside the Extension layer,
        // which it reads. Every route merged above is covered.
        .layer(axum::middleware::from_fn(shares::gate))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .layer(Extension(files));
    Router::new().nest("/files/v2", v2)
}

/// Runs a handler body on the blocking pool (the engine is synchronous) and
/// maps its error with the open drive.
pub(crate) async fn run<T: Send + 'static>(
    files: &FilesV2,
    f: impl FnOnce(&Inner) -> Result<T, Fail> + Send + 'static,
) -> Result<T, ApiError> {
    let inner = files.inner().await?;
    tokio::task::spawn_blocking(move || f(&inner).map_err(|e| inner.fail(e)))
        .await
        .map_err(|_| ApiError::internal())?
}

fn json<T: DeserializeOwned>(body: Result<Json<T>, JsonRejection>) -> Result<T, ApiError> {
    body.map(|Json(v)| v).map_err(Into::into)
}

fn query<T>(q: Result<Query<T>, QueryRejection>) -> Result<T, ApiError> {
    q.map(|Query(v)| v).map_err(Into::into)
}

fn bytes(b: Result<Bytes, BytesRejection>) -> Result<Bytes, ApiError> {
    b.map_err(Into::into)
}

pub(crate) fn mutation(v: &Inner, m: &Mutation) -> Result<MutationResponse, Fail> {
    let root = v.drive.root(&m.item.root_id)?;
    let conflict = m.conflict.as_ref().map(|c| c.to_info(permissions(&root, &c.conflict_item)));
    if let Some(info) = &conflict {
        super::events::conflict(v, info);
    }
    Ok(MutationResponse { item: v.item(&root, &m.item), conflict, cursor: m.cursor.clone() })
}

/// [`mutation`] for this caller (a guest gets its role's permissions).
pub(crate) fn mutation_as(v: &Inner, req: &Req, m: &Mutation) -> Result<MutationResponse, Fail> {
    let Some(g) = &req.guest else { return mutation(v, m) };
    let root = v.drive.root(&m.item.root_id)?;
    // The event goes to the owner, so it carries the owner's permissions; the
    // response carries the guest's. A guest's conflict copy is still a
    // conflict on the owner's drive.
    if let Some(c) = &m.conflict {
        super::events::conflict(v, &c.to_info(permissions(&root, &c.conflict_item)));
    }
    let conflict = m.conflict.as_ref().map(|c| c.to_info(shares::permissions(g.claims.role, &root, &c.conflict_item)));
    Ok(MutationResponse { item: v.item_as(req, &root, &m.item), conflict, cursor: m.cursor.clone() })
}

fn now(v: &Inner) -> String {
    v.drive.now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

// ---- identity and roots ----

async fn ping(Extension(files): Files) -> Res<PingResponse> {
    let inner = files.inner().await?;
    Ok(Json(PingResponse {
        computer_id: inner.cfg.computer_id.clone(),
        protocol: PROTOCOL.to_string(),
        runtime_version: env!("CARGO_PKG_VERSION").to_string(),
        computer_state: ComputerState::Awake,
    }))
}

async fn whoami(Extension(files): Files, req: Req) -> Res<WhoAmIResponse> {
    run(&files, move |v| {
        if let Some(g) = req.guest.clone() {
            return Ok(Json(shares::whoami(v, &req, &g)?));
        }
        let grants = v
            .drive
            .roots()?
            .into_iter()
            .map(|r| {
                let root_item = v.drive.stat(&r.root_item_id)?;
                Ok(RootGrant { root_id: r.root_id.clone(), permissions: permissions(&r, &root_item), item_scope: None })
            })
            .collect::<Result<Vec<_>, Fail>>()?;
        Ok(Json(WhoAmIResponse {
            principal: req.principal.clone(),
            computer_id: v.cfg.computer_id.clone(),
            grants,
            freshness: live(),
        }))
    })
    .await
}

async fn list_roots(Extension(files): Files, req: Req) -> Res<RootsResponse> {
    run(&files, move |v| {
        roots::refresh_providers(v)?;
        if let Err(e) = memory::sync_view(v, &req.user_id) {
            tracing::warn!(error = ?v.fail(e).body.message, "files v2: memory drive view not refreshed");
        }
        let all = v.drive.roots()?;
        let out = all.iter().map(|r| roots::to_wire(v, r, &all, &req)).collect::<Result<Vec<_>, _>>()?;
        Ok(Json(RootsResponse { computer_id: v.cfg.computer_id.clone(), roots: out, freshness: live() }))
    })
    .await
}

async fn add_root(Extension(files): Files, req: Req, body: Result<Json<AddRootRequest>, JsonRejection>) -> Res<RootResponse> {
    let body = json(body)?;
    run(&files, move |v| {
        let rec = roots::add_user_root(v, &req, &body.name, &body.local_path, body.policy)?;
        let all = v.drive.roots()?;
        Ok(Json(RootResponse { root: roots::to_wire(v, &rec, &all, &req)? }))
    })
    .await
}

async fn update_root(
    Extension(files): Files,
    req: Req,
    Path(root_id): Path<String>,
    body: Result<Json<UpdateRootRequest>, JsonRejection>,
) -> Res<RootResponse> {
    let body = json(body)?;
    run(&files, move |v| {
        let mut rec = v.drive.root(&root_id)?;
        if let Some(name) = &body.name {
            let name = naming::validate_name(name)?;
            meta::set_root_name(&v.meta.lock().expect("meta lock"), &root_id, &name)?;
        }
        if let Some(policy) = body.policy {
            roots::check_policy_change(&rec, &req, &policy)?;
            rec = v.drive.update_root_policy(&root_id, policy)?;
        }
        let all = v.drive.roots()?;
        Ok(Json(RootResponse { root: roots::to_wire(v, &rec, &all, &req)? }))
    })
    .await
}

async fn remove_root(Extension(files): Files, _req: Req, Path(root_id): Path<String>) -> Result<StatusCode, ApiError> {
    run(&files, move |v| {
        let rec = v.drive.root(&root_id)?;
        if rec.kind != allternit_files_core::RootKind::UserAdded {
            return Err(ApiError::forbidden("Only folders you added can be removed. Built-in folders stay.").into());
        }
        if let Some(w) = &v.watcher {
            let _ = w.unwatch_root(&rec.local_path);
        }
        v.drive.remove_root(&root_id)?;
        meta::forget_root(&v.meta.lock().expect("meta lock"), &root_id)?;
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

// ---- reading ----

async fn list(
    Extension(files): Files,
    req: Req,
    Path(root_id): Path<String>,
    q: Result<Query<ListQuery>, QueryRejection>,
) -> Res<ListResponse> {
    let q = query(q)?;
    run(&files, move |v| {
        let root = v.guard(&req, &root_id, Need::Read)?;
        if memory::is_memory(&root) {
            memory::sync_view(v, &req.user_id)?;
        }
        let parent = v.drive.stat(&q.parent_id)?;
        if let Some(g) = &req.guest {
            shares::check_subtree(v, g, &parent)?;
        }
        if parent.root_id != root_id {
            return Err(ApiError::new(ErrorCode::NotFound, "That folder is not in this root.").into());
        }
        let mut items = v.drive.list(&parent.id)?;
        match q.sort.unwrap_or(ListQuerySort::Name) {
            ListQuerySort::Name => items.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.name.cmp(&b.name))),
            ListQuerySort::Modified => items.sort_by(|a, b| b.modified_at.cmp(&a.modified_at)),
            ListQuerySort::Size => items.sort_by(|a, b| b.size.cmp(&a.size)),
        }
        let limit = q.limit.unwrap_or(500).clamp(1, 1000) as usize;
        let start = page_start(q.page_token.as_deref(), "l1.")?;
        let next = (start + limit < items.len()).then(|| format!("l1.{}", start + limit));
        let page = items.iter().skip(start).take(limit).map(|i| v.item_as(&req, &root, i)).collect();
        Ok(Json(ListResponse {
            root_id: root_id.clone(),
            parent: v.item_as(&req, &root, &parent),
            items: page,
            next_page_token: next,
            cursor: v.drive.current_cursor(&root_id)?,
            freshness: live(),
        }))
    })
    .await
}

fn page_start(token: Option<&str>, prefix: &str) -> Result<usize, ApiError> {
    match token {
        None => Ok(0),
        Some(t) => t.strip_prefix(prefix).and_then(|n| n.parse().ok()).ok_or_else(|| ApiError::invalid("bad page_token")),
    }
}

async fn stat(Extension(files): Files, req: Req, Path(item_id): Path<String>) -> Res<StatResponse> {
    run(&files, move |v| {
        let (root, item) = v.guard_item(&req, &item_id, Need::Read)?;
        Ok(Json(StatResponse { item: v.item_as(&req, &root, &item), freshness: live() }))
    })
    .await
}

/// `bytes=a-b`, `bytes=a-`, `bytes=-n` against `size`. `Ok(None)`: no range or
/// one we serve whole (several ranges). `Err(())`: unsatisfiable.
fn parse_range(h: &str, size: u64) -> Result<Option<(u64, u64)>, ()> {
    let Some(spec) = h.trim().strip_prefix("bytes=") else { return Ok(None) };
    if spec.contains(',') {
        return Ok(None);
    }
    let (a, b) = spec.split_once('-').ok_or(())?;
    let (a, b) = (a.trim(), b.trim());
    let (start, end) = if a.is_empty() {
        let n: u64 = b.parse().map_err(|_| ())?;
        if n == 0 {
            return Err(());
        }
        (size.saturating_sub(n), size.saturating_sub(1))
    } else {
        let s: u64 = a.parse().map_err(|_| ())?;
        let e = if b.is_empty() { size.saturating_sub(1) } else { b.parse::<u64>().map_err(|_| ())?.min(size.saturating_sub(1)) };
        (s, e)
    };
    if size == 0 || start >= size || end < start {
        return Err(());
    }
    Ok(Some((start, end)))
}

fn etag_matches(h: &str, rev: &str) -> bool {
    h.split(',').map(|t| t.trim().trim_start_matches("W/").trim_matches('"')).any(|t| t == "*" || t == rev)
}

async fn content(Extension(files): Files, req: Req, Path(item_id): Path<String>, headers: HeaderMap) -> Result<Response, ApiError> {
    let (rec, file) = run(&files, move |v| {
        v.guard_item(&req, &item_id, Need::Read)?;
        Ok(v.drive.open_content(&item_id)?)
    })
    .await?;
    let rev = rec.rev();
    let etag = format!("\"{rev}\"");
    if headers.get(header::IF_NONE_MATCH).and_then(|h| h.to_str().ok()).is_some_and(|h| etag_matches(h, &rev)) {
        return Ok((StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response());
    }
    let size = file.metadata().map(|m| m.len()).unwrap_or(rec.size);
    let range = match headers.get(header::RANGE).and_then(|h| h.to_str().ok()) {
        None => None,
        Some(h) => match parse_range(h, size) {
            Ok(r) => r,
            Err(()) => {
                let mut e = ApiError::invalid("That byte range is outside the file.");
                e.status = StatusCode::RANGE_NOT_SATISFIABLE;
                let mut resp = e.into_response();
                resp.headers_mut().insert(header::CONTENT_RANGE, format!("bytes */{size}").parse().expect("header"));
                return Ok(resp);
            }
        },
    };
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let mut f = tokio::fs::File::from_std(file);
    let (start, len) = match range {
        Some((s, e)) => (s, e - s + 1),
        None => (0, size),
    };
    if start > 0 {
        f.seek(std::io::SeekFrom::Start(start)).await.map_err(|_| ApiError::internal())?;
    }
    let body = Body::from_stream(tokio_util::io::ReaderStream::new(f.take(len)));
    let mut resp = Response::builder()
        .status(if range.is_some() { StatusCode::PARTIAL_CONTENT } else { StatusCode::OK })
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, len)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::ETAG, etag)
        .header(header::CACHE_CONTROL, "private, no-cache");
    if let Some((s, e)) = range {
        resp = resp.header(header::CONTENT_RANGE, format!("bytes {s}-{e}/{size}"));
    }
    resp.body(body).map_err(|_| ApiError::internal())
}

#[derive(Deserialize)]
struct PageQuery {
    page_token: Option<String>,
}

async fn manifest(
    Extension(files): Files,
    req: Req,
    Path(item_id): Path<String>,
    q: Result<Query<PageQuery>, QueryRejection>,
) -> Res<Manifest> {
    let q = query(q)?;
    run(&files, move |v| {
        v.guard_item(&req, &item_id, Need::Read)?;
        Ok(Json(v.drive.manifest(&item_id, q.page_token.as_deref())?))
    })
    .await
}

async fn chunk(Extension(files): Files, _req: Req, Path(hash): Path<String>) -> Result<Response, ApiError> {
    if !chunker::is_hash(&hash) {
        return Err(ApiError::invalid("Not a blake3 chunk hash."));
    }
    let data = run(&files, move |v| Ok(v.drive.get_chunk(&hash)?)).await?;
    Ok(([(header::CONTENT_TYPE, "application/octet-stream")], data).into_response())
}

async fn have_need(Extension(files): Files, _req: Req, body: Result<Json<HaveNeedRequest>, JsonRejection>) -> Res<HaveNeedResponse> {
    let body = json(body)?;
    if body.hashes.len() > 10_000 {
        return Err(ApiError::invalid("At most 10000 hashes per request."));
    }
    run(&files, move |v| Ok(Json(v.drive.have_need(&body.hashes)?))).await
}

// ---- writing ----

async fn small_write(
    Extension(files): Files,
    req: Req,
    Path(root_id): Path<String>,
    q: Result<Query<SmallWriteQuery>, QueryRejection>,
    body: Result<Bytes, BytesRejection>,
) -> Res<MutationResponse> {
    let q = query(q)?;
    let body = bytes(body)?;
    run(&files, move |v| {
        let root = v.guard(&req, &root_id, Need::Write)?;
        let mut on_conflict = q.on_conflict.unwrap_or(OnConflict::KeepBoth);
        if let Some(g) = &req.guest {
            shares::check_subtree(v, g, &v.drive.stat(&q.parent_id)?)?;
            shares::check_new_only(g, q.base_rev.is_some(), false)?;
            if g.upload_only() {
                // A drop never replaces anything: a name clash keeps both.
                on_conflict = OnConflict::KeepBoth;
            }
        }
        let offset = utc_offset(q.utc_offset_minutes)?;
        if memory::is_memory(&root) {
            let (item, cursor) = memory::write(v, &req.user_id, &q.parent_id, &q.name, &body, q.base_rev.as_deref())?;
            return Ok(Json(MutationResponse { item: v.item(&root, &item), conflict: None, cursor }));
        }
        let device = EditingDevice { name: req.device_name_or(q.device_name.clone()), utc_offset_minutes: offset };
        let m = v.drive.write_small(
            &root_id,
            &q.parent_id,
            &q.name,
            &body,
            q.base_rev.as_deref(),
            on_conflict,
            &req.principal,
            &device,
        )?;
        Ok(Json(mutation_as(v, &req, &m)?))
    })
    .await
}

async fn upload_create(Extension(files): Files, req: Req, body: Result<Json<UploadCreateRequest>, JsonRejection>) -> Res<UploadStatus> {
    let mut body = json(body)?;
    let guest = req.guest.clone();
    let status = run(&files, move |v| {
        let root = v.guard(&req, &body.root_id, Need::Write)?;
        if memory::is_memory(&root) {
            return Err(memory::refuse("take uploads"));
        }
        if let Some(g) = &req.guest {
            shares::check_subtree(v, g, &v.drive.stat(&body.parent_id)?)?;
            shares::check_new_only(g, body.base_rev.is_some(), body.item_id.is_some())?;
            if g.upload_only() {
                body.on_conflict = OnConflict::KeepBoth;
            }
        }
        body.device_name = req.device_name_or(body.device_name.take());
        Ok(v.drive.create_upload(body, &req.principal)?)
    })
    .await?;
    // Only the guest that opened a session may use it (`shares::gate`).
    if let Some(g) = guest {
        files.guests.remember_session(&status.session_id, &g.claims.guest_id);
    }
    Ok(Json(status))
}

async fn upload_append(
    Extension(files): Files,
    _req: Req,
    Path(session_id): Path<String>,
    body: Result<Json<UploadChunksAppend>, JsonRejection>,
) -> Res<UploadStatus> {
    let body = json(body)?;
    run(&files, move |v| Ok(Json(v.drive.append_upload_chunks(&session_id, &body.chunks)?))).await
}

async fn upload_chunk(
    Extension(files): Files,
    _req: Req,
    Path((session_id, hash)): Path<(String, String)>,
    body: Result<Bytes, BytesRejection>,
) -> Res<ChunkPutResponse> {
    let body = bytes(body)?;
    run(&files, move |v| Ok(Json(v.drive.put_upload_chunk(&session_id, &hash, &body)?))).await
}

async fn upload_status(Extension(files): Files, _req: Req, Path(session_id): Path<String>) -> Res<UploadStatus> {
    run(&files, move |v| Ok(Json(v.drive.upload_status(&session_id)?))).await
}

async fn upload_commit(Extension(files): Files, req: Req, Path(session_id): Path<String>) -> Res<UploadCommitResponse> {
    let guest = req.guest.is_some();
    let id = session_id.clone();
    let out = run(&files, move |v| {
        let m = v.drive.commit_upload(&session_id)?;
        let r = mutation_as(v, &req, &m)?;
        Ok(Json(UploadCommitResponse { item: r.item, conflict: r.conflict, cursor: r.cursor }))
    })
    .await?;
    if guest {
        files.guests.forget_session(&id);
    }
    Ok(out)
}

async fn upload_abort(Extension(files): Files, req: Req, Path(session_id): Path<String>) -> Result<StatusCode, ApiError> {
    let guest = req.guest.is_some();
    let id = session_id.clone();
    run(&files, move |v| {
        v.drive.abort_upload(&session_id)?;
        Ok(StatusCode::NO_CONTENT)
    })
    .await?;
    if guest {
        files.guests.forget_session(&id);
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn mkdir(Extension(files): Files, req: Req, body: Result<Json<MkdirRequest>, JsonRejection>) -> Res<MutationResponse> {
    let body = json(body)?;
    run(&files, move |v| {
        let (root, _) = v.guard_item(&req, &body.parent_id, Need::Write)?;
        if memory::is_memory(&root) {
            return Err(memory::refuse("make folders"));
        }
        let m = v.drive.mkdir(&body.parent_id, &body.name, body.on_conflict, &req.principal)?;
        Ok(Json(mutation_as(v, &req, &m)?))
    })
    .await
}

async fn move_item(Extension(files): Files, req: Req, body: Result<Json<MoveRequest>, JsonRejection>) -> Res<MutationResponse> {
    let body = json(body)?;
    run(&files, move |v| {
        let (root, _) = v.guard_item(&req, &body.item_id, Need::Write)?;
        let dest = match &body.new_parent_id {
            Some(p) => Some(v.guard_item(&req, p, Need::Write)?.0),
            None => None,
        };
        if memory::is_memory(&root) || dest.as_ref().is_some_and(memory::is_memory) {
            return Err(memory::refuse("move or rename files"));
        }
        let m = v.drive.move_item(
            &body.item_id,
            &body.base_rev,
            body.new_parent_id.as_deref(),
            body.new_name.as_deref(),
            body.on_conflict,
            &req.principal,
        )?;
        Ok(Json(mutation(v, &m)?))
    })
    .await
}

async fn copy(Extension(files): Files, req: Req, body: Result<Json<CopyRequest>, JsonRejection>) -> Res<CopyResponse> {
    let body = json(body)?;
    if body.dest_computer_id.as_deref().is_some_and(|c| c != files.cfg.computer_id) {
        return super::sync::copy_to_computer(files.clone(), req, body).await.map(Json);
    }
    run(&files, move |v| {
        v.guard_item(&req, &body.item_id, Need::Read)?;
        let (dest, _) = v.guard_item(&req, &body.dest_parent_id, Need::Write)?;
        if memory::is_memory(&dest) {
            return Err(memory::refuse("take copies"));
        }
        let m = v.drive.copy_item(&body.item_id, &body.dest_parent_id, body.new_name.as_deref(), body.on_conflict, &req.principal)?;
        let r = mutation(v, &m)?;
        Ok(Json(CopyResponse { item: Some(r.item), job: None, cursor: Some(r.cursor) }))
    })
    .await
}

async fn delete(Extension(files): Files, req: Req, body: Result<Json<DeleteRequest>, JsonRejection>) -> Res<DeleteResponse> {
    let body = json(body)?;
    run(&files, move |v| {
        let (root, item) = v.guard_item(&req, &body.item_id, Need::Write)?;
        if memory::is_memory(&root) {
            let cursor = memory::delete(v, &req.user_id, &item, body.base_rev.as_deref())?;
            return Ok(Json(DeleteResponse { trashed: None, cursor }));
        }
        let (trashed, cursor) = v.drive.delete(&body.item_id, body.base_rev.as_deref(), body.permanent, &req.principal)?;
        Ok(Json(DeleteResponse { trashed: trashed.map(|t| t.to_entry(permissions(&root, &t.item))), cursor }))
    })
    .await
}

async fn trash(
    Extension(files): Files,
    req: Req,
    Path(root_id): Path<String>,
    _q: Result<Query<PageQuery>, QueryRejection>,
) -> Res<TrashListResponse> {
    run(&files, move |v| {
        let root = v.guard(&req, &root_id, Need::Read)?;
        let entries = v.drive.trash_list(&root_id)?.iter().map(|t| t.to_entry(permissions(&root, &t.item))).collect();
        Ok(Json(TrashListResponse { root_id, entries, next_page_token: None, freshness: live() }))
    })
    .await
}

async fn restore(Extension(files): Files, req: Req, body: Result<Json<RestoreRequest>, JsonRejection>) -> Res<MutationResponse> {
    let body = json(body)?;
    run(&files, move |v| {
        let mut owner: Option<RootRecord> = None;
        for r in v.drive.roots()? {
            if v.drive.trash_list(&r.root_id)?.iter().any(|t| t.trash_id == body.trash_id) {
                owner = Some(r);
                break;
            }
        }
        let root = owner.ok_or_else(|| ApiError::new(ErrorCode::NotFound, "That item is no longer in the trash."))?;
        v.guard(&req, &root.root_id, Need::Write)?;
        let m = v.drive.restore(&body.trash_id, body.on_conflict, &req.principal)?;
        Ok(Json(mutation(v, &m)?))
    })
    .await
}

async fn versions(Extension(files): Files, req: Req, Path(item_id): Path<String>) -> Res<VersionsResponse> {
    run(&files, move |v| {
        v.guard_item(&req, &item_id, Need::Read)?;
        Ok(Json(VersionsResponse { item_id: item_id.clone(), versions: v.drive.versions(&item_id)?, freshness: live() }))
    })
    .await
}

async fn restore_version(
    Extension(files): Files,
    req: Req,
    Path(item_id): Path<String>,
    body: Result<Json<RestoreVersionRequest>, JsonRejection>,
) -> Res<MutationResponse> {
    let body = json(body)?;
    run(&files, move |v| {
        let (root, _) = v.guard_item(&req, &item_id, Need::Write)?;
        if memory::is_memory(&root) {
            return Err(memory::refuse("restore versions (git keeps its history)"));
        }
        let m = v.drive.restore_version(&item_id, &body.version_id, &body.base_rev, &req.principal)?;
        Ok(Json(mutation(v, &m)?))
    })
    .await
}

// ---- search, pins, comments, jobs, replicas ----

async fn search(Extension(files): Files, req: Req, body: Result<Json<SearchRequest>, JsonRejection>) -> Res<SearchResponse> {
    let body = json(body)?;
    let query = body.query.trim().to_string();
    if query.is_empty() || query.len() > 512 {
        return Err(ApiError::invalid("Search for 1 to 512 bytes of text."));
    }
    let limit = body.limit.unwrap_or(50).clamp(1, 200) as usize;
    let start = page_start(body.page_token.as_deref(), "s1.")?;
    run(&files, move |v| {
        // Only roots this caller may read now: its kind (agents need agent_process,
        // guests get nothing here) and the unlock for protected ones.
        let roots: Vec<RootRecord> = intel::scoped_roots(v, &req, body.root_ids.as_deref())?;
        if body.mode != SearchMode::Name {
            return Ok(Json(intel::search_content(v, &req, &roots, &body, &query, limit, start)?));
        }
        let ids: Vec<String> = roots.iter().map(|r| r.root_id.clone()).collect();
        let found = meta::search_names(
            &v.cfg.data_dir.join("engine"),
            &query,
            &ids,
            body.kind,
            body.modified_after.as_deref(),
            limit + 1,
            start,
        )?;
        let next = (found.len() > limit).then(|| format!("s1.{}", start + limit));
        let mut hits = Vec::new();
        for (id, score) in found.into_iter().take(limit) {
            if let Ok(rec) = v.drive.stat(&id) {
                if let Some(root) = roots.iter().find(|r| r.root_id == rec.root_id) {
                    hits.push(SearchHit { item: v.item(root, &rec), score, snippet: None });
                }
            }
        }
        Ok(Json(SearchResponse { hits, next_page_token: next, index_state: SearchResponseIndexState::Ready, freshness: live() }))
    })
    .await
}

async fn pin(Extension(files): Files, req: Req, body: Result<Json<PinRequest>, JsonRejection>) -> Res<PinResponse> {
    let body = json(body)?;
    if body.device_id.trim().is_empty() || body.device_id.len() > 200 {
        return Err(ApiError::invalid("device_id is required."));
    }
    if req.principal.kind == PrincipalKind::OwnDevice && req.principal.id != body.device_id {
        return Err(ApiError::forbidden("A device can only pin for itself."));
    }
    run(&files, move |v| {
        let (root, _) = v.guard_item(&req, &body.item_id, Need::Read)?;
        if !root.policy.offline && body.pinned {
            return Err(ApiError::forbidden("This folder can't be kept offline.").into());
        }
        let m = v.meta.lock().expect("meta lock");
        meta::set_pin(&m, &body.item_id, &body.device_id, body.pinned, &now(v))?;
        Ok(Json(PinResponse { item_id: body.item_id.clone(), pinned_on: meta::pins(&m, &body.item_id)? }))
    })
    .await
}

async fn comments(Extension(files): Files, req: Req, Path(item_id): Path<String>) -> Res<Vec<Comment>> {
    run(&files, move |v| {
        v.guard_item(&req, &item_id, Need::Read)?;
        Ok(Json(meta::comments(&v.meta.lock().expect("meta lock"), &item_id)?))
    })
    .await
}

async fn add_comment(Extension(files): Files, req: Req, body: Result<Json<CommentCreateRequest>, JsonRejection>) -> Res<Comment> {
    let body = json(body)?;
    let text = body.body.trim().to_string();
    if text.is_empty() || text.chars().count() > 10_000 {
        return Err(ApiError::invalid("A comment is 1 to 10000 characters."));
    }
    run(&files, move |v| {
        v.guard_item(&req, &body.item_id, Need::Read)?;
        let comment = Comment {
            comment_id: format!("cm_{}", hex::encode(rand::random::<[u8; 12]>())),
            item_id: body.item_id.clone(),
            author: req.principal.clone(),
            body: text,
            created_at: now(v),
        };
        meta::add_comment(&v.meta.lock().expect("meta lock"), &comment)?;
        Ok(Json(comment))
    })
    .await
}

/// Jobs: model transfers and routed files run in memory (`models.rs`; a
/// restart forgets them, and the pull is idempotent, so start it again);
/// cross-computer copies, moves and replica syncs are stored (`sync.rs`).
/// Copies inside this computer run synchronously and have no job.
async fn job(Extension(files): Files, _req: Req, Path(job_id): Path<String>) -> Res<JobRef> {
    if let Some(j) = super::models::job(&job_id) {
        files.inner().await?;
        return Ok(Json(j));
    }
    super::sync::job(&files, job_id).await
}
