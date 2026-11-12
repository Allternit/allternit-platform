//! `POST /v1/responses`, `GET /v1/responses/{id}` and friends (OpenAI
//! Responses shape; scope `inference` with `model`, `agents` with `agent`).
//! Owned by **lane 2 (Responses + Messages)** of the Platform API parity program.
//!
//! Seam stub: registers nothing yet. Lane 2 adds its routes inside `register`;
//! the transcript lives in [`super::items`] (frozen API) and pending approvals
//! come from [`super::approvals::pending_for_response`].

use super::RouteTable;

pub fn register(table: RouteTable) -> RouteTable {
    table
}
