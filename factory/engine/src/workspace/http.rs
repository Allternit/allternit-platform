//! Workspace reads and proof upload over HTTP (API.md §3 Workspace):
//!
//! * `GET  /api/factory/campaigns/:id/board` → `Board`
//! * `GET  /api/factory/nodes?assignee=<address>&status=open|all` → `{ nodes: NodeCard[] }`
//! * `GET  /api/factory/nodes/:dagId/:nodeId` → `NodePage`
//! * `POST /api/factory/nodes/:dagId/:nodeId/proof` (multipart `line`, `file`)
//!   → `{ path, receiptId, sha256 }`
//!
//! Reads are pure functions of the workspace root and the ledger
//! ([`board`], [`node_page`]). Proof goes through the Gate ([`proof::add`]):
//! the upload is saved to a temp file under its own (sanitized) name first.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Multipart, Path as AxPath, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::agents::http::error_response;
use crate::core::types::{AllternitEvent, LedgerQuery};
use crate::gate::Gate;
use crate::ledger::Ledger;
use crate::workspace::{board, node_page, proof};

/// Uploads larger than this are refused (proof files are evidence, not data dumps).
pub const MAX_PROOF_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone)]
struct WsState {
    root: PathBuf,
    ledger: Arc<Ledger>,
    gate: Arc<Gate>,
}

/// The workspace router over `root`, writing proof through `gate`.
pub fn router(root: PathBuf, ledger: Arc<Ledger>, gate: Arc<Gate>) -> Router {
    Router::new()
        .route("/api/factory/campaigns/:id/board", get(board_h))
        .route("/api/factory/nodes", get(nodes_h))
        .route("/api/factory/nodes/:dag/:node", get(node_h))
        .route("/api/factory/nodes/:dag/:node/proof", post(proof_h))
        .layer(axum::extract::DefaultBodyLimit::max(MAX_PROOF_BYTES + 64 * 1024))
        .with_state(WsState { root, ledger, gate })
}

/// `GET /api/factory/campaigns/:id/board` over a workspace.
pub async fn board(root: PathBuf, ledger: Arc<Ledger>, gate: Arc<Gate>, id: String) -> Response {
    board_h(State(WsState { root, ledger, gate }), AxPath(id)).await
}

/// The node folder's page (`NodePage`) as a value, or the error response.
pub async fn node_page_value(root: PathBuf, ledger: Arc<Ledger>, dag: &str, node: &str) -> Result<Value, Response> {
    let evs = ledger
        .query(LedgerQuery::default())
        .await
        .map_err(|e| error_response("internal", format!("reading the ledger failed: {e:#}"), "Check the workspace's .allternit/ledger.", Value::Null))?;
    let page = node_page::build(&root, &evs, dag, node).map_err(|e| {
        error_response("not_found", e.to_string(), "Check the dag and node ids (GET /api/factory/campaigns/:id/board).", Value::Null)
    })?;
    Ok(serde_json::to_value(page).unwrap_or_default())
}

/// `POST /api/factory/nodes/:dagId/:nodeId/proof` (multipart `line`, `file`).
pub async fn proof_upload(root: PathBuf, ledger: Arc<Ledger>, gate: Arc<Gate>, dag: String, node: String, form: Multipart) -> Response {
    proof_h(State(WsState { root, ledger, gate }), AxPath((dag, node)), form).await
}

async fn events(st: &WsState) -> Result<Vec<AllternitEvent>, Response> {
    st.ledger
        .query(LedgerQuery::default())
        .await
        .map_err(|e| error_response("internal", format!("reading the ledger failed: {e:#}"), "Check the workspace's .allternit/ledger.", Value::Null))
}

async fn board_h(State(st): State<WsState>, AxPath(id): AxPath<String>) -> Response {
    let evs = match events(&st).await {
        Ok(e) => e,
        Err(r) => return r,
    };
    match board::build(&st.root, &evs, &id) {
        Ok(b) => Json(b).into_response(),
        Err(e) => error_response("not_found", e.to_string(), "List campaigns with GET /api/factory/campaigns.", Value::Null),
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct NodesQuery {
    assignee: Option<String>,
    status: Option<String>,
}

async fn nodes_h(State(st): State<WsState>, Query(q): Query<NodesQuery>) -> Response {
    let Some(assignee) = q.assignee.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
        return error_response("usage", "assignee is required", "Pass ?assignee=<slug@team>.", Value::Null);
    };
    let open_only = match q.status.as_deref().unwrap_or("open") {
        "open" => true,
        "all" => false,
        other => return error_response("usage", format!("status must be open or all, not {other:?}"), "Pass ?status=open or ?status=all.", Value::Null),
    };
    let evs = match events(&st).await {
        Ok(e) => e,
        Err(r) => return r,
    };
    Json(json!({ "nodes": board::cards_for_assignee(&st.root, &evs, assignee, open_only) })).into_response()
}

async fn node_h(State(st): State<WsState>, AxPath((dag, node)): AxPath<(String, String)>) -> Response {
    let evs = match events(&st).await {
        Ok(e) => e,
        Err(r) => return r,
    };
    match node_page::build(&st.root, &evs, &dag, &node) {
        Ok(p) => Json(p).into_response(),
        Err(e) => error_response("not_found", e.to_string(), "Check the dag and node ids (GET /api/factory/campaigns/:id/board).", Value::Null),
    }
}

async fn proof_h(State(st): State<WsState>, AxPath((dag, node)): AxPath<(String, String)>, mut form: Multipart) -> Response {
    let mut line: Option<String> = None;
    let mut file: Option<(String, Vec<u8>)> = None;
    loop {
        match form.next_field().await {
            Ok(Some(field)) => {
                let name = field.name().unwrap_or("").to_string();
                let file_name = field.file_name().map(str::to_string);
                match field.bytes().await {
                    Ok(bytes) if name == "line" => line = Some(String::from_utf8_lossy(&bytes).trim().to_string()),
                    Ok(bytes) if name == "file" => {
                        if bytes.len() > MAX_PROOF_BYTES {
                            return error_response("usage", format!("proof file is larger than {MAX_PROOF_BYTES} bytes"), "Attach a smaller file or a summary of it.", Value::Null);
                        }
                        file = Some((file_name.unwrap_or_else(|| "file".into()), bytes.to_vec()));
                    }
                    Ok(_) => {}
                    Err(e) => return error_response("usage", format!("bad multipart body: {e}"), "Send multipart fields line and file.", Value::Null),
                }
            }
            Ok(None) => break,
            Err(e) => return error_response("usage", format!("bad multipart body: {e}"), "Send multipart fields line and file.", Value::Null),
        }
    }
    let (Some(line), Some((name, bytes))) = (line.filter(|l| !l.is_empty()), file) else {
        return error_response("usage", "proof needs both a line and a file", "Send multipart fields line (the Proof contract line or its number) and file.", Value::Null);
    };
    // Keep the uploaded name (sanitized again by proof add) in a private temp dir.
    let tmp = match tempfile::tempdir() {
        Ok(t) => t,
        Err(e) => return error_response("internal", format!("temp dir: {e}"), "Retry.", Value::Null),
    };
    let base = std::path::Path::new(&name).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
    let path = tmp.path().join(if base.is_empty() { "file".to_string() } else { base });
    if let Err(e) = std::fs::write(&path, &bytes) {
        return error_response("internal", format!("saving the upload failed: {e}"), "Retry.", Value::Null);
    }
    match proof::add(&st.gate, &dag, &node, &line, &path).await {
        Ok(added) => Json(added).into_response(),
        Err(e) => {
            let fact = format!("{e:#}");
            let code = if crate::gate::GateError::from_anyhow(&e).is_some() {
                "refused"
            } else if fact.contains("not found") {
                "not_found"
            } else {
                "usage"
            };
            error_response(code, fact, "Check the node and its SPEC.md Proof contract lines.", Value::Null)
        }
    }
}
