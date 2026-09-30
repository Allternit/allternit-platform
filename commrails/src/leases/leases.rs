use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use chrono::{Duration, Utc};
use serde_json::json;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    Pool, Row, Sqlite,
};
use tokio::sync::RwLock;
use tokio::time::{self};

use crate::core::ids::create_event_id;
use crate::core::io::ensure_dir;
use crate::core::types::{AllternitEvent, Actor, ActorType, LeaseRecord, LeaseRequest};
use crate::core::EventSink;

#[derive(Clone)]
pub struct LeasesOptions {
    pub root_dir: Option<PathBuf>,
    pub leases_dir: Option<PathBuf>,
    pub event_sink: Option<Arc<dyn EventSink>>,
    pub actor_id: Option<String>,
    pub auto_renewal_enabled: bool,
    pub auto_renewal_threshold_seconds: u64,  // Renew when lease has this many seconds remaining
    pub auto_renewal_interval_seconds: u64,   // Check for renewals every N seconds
    pub auto_renewal_extend_seconds: u64,     // Extend lease by this many seconds
}

impl Default for LeasesOptions {
    fn default() -> Self {
        Self {
            root_dir: None,
            leases_dir: None,
            event_sink: None,
            actor_id: None,
            auto_renewal_enabled: true,
            auto_renewal_threshold_seconds: 300,  // 5 minutes
            auto_renewal_interval_seconds: 60,    // Check every minute
            auto_renewal_extend_seconds: 600,     // Extend by 10 minutes
        }
    }
}

pub struct Leases {
    pool: Pool<Sqlite>,
    event_sink: Option<Arc<dyn EventSink>>,
    actor_id: String,
    auto_renewal_enabled: bool,
    auto_renewal_threshold_seconds: u64,
    auto_renewal_extend_seconds: u64,
    auto_renewal_handle: Arc<RwLock<Option<tokio::task::JoinHandle<()>>>>,
}

impl Leases {
    pub async fn new(opts: LeasesOptions) -> Result<Self> {
        let root_dir = opts
            .root_dir
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")));
        let leases_dir = opts
            .leases_dir
            .unwrap_or_else(|| PathBuf::from(".allternit/leases"));
        let base = if leases_dir.is_absolute() {
            leases_dir
        } else {
            root_dir.join(leases_dir)
        };
        ensure_dir(&base)?;
        let db_path = base.join("leases.db");
        let connect_opts = SqliteConnectOptions::new()
            .filename(&db_path)
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(connect_opts)
            .await?;
        let leases = Self {
            pool,
            event_sink: opts.event_sink,
            actor_id: opts.actor_id.unwrap_or_else(|| "gate".to_string()),
            auto_renewal_enabled: opts.auto_renewal_enabled,
            auto_renewal_threshold_seconds: opts.auto_renewal_threshold_seconds,
            auto_renewal_extend_seconds: opts.auto_renewal_extend_seconds,
            auto_renewal_handle: Arc::new(RwLock::new(None)),
        };
        leases.ensure_schema().await?;
        
        // Start auto-renewal if enabled
        if opts.auto_renewal_enabled {
            leases.start_auto_renewal(opts.auto_renewal_interval_seconds).await;
        }
        
        Ok(leases)
    }

    /// Start background auto-renewal task
    pub async fn start_auto_renewal(&self, interval_seconds: u64) {
        if !self.auto_renewal_enabled {
            return;
        }

        let pool = self.pool.clone();
        let event_sink = self.event_sink.clone();
        let actor_id = self.actor_id.clone();
        let threshold_seconds = self.auto_renewal_threshold_seconds;
        let extend_seconds = self.auto_renewal_extend_seconds;
        let handle = self.auto_renewal_handle.clone();

        let task = tokio::spawn(async move {
            let mut interval = time::interval(tokio::time::Duration::from_secs(interval_seconds));
            
            loop {
                interval.tick().await;
                
                // Find leases that need renewal
                let expiring_leases = sqlx::query(
                    "SELECT lease_id, wih_id, granted_until FROM leases 
                     WHERE status = 'granted' 
                     AND granted_until <= datetime('now', ?)"
                )
                .bind(&format!("+{} seconds", threshold_seconds))
                .fetch_all(&pool)
                .await;

                if let Ok(rows) = expiring_leases {
                    for row in rows {
                        let lease_id: String = row.get("lease_id");
                        let wih_id: String = row.get("wih_id");
                        let _granted_until: String = row.get("granted_until");
                        
                        // Calculate new expiry
                        let new_expiry = Utc::now() + Duration::seconds(extend_seconds as i64);
                        let new_expiry_str = new_expiry.to_rfc3339();
                        
                        // Renew the lease
                        let result = sqlx::query(
                            "UPDATE leases SET status = ?, granted_until = ? WHERE lease_id = ?"
                        )
                        .bind("granted")
                        .bind(&new_expiry_str)
                        .bind(&lease_id)
                        .execute(&pool)
                        .await;

                        if let Ok(_) = result {
                            tracing::info!("Auto-renewed lease {} for WIH {}", lease_id, wih_id);
                            
                            // Emit renewal event if event sink available
                            if let Some(sink) = &event_sink {
                                let event = AllternitEvent {
                                    event_id: create_event_id(),
                                    ts: Utc::now().to_rfc3339(),
                                    actor: Actor {
                                        r#type: ActorType::Gate,
                                        id: actor_id.clone(),
                                    },
                                    r#type: "LeaseAutoRenewed".to_string(),
                                    payload: json!({
                                        "lease_id": lease_id,
                                        "wih_id": wih_id,
                                        "granted_until": new_expiry_str,
                                        "extend_seconds": extend_seconds,
                                    }),
                                    provenance: None,
                                    scope: Default::default(),
                                };
                                let _ = sink.append(event).await;
                            }
                        }
                    }
                }
            }
        });

        let mut handle_guard = handle.write().await;
        *handle_guard = Some(task);
    }

    /// Stop auto-renewal task
    pub async fn stop_auto_renewal(&self) {
        let mut handle_guard = self.auto_renewal_handle.write().await;
        if let Some(handle) = handle_guard.take() {
            handle.abort();
            tracing::info!("Auto-renewal task stopped");
        }
    }

    pub async fn request(&self, req: LeaseRequest) -> Result<()> {
        let paths_json = serde_json::to_string(&req.paths)?;
        sqlx::query(
            "INSERT INTO leases (lease_id, wih_id, agent_id, paths, requested_at, ttl_seconds, status)
             VALUES (?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(&req.lease_id)
        .bind(&req.wih_id)
        .bind(&req.agent_id)
        .bind(paths_json)
        .bind(&req.requested_at)
        .bind(req.ttl_seconds)
        .bind("requested")
        .execute(&self.pool)
        .await?;

        self.emit(
            "LeaseRequested",
            json!({
                "lease_id": req.lease_id,
                "wih_id": req.wih_id,
                "agent_id": req.agent_id,
                "paths": req.paths,
                "requested_at": req.requested_at,
                "ttl_seconds": req.ttl_seconds
            }),
        )
        .await?;
        Ok(())
    }

    pub async fn grant(&self, lease_id: &str, until_iso: &str) -> Result<()> {
        sqlx::query("UPDATE leases SET status = ?, granted_until = ?, denied_reason = NULL WHERE lease_id = ?")
            .bind("granted")
            .bind(until_iso)
            .bind(lease_id)
            .execute(&self.pool)
            .await?;

        self.emit(
            "LeaseGranted",
            json!({ "lease_id": lease_id, "granted_until": until_iso }),
        )
        .await?;
        Ok(())
    }

    pub async fn deny(&self, lease_id: &str, reason: &str) -> Result<()> {
        sqlx::query("UPDATE leases SET status = ?, denied_reason = ? WHERE lease_id = ?")
            .bind("denied")
            .bind(reason)
            .bind(lease_id)
            .execute(&self.pool)
            .await?;

        self.emit(
            "LeaseDenied",
            json!({ "lease_id": lease_id, "reason": reason }),
        )
        .await?;
        Ok(())
    }

    pub async fn renew(&self, lease_id: &str, until_iso: &str) -> Result<()> {
        sqlx::query("UPDATE leases SET status = ?, granted_until = ? WHERE lease_id = ?")
            .bind("granted")
            .bind(until_iso)
            .bind(lease_id)
            .execute(&self.pool)
            .await?;

        self.emit(
            "LeaseRenewed",
            json!({ "lease_id": lease_id, "granted_until": until_iso }),
        )
        .await?;
        Ok(())
    }

    pub async fn release(&self, lease_id: &str) -> Result<()> {
        sqlx::query("UPDATE leases SET status = ? WHERE lease_id = ?")
            .bind("released")
            .bind(lease_id)
            .execute(&self.pool)
            .await?;

        self.emit("LeaseReleased", json!({ "lease_id": lease_id }))
            .await?;
        Ok(())
    }

    pub async fn release_for_wih(&self, wih_id: &str) -> Result<Vec<String>> {
        let rows = sqlx::query(
            "SELECT lease_id
             FROM leases
             WHERE wih_id = ? AND status IN ('granted', 'requested')",
        )
        .bind(wih_id)
        .fetch_all(&self.pool)
        .await?;

        let mut released = Vec::new();
        for row in rows {
            let lease_id: String = row.try_get("lease_id")?;
            self.release(&lease_id).await?;
            released.push(lease_id);
        }
        Ok(released)
    }

    pub async fn active_paths_for_wih(&self, wih_id: &str) -> Result<Vec<String>> {
        let rows = sqlx::query(
            "SELECT paths
             FROM leases
             WHERE wih_id = ? AND status IN ('granted', 'requested')",
        )
        .bind(wih_id)
        .fetch_all(&self.pool)
        .await?;

        let mut paths = Vec::new();
        for row in rows {
            let paths_raw: String = row.try_get("paths")?;
            let mut entry: Vec<String> = serde_json::from_str(&paths_raw).unwrap_or_default();
            paths.append(&mut entry);
        }
        Ok(paths)
    }

    pub async fn check_coverage(
        &self,
        paths: &[String],
        now_iso: &str,
    ) -> Result<(bool, Vec<String>)> {
        let now_ts = chrono::DateTime::parse_from_rfc3339(now_iso)
            .map(|d| d.timestamp())
            .unwrap_or(0);
        let rows = sqlx::query(
            "SELECT lease_id, wih_id, agent_id, paths, requested_at, ttl_seconds, status, granted_until, denied_reason
             FROM leases
             WHERE status = 'granted'"
        )
        .fetch_all(&self.pool)
        .await?;

        let mut leases = Vec::new();
        for row in rows {
            let paths_raw: String = row.try_get("paths")?;
            let paths_vec: Vec<String> = serde_json::from_str(&paths_raw).unwrap_or_default();
            leases.push(LeaseRecord {
                lease_id: row.try_get("lease_id")?,
                wih_id: row.try_get("wih_id")?,
                agent_id: row.try_get("agent_id")?,
                paths: paths_vec,
                requested_at: row.try_get("requested_at")?,
                ttl_seconds: row.try_get("ttl_seconds")?,
                status: row.try_get("status")?,
                granted_until: row.try_get("granted_until")?,
                denied_reason: row.try_get("denied_reason")?,
            });
        }

        let active: Vec<LeaseRecord> = leases
            .into_iter()
            .filter(|lease| {
                if let Some(until) = &lease.granted_until {
                    chrono::DateTime::parse_from_rfc3339(until)
                        .map(|d| d.timestamp() >= now_ts)
                        .unwrap_or(false)
                } else {
                    true
                }
            })
            .collect();

        let mut missing = Vec::new();
        for path in paths {
            let covered = active
                .iter()
                .any(|lease| lease.paths.iter().any(|p| matches_path(p, path)));
            if !covered {
                missing.push(path.clone());
            }
        }

        Ok((missing.is_empty(), missing))
    }

    async fn ensure_schema(&self) -> Result<()> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS leases (
                lease_id TEXT PRIMARY KEY,
                wih_id TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                paths TEXT NOT NULL,
                requested_at TEXT NOT NULL,
                ttl_seconds INTEGER,
                status TEXT NOT NULL,
                granted_until TEXT,
                denied_reason TEXT
            )",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn emit(&self, event_type: &str, payload: serde_json::Value) -> Result<()> {
        let Some(sink) = &self.event_sink else {
            return Ok(());
        };
        let event = AllternitEvent {
            event_id: create_event_id(),
            ts: Utc::now().to_rfc3339(),
            actor: Actor {
                r#type: ActorType::Gate,
                id: self.actor_id.clone(),
            },
            scope: None,
            r#type: event_type.to_string(),
            payload,
            provenance: None,
        };
        sink.append(event).await?;
        Ok(())
    }

    /// Get lease by ID
    pub async fn get(&self, lease_id: &str) -> Result<Option<LeaseRecord>> {
        let row = sqlx::query(
            "SELECT lease_id, wih_id, agent_id, paths, requested_at, ttl_seconds, status, granted_until, denied_reason
             FROM leases WHERE lease_id = ?",
        )
        .bind(lease_id)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(row) => {
                let paths_json: String = row.try_get("paths")?;
                let paths: Vec<String> = serde_json::from_str(&paths_json).unwrap_or_default();
                
                Ok(Some(LeaseRecord {
                    lease_id: row.try_get("lease_id")?,
                    wih_id: row.try_get("wih_id")?,
                    agent_id: row.try_get("agent_id")?,
                    paths,
                    requested_at: row.try_get("requested_at")?,
                    ttl_seconds: row.try_get("ttl_seconds")?,
                    status: row.try_get("status")?,
                    granted_until: row.try_get("granted_until")?,
                    denied_reason: row.try_get("denied_reason")?,
                }))
            }
            None => Ok(None),
        }
    }

    /// List leases, optionally filtered by holder
    pub async fn list(&self, holder: Option<&str>) -> Result<Vec<LeaseRecord>> {
        let query = if holder.is_some() {
            "SELECT lease_id, wih_id, agent_id, paths, requested_at, ttl_seconds, status, granted_until, denied_reason
             FROM leases WHERE agent_id = ? AND status IN ('granted', 'requested')"
        } else {
            "SELECT lease_id, wih_id, agent_id, paths, requested_at, ttl_seconds, status, granted_until, denied_reason
             FROM leases WHERE status IN ('granted', 'requested')"
        };

        let mut builder = sqlx::query(query);
        if let Some(h) = holder {
            builder = builder.bind(h);
        }

        let rows = builder.fetch_all(&self.pool).await?;
        let mut leases = Vec::new();

        for row in rows {
            let paths_json: String = row.try_get("paths")?;
            let paths: Vec<String> = serde_json::from_str(&paths_json).unwrap_or_default();
            
            leases.push(LeaseRecord {
                lease_id: row.try_get("lease_id")?,
                wih_id: row.try_get("wih_id")?,
                agent_id: row.try_get("agent_id")?,
                paths,
                requested_at: row.try_get("requested_at")?,
                ttl_seconds: row.try_get("ttl_seconds")?,
                status: row.try_get("status")?,
                granted_until: row.try_get("granted_until")?,
                denied_reason: row.try_get("denied_reason")?,
            });
        }

        Ok(leases)
    }
}

/// Whether `lease_path` covers `candidate`. `dir/**` covers `dir` itself and
/// anything under `dir/`, on a directory boundary: it never covers a sibling
/// such as `dir-private` or `dir2`.
pub(crate) fn matches_path(lease_path: &str, candidate: &str) -> bool {
    if let Some(dir) = lease_path.strip_suffix("/**") {
        return candidate == dir || candidate.starts_with(&format!("{dir}/"));
    }
    if let Some(prefix) = lease_path.strip_suffix('*') {
        return candidate.starts_with(prefix);
    }
    candidate == lease_path || candidate.starts_with(&format!("{}/", lease_path))
}

#[cfg(test)]
mod tests {
    use super::matches_path;

    #[test]
    fn recursive_lease_matches_on_a_directory_boundary() {
        assert!(matches_path("src/**", "src"));
        assert!(matches_path("src/**", "src/a.rs"));
        assert!(matches_path("src/**", "src/deep/b.rs"));
        assert!(!matches_path("src/**", "src-private/secret.ts"));
        assert!(!matches_path("src/**", "src2/x"));
        assert!(!matches_path("src/**", "srcx"));
        assert!(matches_path("a/b/**", "a/b/c"));
        assert!(!matches_path("a/b/**", "a/bc/d"));
        assert!(matches_path("docs", "docs/x.md"));
        assert!(!matches_path("docs", "docs2/x.md"));
    }
}
