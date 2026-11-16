//! HTTP handlers for the intelligence routes (under `/files/v2`, see
//! `surfaces/docs/api/files-drive.mdx`). The wire shapes are local to the
//! runtime for now; the contract's `SearchRequest`, `SearchHit` and `Item`
//! (tags, provenance, `has_thumbnail`) already carry what clients need for
//! search and labels.

use std::sync::Arc;

use allternit_files_core::{ErrorCode, Item, PrincipalKind, Provenance, ProvenanceSource};
use allternit_files_engine::{ItemRecord, RootRecord};
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Path, Query};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};

use super::{dedup, now_string, read_document, summarize};
use crate::files_v2::routes::run;
use crate::files_v2::{ApiError, Fail, FilesV2, Inner, Need, Req};

type Files = Extension<Arc<FilesV2>>;
type Res<T> = Result<Json<T>, ApiError>;

fn json<T>(body: Result<Json<T>, JsonRejection>) -> Result<T, ApiError> {
    body.map(|Json(v)| v).map_err(Into::into)
}

fn query<T>(q: Result<Query<T>, QueryRejection>) -> Result<T, ApiError> {
    q.map(|Query(v)| v).map_err(Into::into)
}

/// Guests reach files only through a share; these routes are not that path.
fn not_guest(req: &Req) -> Result<(), ApiError> {
    if req.principal.kind == PrincipalKind::Guest {
        return Err(ApiError::forbidden("Guests use the share link, not this route."));
    }
    Ok(())
}

/// The owner or one of the owner's own devices.
fn owner_only(req: &Req) -> Result<(), ApiError> {
    match req.principal.kind {
        PrincipalKind::Owner | PrincipalKind::OwnDevice => Ok(()),
        _ => Err(ApiError::forbidden("Only the owner can change this.")),
    }
}

fn agent_gate(req: &Req, root: &RootRecord) -> Result<(), ApiError> {
    if req.principal.kind == PrincipalKind::Agent && !root.policy.agent_process {
        return Err(ApiError::new(ErrorCode::AgentProcessDenied, "This folder doesn't allow agents to work on its files."));
    }
    Ok(())
}

/// An item the caller may read, after the guest, agent and unlock checks.
fn read_item(v: &Inner, req: &Req, item_id: &str, need: Need) -> Result<(RootRecord, ItemRecord), Fail> {
    not_guest(req)?;
    let (root, rec) = v.guard_item(req, item_id, need)?;
    agent_gate(req, &root)?;
    Ok((root, rec))
}

fn clean_tag(t: &str) -> Result<String, ApiError> {
    let t = t.trim();
    if t.is_empty() || t.chars().count() > 64 || t.chars().any(|c| c.is_control()) {
        return Err(ApiError::invalid("A tag is 1 to 64 characters with no control characters."));
    }
    Ok(t.to_string())
}

// ---- status and opt-in ----

#[derive(Serialize)]
pub(crate) struct StatusBody {
    pub caught_up: bool,
    pub indexed_items: u64,
    pub embedding_models: Vec<String>,
    pub cpu_percent: u32,
    pub batch_files: usize,
    pub opted_in_roots: Vec<String>,
}

pub(crate) async fn status(Extension(files): Files, req: Req) -> Res<StatusBody> {
    run(&files, move |v| {
        not_guest(&req)?;
        let b = v.intel.budget();
        let mut opted = Vec::new();
        for r in v.drive.roots()?.into_iter().filter(|r| r.policy.protected) {
            if v.intel.opted_in(&r.root_id)? {
                opted.push(r.root_id);
            }
        }
        Ok(Json(StatusBody {
            caught_up: v.intel.caught_up(),
            indexed_items: v.intel.indexed_count()?,
            embedding_models: v.intel.vector_models()?,
            cpu_percent: b.cpu_percent,
            batch_files: b.batch_files,
            opted_in_roots: opted,
        }))
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OptInRequest {
    pub index: bool,
}

#[derive(Serialize)]
pub(crate) struct OptInResponse {
    pub root_id: String,
    pub index: bool,
}

/// Opt a protected root in or out of indexing. Needs the device unlock
/// (checked by the root guard) and never works over a relay, like loosening
/// protection in `PATCH /roots`.
pub(crate) async fn set_root(Extension(files): Files, req: Req, Path(root_id): Path<String>, body: Result<Json<OptInRequest>, JsonRejection>) -> Res<OptInResponse> {
    let body = json(body)?;
    run(&files, move |v| {
        owner_only(&req)?;
        if req.relayed {
            return Err(ApiError::forbidden("Change this on the computer itself or a signed-in own device on the same network.").into());
        }
        v.guard(&req, &root_id, Need::Read)?;
        v.intel.set_opt_in(&root_id, body.index)?;
        Ok(Json(OptInResponse { root_id: root_id.clone(), index: body.index }))
    })
    .await
}

// ---- tags ----

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TagRequest {
    pub item_id: String,
    #[serde(default)]
    pub add: Vec<String>,
    #[serde(default)]
    pub remove: Vec<String>,
}

#[derive(Serialize)]
pub(crate) struct TagResponse {
    pub item_id: String,
    pub tags: Vec<String>,
}

pub(crate) async fn change_tags(Extension(files): Files, req: Req, body: Result<Json<TagRequest>, JsonRejection>) -> Res<TagResponse> {
    let body = json(body)?;
    if body.add.len() > 50 || body.remove.len() > 50 {
        return Err(ApiError::invalid("Change at most 50 tags per request."));
    }
    let add = body.add.iter().map(|t| clean_tag(t)).collect::<Result<Vec<_>, _>>()?;
    let remove = body.remove.iter().map(|t| clean_tag(t)).collect::<Result<Vec<_>, _>>()?;
    run(&files, move |v| {
        read_item(v, &req, &body.item_id, Need::Write)?;
        let tags = v.intel.change_tags(&body.item_id, &add, &remove)?;
        Ok(Json(TagResponse { item_id: body.item_id.clone(), tags }))
    })
    .await
}

#[derive(Serialize)]
pub(crate) struct TagCount {
    pub tag: String,
    pub count: u64,
}

pub(crate) async fn tag_counts(Extension(files): Files, req: Req) -> Res<Vec<TagCount>> {
    run(&files, move |v| {
        not_guest(&req)?;
        Ok(Json(v.intel.tag_counts()?.into_iter().map(|(tag, count)| TagCount { tag, count }).collect()))
    })
    .await
}

#[derive(Serialize)]
pub(crate) struct ItemsBody {
    pub items: Vec<Item>,
}

/// Items in the readable roots, in the order given, skipping what the caller can't read.
fn readable_items(v: &Inner, req: &Req, ids: Vec<String>) -> Vec<Item> {
    let mut out = Vec::new();
    for id in ids {
        if let Ok((root, rec)) = read_item(v, req, &id, Need::Read) {
            out.push(v.item(&root, &rec));
        }
    }
    out
}

pub(crate) async fn tagged(Extension(files): Files, req: Req, Path(tag): Path<String>) -> Res<ItemsBody> {
    run(&files, move |v| {
        not_guest(&req)?;
        let ids = v.intel.items_with_tag(&tag)?;
        Ok(Json(ItemsBody { items: readable_items(v, &req, ids) }))
    })
    .await
}

// ---- provenance ----

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProvenanceRequest {
    pub item_id: String,
    pub provenance: Provenance,
}

/// Labels an item with where it came from. Anything from an agent is always
/// labelled AI-generated, whatever the request says.
pub(crate) async fn set_provenance(Extension(files): Files, req: Req, body: Result<Json<ProvenanceRequest>, JsonRejection>) -> Res<Provenance> {
    let mut body = json(body)?;
    run(&files, move |v| {
        read_item(v, &req, &body.item_id, Need::Write)?;
        if req.principal.kind == PrincipalKind::Agent {
            body.provenance.source = ProvenanceSource::Agent;
            body.provenance.agent_id = Some(req.principal.id.clone());
        }
        if body.provenance.source == ProvenanceSource::Agent {
            body.provenance.ai_generated = true;
        }
        v.intel.set_provenance(&body.item_id, &body.provenance)?;
        Ok(Json(body.provenance.clone()))
    })
    .await
}

// ---- duplicates ----

#[derive(Deserialize)]
pub(crate) struct DedupQuery {
    pub root_id: Option<String>,
    pub limit: Option<u64>,
}

#[derive(Serialize)]
pub(crate) struct DedupGroupBody {
    pub content_hash: String,
    pub size: u64,
    /// Bytes freed by keeping one copy.
    pub reclaimable_bytes: u64,
    pub items: Vec<Item>,
}

pub(crate) async fn dedup_report(Extension(files): Files, req: Req, q: Result<Query<DedupQuery>, QueryRejection>) -> Res<Vec<DedupGroupBody>> {
    let q = query(q)?;
    let limit = q.limit.unwrap_or(50).clamp(1, 200) as usize;
    run(&files, move |v| {
        not_guest(&req)?;
        let only = q.root_id.clone().map(|r| vec![r]);
        let roots = super::scoped_roots(v, &req, only.as_deref())?;
        let ids: Vec<String> = roots.iter().map(|r| r.root_id.clone()).collect();
        let mut out = Vec::new();
        for g in dedup(&v.cfg.data_dir.join("engine"), &ids, limit)? {
            let items: Vec<Item> = g
                .item_ids
                .iter()
                .filter_map(|id| v.drive.stat(id).ok())
                .filter(|rec| !rec.link_escapes)
                .filter_map(|rec| roots.iter().find(|r| r.root_id == rec.root_id).map(|root| v.item(root, &rec)))
                .collect();
            if items.len() > 1 {
                out.push(DedupGroupBody { content_hash: g.content_hash, size: g.size, reclaimable_bytes: g.size * (items.len() as u64 - 1), items });
            }
        }
        Ok(Json(out))
    })
    .await
}

// ---- collections ----

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CollectionCreate {
    pub name: String,
}

#[derive(Serialize)]
pub(crate) struct CollectionBody {
    pub collection_id: String,
    pub name: String,
    pub item_count: u64,
}

#[derive(Serialize)]
pub(crate) struct CollectionDetail {
    pub collection_id: String,
    pub name: String,
    pub items: Vec<Item>,
}

pub(crate) async fn collections(Extension(files): Files, req: Req) -> Res<Vec<CollectionBody>> {
    run(&files, move |v| {
        not_guest(&req)?;
        let rows = v.intel.collections()?;
        Ok(Json(rows.into_iter().map(|(collection_id, name, item_count)| CollectionBody { collection_id, name, item_count }).collect()))
    })
    .await
}

pub(crate) async fn create_collection(Extension(files): Files, req: Req, body: Result<Json<CollectionCreate>, JsonRejection>) -> Res<CollectionBody> {
    let body = json(body)?;
    let name = body.name.trim().to_string();
    if name.is_empty() || name.chars().count() > 120 || name.chars().any(|c| c.is_control()) {
        return Err(ApiError::invalid("A collection name is 1 to 120 characters."));
    }
    run(&files, move |v| {
        owner_only(&req)?;
        let id = format!("col_{}", hex::encode(rand::random::<[u8; 8]>()));
        v.intel.create_collection(&id, &name, &now_string(v))?;
        Ok(Json(CollectionBody { collection_id: id, name: name.clone(), item_count: 0 }))
    })
    .await
}

pub(crate) async fn collection(Extension(files): Files, req: Req, Path(id): Path<String>) -> Res<CollectionDetail> {
    run(&files, move |v| {
        not_guest(&req)?;
        let name = v.intel.collection_name(&id)?.ok_or_else(|| ApiError::new(ErrorCode::NotFound, "No such collection."))?;
        let ids = v.intel.collection_items(&id)?;
        Ok(Json(CollectionDetail { collection_id: id.clone(), name, items: readable_items(v, &req, ids) }))
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CollectionChange {
    pub item_id: String,
    #[serde(default)]
    pub remove: bool,
}

pub(crate) async fn change_collection(
    Extension(files): Files,
    req: Req,
    Path(id): Path<String>,
    body: Result<Json<CollectionChange>, JsonRejection>,
) -> Res<CollectionBody> {
    let body = json(body)?;
    run(&files, move |v| {
        owner_only(&req)?;
        let name = v.intel.collection_name(&id)?.ok_or_else(|| ApiError::new(ErrorCode::NotFound, "No such collection."))?;
        if !body.remove {
            read_item(v, &req, &body.item_id, Need::Read)?;
        }
        v.intel.collection_change(&id, &body.item_id, !body.remove, &now_string(v))?;
        let count = v.intel.collection_items(&id)?.len() as u64;
        Ok(Json(CollectionBody { collection_id: id.clone(), name, item_count: count }))
    })
    .await
}

pub(crate) async fn delete_collection(Extension(files): Files, req: Req, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    run(&files, move |v| {
        owner_only(&req)?;
        if v.intel.delete_collection(&id)? {
            Ok(StatusCode::NO_CONTENT)
        } else {
            Err(ApiError::new(ErrorCode::NotFound, "No such collection.").into())
        }
    })
    .await
}

// ---- previews ----

#[derive(Deserialize)]
pub(crate) struct PreviewQuery {
    /// `thumbnail` (default for images) or `text`.
    pub kind: Option<String>,
}

pub(crate) async fn preview(Extension(files): Files, req: Req, Path(item_id): Path<String>, q: Result<Query<PreviewQuery>, QueryRejection>) -> Result<Response, ApiError> {
    let q = query(q)?;
    run(&files, move |v| {
        let (_, rec) = read_item(v, &req, &item_id, Need::Read)?;
        let wanted: Vec<&str> = match q.kind.as_deref() {
            Some("thumbnail") => vec!["thumbnail"],
            Some("text") => vec!["text"],
            Some(_) => return Err(ApiError::invalid("kind is thumbnail or text.").into()),
            None => vec!["thumbnail", "text"],
        };
        for kind in wanted {
            if let Some((mime, data)) = v.intel.preview(&item_id, kind)? {
                // A preview of an older version is never served for the new one.
                if v.intel.indexed_hash(&item_id)?.as_deref() == rec.content_hash.as_deref() {
                    let etag = format!("\"{}-{kind}\"", rec.content_hash.clone().unwrap_or_default());
                    return Ok((StatusCode::OK, [(header::CONTENT_TYPE, mime), (header::ETAG, etag), (header::CACHE_CONTROL, "private, max-age=0, must-revalidate".to_string())], data).into_response());
                }
            }
        }
        Err(ApiError::new(ErrorCode::NotFound, "No preview yet. It is made when the computer is awake and the index has reached this file.").into())
    })
    .await
}

// ---- explicit actions and folder rules ----

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ActionRequest {
    pub item_id: String,
    /// `summarize` or `transcribe`.
    pub action: String,
}

#[derive(Serialize)]
pub(crate) struct ActionResponse {
    pub item_id: String,
    pub action: String,
    pub summary: String,
    /// `extractive`: sentences picked from the file, no model.
    pub method: String,
    pub ai_generated: bool,
}

/// A summary or transcript the user asked for. Summaries are extractive and
/// cost nothing; transcription needs a model this runtime doesn't have.
pub(crate) async fn action(Extension(files): Files, req: Req, body: Result<Json<ActionRequest>, JsonRejection>) -> Res<ActionResponse> {
    let body = json(body)?;
    match body.action.as_str() {
        "summarize" => {}
        "transcribe" => return Err(ApiError::unsupported("Transcription needs a speech model, which this computer doesn't have yet.")),
        _ => return Err(ApiError::invalid("action is summarize or transcribe.")),
    }
    run(&files, move |v| {
        let (_, rec) = read_item(v, &req, &body.item_id, Need::Read)?;
        let hash = rec.content_hash.clone().unwrap_or_default();
        if let Some((h, text)) = v.intel.summary(&body.item_id)? {
            if h == hash {
                return Ok(Json(ActionResponse { item_id: body.item_id.clone(), action: body.action.clone(), summary: text, method: "extractive".into(), ai_generated: true }));
            }
        }
        let path = v.drive.local_path(&body.item_id)?;
        let text = read_document(&path, &rec.name).ok_or_else(|| ApiError::unsupported("Only text, PDF and Word files can be summarized so far."))?;
        let summary = summarize(&text, 3);
        v.intel.store_summary(&body.item_id, &hash, &summary, &now_string(v))?;
        Ok(Json(ActionResponse { item_id: body.item_id.clone(), action: body.action.clone(), summary, method: "extractive".into(), ai_generated: true }))
    })
    .await
}

#[derive(Serialize)]
pub(crate) struct RuleBody {
    pub rule_id: String,
    pub folder_id: String,
    pub action: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuleCreate {
    pub folder_id: String,
    pub action: String,
}

pub(crate) async fn rules(Extension(files): Files, req: Req) -> Res<Vec<RuleBody>> {
    run(&files, move |v| {
        owner_only(&req)?;
        Ok(Json(v.intel.rules()?.into_iter().map(|(rule_id, folder_id, action)| RuleBody { rule_id, folder_id, action }).collect()))
    })
    .await
}

pub(crate) async fn create_rule(Extension(files): Files, req: Req, body: Result<Json<RuleCreate>, JsonRejection>) -> Res<RuleBody> {
    let body = json(body)?;
    if body.action != "summarize" {
        return Err(ApiError::invalid("A folder rule can only summarize for now."));
    }
    run(&files, move |v| {
        owner_only(&req)?;
        let (_, rec) = read_item(v, &req, &body.folder_id, Need::Read)?;
        if rec.kind != allternit_files_core::ItemKind::Folder {
            return Err(ApiError::invalid("A folder rule needs a folder.").into());
        }
        let id = format!("rule_{}", hex::encode(rand::random::<[u8; 8]>()));
        v.intel.add_rule(&id, &body.folder_id, &body.action, &now_string(v))?;
        Ok(Json(RuleBody { rule_id: id, folder_id: body.folder_id.clone(), action: body.action.clone() }))
    })
    .await
}

pub(crate) async fn delete_rule(Extension(files): Files, req: Req, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    run(&files, move |v| {
        owner_only(&req)?;
        if v.intel.delete_rule(&id)? {
            Ok(StatusCode::NO_CONTENT)
        } else {
            Err(ApiError::new(ErrorCode::NotFound, "No such rule.").into())
        }
    })
    .await
}
