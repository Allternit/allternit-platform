//! Terminal API routes on Windows.
//!
//! Code Mode terminals run as panes in the Allternit Factory pane engine, which
//! talks over a Unix socket (terminal_routes.rs, `#[cfg(unix)]`). This module
//! keeps the same names so the rest of allternit-api builds on Windows, and
//! answers every terminal request with a plain 501 instead.
use std::path::PathBuf;
use std::sync::Arc;

use axum::{http::StatusCode, response::IntoResponse, Json, Router};
use serde_json::json;

use crate::AppState;

/// The `allternit-factory` binary: `$ALLTERNIT_FACTORY_BIN`, else the one next
/// to this executable, else `PATH` (same order as on Unix).
pub(crate) fn factory_bin() -> PathBuf {
    if let Some(bin) = std::env::var_os("ALLTERNIT_FACTORY_BIN").filter(|v| !v.is_empty()) {
        return PathBuf::from(bin);
    }
    if let Some(sibling) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("allternit-factory.exe")))
        .filter(|p| p.is_file())
    {
        return sibling;
    }
    PathBuf::from("allternit-factory")
}

/// Placeholder so `AppState` has the same shape on every platform.
#[derive(Clone, Default)]
pub struct TerminalSessionStore;

impl TerminalSessionStore {
    pub fn new() -> Self {
        Self
    }
}

async fn unavailable() -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({
            "error": "terminals_unavailable",
            "message": "Terminals in Code mode need the Factory pane engine, which runs on macOS and Linux."
        })),
    )
}

pub fn terminal_router() -> Router<Arc<AppState>> {
    Router::new().fallback(unavailable)
}
