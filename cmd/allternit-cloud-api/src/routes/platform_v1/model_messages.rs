//! `POST /v1/messages`: the Anthropic Messages-shaped model route (scope
//! `inference`, ids `mdl_`). Owned by **lane 2 (Responses + Messages)** of the
//! Platform API parity program. SMS moved to `/v1/texts` ([`super::texts`]).
//!
//! Seam stub: registers nothing yet. Lane 2 adds `/v1/messages` inside `register`.

use super::RouteTable;

pub fn register(table: RouteTable) -> RouteTable {
    table
}
