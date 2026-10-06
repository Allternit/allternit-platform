//! allternit-workspace-service
//!
//! Provides terminal session/pane management for cowork team workspaces.
//! Consumed by `rails-service/workspace/client.rs` at port 3021.
//!
//! Also hosts the in-process skills registry — a list of team skills available
//! for a given workspace, used by the gizzi bundled-skills loader.

pub mod sessions;
pub mod skills;
pub mod routes;
pub mod pane;

use axum::{routing::{get, post, delete}, Router};
use std::sync::Arc;
use tower_http::cors::CorsLayer;

pub use sessions::{SessionStore, SessionRecord, PaneRecord};
pub use skills::{SkillRegistry, SkillRecord};

/// Shared application state
#[derive(Clone)]
pub struct AppState {
    pub sessions: Arc<SessionStore>,
    pub skills: Arc<SkillRegistry>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(SessionStore::new()),
            skills: Arc::new(SkillRegistry::new()),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        // Health
        .route("/health", get(routes::health))
        // Sessions
        .route("/sessions", post(routes::create_session))
        .route("/sessions/:id", get(routes::get_session))
        .route("/sessions/:id", delete(routes::delete_session))
        // Panes
        .route("/sessions/:id/panes", post(routes::create_pane))
        .route("/panes/:id", delete(routes::delete_pane))
        .route("/panes/:id/capture", get(routes::capture_pane))
        .route("/panes/:id/send", post(routes::send_keys))
        .route("/panes/:id/logs", get(routes::stream_pane_logs))
        // Skills registry
        .route("/skills", get(routes::list_skills))
        .route("/skills", post(routes::register_skill))
        .route("/skills/:id", get(routes::get_skill))
        .route("/skills/:id", delete(routes::delete_skill))
        .layer(CorsLayer::permissive())
        .with_state(state)
}
