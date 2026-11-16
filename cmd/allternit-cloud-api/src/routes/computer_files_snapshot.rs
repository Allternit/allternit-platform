//! Files drive asleep snapshot (contracts/files-drive/SPEC.md sections 9, 11).
//!
//! While a computer sleeps, the relay answers `roots`, `list`, `stat`,
//! `changes` and name `search` from the metadata snapshot its runtime left in
//! the user's R2 space, with `served_by: snapshot` and `computer_state:
//! asleep`. Nothing here ever wakes the computer or touches its idle clock.
//!
//! ```text
//! allternit-user-files / files-snapshots/<computer_id>/v1/manifest.json         SnapshotManifest
//! allternit-user-files / files-snapshots/<computer_id>/v1/roots/<root_id>.json  SnapshotRootFile
//! ```
//!
//! The runtime writes them through cloud-api (`PUT /api/v1/computer-files/
//! snapshot/manifest` and `.../snapshot/roots/:root_id`, signed like the
//! presence heartbeat; see `computer_files`), never with R2 credentials of its
//! own. Ingest drops `protected` roots and `path_hint`, so a snapshot never
//! holds Memory Drive names or local paths.

use std::sync::Arc;

use allternit_files_core::{
    ChangesQuery, ChangesResponse, ComputerState, ErrorCode, Freshness, Item, ItemKind, ListQuery,
    ListQuerySort, ListResponse, Permissions, Root, RootPolicy, RootsResponse, SearchHit,
    SearchMode, SearchRequest, SearchResponse, SearchResponseIndexState, ServedBy,
    SnapshotItem, SnapshotManifest, SnapshotRootFile, StatResponse,
};
use axum::{
    extract::Query,
    http::{StatusCode, Uri},
    response::{IntoResponse, Response},
    Json,
};

use crate::services::r2::{ObjectStore, R2Client};

pub const SNAPSHOT_PREFIX: &str = "files-snapshots";
const LIST_DEFAULT: usize = 200;
const LIST_MAX: usize = 1000;
const SEARCH_DEFAULT: usize = 50;
const SEARCH_MAX: usize = 200;

pub fn manifest_key(computer_id: &str) -> String {
    format!("{SNAPSHOT_PREFIX}/{computer_id}/v1/manifest.json")
}

pub fn root_key(computer_id: &str, root_id: &str) -> String {
    format!("{SNAPSHOT_PREFIX}/{computer_id}/v1/roots/{root_id}.json")
}

/// The object store the snapshot lives in: R2 from `ALLTERNIT_R2_*`, or a
/// test store. `None` when R2 is not configured (the relay then answers
/// `asleep`, as for "no snapshot yet").
pub(crate) fn snapshot_store() -> Option<Arc<dyn ObjectStore>> {
    #[cfg(test)]
    if let Some(store) = test_store::get() {
        return Some(store);
    }
    R2Client::from_env().ok().map(|client| Arc::new(client) as Arc<dyn ObjectStore>)
}

#[cfg(test)]
pub(crate) mod test_store {
    use super::*;
    use std::sync::Mutex;

    static STORE: Mutex<Option<Arc<dyn ObjectStore>>> = Mutex::new(None);

    pub(crate) fn set(store: Option<Arc<dyn ObjectStore>>) {
        *STORE.lock().unwrap_or_else(|e| e.into_inner()) = store;
    }

    pub(crate) fn get() -> Option<Arc<dyn ObjectStore>> {
        STORE.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// What a v2 Files request to a sleeping computer is.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum AsleepRequest {
    /// Answered from the snapshot.
    Snapshot(SnapshotQuery),
    /// Other metadata (trash, versions, comments, jobs, ...): `asleep`.
    Metadata,
    /// Content, manifests, chunks, uploads, writes, content search:
    /// `wake_consent_required`.
    Content,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SnapshotQuery {
    Roots,
    List { root_id: String, query: ListQuery },
    Stat { item_id: String },
    Changes { root_id: String, query: ChangesQuery },
    Search(SearchRequest),
}

/// Sort a relayed v2 request into snapshot / metadata / content. `path` is
/// the full relayed path (`/api/v1/files/v2/...?query`).
pub(crate) fn classify(method: &str, path: &str, body: &[u8]) -> AsleepRequest {
    let bare = path.split(['?', '#']).next().unwrap_or_default();
    let rest = bare
        .strip_prefix(super::runtime_relay::FILES_V2_PREFIX)
        .unwrap_or_default()
        .trim_end_matches('/');
    let segments: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
    let uri: Option<Uri> = path.parse().ok();
    let get = method.eq_ignore_ascii_case("GET");
    let post = method.eq_ignore_ascii_case("POST");
    match (get, post, segments.as_slice()) {
        (true, _, ["roots"]) => AsleepRequest::Snapshot(SnapshotQuery::Roots),
        (true, _, ["roots", root_id, "list"]) => match uri.as_ref().and_then(|u| Query::<ListQuery>::try_from_uri(u).ok()) {
            Some(Query(query)) => AsleepRequest::Snapshot(SnapshotQuery::List { root_id: root_id.to_string(), query }),
            None => AsleepRequest::Metadata,
        },
        (true, _, ["roots", root_id, "changes"]) => match uri.as_ref().and_then(|u| Query::<ChangesQuery>::try_from_uri(u).ok()) {
            Some(Query(query)) => AsleepRequest::Snapshot(SnapshotQuery::Changes { root_id: root_id.to_string(), query }),
            None => AsleepRequest::Metadata,
        },
        (true, _, ["items", item_id]) => AsleepRequest::Snapshot(SnapshotQuery::Stat { item_id: item_id.to_string() }),
        (_, true, ["search"]) => match serde_json::from_slice::<SearchRequest>(body) {
            Ok(request) if request.mode == SearchMode::Name => AsleepRequest::Snapshot(SnapshotQuery::Search(request)),
            Ok(_) => AsleepRequest::Content,
            Err(_) => AsleepRequest::Metadata,
        },
        (true, _, segs) if !segs.iter().any(|s| matches!(*s, "content" | "manifest" | "chunks" | "uploads")) => {
            AsleepRequest::Metadata
        }
        _ => AsleepRequest::Content,
    }
}

/// Why a snapshot answer is not available: the relay then answers `asleep`
/// (or `not_found` for an id the snapshot does not hold).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SnapshotMiss {
    /// No snapshot (never written, R2 not configured, unreadable, or names
    /// encrypted: "names unavailable").
    Unavailable,
    /// The snapshot is there but has no such root or item.
    NotFound,
}

async fn read_json<T: serde::de::DeserializeOwned>(store: &dyn ObjectStore, key: &str) -> Option<T> {
    let bytes = match store.get(crate::services::user_files::BUCKET, key).await {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return None,
        Err(error) => {
            tracing::warn!(%key, "files snapshot: read failed: {error:?}");
            return None;
        }
    };
    serde_json::from_slice(&bytes).ok()
}

pub(crate) async fn load_manifest(store: &dyn ObjectStore, computer_id: &str) -> Option<SnapshotManifest> {
    let manifest: SnapshotManifest = read_json(store, &manifest_key(computer_id)).await?;
    // Defence in depth: never serve another computer's manifest or an
    // encrypted one (v2 readers show "names unavailable").
    (manifest.computer_id == computer_id && !manifest.names_encrypted && manifest.roots.is_some()).then_some(manifest)
}

async fn load_root(store: &dyn ObjectStore, computer_id: &str, root_id: &str) -> Option<SnapshotRootFile> {
    let file: SnapshotRootFile = read_json(store, &root_key(computer_id, root_id)).await?;
    (file.root_id == root_id && !file.names_encrypted && file.items.is_some()).then_some(file)
}

/// Roots a snapshot may show: never `protected` ones.
fn visible_roots(manifest: &SnapshotManifest) -> Vec<Root> {
    manifest
        .roots
        .clone()
        .unwrap_or_default()
        .into_iter()
        .filter(|root| !root.policy.protected)
        .map(|mut root| {
            root.path_hint = None;
            root
        })
        .collect()
}

fn freshness(snapshot_at: &str) -> Freshness {
    Freshness {
        served_by: ServedBy::Snapshot,
        computer_state: ComputerState::Asleep,
        snapshot_at: Some(snapshot_at.to_string()),
    }
}

/// The owner's permissions on a snapshot item, from its root's policy (the
/// runtime re-checks everything once it is awake). Never `execute`.
fn permissions(policy: &RootPolicy, kind: ItemKind) -> Permissions {
    Permissions {
        list: policy.read && kind == ItemKind::Folder,
        read: policy.read,
        write: policy.write,
        delete: policy.write,
        share: policy.shareable,
        comment: policy.read,
        agent_process: policy.agent_process,
        transfer: policy.read,
        execute: false,
    }
}

fn item_from_snapshot(root: &Root, item: &SnapshotItem) -> Item {
    Item {
        id: item.id.clone(),
        root_id: root.id.clone(),
        parent_id: item.parent_id.clone(),
        name: item.name.clone(),
        kind: item.kind,
        size: if item.kind == ItemKind::Folder { 0 } else { item.size },
        rev: item.rev.clone(),
        meta_version: 0,
        content_hash: None,
        mime: None,
        created_at: item.modified_at.clone(),
        modified_at: item.modified_at.clone(),
        modified_by: None,
        root_link: item.root_link.clone(),
        pinned_on: None,
        tags: None,
        provenance: None,
        has_thumbnail: None,
        permissions: permissions(&root.policy, item.kind),
    }
}

/// The root folder itself (snapshots list its children, not the folder).
fn root_folder(root: &Root, file: &SnapshotRootFile) -> Item {
    Item {
        id: root.root_item_id.clone(),
        root_id: root.id.clone(),
        parent_id: None,
        name: root.name.clone(),
        kind: ItemKind::Folder,
        size: 0,
        rev: format!("snapshot-{}", file.cursor),
        meta_version: 0,
        content_hash: None,
        mime: None,
        created_at: file.written_at.clone(),
        modified_at: file.written_at.clone(),
        modified_by: None,
        root_link: None,
        pinned_on: None,
        tags: None,
        provenance: None,
        has_thumbnail: None,
        permissions: permissions(&root.policy, ItemKind::Folder),
    }
}

fn find_root<'a>(roots: &'a [Root], root_id: &str) -> Option<&'a Root> {
    roots.iter().find(|root| root.id == root_id)
}

/// Answer one snapshot query. Pure of HTTP routing so tests can drive it.
pub(crate) async fn answer(
    store: &dyn ObjectStore,
    computer_id: &str,
    query: &SnapshotQuery,
) -> Result<Response, SnapshotMiss> {
    let manifest = load_manifest(store, computer_id).await.ok_or(SnapshotMiss::Unavailable)?;
    let roots = visible_roots(&manifest);
    match query {
        SnapshotQuery::Roots => Ok(Json(RootsResponse {
            computer_id: computer_id.to_string(),
            roots,
            freshness: freshness(&manifest.written_at),
        })
        .into_response()),
        SnapshotQuery::List { root_id, query } => {
            let root = find_root(&roots, root_id).ok_or(SnapshotMiss::NotFound)?;
            let file = load_root(store, computer_id, root_id).await.ok_or(SnapshotMiss::Unavailable)?;
            let items = file.items.as_deref().unwrap_or_default();
            let parent = if query.parent_id == root.root_item_id {
                root_folder(root, &file)
            } else {
                let folder = items
                    .iter()
                    .find(|item| item.id == query.parent_id && item.kind == ItemKind::Folder)
                    .ok_or(SnapshotMiss::NotFound)?;
                item_from_snapshot(root, folder)
            };
            let mut children: Vec<Item> = items
                .iter()
                .filter(|item| item.parent_id.as_deref() == Some(query.parent_id.as_str()))
                .map(|item| item_from_snapshot(root, item))
                .collect();
            match query.sort.unwrap_or(ListQuerySort::Name) {
                ListQuerySort::Name => children.sort_by_key(|item| item.name.to_lowercase()),
                ListQuerySort::Modified => children.sort_by(|a, b| b.modified_at.cmp(&a.modified_at)),
                ListQuerySort::Size => children.sort_by(|a, b| b.size.cmp(&a.size)),
            }
            let offset: usize = query.page_token.as_deref().and_then(|t| t.parse().ok()).unwrap_or(0);
            let limit = query.limit.map(|l| l as usize).unwrap_or(LIST_DEFAULT).clamp(1, LIST_MAX);
            let total = children.len();
            let page: Vec<Item> = children.into_iter().skip(offset).take(limit).collect();
            let next = offset + page.len();
            Ok(Json(ListResponse {
                root_id: root.id.clone(),
                parent,
                items: page,
                next_page_token: (next < total).then(|| next.to_string()),
                cursor: file.cursor.clone(),
                freshness: freshness(&file.written_at),
            })
            .into_response())
        }
        SnapshotQuery::Stat { item_id } => {
            for root in &roots {
                let Some(file) = load_root(store, computer_id, &root.id).await else {
                    continue;
                };
                if root.root_item_id == *item_id {
                    return Ok(Json(StatResponse { item: root_folder(root, &file), freshness: freshness(&file.written_at) }).into_response());
                }
                if let Some(item) = file.items.as_deref().unwrap_or_default().iter().find(|item| item.id == *item_id) {
                    return Ok(Json(StatResponse { item: item_from_snapshot(root, item), freshness: freshness(&file.written_at) }).into_response());
                }
            }
            Err(SnapshotMiss::NotFound)
        }
        SnapshotQuery::Changes { root_id, query } => {
            find_root(&roots, root_id).ok_or(SnapshotMiss::NotFound)?;
            let file = load_root(store, computer_id, root_id).await.ok_or(SnapshotMiss::Unavailable)?;
            // A snapshot has no change history. At the snapshot's cursor there
            // is nothing new; any other cursor re-lists from the snapshot.
            Ok(Json(ChangesResponse {
                root_id: root_id.clone(),
                changes: Vec::new(),
                cursor: file.cursor.clone(),
                has_more: false,
                reset_required: query.cursor != file.cursor,
                freshness: freshness(&file.written_at),
            })
            .into_response())
        }
        SnapshotQuery::Search(request) => {
            let needle = request.query.trim().to_lowercase();
            let limit = request.limit.map(|l| l as usize).unwrap_or(SEARCH_DEFAULT).clamp(1, SEARCH_MAX);
            let offset: usize = request.page_token.as_deref().and_then(|t| t.parse().ok()).unwrap_or(0);
            let mut hits = Vec::new();
            let mut snapshot_at = manifest.written_at.clone();
            for root in &roots {
                if request.root_ids.as_ref().is_some_and(|ids| !ids.contains(&root.id)) {
                    continue;
                }
                let Some(file) = load_root(store, computer_id, &root.id).await else {
                    continue;
                };
                snapshot_at = snapshot_at.min(file.written_at.clone());
                for item in file.items.as_deref().unwrap_or_default() {
                    if needle.is_empty() || !item.name.to_lowercase().contains(&needle) {
                        continue;
                    }
                    if request.kind.is_some_and(|kind| kind != item.kind) {
                        continue;
                    }
                    if request.modified_after.as_ref().is_some_and(|after| item.modified_at <= *after) {
                        continue;
                    }
                    hits.push(SearchHit { item: item_from_snapshot(root, item), score: 1.0, snippet: None });
                }
            }
            let total = hits.len();
            let page: Vec<SearchHit> = hits.into_iter().skip(offset).take(limit).collect();
            let next = offset + page.len();
            Ok(Json(SearchResponse {
                hits: page,
                next_page_token: (next < total).then(|| next.to_string()),
                index_state: SearchResponseIndexState::NamesOnly,
                freshness: freshness(&snapshot_at),
            })
            .into_response())
        }
    }
}

/// Clean a manifest the runtime sent: its own computer only, protected roots
/// and local paths dropped. `Err` = refuse the upload.
pub(crate) fn clean_manifest(mut manifest: SnapshotManifest, computer_id: &str) -> Result<SnapshotManifest, &'static str> {
    if manifest.computer_id != computer_id {
        return Err("computer_id does not match the signing runtime");
    }
    if manifest.schema != "files-snapshot/1" {
        return Err("unknown snapshot schema");
    }
    if let Some(roots) = manifest.roots.take() {
        manifest.roots = Some(
            roots
                .into_iter()
                .filter(|root| !root.policy.protected)
                .map(|mut root| {
                    root.path_hint = None;
                    root
                })
                .collect(),
        );
    }
    Ok(manifest)
}

pub(crate) fn check_root_file(file: &SnapshotRootFile, root_id: &str) -> Result<(), &'static str> {
    if file.root_id != root_id {
        return Err("root_id does not match the path");
    }
    if file.schema != "files-snapshot-root/1" {
        return Err("unknown snapshot schema");
    }
    if root_id.is_empty() || root_id.len() > 128 || !root_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return Err("invalid root_id");
    }
    Ok(())
}

/// The error a sleeping computer answers when the snapshot cannot help.
pub(crate) fn miss_response(miss: SnapshotMiss, hint: allternit_files_core::WakeHint) -> Response {
    match miss {
        SnapshotMiss::NotFound => super::computer_files::files_error(
            StatusCode::NOT_FOUND,
            ErrorCode::NotFound,
            "Not in this computer's snapshot",
            Some(ComputerState::Asleep),
            None,
            None,
        ),
        SnapshotMiss::Unavailable => super::computer_files::files_error(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::Asleep,
            "The computer is asleep",
            Some(ComputerState::Asleep),
            Some(hint),
            None,
        ),
    }
}
