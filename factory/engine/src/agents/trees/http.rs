//! Trees over HTTP (proxied by allternit-api for Desktop, web and phone):
//!
//! * `GET  /api/factory/trees?path=&all=` → `{ trees, machine, overlaps }`
//! * `GET  /api/factory/trees/:id/checkpoints` → `{ checkpoints }` (newest first)
//! * `POST /api/factory/trees/:id/rewind` body `{ checkpoint }` → `{ tree, savedAs }`
//!
//! The app's caller is the person, so rewind needs no ownership flag here.
//! git and `ps` block, so every handler runs on `spawn_blocking`.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path as AxPath, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::api::service::ServiceState;

use super::config::TreesConfig;
use super::lease::LeaseStore;
use super::ops::{self, ListRequest};
use super::{checkpoint, TreeError};

pub fn route() -> Router<Arc<ServiceState>> {
    Router::new()
        .route("/api/factory/trees", get(list_h))
        .route("/api/factory/trees/:id/checkpoints", get(checkpoints_h))
        .route("/api/factory/trees/:id/rewind", post(rewind_h))
}

fn err(e: TreeError) -> Response {
    crate::api::factory::error(e.code, e.fact, e.action)
}

async fn blocking<T: serde::Serialize + Send + 'static>(f: impl FnOnce() -> Result<T, TreeError> + Send + 'static) -> Response {
    match tokio::task::spawn_blocking(f).await {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(e)) => err(e),
        Err(e) => err(TreeError::internal(e)),
    }
}

#[derive(Deserialize)]
struct ListQ {
    path: Option<String>,
    all: Option<bool>,
}

async fn list_h(State(state): State<Arc<ServiceState>>, Query(q): Query<ListQ>) -> Response {
    let root = state.root_dir.clone();
    blocking(move || {
        let path = q.path.filter(|p| !p.trim().is_empty()).map(PathBuf::from);
        ops::list(
            &LeaseStore::open_default(),
            ListRequest { repos: vec![root], path: path.as_deref(), all: q.all.unwrap_or(false), with_size: false },
        )
    })
    .await
}

async fn checkpoints_h(AxPath(id): AxPath<String>) -> Response {
    blocking(move || {
        let store = LeaseStore::open_default();
        let lease = store.get(&id)?.ok_or_else(|| TreeError::not_found(format!("no tree {id}"), "List trees with GET /api/factory/trees."))?;
        Ok(json!({ "checkpoints": checkpoint::list(&lease)? }))
    })
    .await
}

#[derive(Deserialize)]
struct RewindBody {
    checkpoint: u32,
}

async fn rewind_h(AxPath(id): AxPath<String>, Json(body): Json<RewindBody>) -> Response {
    blocking(move || {
        let store = LeaseStore::open_default();
        let lease = store.get(&id)?.ok_or_else(|| TreeError::not_found(format!("no tree {id}"), "List trees with GET /api/factory/trees."))?;
        let saved_as = checkpoint::rewind(&store, &lease, body.checkpoint)?;
        let lease = store.get(&id)?.unwrap_or(lease);
        let tree = ops::view(&lease, &TreesConfig::load(&lease.repo_path).unwrap_or_default(), false);
        Ok(json!({ "tree": tree, "savedAs": saved_as }))
    })
    .await
}
