//! Mesh enrollment for first-party apps.
//!
//! A signed-in (Clerk) user hits `POST /api/v1/mesh/enroll` to mint a
//! Headscale preauth key so their device can join the tailnet. Each customer
//! maps to one Headscale user named `clerk-<clerk_user_id>` (sanitized to the
//! Headscale username charset); the key is single-use, non-ephemeral, and
//! expires after 24 hours. Nodes stay user-owned (no ACL tags) so the
//! tailnet's `autogroup:self` policy isolates each customer to their own
//! devices — see `infrastructure/mesh/headscale/policy.hujson`.
//!
//! The gRPC admin API is reached at `HEADSCALE_GRPC_ADDR`, authenticated
//! per-call with the Headscale API key as a bearer token in the
//! `authorization` metadata header.

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use chrono::{DateTime, Duration, Utc};
use std::sync::Arc;
use tokio::sync::Mutex;
use tonic::transport::Channel;

use crate::{routes::runtime_pairing, ApiError, ApiState};

/// Generated from the vendored Headscale v0.29.2 protos (`proto/`).
pub mod proto {
    tonic::include_proto!("headscale.v1");
}

use proto::headscale_service_client::HeadscaleServiceClient;

/// Default address of the Headscale gRPC admin API.
/// gRPC admin API on the same VPS as cloud-api (`mail.news.allternit.com`).
/// Not Fly. Public 50443 is closed; Headscale listens locally (see
/// infrastructure/vps-desktop-cloud/api.env.template).
const DEFAULT_GRPC_ADDR: &str = "http://127.0.0.1:50443";

/// Public control URL handed to clients for `tailscale up --login-server`.
/// Headscale is nginx-proxied on the mail VPS (phase14: listen 8081, TLS 8444).
const DEFAULT_CONTROL_URL: &str = "https://mail.news.allternit.com:8444";

/// Minted keys are single-use and short-lived; the app is expected to
/// enroll immediately.
const PREAUTH_KEY_TTL: Duration = Duration::hours(24);

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new().route("/api/v1/mesh/enroll", post(enroll))
}

/// Maps a Clerk user id to a valid Headscale username: `clerk-` prefix plus
/// the id forced to lowercase alphanumerics and dashes (Headscale usernames
/// must be valid host-ish labels, and Clerk ids contain underscores).
fn mesh_user_name(clerk_user_id: &str) -> String {
    let mut name = String::with_capacity("clerk-".len() + clerk_user_id.len());
    name.push_str("clerk-");
    let mut last_was_dash = true; // treat the prefix dash as a dash for collapsing
    for ch in clerk_user_id.chars() {
        let ch = ch.to_ascii_lowercase();
        let valid = if ch.is_ascii_alphanumeric() {
            Some(ch)
        } else if ch == '-' && !last_was_dash {
            Some('-')
        } else if !last_was_dash {
            // Any other character (underscore, dot, ...) becomes a dash.
            Some('-')
        } else {
            None
        };
        if let Some(ch) = valid {
            last_was_dash = ch == '-';
            name.push(ch);
        }
    }
    while name.ends_with('-') {
        name.pop();
    }
    name
}

/// An error from the Headscale admin API. Messages are already scrubbed of
/// credentials by the client before they reach the handler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeshError {
    /// The Headscale user already existed when we tried to create it.
    UserAlreadyExists,
    /// Any other gRPC/transport failure.
    Rpc(String),
}

impl std::fmt::Display for MeshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MeshError::UserAlreadyExists => write!(f, "headscale user already exists"),
            MeshError::Rpc(message) => write!(f, "headscale RPC failed: {message}"),
        }
    }
}

/// A minted preauth key ready to hand to the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enrollment {
    pub control_url: String,
    pub auth_key: String,
    pub expires_at: DateTime<Utc>,
    pub mesh_user: String,
}

/// The slice of the Headscale admin API enrollment needs, abstracted so the
/// orchestration is testable without a reachable gRPC server.
#[async_trait::async_trait]
pub trait HeadscaleAdmin: Send + Sync {
    /// Returns the id of the Headscale user with this exact name, if any.
    async fn find_user_id(&self, name: &str) -> Result<Option<u64>, MeshError>;
    /// Creates the Headscale user and returns its id.
    async fn create_user(&self, name: &str) -> Result<u64, MeshError>;
    /// Mints a single-use, non-ephemeral preauth key for the user.
    async fn create_preauth_key(
        &self,
        user_id: u64,
        expiration: DateTime<Utc>,
    ) -> Result<(String, DateTime<Utc>), MeshError>;
}

/// Enrollment against Headscale, built from env config at startup. Absent
/// when `HEADSCALE_API_KEY` is unset — the endpoint then answers 503.
pub struct MeshService {
    admin: Arc<dyn HeadscaleAdmin>,
    control_url: String,
}

impl MeshService {
    /// Reads `HEADSCALE_API_KEY` (required), `HEADSCALE_GRPC_ADDR` and
    /// `HEADSCALE_CONTROL_URL` (both defaulted) from the environment,
    /// following the crate's env-config convention. Returns `None` when the
    /// API key is missing or the gRPC address is invalid.
    pub fn from_env() -> Option<Arc<Self>> {
        let api_key = std::env::var("HEADSCALE_API_KEY")
            .ok()
            .filter(|key| !key.is_empty());
        let Some(api_key) = api_key else {
            tracing::warn!("HEADSCALE_API_KEY unset; mesh enrollment endpoint disabled");
            return None;
        };
        let grpc_addr =
            std::env::var("HEADSCALE_GRPC_ADDR").unwrap_or_else(|_| DEFAULT_GRPC_ADDR.to_string());
        let control_url = std::env::var("HEADSCALE_CONTROL_URL")
            .unwrap_or_else(|_| DEFAULT_CONTROL_URL.to_string());
        match GrpcHeadscaleAdmin::new(&grpc_addr, api_key) {
            Ok(admin) => {
                tracing::info!(grpc_addr, "Mesh enrollment service initialized");
                Some(Arc::new(Self {
                    admin: Arc::new(admin),
                    control_url,
                }))
            }
            Err(error) => {
                tracing::warn!("Failed to initialize mesh enrollment service: {}", error);
                None
            }
        }
    }

    /// A service over any admin backend (tests use a fake Headscale).
    pub fn with_admin(admin: Arc<dyn HeadscaleAdmin>, control_url: &str) -> Self {
        Self {
            admin,
            control_url: control_url.to_string(),
        }
    }

    /// Finds the customer's Headscale user, creating it on first enrollment.
    /// A concurrent enroll can win the create race, in which case we fall
    /// back to looking the user up again.
    async fn ensure_user_id(&self, name: &str) -> Result<u64, MeshError> {
        if let Some(id) = self.admin.find_user_id(name).await? {
            return Ok(id);
        }
        match self.admin.create_user(name).await {
            Ok(id) => Ok(id),
            Err(MeshError::UserAlreadyExists) => self
                .admin
                .find_user_id(name)
                .await?
                .ok_or(MeshError::Rpc(
                    "headscale user exists but could not be listed".to_string(),
                )),
            Err(error) => Err(error),
        }
    }

    /// Mints a preauth key for the given Clerk user id.
    pub async fn enroll(&self, clerk_user_id: &str) -> Result<Enrollment, MeshError> {
        let mesh_user = mesh_user_name(clerk_user_id);
        let user_id = self.ensure_user_id(&mesh_user).await?;
        let expires_at = Utc::now() + PREAUTH_KEY_TTL;
        let (auth_key, expires_at) = self.admin.create_preauth_key(user_id, expires_at).await?;
        Ok(Enrollment {
            control_url: self.control_url.clone(),
            auth_key,
            expires_at,
            mesh_user,
        })
    }
}

/// Headscale admin client. The API key travels as a bearer token in the
/// `authorization` metadata header on every call.
pub struct GrpcHeadscaleAdmin {
    client: Mutex<HeadscaleServiceClient<Channel>>,
    api_key: Arc<str>,
}

impl GrpcHeadscaleAdmin {
    pub fn new(grpc_addr: &str, api_key: String) -> Result<Self, MeshError> {
        // Lazy: the endpoint is reachable only from the control plane,
        // so a startup connect would fail anywhere else.
        let channel = Channel::from_shared(grpc_addr.to_string())
            .map_err(|error| MeshError::Rpc(format!("invalid HEADSCALE_GRPC_ADDR: {error}")))?
            .connect_lazy();
        Ok(Self {
            client: Mutex::new(HeadscaleServiceClient::new(channel)),
            api_key: api_key.into(),
        })
    }

    /// Attaches the API key bearer token to an outgoing request.
    fn authenticate<T>(&self, mut request: tonic::Request<T>) -> tonic::Request<T> {
        match format!("Bearer {}", self.api_key).parse() {
            Ok(value) => {
                request.metadata_mut().insert("authorization", value);
            }
            Err(_) => tracing::error!("HEADSCALE_API_KEY is not a valid gRPC metadata value"),
        }
        request
    }

    /// Maps a gRPC status to a MeshError, scrubbing the API key in case a
    /// server message ever echoes it back.
    fn scrub(&self, status: tonic::Status) -> MeshError {
        let message = status.message().replace(&*self.api_key, "[redacted]");
        if status.code() == tonic::Code::AlreadyExists {
            MeshError::UserAlreadyExists
        } else {
            MeshError::Rpc(message)
        }
    }
}

#[async_trait::async_trait]
impl HeadscaleAdmin for GrpcHeadscaleAdmin {
    async fn find_user_id(&self, name: &str) -> Result<Option<u64>, MeshError> {
        let request = self.authenticate(tonic::Request::new(proto::ListUsersRequest {
            id: 0,
            name: name.to_string(),
            email: String::new(),
        }));
        let response = self
            .client
            .lock()
            .await
            .list_users(request)
            .await
            .map_err(|status| self.scrub(status))?;
        Ok(response
            .into_inner()
            .users
            .into_iter()
            .find(|user| user.name == name)
            .map(|user| user.id))
    }

    async fn create_user(&self, name: &str) -> Result<u64, MeshError> {
        let request = self.authenticate(tonic::Request::new(proto::CreateUserRequest {
            name: name.to_string(),
            display_name: String::new(),
            email: String::new(),
            picture_url: String::new(),
        }));
        let response = self
            .client
            .lock()
            .await
            .create_user(request)
            .await
            .map_err(|status| self.scrub(status))?;
        response
            .into_inner()
            .user
            .map(|user| user.id)
            .ok_or_else(|| MeshError::Rpc("headscale returned no user".to_string()))
    }

    async fn create_preauth_key(
        &self,
        user_id: u64,
        expiration: DateTime<Utc>,
    ) -> Result<(String, DateTime<Utc>), MeshError> {
        // v0.29.2: CreatePreAuthKeyRequest { uint64 user = 1; bool reusable = 2;
        // bool ephemeral = 3; google.protobuf.Timestamp expiration = 4;
        // repeated string acl_tags = 5; }
        //
        // acl_tags deliberately stays empty: per-customer isolation is
        // enforced by the ACL policy (policy.hujson, autogroup:self) over
        // user-owned nodes — one headscale user per customer. Tagging the
        // nodes would strip their user ownership (user XOR tags), and in
        // v0.29.2 tagged nodes cannot match autogroup:self, while per-tag
        // ACL/tagOwners rules cannot be enumerated in a static policy file.
        // See infrastructure/mesh/headscale/policy.hujson for the full
        // rationale.
        let request = self.authenticate(tonic::Request::new(proto::CreatePreAuthKeyRequest {
            user: user_id,
            reusable: false,
            ephemeral: false,
            expiration: Some(prost_types::Timestamp {
                seconds: expiration.timestamp(),
                nanos: expiration.timestamp_subsec_nanos() as i32,
            }),
            acl_tags: vec![],
        }));
        let response = self
            .client
            .lock()
            .await
            .create_pre_auth_key(request)
            .await
            .map_err(|status| self.scrub(status))?;
        let key = response
            .into_inner()
            .pre_auth_key
            .ok_or_else(|| MeshError::Rpc("headscale returned no preauth key".to_string()))?;
        let expires_at = key
            .expiration
            .and_then(|ts| DateTime::from_timestamp(ts.seconds, ts.nanos.max(0) as u32))
            .unwrap_or(expiration);
        Ok((key.key, expires_at))
    }
}

fn mesh_not_configured_response() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({ "error": "mesh_not_configured" })),
    )
        .into_response()
}

fn mesh_upstream_error_response(error: &MeshError) -> Response {
    tracing::error!("Mesh enrollment failed: {}", error);
    (
        StatusCode::BAD_GATEWAY,
        Json(serde_json::json!({
            "error": "mesh_upstream_error",
            "message": error.to_string(),
        })),
    )
        .into_response()
}

/// Resolves who is enrolling: a paired runtime device token enrolls under the
/// device's owner (the owner's user id maps to the same per-customer Headscale
/// user, `clerk-<id>`); otherwise the Clerk session or an `allternit_*` API
/// token resolves to the user id.
/// Mirrors `gizzi_instances::actor_from_headers` — same trust decision, and
/// the call doubles as a lightweight device heartbeat.
async fn enroll_user_id(db: &sqlx::PgPool, headers: &HeaderMap) -> Result<String, ApiError> {
    if let Some(token) = runtime_pairing::device_token_from_headers(headers) {
        let device = runtime_pairing::runtime_device_for_token(db, token, None).await?;
        sqlx::query(
            "UPDATE runtime_devices SET status = 'online', last_seen_at = CURRENT_TIMESTAMP WHERE id = $1",
        )
        .bind(&device.id)
        .execute(db)
        .await?;
        return Ok(device.user_id);
    }
    crate::auth::resolve_user_scoped(db, headers, "compute")
        .await
        .map(|user| user.id)
}

async fn enroll(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Response {
    let user_id = match enroll_user_id(&state.db, &headers).await {
        Ok(user_id) => user_id,
        Err(error) => return error.into_response(),
    };
    let Some(mesh) = &state.mesh_service else {
        return mesh_not_configured_response();
    };
    match mesh.enroll(&user_id).await {
        Ok(enrollment) => Json(serde_json::json!({
            "controlUrl": enrollment.control_url,
            "authKey": enrollment.auth_key,
            "expiresAt": enrollment.expires_at.to_rfc3339(),
            "meshUser": enrollment.mesh_user,
        }))
        .into_response(),
        Err(error) => mesh_upstream_error_response(&error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[test]
    fn mesh_user_name_forces_valid_charset() {
        assert_eq!(mesh_user_name("user_2abcDEF"), "clerk-user-2abcdef");
        assert_eq!(mesh_user_name("user_X.y_Z"), "clerk-user-x-y-z");
        assert_eq!(mesh_user_name("already-valid-1"), "clerk-already-valid-1");
    }

    #[test]
    fn mesh_user_name_collapses_and_trims_dashes() {
        assert_eq!(mesh_user_name("a__b"), "clerk-a-b");
        assert_eq!(mesh_user_name("a_"), "clerk-a");
        assert_eq!(mesh_user_name("___"), "clerk");
    }

    struct MockHeadscaleAdmin {
        users: Mutex<Vec<(u64, String)>>,
        next_user_id: Mutex<u64>,
        create_user_results: Mutex<VecDeque<Result<u64, MeshError>>>,
        create_key_error: Mutex<Option<MeshError>>,
        create_user_calls: Mutex<u32>,
    }

    impl MockHeadscaleAdmin {
        fn new() -> Self {
            Self {
                users: Mutex::new(Vec::new()),
                next_user_id: Mutex::new(1),
                create_user_results: Mutex::new(VecDeque::new()),
                create_key_error: Mutex::new(None),
                create_user_calls: Mutex::new(0),
            }
        }

        fn with_user(id: u64, name: &str) -> Self {
            Self {
                users: Mutex::new(vec![(id, name.to_string())]),
                next_user_id: Mutex::new(id + 1),
                create_user_results: Mutex::new(VecDeque::new()),
                create_key_error: Mutex::new(None),
                create_user_calls: Mutex::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl HeadscaleAdmin for MockHeadscaleAdmin {
        async fn find_user_id(&self, name: &str) -> Result<Option<u64>, MeshError> {
            Ok(self
                .users
                .lock()
                .await
                .iter()
                .find(|(_, user_name)| user_name == name)
                .map(|(id, _)| *id))
        }

        async fn create_user(&self, name: &str) -> Result<u64, MeshError> {
            *self.create_user_calls.lock().await += 1;
            if let Some(result) = self.create_user_results.lock().await.pop_front() {
                return result;
            }
            let mut next_id = self.next_user_id.lock().await;
            let id = *next_id;
            *next_id += 1;
            self.users.lock().await.push((id, name.to_string()));
            Ok(id)
        }

        async fn create_preauth_key(
            &self,
            user_id: u64,
            expiration: DateTime<Utc>,
        ) -> Result<(String, DateTime<Utc>), MeshError> {
            if let Some(error) = self.create_key_error.lock().await.take() {
                return Err(error);
            }
            assert!(user_id > 0, "key must be minted for a DOUBLE PRECISION user id");
            Ok(("hskey-auth-testkey".to_string(), expiration))
        }
    }

    #[tokio::test]
    async fn enroll_reuses_existing_user_without_creating() {
        let admin = Arc::new(MockHeadscaleAdmin::with_user(7, "clerk-user-1"));
        let service = MeshService::with_admin(admin.clone(), DEFAULT_CONTROL_URL);

        let enrollment = service.enroll("user_1").await.unwrap();
        assert_eq!(enrollment.mesh_user, "clerk-user-1");
        assert_eq!(enrollment.auth_key, "hskey-auth-testkey");
        assert_eq!(enrollment.control_url, DEFAULT_CONTROL_URL);
        assert_eq!(*admin.create_user_calls.lock().await, 0);
    }

    #[tokio::test]
    async fn enroll_creates_missing_user_then_mints_key() {
        let admin = Arc::new(MockHeadscaleAdmin::new());
        let service = MeshService::with_admin(admin.clone(), DEFAULT_CONTROL_URL);

        let before = Utc::now();
        let enrollment = service.enroll("user_2").await.unwrap();
        assert_eq!(enrollment.mesh_user, "clerk-user-2");
        assert_eq!(*admin.create_user_calls.lock().await, 1);
        assert!(
            enrollment.expires_at >= before + Duration::hours(24)
                && enrollment.expires_at <= Utc::now() + Duration::hours(24),
            "key expires ~24h out"
        );
    }

    #[tokio::test]
    async fn enroll_recovers_from_concurrent_user_create() {
        let admin = Arc::new(MockHeadscaleAdmin::new());
        // Simulate losing the create race: CreateUser reports AlreadyExists
        // and the user appears on the follow-up lookup.
        admin
            .create_user_results
            .lock()
            .await
            .push_back(Err(MeshError::UserAlreadyExists));
        admin.users.lock().await.push((9, "clerk-user-3".to_string()));
        let service = MeshService::with_admin(admin, DEFAULT_CONTROL_URL);

        let enrollment = service.enroll("user_3").await.unwrap();
        assert_eq!(enrollment.auth_key, "hskey-auth-testkey");
    }

    #[tokio::test]
    async fn enroll_propagates_key_creation_failure() {
        let admin = Arc::new(MockHeadscaleAdmin::new());
        *admin.create_key_error.lock().await =
            Some(MeshError::Rpc("unavailable".to_string()));
        let service = MeshService::with_admin(admin, DEFAULT_CONTROL_URL);

        let error = service.enroll("user_4").await.unwrap_err();
        assert_eq!(error, MeshError::Rpc("unavailable".to_string()));
    }

    #[tokio::test]
    async fn not_configured_maps_to_503_and_upstream_failure_to_502() {
        let response = mesh_not_configured_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body.as_ref(), br#"{"error":"mesh_not_configured"}"#);

        let response = mesh_upstream_error_response(&MeshError::Rpc("broken".to_string()));
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn grpc_error_messages_are_scrubbed_of_the_api_key() {
        let admin = GrpcHeadscaleAdmin::new(DEFAULT_GRPC_ADDR, "hsapi-secret".to_string()).unwrap();
        let status = tonic::Status::new(tonic::Code::Unknown, "bad token hsapi-secret rejected");
        assert_eq!(
            admin.scrub(status),
            MeshError::Rpc("bad token [redacted] rejected".to_string())
        );
        let status = tonic::Status::new(tonic::Code::AlreadyExists, "hsapi-secret");
        assert_eq!(admin.scrub(status), MeshError::UserAlreadyExists);
    }

    #[tokio::test]
    async fn authenticate_sets_bearer_metadata() {
        let admin = GrpcHeadscaleAdmin::new(DEFAULT_GRPC_ADDR, "hsapi-secret".to_string()).unwrap();
        let request = admin.authenticate(tonic::Request::new(()));
        assert_eq!(
            request.metadata().get("authorization").unwrap(),
            "Bearer hsapi-secret"
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn from_env_is_disabled_without_api_key() {
        std::env::remove_var("HEADSCALE_API_KEY");
        assert!(MeshService::from_env().is_none());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn from_env_builds_service_with_defaults() {
        std::env::set_var("HEADSCALE_API_KEY", "hsapi-secret");
        std::env::remove_var("HEADSCALE_GRPC_ADDR");
        std::env::remove_var("HEADSCALE_CONTROL_URL");
        let service = MeshService::from_env().expect("service should build with an API key");
        assert_eq!(service.control_url, DEFAULT_CONTROL_URL);
        std::env::remove_var("HEADSCALE_API_KEY");
    }

    /// Minimal runtime_devices shape for the device-token auth path (mirrors
    /// the gizzi_instances dual-auth tests, including the migration-022
    /// rotation-grace columns runtime_device_for_token falls back to).
    async fn device_test_pool() -> sqlx::PgPool {
        let url = "postgres://allternit:allternit_pg_2026@localhost:5432/allternit_test";
        let schema = format!("test_{}", uuid::Uuid::new_v4().simple());
        let schema_for_hook = schema.clone();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .after_connect(move |conn, _meta| {
                let schema = schema_for_hook.clone();
                Box::pin(async move {
                    sqlx::query(&format!("CREATE SCHEMA IF NOT EXISTS {}", schema))
                        .execute(&mut *conn)
                        .await?;
                    sqlx::query(&format!("SET search_path TO {}", schema))
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(url)
            .await
            .unwrap();
        sqlx::query("DROP TABLE IF EXISTS runtime_devices CASCADE").execute(&pool).await.unwrap();
        sqlx::query(r#"
        CREATE TABLE runtime_devices (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                name TEXT NOT NULL,
                credential_hash TEXT NOT NULL UNIQUE,
                credential_expires_at TIMESTAMPTZ NOT NULL,
                previous_credential_hash TEXT,
                previous_credential_expires_at TIMESTAMPTZ,
                status TEXT NOT NULL DEFAULT 'offline',
                last_seen_at TIMESTAMPTZ,
                revoked_at TIMESTAMPTZ
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn device_headers(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        headers
    }

    async fn insert_device(pool: &sqlx::PgPool, token: &str, revoked: bool) {
        sqlx::query(
            r#"
            INSERT INTO runtime_devices (
                id, user_id, name, credential_hash, credential_expires_at, status, revoked_at
            )
            VALUES ('rd_1', 'user_9', 'byo-vps-1', $1, $2, 'offline', $3)
            "#,
        )
        .bind(runtime_pairing::sha256_hex(token.as_bytes()))
        .bind(Utc::now() + Duration::days(1))
        .bind(if revoked { Some(Utc::now()) } else { None })
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn device_token_enrolls_against_owners_mesh_user() {
        let pool = device_test_pool().await;
        let token = format!("{}testsecret", runtime_pairing::DEVICE_TOKEN_PREFIX);
        insert_device(&pool, &token, false).await;

        // The device token resolves to the device's owner; enrollment mints
        // the key against the owner's per-customer mesh user (clerk-<id>).
        let user_id = enroll_user_id(&pool, &device_headers(&token)).await.unwrap();
        assert_eq!(user_id, "user_9");

        let admin = Arc::new(MockHeadscaleAdmin::new());
        let service = MeshService::with_admin(admin.clone(), DEFAULT_CONTROL_URL);
        let enrollment = service.enroll(&user_id).await.unwrap();
        assert_eq!(enrollment.mesh_user, "clerk-user-9");

        // The enroll call doubles as a lightweight device heartbeat.
        let (status, last_seen_at): (String, Option<DateTime<Utc>>) = sqlx::query_as(
            "SELECT status, last_seen_at FROM runtime_devices WHERE id = 'rd_1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(status, "online");
        assert!(last_seen_at.unwrap() > Utc::now() - Duration::minutes(1));
    }

    #[tokio::test]
    async fn revoked_device_token_cannot_enroll() {
        let pool = device_test_pool().await;
        let token = format!("{}testsecret", runtime_pairing::DEVICE_TOKEN_PREFIX);
        insert_device(&pool, &token, true).await;

        let error = enroll_user_id(&pool, &device_headers(&token))
            .await
            .unwrap_err();
        assert!(
            matches!(error, ApiError::Unauthorized(_)),
            "revoked device credential must be a 401, got {error:?}"
        );
    }
}
