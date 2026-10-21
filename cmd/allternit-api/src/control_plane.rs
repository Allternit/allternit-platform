//! Control-plane / data-plane split for the LLM gateway (P2.12).
//!
//! One `allternit-api` binary, three roles, selected by env:
//!
//! - `ALLTERNIT_ROLE=all` (default) — today's behavior, byte-identical: data
//!   plane (the OpenAI-compatible `/v1/*` proxy surface) and control plane
//!   (the `/api/v1/gateway/*` admin + key-management routes) share the main
//!   listener, and this process owns the gateway background sweeps.
//! - `ALLTERNIT_ROLE=data` — main listener only, minus the control-plane
//!   routes (`gateway_admin_router` and `gateway_keys_router` 404), and the
//!   gateway background sweeps (BYOK route-credential revalidation, batch
//!   worker) are NOT started here.
//! - `ALLTERNIT_ROLE=control` — no data-plane listener at all; only the
//!   admin-plane listener is served, and this role owns the gateway
//!   background sweeps.
//!
//! `ALLTERNIT_ADMIN_LISTEN_ADDR` (e.g. `127.0.0.1:18099`) — when set, the
//! gateway control plane is ALSO served on this second listener (same
//! process) for roles that serve the control plane (`all`, `control`). For
//! `role=control` the admin listener is the ONLY listener; when the var is
//! unset in that role it defaults to [`DEFAULT_ADMIN_LISTEN_ADDR`]
//! (loopback-only, logged at boot). Setting it with `role=data` is a no-op
//! (warned and ignored) — the data plane never serves admin routes.
//!
//! Misconfiguration fails loudly at boot: an unknown `ALLTERNIT_ROLE` value
//! or an unparsable `ALLTERNIT_ADMIN_LISTEN_ADDR` aborts startup with a
//! clear message (`AdminPlane::from_env` error → `std::process::exit(1)` in
//! main.rs).
//!
//! ## Split-process pairing
//!
//! To run the roles as two processes, point both at the SAME
//! `ALLTERNIT_DATA_DIR` (same SQLite file) and set
//! `GATEWAY_SHARED_STATE=sqlite` on both: failover cooldowns and gateway
//! rate-limit counters then live in SQLite (V182 `gateway_shared_state`), so
//! the data-plane process steers/throttles against the state the
//! control-plane process observes and mutates. Example:
//!
//! ```sh
//! # terminal 1 — data plane
//! ALLTERNIT_ROLE=data ALLTERNIT_DATA_DIR=/var/lib/allternit \
//!   GATEWAY_SHARED_STATE=sqlite ALLTERNIT_API_PORT=18013 allternit-api
//! # terminal 2 — control plane
//! ALLTERNIT_ROLE=control ALLTERNIT_DATA_DIR=/var/lib/allternit \
//!   GATEWAY_SHARED_STATE=sqlite ALLTERNIT_ADMIN_LISTEN_ADDR=127.0.0.1:18099 \
//!   allternit-api
//! ```
//!
//! ## What stays shared
//!
//! Admin handlers take `State<Arc<AppState>>` — the same god-object the data
//! plane uses — so `main.rs` builds `AppState` identically in every role
//! (including VM drivers, cowork runtime, fabric schedulers a control-only
//! process never serves traffic for). Full state separation was judged
//! impractical for this pass; the split is at the listener + router +
//! background-task level only. `shared_state::init` also runs in every role:
//! the data plane needs the SQLite-backed cooldown store just as much as the
//! control plane does.

use axum::Router;
use std::net::SocketAddr;
use std::sync::Arc;

use crate::AppState;

/// Default admin-plane bind for `role=control` when
/// `ALLTERNIT_ADMIN_LISTEN_ADDR` is unset: loopback only, the workspace dev
/// admin port.
pub const DEFAULT_ADMIN_LISTEN_ADDR: &str = "127.0.0.1:18099";

/// Which plane(s) this process serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Data plane only: main listener without the gateway control-plane
    /// routes; no gateway background sweeps.
    Data,
    /// Control plane only: admin listener, no data-plane proxy routes; owns
    /// the gateway background sweeps.
    Control,
    /// Both planes (today's single-process behavior).
    All,
}

impl Role {
    /// Parse the `ALLTERNIT_ROLE` value. Unset/empty → `All`; anything else
    /// that is not exactly `data` / `control` / `all` is an error so a typo
    /// can never silently widen the served surface.
    pub fn parse(raw: Option<&str>) -> Result<Role, String> {
        match raw.map(str::trim).filter(|s| !s.is_empty()) {
            None => Ok(Role::All),
            Some("data") => Ok(Role::Data),
            Some("control") => Ok(Role::Control),
            Some("all") => Ok(Role::All),
            Some(other) => Err(format!(
                "invalid ALLTERNIT_ROLE={other:?}: expected one of \"data\", \"control\", \"all\""
            )),
        }
    }

    /// Whether this process serves the data plane on the main listener.
    pub fn serves_data_plane(self) -> bool {
        matches!(self, Role::Data | Role::All)
    }

    /// Whether this process serves the gateway control-plane routes
    /// (on the main listener for `all`, on the admin listener otherwise).
    pub fn serves_control_plane(self) -> bool {
        matches!(self, Role::Control | Role::All)
    }

    /// Whether this process owns the gateway background sweeps (BYOK
    /// route-credential revalidation, batch worker). In a split deployment
    /// exactly one process must own them; that is the control plane.
    pub fn owns_gateway_background_tasks(self) -> bool {
        matches!(self, Role::Control | Role::All)
    }
}

/// Resolved role + admin-listener configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdminPlane {
    pub role: Role,
    /// Address the admin-plane listener binds, when one should be served.
    /// `Some` whenever `ALLTERNIT_ADMIN_LISTEN_ADDR` is set and the role
    /// serves the control plane, and always `Some` for `role=control`
    /// (defaulting to [`DEFAULT_ADMIN_LISTEN_ADDR`]).
    pub admin_listen_addr: Option<SocketAddr>,
    /// True when the admin addr was defaulted rather than configured
    /// (control role without the env var) — main.rs logs this prominently.
    pub admin_addr_defaulted: bool,
    /// True when `ALLTERNIT_ADMIN_LISTEN_ADDR` was set but ignored because
    /// the role does not serve the control plane — main.rs warns.
    pub admin_addr_ignored: bool,
}

impl AdminPlane {
    /// Resolve role + admin-listener config from explicit values (the
    /// testable core; `from_env` is a thin wrapper).
    pub fn resolve(role_raw: Option<&str>, addr_raw: Option<&str>) -> Result<AdminPlane, String> {
        let role = Role::parse(role_raw)?;
        let addr = match addr_raw.map(str::trim).filter(|s| !s.is_empty()) {
            None => None,
            Some(s) => Some(s.parse::<SocketAddr>().map_err(|e| {
                format!("invalid ALLTERNIT_ADMIN_LISTEN_ADDR={s:?}: {e} (expected host:port)")
            })?),
        };
        Ok(match (role, addr) {
            (Role::Data, addr) => AdminPlane {
                role,
                admin_listen_addr: None,
                admin_addr_defaulted: false,
                admin_addr_ignored: addr.is_some(),
            },
            (Role::Control, None) => AdminPlane {
                role,
                admin_listen_addr: Some(
                    DEFAULT_ADMIN_LISTEN_ADDR
                        .parse()
                        .expect("DEFAULT_ADMIN_LISTEN_ADDR parses"),
                ),
                admin_addr_defaulted: true,
                admin_addr_ignored: false,
            },
            (role, addr) => AdminPlane {
                role,
                // `all` without the env var serves the control plane on the
                // main listener only (today's behavior).
                admin_listen_addr: addr,
                admin_addr_defaulted: false,
                admin_addr_ignored: false,
            },
        })
    }

    /// Resolve from `ALLTERNIT_ROLE` / `ALLTERNIT_ADMIN_LISTEN_ADDR`.
    pub fn from_env() -> Result<AdminPlane, String> {
        Self::resolve(
            std::env::var("ALLTERNIT_ROLE").ok().as_deref(),
            std::env::var("ALLTERNIT_ADMIN_LISTEN_ADDR").ok().as_deref(),
        )
    }
}

/// The gateway control-plane routes, ready to merge into the `/api/v1`
/// chain: admin observability/tenant config plus virtual-key management.
/// Both are Clerk-protected by the `auth_middleware` layer on the protected
/// router in main.rs (or by the layer inside [`gateway_admin_plane_app`]).
pub fn gateway_control_routes() -> Router<Arc<AppState>> {
    Router::new()
        .merge(crate::llm_gateway::gateway_keys_router())
        .merge(crate::llm_gateway::admin_routes::gateway_admin_router())
}

/// Merge the gateway control-plane routes into a `/api/v1` route chain when
/// the role serves the control plane; pass the chain through unchanged for
/// `role=data` (admin paths then 404 on the data plane).
pub fn with_gateway_control_routes(
    v1_routes: Router<Arc<AppState>>,
    role: Role,
) -> Router<Arc<AppState>> {
    if role.serves_control_plane() {
        v1_routes.merge(gateway_control_routes())
    } else {
        v1_routes
    }
}

/// Standalone admin-plane application for the second listener (state
/// applied): `/health/*` for probes plus the Clerk-protected
/// `/api/v1/gateway/*` control-plane routes. No data-plane proxy routes —
/// `/v1/*` 404s here.
pub fn gateway_admin_plane_app(state: Arc<AppState>) -> Router {
    let api = gateway_control_routes().layer(axum::middleware::from_fn_with_state(
        state.clone(),
        crate::auth::auth_middleware,
    ));
    Router::new()
        .nest("/health", crate::health::health_router())
        .nest("/api/v1", api)
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use rusqlite::params;
    use std::path::Path;
    use tower::ServiceExt;

    // ── config parsing / boot gating (pure) ──────────────────────────────

    #[test]
    fn role_defaults_to_all_when_unset() {
        assert_eq!(Role::parse(None).unwrap(), Role::All);
        assert_eq!(Role::parse(Some("")).unwrap(), Role::All);
        assert_eq!(Role::parse(Some("  ")).unwrap(), Role::All);
    }

    #[test]
    fn role_parses_each_value() {
        assert_eq!(Role::parse(Some("data")).unwrap(), Role::Data);
        assert_eq!(Role::parse(Some("control")).unwrap(), Role::Control);
        assert_eq!(Role::parse(Some("all")).unwrap(), Role::All);
    }

    #[test]
    fn role_rejects_unknown_values_loudly() {
        let err = Role::parse(Some("dataplane")).unwrap_err();
        assert!(err.contains("invalid ALLTERNIT_ROLE"), "{err}");
        assert!(Role::parse(Some("ALL")).is_err());
    }

    #[test]
    fn role_capabilities() {
        assert!(Role::Data.serves_data_plane());
        assert!(!Role::Data.serves_control_plane());
        assert!(!Role::Data.owns_gateway_background_tasks());
        assert!(!Role::Control.serves_data_plane());
        assert!(Role::Control.serves_control_plane());
        assert!(Role::Control.owns_gateway_background_tasks());
        assert!(Role::All.serves_data_plane());
        assert!(Role::All.serves_control_plane());
        assert!(Role::All.owns_gateway_background_tasks());
    }

    #[test]
    fn unset_admin_addr_unset_role_is_todays_behavior() {
        let plane = AdminPlane::resolve(None, None).unwrap();
        assert_eq!(plane.role, Role::All);
        assert_eq!(plane.admin_listen_addr, None);
        assert!(!plane.admin_addr_defaulted);
        assert!(!plane.admin_addr_ignored);
    }

    #[test]
    fn admin_addr_adds_second_listener_for_all_role() {
        let plane = AdminPlane::resolve(None, Some("127.0.0.1:18099")).unwrap();
        assert_eq!(plane.role, Role::All);
        assert_eq!(
            plane.admin_listen_addr,
            Some("127.0.0.1:18099".parse().unwrap())
        );
    }

    #[test]
    fn control_role_defaults_admin_addr() {
        let plane = AdminPlane::resolve(Some("control"), None).unwrap();
        assert_eq!(
            plane.admin_listen_addr,
            Some(DEFAULT_ADMIN_LISTEN_ADDR.parse().unwrap())
        );
        assert!(plane.admin_addr_defaulted);
    }

    #[test]
    fn control_role_uses_configured_admin_addr() {
        let plane = AdminPlane::resolve(Some("control"), Some("0.0.0.0:9090")).unwrap();
        assert_eq!(
            plane.admin_listen_addr,
            Some("0.0.0.0:9090".parse().unwrap())
        );
        assert!(!plane.admin_addr_defaulted);
    }

    #[test]
    fn data_role_ignores_admin_addr() {
        let plane = AdminPlane::resolve(Some("data"), Some("127.0.0.1:18099")).unwrap();
        assert_eq!(plane.admin_listen_addr, None);
        assert!(plane.admin_addr_ignored);
    }

    #[test]
    fn invalid_admin_addr_fails_loudly() {
        let err = AdminPlane::resolve(Some("control"), Some("not-an-addr")).unwrap_err();
        assert!(err.contains("invalid ALLTERNIT_ADMIN_LISTEN_ADDR"), "{err}");
    }

    // ── router composition per role ──────────────────────────────────────

    async fn test_app_state(temp: &Path) -> Arc<AppState> {
        let config = crate::AppConfig {
            company: Default::default(),
            user: Default::default(),
        };
        let db = crate::db::DbHandle::new(temp.join("test.db")).expect("test db");
        let conn = db.connect().expect("test db conn");
        conn.execute(
            "INSERT OR IGNORE INTO organizations (id, name) VALUES (?1, 'Test Org')",
            params!["org-1"],
        )
        .unwrap();
        drop(conn);
        let auth_config = crate::auth::AuthConfig::from_app_config(&config);
        let jwks = crate::auth::JwksManager::new(&auth_config);
        let rails = crate::rails::RailsState::new(temp.join("rails"))
            .await
            .expect("test rails");
        let desktop_host_registry =
            crate::desktop_host_registry::DesktopHostRegistry::new(db.clone());
        Arc::new(AppState {
            config,
            db: db.clone(),
            data_dir: temp.to_path_buf(),
            jwks,
            auth_config,
            rails,
            vm_driver: None,
            incus_driver: None,
            desktop_host_registry,
            desktop_host_provisioner: None,
            bot_desktop_sessions: Arc::new(tokio::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            computer_guest_tokens: Arc::new(tokio::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            vm_sessions: crate::vm_session_routes::new_vm_session_store(),
            cowork_scheduler: None,
            cowork_background: None,
            cowork_run_manager: None,
            webhook_secret: None,
            office_runtime: Arc::new(tokio::sync::RwLock::new(
                crate::office_routes::OfficeRuntimeFile::default(),
            )),
            office_cli_docs: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            office_cli_watches: Arc::new(
                tokio::sync::RwLock::new(std::collections::HashMap::new()),
            ),
            office_cli_mcp_sessions: Arc::new(tokio::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            design_skill_cache: crate::design_connector_routes::DesignSkillCache::new(),
            #[cfg(unix)]
            terminal_sessions: crate::terminal_routes::TerminalSessionStore::new(),
            mcp_dispatcher: crate::mcp_dispatcher::McpDispatcher::new(),
            approval_store: Arc::new(crate::permission_policy::ApprovalStore::new()),
            passkey_state: None,
            resource_class_catalog: crate::fabric::sku::ResourceClassCatalog::builtin(),
            fabric_node_provider:
                allternit_computer_cloud::providers::fabric_node::FabricNodeProvider::new(
                    std::sync::Arc::new(
                        allternit_computer_cloud::providers::fabric_node::FabricNodePool::new(),
                    ),
                    "__test__".to_string(),
                ),
            fabric_provider_registry:
                allternit_computer_cloud::fabric::FabricProviderRegistry::empty(),
            fabric_scheduler: crate::fabric::Scheduler::new(
                crate::fabric::CostEngine::default_engine(),
            ),
            fabric_price_cache: crate::fabric::PriceCache::new(db.clone()),
            os_control_plane: None,
            dp_jwks: crate::auth_dp_jwt::DataPlaneJwks::disabled(),
            deployment_scheduler: Arc::new(
                crate::deployment_scheduler::DeploymentSchedulerState::new(),
            ),
        })
    }

    /// A control-plane route exists when the handler runs at all. Without a
    /// Clerk session `auth_middleware` rejects with 401 — anything but 404
    /// proves the route is mounted.
    async fn get_status(app: Router, method: &str, uri: &str) -> StatusCode {
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        app.oneshot(req).await.unwrap().status()
    }

    #[tokio::test]
    async fn data_role_v1_chain_404s_admin_routes() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = test_app_state(&temp).await;
        let app = Router::new()
            .nest(
                "/api/v1",
                with_gateway_control_routes(Router::new(), Role::Data),
            )
            .with_state(state);
        assert_eq!(
            get_status(app, "GET", "/api/v1/gateway/usage").await,
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn all_role_v1_chain_serves_admin_routes() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = test_app_state(&temp).await;
        let app = Router::new()
            .nest(
                "/api/v1",
                with_gateway_control_routes(Router::new(), Role::All),
            )
            .with_state(state);
        // No auth layer here (main.rs applies it on the protected router);
        // the handler runs and rejects the missing AuthUser extension —
        // the point is the route is NOT a 404.
        assert_ne!(
            get_status(app, "GET", "/api/v1/gateway/usage").await,
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn admin_plane_app_serves_admin_and_404s_proxy() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = test_app_state(&temp).await;
        let app = gateway_admin_plane_app(state);

        // Admin route exists: auth middleware rejects the anonymous call
        // with 401 (route mounted, past routing).
        assert_eq!(
            get_status(app.clone(), "GET", "/api/v1/gateway/usage").await,
            StatusCode::UNAUTHORIZED
        );
        // Key management is on the control plane too.
        assert_eq!(
            get_status(app.clone(), "GET", "/api/v1/gateway/keys").await,
            StatusCode::UNAUTHORIZED
        );
        // Data-plane proxy routes are not served on the admin listener.
        assert_eq!(
            get_status(app.clone(), "POST", "/v1/chat/completions").await,
            StatusCode::NOT_FOUND
        );
        // Health probes work unauthenticated on the admin listener.
        assert_eq!(get_status(app, "GET", "/health/live").await, StatusCode::OK);
    }
}
