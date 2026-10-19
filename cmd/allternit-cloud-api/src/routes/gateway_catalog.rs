//! `GET /api/v1/gateway/catalog`: the vendor packs and bot projects Settings →
//! Connections lists. The same file allternit-api serves on the user's
//! machine (`assets/gateway-catalog.json`), answered here so the web app
//! never depends on the runtime version of the computer it relays to.
//! Static display metadata only: connection state stays on the data plane.

use axum::{
    http::header,
    response::{IntoResponse, Response},
};

const CATALOG_JSON: &str = include_str!("../../../allternit-api/assets/gateway-catalog.json");

pub async fn catalog() -> Response {
    ([(header::CONTENT_TYPE, "application/json"), (header::CACHE_CONTROL, "public, max-age=300")], CATALOG_JSON).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_valid_json_with_packs_and_projects() {
        let v: serde_json::Value = serde_json::from_str(CATALOG_JSON).expect("valid JSON");
        assert!(v["packs"].as_array().is_some_and(|a| !a.is_empty()));
        assert!(v["botProjects"].as_array().is_some_and(|a| !a.is_empty()));
    }
}
