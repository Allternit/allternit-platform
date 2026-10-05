//! Workspace integration module
//!
//! Provides integration with the workspace service for terminal management.

pub mod client;

pub use client::{
    CreatePaneRequest, CreateSessionRequest, PaneMetadata, PaneResponse, SessionMetadata,
    SessionResponse, WorkspaceClient,
};
