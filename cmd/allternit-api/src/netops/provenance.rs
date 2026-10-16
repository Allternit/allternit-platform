// owned by WP5 (stub from the WP1 scaffold: real signatures, no-op bodies).
//! Provenance (contract C4): a trust label on every input that reaches a
//! thread, and nonce fences around untrusted text. "Tainted" means below
//! [`Trust::Team`]. `provenance_json = {trust, sourceKind, sourceRef, excerpt, flags}`.
//! Stub until WP5 lands: everything is labelled `Owner` and nothing is fenced.

use std::sync::Arc;

use axum::Router;
use rusqlite::Connection;
use serde_json::{json, Value};

use crate::AppState;

/// Who an input came from, most trusted first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Trust {
    Owner,
    Team,
    Contact,
    VendorBot,
    ToolOutput,
    Web,
    Stranger,
}

impl Trust {
    pub fn as_str(self) -> &'static str {
        match self {
            Trust::Owner => "owner",
            Trust::Team => "team",
            Trust::Contact => "contact",
            Trust::VendorBot => "vendor_bot",
            Trust::ToolOutput => "tool_output",
            Trust::Web => "web",
            Trust::Stranger => "stranger",
        }
    }

    /// Below `Team`.
    pub fn tainted(self) -> bool {
        self > Trust::Team
    }
}

/// The label recorded for one input.
#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    pub trust: Trust,
    pub source_kind: String,
    pub source_ref: String,
    pub excerpt: String,
    pub flags: Vec<String>,
}

impl Label {
    /// `provenance_json`.
    pub fn to_json(&self) -> Value {
        json!({ "trust": self.trust.as_str(), "sourceKind": self.source_kind, "sourceRef": self.source_ref, "excerpt": self.excerpt, "flags": self.flags })
    }
}

/// Scan, record and label one input on `thread_id`. Stub: labels it `Owner`, records nothing.
pub fn label(_conn: &Connection, _thread_id: &str, _trust: Trust, source_kind: &str, source_ref: &str, text: &str) -> Label {
    Label { trust: Trust::Owner, source_kind: source_kind.to_string(), source_ref: source_ref.to_string(), excerpt: text.chars().take(200).collect(), flags: vec![] }
}

/// Wrap tainted text in a nonce fence. Stub: the text unchanged.
pub fn fence(text: &str, _label: &Label) -> String {
    text.to_string()
}

/// Stub: no routes.
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
}

/// Stub: nothing runs.
pub fn spawn(_state: Arc<AppState>) {}
