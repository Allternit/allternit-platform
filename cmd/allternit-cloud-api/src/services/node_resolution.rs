//! Default data-plane node resolution for control-plane proxying (P1).
//!
//! A control-plane handler that serves node-stateful namespaces (agent
//! sessions, and later jobs/office/beta/rails) needs to answer one question
//! before it can relay: *which of the caller's registered nodes should run
//! this request?* This module is that decision, per the P1 route inventory
//! (docs/architecture/2026-09-04-p1-route-inventory.md §4): the default node
//! is the caller's **healthy registered device with the most recent
//! `last_seen`** — a local desktop, a paired box, and a provisioned
//! container are all `runtime_devices` rows and are treated uniformly.
//!
//! Health is judged from registry metadata alone: `status = 'online'`, a
//! `last_seen_at` fresher than the staleness window (env
//! `ALLTERNIT_NODE_STALE_AFTER_SECS`, default 600s — the same 10-minute
//! heartbeat grace `runtime_pairing::list_runtime_devices` uses), and an
//! unexpired device credential. Liveness via relay/tailnet probes is a later
//! tranche; v1 prefers a possibly-stale pick over refusing to route.
//!
//! When no healthy node exists the caller gets a deliberate 428
//! (`ApiError::PreconditionRequired`), not a 404: the URL is fine, the
//! account has no data plane yet — "pair a device".

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};

use crate::ApiError;

/// Env override for how fresh `last_seen_at` must be for a node to count as
/// healthy. Default 600s, matching Desktop's 45s heartbeat plus margin and
/// the pairing UI's online/offline flip.
const STALE_AFTER_SECS_ENV: &str = "ALLTERNIT_NODE_STALE_AFTER_SECS";
const DEFAULT_STALE_AFTER_SECS: u64 = 600;

fn staleness_window() -> Duration {
    std::env::var(STALE_AFTER_SECS_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(|seconds| Duration::seconds(seconds as i64))
        .unwrap_or(Duration::seconds(DEFAULT_STALE_AFTER_SECS as i64))
}

/// How a node is reached. Mirrors the `kind` CHECK on runtime_devices
/// (migrations_pg/012); stored as data, not enum-mapped, so a future kind
/// never breaks reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeKind(pub String);

impl NodeKind {
    pub const LOCAL: &'static str = "local";
    pub const PAIRED: &'static str = "paired";
    pub const PROVISIONED: &'static str = "provisioned";
}

/// The resolved default node: everything a control-plane handler needs to
/// address the node (today that is just the relay-addressed device id).
#[derive(Debug, Clone)]
pub struct ResolvedNode {
    pub device_id: String,
    pub name: String,
    pub kind: NodeKind,
    pub last_seen_at: Option<DateTime<Utc>>,
}

/// One registry candidate as returned by the store. Filtering/health rules
/// live in [`resolve_default_node`] so every store benefits uniformly.
#[derive(Debug, Clone)]
pub struct NodeCandidate {
    pub device_id: String,
    pub name: String,
    pub kind: String,
    pub status: String,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub credential_expires_at: DateTime<Utc>,
}

impl NodeCandidate {
    fn recency(&self) -> DateTime<Utc> {
        self.last_seen_at.unwrap_or(self.created_at)
    }
}

/// Store boundary for node resolution — the seam tests mock.
#[async_trait]
pub trait NodeStore: Send + Sync {
    /// All non-revoked devices owned by the user, any status/kind.
    async fn candidate_nodes(&self, user_id: &str) -> Result<Vec<NodeCandidate>, ApiError>;
}

/// Store backed by the control-plane Postgres (`runtime_devices`).
pub struct PgNodeStore<'a> {
    db: &'a sqlx::PgPool,
}

impl<'a> PgNodeStore<'a> {
    pub fn new(db: &'a sqlx::PgPool) -> Self {
        Self { db }
    }
}

#[async_trait]
impl NodeStore for PgNodeStore<'_> {
    async fn candidate_nodes(&self, user_id: &str) -> Result<Vec<NodeCandidate>, ApiError> {
        let rows = sqlx::query_as::<_, NodeCandidateRow>(
            r#"
            SELECT id, name, kind, status, last_seen_at, created_at,
                   credential_expires_at
            FROM runtime_devices
            WHERE user_id = $1 AND revoked_at IS NULL
            "#,
        )
        .bind(user_id)
        .fetch_all(self.db)
        .await?;
        Ok(rows.into_iter().map(NodeCandidateRow::into_candidate).collect())
    }
}

#[derive(sqlx::FromRow)]
struct NodeCandidateRow {
    id: String,
    name: String,
    kind: Option<String>,
    status: String,
    last_seen_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    credential_expires_at: DateTime<Utc>,
}

impl NodeCandidateRow {
    fn into_candidate(self) -> NodeCandidate {
        NodeCandidate {
            device_id: self.id,
            name: self.name,
            // Rows that predate migrations_pg/012 read as 'paired'.
            kind: self.kind.unwrap_or_else(|| NodeKind::PAIRED.to_string()),
            status: self.status,
            last_seen_at: self.last_seen_at,
            created_at: self.created_at,
            credential_expires_at: self.credential_expires_at,
        }
    }
}

/// Resolve the caller's default data-plane node: the healthy device with the
/// most recent activity, regardless of kind. All kinds are treated
/// uniformly — a provisioned container outranks a desktop only when its
/// `last_seen_at` is fresher.
///
/// Returns `ApiError::PreconditionRequired` (428, "pair a device") when the
/// user has no healthy node.
pub async fn resolve_default_node(
    store: &dyn NodeStore,
    user_id: &str,
) -> Result<ResolvedNode, ApiError> {
    resolve_default_node_preferring(store, user_id, &std::collections::HashSet::new()).await
}

/// [`resolve_default_node`], but a node holding a live relay connection
/// (`connected`) beats one that only heartbeats: a registration that
/// heartbeats without relaying would otherwise win on recency and answer
/// `runtime_offline`. Disconnected nodes stay eligible (hosted runtimes wake
/// on demand).
pub async fn resolve_default_node_preferring(
    store: &dyn NodeStore,
    user_id: &str,
    connected: &std::collections::HashSet<String>,
) -> Result<ResolvedNode, ApiError> {
    let candidates = store.candidate_nodes(user_id).await?;
    let stale_before = Utc::now() - staleness_window();

    let mut healthy: Vec<&NodeCandidate> = candidates
        .iter()
        .filter(|candidate| {
            candidate.status == "online"
                && candidate
                    .last_seen_at
                    .map(|seen| seen >= stale_before)
                    .unwrap_or(false)
                && candidate.credential_expires_at > Utc::now()
        })
        .collect();
    // Most recent activity wins; ties break on the id so the choice is
    // deterministic across calls.
    healthy.sort_by(|a, b| {
        connected
            .contains(&b.device_id)
            .cmp(&connected.contains(&a.device_id))
            .then_with(|| b.recency().cmp(&a.recency()))
            .then_with(|| a.device_id.cmp(&b.device_id))
    });

    let Some(node) = healthy.first() else {
        return Err(ApiError::PreconditionRequired(
            "No data-plane node registered for this account — pair a device (or start a hosted runtime) and try again".to_string(),
        ));
    };
    Ok(ResolvedNode {
        device_id: node.device_id.clone(),
        name: node.name.clone(),
        kind: NodeKind(node.kind.clone()),
        last_seen_at: node.last_seen_at,
    })
}

/// Node that stays up when the laptop sleeps: provisioned, then paired,
/// never `local`. Used for Cowork cloud continuation ingest.
pub async fn resolve_continuation_node(
    store: &dyn NodeStore,
    user_id: &str,
) -> Result<ResolvedNode, ApiError> {
    let candidates = store.candidate_nodes(user_id).await?;
    let stale_before = Utc::now() - staleness_window();
    let mut healthy: Vec<&NodeCandidate> = candidates
        .iter()
        .filter(|candidate| {
            candidate.kind != NodeKind::LOCAL
                && candidate.status == "online"
                && candidate
                    .last_seen_at
                    .map(|seen| seen >= stale_before)
                    .unwrap_or(false)
                && candidate.credential_expires_at > Utc::now()
        })
        .collect();
    healthy.sort_by(|a, b| {
        let rank = |kind: &str| match kind {
            NodeKind::PROVISIONED => 0,
            NodeKind::PAIRED => 1,
            _ => 2,
        };
        rank(&a.kind)
            .cmp(&rank(&b.kind))
            .then_with(|| b.recency().cmp(&a.recency()))
            .then_with(|| a.device_id.cmp(&b.device_id))
    });
    let Some(node) = healthy.first() else {
        return Err(ApiError::PreconditionRequired(
            "No always-on data-plane node for cloud continuation — start a hosted runtime or pair a box that stays online".to_string(),
        ));
    };
    Ok(ResolvedNode {
        device_id: node.device_id.clone(),
        name: node.name.clone(),
        kind: NodeKind(node.kind.clone()),
        last_seen_at: node.last_seen_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serializes the env-touching test against other env-touching tests.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn candidate(
        id: &str,
        kind: &str,
        status: &str,
        last_seen: Option<DateTime<Utc>>,
        credential_expires: DateTime<Utc>,
    ) -> NodeCandidate {
        NodeCandidate {
            device_id: id.to_string(),
            name: format!("node-{id}"),
            kind: kind.to_string(),
            status: status.to_string(),
            last_seen_at: last_seen,
            created_at: Utc::now() - Duration::days(30),
            credential_expires_at: credential_expires,
        }
    }

    fn online(id: &str, kind: &str, seen_secs_ago: i64) -> NodeCandidate {
        candidate(
            id,
            kind,
            "online",
            Some(Utc::now() - Duration::seconds(seen_secs_ago)),
            Utc::now() + Duration::days(30),
        )
    }

    struct MockStore {
        candidates: Vec<NodeCandidate>,
    }

    #[async_trait]
    impl NodeStore for MockStore {
        async fn candidate_nodes(&self, _user_id: &str) -> Result<Vec<NodeCandidate>, ApiError> {
            Ok(self.candidates.clone())
        }
    }

    fn precondition_message(error: &ApiError) -> String {
        match error {
            ApiError::PreconditionRequired(message) => message.clone(),
            other => panic!("expected PreconditionRequired, got {other}"),
        }
    }

    #[tokio::test]
    async fn no_devices_is_a_pair_a_device_precondition_error() {
        let store = MockStore { candidates: vec![] };
        let error = resolve_default_node(&store, "user_1").await.unwrap_err();
        assert!(
            matches!(error, ApiError::PreconditionRequired(_)),
            "no node must be a deliberate 428-style error, got {error}"
        );
        assert!(
            precondition_message(&error).contains("pair a device"),
            "error tells the caller what to do: {error}"
        );
    }

    #[tokio::test]
    async fn offline_revoked_expired_and_stale_devices_do_not_resolve() {
        let store = MockStore {
            candidates: vec![
                candidate(
                    "off",
                    NodeKind::PAIRED,
                    "offline",
                    Some(Utc::now()),
                    Utc::now() + Duration::days(30),
                ),
                candidate(
                    "stale",
                    NodeKind::PAIRED,
                    "online",
                    Some(Utc::now() - Duration::minutes(60)),
                    Utc::now() + Duration::days(30),
                ),
                candidate(
                    "expired-cred",
                    NodeKind::PAIRED,
                    "online",
                    Some(Utc::now()),
                    Utc::now() - Duration::days(1),
                ),
            ],
        };
        assert!(resolve_default_node(&store, "user_1").await.is_err());
    }

    #[tokio::test]
    async fn continuation_node_skips_local_and_prefers_provisioned() {
        let store = MockStore {
            candidates: vec![
                online("laptop", NodeKind::LOCAL, 1),
                online("box", NodeKind::PAIRED, 5),
                online("hosted", NodeKind::PROVISIONED, 30),
            ],
        };
        let node = resolve_continuation_node(&store, "user_1").await.unwrap();
        assert_eq!(node.device_id, "hosted");
        let store = MockStore {
            candidates: vec![online("laptop", NodeKind::LOCAL, 1)],
        };
        assert!(resolve_continuation_node(&store, "user_1").await.is_err());
    }

    #[tokio::test]
    async fn picks_most_recent_healthy_device_regardless_of_kind() {
        let store = MockStore {
            candidates: vec![
                online("provisioned-new", NodeKind::PROVISIONED, 10),
                online("local-mid", NodeKind::LOCAL, 30),
                online("paired-old", NodeKind::PAIRED, 90),
            ],
        };
        let node = resolve_default_node(&store, "user_1").await.unwrap();
        assert_eq!(node.device_id, "provisioned-new");
        assert_eq!(node.kind, NodeKind(NodeKind::PROVISIONED.to_string()));

        // Same rows, different recency: a local desktop wins when freshest.
        let store = MockStore {
            candidates: vec![
                online("provisioned-old", NodeKind::PROVISIONED, 90),
                online("local-new", NodeKind::LOCAL, 5),
            ],
        };
        let node = resolve_default_node(&store, "user_1").await.unwrap();
        assert_eq!(node.device_id, "local-new");
        assert_eq!(node.kind, NodeKind(NodeKind::LOCAL.to_string()));
    }

    #[tokio::test]
    async fn staleness_window_is_env_configurable() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var(STALE_AFTER_SECS_ENV, "3600");

        // 20 minutes unseen is stale at the default 120s but healthy here.
        let store = MockStore {
            candidates: vec![online("tolerant", NodeKind::PAIRED, 20 * 60)],
        };
        let node = resolve_default_node(&store, "user_1").await.unwrap();
        assert_eq!(node.device_id, "tolerant");

        std::env::remove_var(STALE_AFTER_SECS_ENV);
    }

    #[tokio::test]
    async fn relay_connected_node_beats_a_fresher_heartbeat_only_one() {
        // Live 2026-09-30: a heartbeat-only registration won on recency and
        // every relayed call answered runtime_offline.
        let store = MockStore {
            candidates: vec![
                online("heartbeat-only", NodeKind::PAIRED, 5),
                online("relaying", NodeKind::PAIRED, 60),
            ],
        };
        let connected = std::collections::HashSet::from(["relaying".to_string()]);
        let node = resolve_default_node_preferring(&store, "user_1", &connected).await.unwrap();
        assert_eq!(node.device_id, "relaying");
        // Without presence info, recency still decides.
        let node = resolve_default_node(&store, "user_1").await.unwrap();
        assert_eq!(node.device_id, "heartbeat-only");
    }

    #[tokio::test]
    async fn missing_last_seen_never_resolves() {
        let store = MockStore {
            candidates: vec![candidate(
                "never-seen",
                NodeKind::PAIRED,
                "online",
                None,
                Utc::now() + Duration::days(30),
            )],
        };
        assert!(resolve_default_node(&store, "user_1").await.is_err());
    }

    // ---- Live-PG coverage: PgNodeStore + the migrations_pg/012 shape ----
    // Follows the schema-per-test pattern from auth::resolve::tests.

    async fn test_pool() -> sqlx::PgPool {
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
        pool
    }

    /// Post-migrations_pg/012 runtime_devices shape (plus the pre-existing
    /// columns the resolver reads).
    async fn create_runtime_devices(pool: &sqlx::PgPool) {
        sqlx::query(
            r#"
            CREATE TABLE runtime_devices (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                name TEXT NOT NULL,
                kind TEXT NOT NULL DEFAULT 'paired',
                status TEXT NOT NULL DEFAULT 'offline',
                last_seen_at TIMESTAMPTZ,
                created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
                credential_expires_at TIMESTAMPTZ NOT NULL,
                revoked_at TIMESTAMPTZ
            )
            "#,
        )
        .execute(pool)
        .await
        .unwrap();
    }

    async fn insert_device(pool: &sqlx::PgPool, row: &NodeCandidate) {
        sqlx::query(
            r#"
            INSERT INTO runtime_devices (
                id, user_id, name, kind, status, last_seen_at,
                credential_expires_at, revoked_at
            ) VALUES ($1, 'user_1', $2, $3, $4, $5, $6, NULL)
            "#,
        )
        .bind(&row.device_id)
        .bind(&row.name)
        .bind(&row.kind)
        .bind(&row.status)
        .bind(row.last_seen_at)
        .bind(row.credential_expires_at)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn pg_store_resolves_default_node_and_skips_unhealthy() {
        let pool = test_pool().await;
        create_runtime_devices(&pool).await;
        insert_device(&pool, &online("rt_healthy", NodeKind::LOCAL, 15)).await;
        insert_device(&pool, &online("rt_stale", NodeKind::PROVISIONED, 3600)).await;
        sqlx::query(
            "UPDATE runtime_devices SET revoked_at = CURRENT_TIMESTAMP WHERE id = 'rt_stale'",
        )
        .execute(&pool)
        .await
        .unwrap();

        let store = PgNodeStore::new(&pool);
        let candidates = store.candidate_nodes("user_1").await.unwrap();
        assert_eq!(candidates.len(), 1, "revoked rows are excluded");

        let node = resolve_default_node(&store, "user_1").await.unwrap();
        assert_eq!(node.device_id, "rt_healthy");
        assert_eq!(node.kind, NodeKind(NodeKind::LOCAL.to_string()));

        let error = resolve_default_node(&store, "someone_else").await.unwrap_err();
        assert!(matches!(error, ApiError::PreconditionRequired(_)));
    }

    const MIGRATION_012: &str = include_str!("../../migrations_pg/012_data_plane_nodes.sql");
    const MIGRATION_013: &str = include_str!("../../migrations_pg/013_gizzi_instances_backfill.sql");

    /// Applies a migrations_pg file to a scratch schema (public. rewritten to
    /// the schema), statement by statement. sqlx migrations are up-only in
    /// this repo; there is no down-migration pattern to follow
    /// (tests/migrations.rs applies the SQLite lineage to a fresh DB only).
    async fn apply_migration_sql(pool: &sqlx::PgPool, schema: &str, sql: &str) {
        // Strip line comments before splitting: migration header prose
        // contains semicolons that would otherwise parse as statement ends.
        let without_comments = sql
            .lines()
            .map(|line| line.split_once("--").map(|(code, _)| code).unwrap_or(line))
            .collect::<Vec<_>>()
            .join("\n");
        let rewritten = without_comments.replace("public.", &format!("{schema}."));
        for statement in rewritten.split(';') {
            let statement = statement.trim();
            // The OWNER TO line needs superuser privileges the test role
            // lacks; ownership is an ops concern, not schema.
            if statement.is_empty() || statement.contains("OWNER TO") {
                continue;
            }
            sqlx::query(statement).execute(pool).await.unwrap();
        }
    }

    /// Applies migrations_pg/012 to a scratch schema — twice, proving the
    /// IF NOT EXISTS up statements are idempotent.
    async fn apply_migration_012(pool: &sqlx::PgPool, schema: &str) {
        apply_migration_sql(pool, schema, MIGRATION_012).await;
    }

    #[tokio::test]
    async fn migration_012_applies_idempotently() {
        let pool = test_pool().await;
        // Stub only what the migration touches: the pre-012 runtime_devices
        // (as created by the original pgloader import) and users (FK target).
        sqlx::query("CREATE TABLE users (id TEXT PRIMARY KEY)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            r#"
            CREATE TABLE runtime_devices (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL REFERENCES users(id),
                status TEXT NOT NULL DEFAULT 'offline',
                revoked_at TIMESTAMPTZ,
                last_seen_at TIMESTAMPTZ,
                created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
                credential_expires_at TIMESTAMPTZ NOT NULL
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        let schema: String =
            sqlx::query_scalar("SELECT current_schema()").fetch_one(&pool).await.unwrap();
        apply_migration_012(&pool, &schema).await;
        apply_migration_012(&pool, &schema).await;

        // Existing rows backfill to kind 'paired'; new kinds are accepted.
        sqlx::query(
            "INSERT INTO users (id) VALUES ('user_1'), ('user_2'), ('user_3')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            INSERT INTO runtime_devices (id, user_id, credential_expires_at)
            VALUES ('rt_legacy', 'user_1', CURRENT_TIMESTAMP + interval '30 days')
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let kind: String =
            sqlx::query_scalar("SELECT kind FROM runtime_devices WHERE id = 'rt_legacy'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(kind, "paired");

        sqlx::query(
            r#"
            INSERT INTO runtime_devices (id, user_id, kind, capacity, credential_expires_at)
            VALUES ('rt_local', 'user_2', 'local', '{"cores": 8}'::jsonb,
                    CURRENT_TIMESTAMP + interval '30 days')
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let capacity: serde_json::Value =
            sqlx::query_scalar("SELECT capacity FROM runtime_devices WHERE id = 'rt_local'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(capacity["cores"], 8);

        // Preference table round-trips.
        sqlx::query(
            "INSERT INTO user_node_preferences (user_id, surface, node_id) VALUES ('user_2', '*', 'rt_local')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let preferred: String = sqlx::query_scalar(
            "SELECT node_id FROM user_node_preferences WHERE user_id = 'user_2' AND surface = '*'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(preferred, "rt_local");
    }

    /// Post-migration-013 backfill coverage: every gizzi_instances row
    /// becomes a kind='local' runtime_devices row (derived id 'gi_' ||
    /// instance id), the INSERT...SELECT is idempotent under re-application,
    /// and the resolver treats backfilled rows like any other registry row.
    #[tokio::test]
    async fn migration_013_backfills_gizzi_instances_and_resolves() {
        let pool = test_pool().await;
        // Same pre-012 stubs as migration_012_applies_idempotently (users FK
        // target + pre-012 runtime_devices), plus the PG-shape
        // gizzi_instances from migrations_pg/001 (pgloader: all columns
        // nullable except the id PK).
        sqlx::query("CREATE TABLE users (id TEXT PRIMARY KEY)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            r#"
            CREATE TABLE runtime_devices (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL REFERENCES users(id),
                name TEXT NOT NULL,
                runtime_type TEXT,
                capabilities TEXT NOT NULL DEFAULT '[]',
                credential_hash TEXT,
                credential_expires_at TIMESTAMPTZ,
                status TEXT NOT NULL DEFAULT 'offline',
                last_seen_at TIMESTAMPTZ,
                created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
                revoked_at TIMESTAMPTZ
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("CREATE TABLE gizzi_instances (id TEXT PRIMARY KEY, user_id TEXT, name TEXT, url TEXT, created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP, updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP)")
            .execute(&pool)
            .await
            .unwrap();

        let schema: String =
            sqlx::query_scalar("SELECT current_schema()").fetch_one(&pool).await.unwrap();
        apply_migration_012(&pool, &schema).await;

        sqlx::query(
            "INSERT INTO users (id) VALUES ('user_1'), ('user_2')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            INSERT INTO gizzi_instances (id, user_id, name, url, created_at, updated_at)
            VALUES ('inst_fresh', 'user_1', 'default', 'https://fresh.trycloudflare.com',
                    CURRENT_TIMESTAMP, CURRENT_TIMESTAMP),
                   ('inst_stale', 'user_1', 'old-box', 'https://old.trycloudflare.com',
                    CURRENT_TIMESTAMP - interval '2 days',
                    CURRENT_TIMESTAMP - interval '2 days'),
                   ('inst_nourl', 'user_2', 'broken', NULL,
                    CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        // Twice: ON CONFLICT (id) DO NOTHING makes the backfill idempotent.
        apply_migration_sql(&pool, &schema, MIGRATION_013).await;
        apply_migration_sql(&pool, &schema, MIGRATION_013).await;

        let backfilled: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM runtime_devices WHERE id LIKE 'gi\\_inst%' ESCAPE '\\'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(backfilled, 2, "NULL-url instances must not backfill");

        let row: (String, String, String, String, String) = sqlx::query_as(
            r#"
            SELECT kind, endpoint_url, runtime_type, status, capabilities
            FROM runtime_devices WHERE id = 'gi_inst_fresh'
            "#,
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, NodeKind::LOCAL);
        assert_eq!(row.1, "https://fresh.trycloudflare.com");
        assert_eq!(row.2, "desktop");
        assert_eq!(row.3, "online");
        assert_eq!(row.4, "[]");

        // Credential-less rows must not expire out of the health check.
        let expires: DateTime<Utc> = sqlx::query_scalar(
            "SELECT credential_expires_at FROM runtime_devices WHERE id = 'gi_inst_fresh'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(expires > Utc::now() + Duration::days(365 * 50));

        // Re-application after rows gained state must not clobber them.
        sqlx::query("UPDATE runtime_devices SET status = 'offline' WHERE id = 'gi_inst_stale'")
            .execute(&pool)
            .await
            .unwrap();
        apply_migration_sql(&pool, &schema, MIGRATION_013).await;
        let stale_status: String = sqlx::query_scalar(
            "SELECT status FROM runtime_devices WHERE id = 'gi_inst_stale'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(stale_status, "offline", "DO NOTHING keeps later state");

        // Resolver: backfilled rows are ordinary runtime_devices rows — the
        // fresh instance resolves as the default node, the stale one alone
        // does not.
        let store = PgNodeStore::new(&pool);
        let node = resolve_default_node(&store, "user_1").await.unwrap();
        assert_eq!(node.device_id, "gi_inst_fresh");
        assert_eq!(node.kind, NodeKind(NodeKind::LOCAL.to_string()));

        sqlx::query("DELETE FROM runtime_devices WHERE id = 'gi_inst_fresh'")
            .execute(&pool)
            .await
            .unwrap();
        let error = resolve_default_node(&store, "user_1").await.unwrap_err();
        assert!(
            matches!(error, ApiError::PreconditionRequired(_)),
            "a stale backfilled node alone must not resolve: {error}"
        );
    }
}
