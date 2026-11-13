//! `GET /v1/server/info`: safe before sign-in, reveals no secrets.

use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use super::settings::SecretBackendKind;
use super::store::MAX_TOOL_RESULT_BYTES;
use super::{on_get, RouteBook, SelfHostState};

pub fn mount(book: RouteBook) -> RouteBook {
    book.add("/v1/server/info", vec![on_get(server_info)])
}

pub async fn server_info(State(st): State<SelfHostState>) -> Json<Value> {
    let s = &st.settings;
    let oidc = s.auth.oidc.is_some();
    let mut auth_modes = vec!["api_key"];
    if oidc {
        auth_modes.push("oidc");
    }
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "provider": s.inference.provider.as_str(),
        "features": {
            "web_tools": s.web_tools.enabled,
            "prometheus": s.observability.prometheus.enabled,
            "oidc": oidc,
            "vault_secrets": s.secrets.backend == SecretBackendKind::Vault,
            "postgres": st.store.backend() == "postgres",
        },
        "auth_modes": auth_modes,
        "public_url": s.server.public_url,
        "max_tool_result_bytes": MAX_TOOL_RESULT_BYTES,
    }))
}
