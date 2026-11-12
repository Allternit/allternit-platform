//! `POST /v1/decisions`, `GET /v1/decisions/{id}`, `PATCH /v1/decisions/{id}`
//! (scope `decisions`, meter `decision_questions`). Owned by **lane 1
//! (Decisions)** of the Platform API parity program.
//!
//! Seam stub: registers nothing yet. Lane 1 adds its routes inside `register`
//! (`table.add("/v1/decisions", &["POST"], post(create))...`); `mod.rs` already
//! calls it, so no lane edits `mod.rs`.

use super::RouteTable;

pub fn register(table: RouteTable) -> RouteTable {
    table
}
