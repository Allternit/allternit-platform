//! Cloud Android phones (redroid) that show up as Phone Link devices.
//!
//! A cloud phone is one redroid Docker container on a fleet host
//! (`provisioned_hosts` with `phones_enabled = TRUE`). Redroid needs the
//! binder kernel driver and a privileged container, so it cannot live inside
//! the unprivileged Incus containers cloud computers use; instead the control
//! plane SSHes to the host and runs
//! `/opt/allternit/cloud-phone/run-phone.sh` (infrastructure/cloud-phone/).
//! adb is published on the host's mesh address only; the Desktop app adds the
//! phone by that address and runs the normal Phone Link Android setup.
//!
//! Off by default: [`cloud_phones_enabled`] reads `CLOUD_PHONES_ENABLED`, and
//! no host carries `phones_enabled` until an operator sets it. There is no
//! Stripe product for phones; rows carry `plan = 'preview'`.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

use crate::ApiError;

pub const CLOUD_PHONES_ENABLED_ENV: &str = "CLOUD_PHONES_ENABLED";
pub const NOT_AVAILABLE_MESSAGE: &str = "Cloud phones are not available yet";

/// Where setup-host.sh installs the runner on every phone host.
const RUN_PHONE_PATH: &str = "/opt/allternit/cloud-phone/run-phone.sh";
/// First adb port handed out on a host; phone N on a host gets BASE + N.
const ADB_PORT_BASE: i32 = 5555;
/// The Phone Link app's port sits this far above the phone's adb port.
const LINK_PORT_OFFSET: i32 = 1000;
const DEFAULT_PLAN: &str = "preview";

fn env_i64(name: &str, default: i64) -> i64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(default)
}

/// `CLOUD_PHONES_ENABLED=1|true` turns the routes on. Anything else (or
/// unset) keeps them answering 503.
pub fn cloud_phones_enabled() -> bool {
    std::env::var(CLOUD_PHONES_ENABLED_ENV)
        .map(|value| matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

pub fn ensure_enabled() -> Result<(), ApiError> {
    if cloud_phones_enabled() {
        Ok(())
    } else {
        Err(ApiError::ServiceUnavailable(NOT_AVAILABLE_MESSAGE.to_string()))
    }
}

/// Runs `run-phone.sh` on a phone host. The SSH implementation is the
/// production path; tests swap in a fake.
#[async_trait]
pub trait PhoneHostExec: Send + Sync {
    /// `ssh_target` is `provisioned_hosts.phone_ssh_target`; `args` are
    /// already validated (no shell metacharacters). Returns stdout.
    async fn run_phone(&self, ssh_target: &str, args: &[String]) -> Result<String, ApiError>;
}

pub struct SshPhoneHostExec;

#[async_trait]
impl PhoneHostExec for SshPhoneHostExec {
    async fn run_phone(&self, ssh_target: &str, args: &[String]) -> Result<String, ApiError> {
        if !is_safe_ssh_target(ssh_target) {
            return Err(ApiError::Internal(format!(
                "cloud phone host has an invalid ssh target: {ssh_target}"
            )));
        }
        let output = tokio::process::Command::new("ssh")
            .arg("-o")
            .arg("BatchMode=yes")
            .arg("-o")
            .arg("ConnectTimeout=15")
            .arg(ssh_target)
            .arg(RUN_PHONE_PATH)
            .args(args)
            .output()
            .await
            .map_err(|error| ApiError::SshError(format!("ssh to phone host failed: {error}")))?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        if output.status.success() {
            Ok(stdout)
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(ApiError::SshError(format!(
                "run-phone.sh {} failed: {}",
                args.first().map(String::as_str).unwrap_or(""),
                stderr.trim()
            )))
        }
    }
}

fn is_safe_ssh_target(target: &str) -> bool {
    !target.is_empty()
        && !target.starts_with('-')
        && target
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '-' | '_' | ':'))
}

/// `allternit-phone-<12 hex>`; matches run-phone.sh's name check.
pub fn container_name_for(phone_id: &str) -> String {
    let short: String = phone_id
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .take(12)
        .collect::<String>()
        .to_ascii_lowercase();
    format!("allternit-phone-{short}")
}

/// Parses the `ADB_ADDRESS=ip:port` line run-phone.sh prints on create.
pub fn parse_adb_address(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .find_map(|line| line.trim().strip_prefix("ADB_ADDRESS="))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudPhoneView {
    pub id: String,
    pub label: Option<String>,
    pub status: String,
    pub plan: String,
    /// `meshIp:port` the Desktop app passes to `adb connect`. Present once
    /// the container is running.
    pub adb_address: Option<String>,
    pub mesh_ip: Option<String>,
    pub adb_port: Option<i32>,
    /// `meshIp:port` of the Phone Link app inside the phone (iCTRL), used by
    /// the Desktop Phone Link setup instead of a Wi-Fi address.
    pub link_address: Option<String>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct PhoneRow {
    id: String,
    host_id: Option<String>,
    container_name: String,
    label: Option<String>,
    mesh_ip: Option<String>,
    adb_port: Option<i32>,
    link_port: Option<i32>,
    status: String,
    plan: String,
    last_error: Option<String>,
    created_at: DateTime<Utc>,
}

const PHONE_COLUMNS: &str = "id, host_id, container_name, label, mesh_ip, adb_port, link_port, status, plan, \
     last_error, created_at";

impl From<PhoneRow> for CloudPhoneView {
    fn from(row: PhoneRow) -> Self {
        let running = row.status == "running";
        let address = |port: Option<i32>| match (&row.mesh_ip, port) {
            (Some(ip), Some(port)) if running => Some(format!("{ip}:{port}")),
            _ => None,
        };
        let adb_address = address(row.adb_port);
        let link_address = address(row.link_port);
        Self {
            id: row.id,
            label: row.label,
            status: row.status,
            plan: row.plan,
            adb_address,
            mesh_ip: row.mesh_ip,
            adb_port: row.adb_port,
            link_address,
            last_error: row.last_error,
            created_at: row.created_at,
        }
    }
}

#[derive(sqlx::FromRow)]
struct PhoneHostRow {
    id: String,
    phone_ssh_target: String,
    phone_mesh_ip: String,
    phone_slots: i32,
}

/// The narrowest Headscale ACL rule that lets one customer reach one cloud
/// phone: the owner's mesh user (`clerk-<id>@`, the user every device of
/// theirs enrolls under — routes::mesh) to the phone host's mesh IP on the
/// phone's adb and Phone Link ports only. The production policy is
/// `autogroup:member → autogroup:self:*`, and the phone host is not owned by
/// the customer, so without this rule the customer's Desktop cannot reach it.
pub fn acl_rule_for(user_id: &str, mesh_ip: &str, adb_port: i32, link_port: i32) -> serde_json::Value {
    serde_json::json!({
        "action": "accept",
        "src": [format!("{}@", crate::routes::mesh::mesh_user_name(user_id))],
        "dst": [format!("{mesh_ip}:{adb_port},{link_port}")],
    })
}

#[derive(Clone)]
pub struct CloudPhoneService {
    db: PgPool,
    exec: Arc<dyn PhoneHostExec>,
}

impl CloudPhoneService {
    pub fn new(db: PgPool) -> Self {
        Self::with_exec(db, Arc::new(SshPhoneHostExec))
    }

    pub fn with_exec(db: PgPool, exec: Arc<dyn PhoneHostExec>) -> Self {
        Self { db, exec }
    }

    pub async fn list_for_user(&self, user_id: &str) -> Result<Vec<CloudPhoneView>, ApiError> {
        let rows: Vec<PhoneRow> = sqlx::query_as(&format!(
            "SELECT {PHONE_COLUMNS} FROM cloud_phones \
             WHERE user_id = $1 AND status <> 'deleted' ORDER BY created_at"
        ))
        .bind(user_id)
        .fetch_all(&self.db)
        .await?;
        Ok(rows.into_iter().map(CloudPhoneView::from).collect())
    }

    /// Picks the phone host with the most free slots, records the phone, and
    /// starts its container. Returns the phone with its adb address.
    pub async fn create(
        &self,
        user_id: &str,
        label: Option<&str>,
    ) -> Result<CloudPhoneView, ApiError> {
        let max_per_user = env_i64("CLOUD_PHONES_MAX_PER_USER", 1);
        let live: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM cloud_phones \
             WHERE user_id = $1 AND status NOT IN ('deleted', 'error')",
        )
        .bind(user_id)
        .fetch_one(&self.db)
        .await?;
        if live >= max_per_user {
            return Err(ApiError::Conflict(format!(
                "This account already has {live} cloud phone(s); the limit is {max_per_user}"
            )));
        }

        let memory_mb = env_i64("CLOUD_PHONES_MEMORY_MB", 3072).max(1024) as i32;
        let host = self.select_phone_host().await?;
        let port = self.free_port(&host).await?;
        let id = Uuid::new_v4().simple().to_string();
        let container = container_name_for(&id);
        let label = label
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.chars().take(80).collect::<String>());

        sqlx::query(
            "INSERT INTO cloud_phones \
               (id, user_id, host_id, container_name, label, mesh_ip, adb_port, link_port, \
                status, plan, memory_mb) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'provisioning', $9, $10)",
        )
        .bind(&id)
        .bind(user_id)
        .bind(&host.id)
        .bind(&container)
        .bind(&label)
        .bind(&host.phone_mesh_ip)
        .bind(port)
        .bind(port + LINK_PORT_OFFSET)
        .bind(DEFAULT_PLAN)
        .bind(memory_mb)
        .execute(&self.db)
        .await
        .map_err(|error| {
            let port_taken =
                matches!(&error, sqlx::Error::Database(db) if db.is_unique_violation());
            if port_taken {
                ApiError::ServiceUnavailable(
                    "Cloud phone hosts are busy; try again in a moment".to_string(),
                )
            } else {
                ApiError::DatabaseError(error)
            }
        })?;

        let args = vec![
            "create".to_string(),
            container.clone(),
            host.phone_mesh_ip.clone(),
            port.to_string(),
            (port + LINK_PORT_OFFSET).to_string(),
            memory_mb.to_string(),
        ];
        match self.exec.run_phone(&host.phone_ssh_target, &args).await {
            Ok(stdout) => {
                let expected = format!("{}:{port}", host.phone_mesh_ip);
                if parse_adb_address(&stdout).as_deref() != Some(expected.as_str()) {
                    tracing::warn!(
                        phone_id = %id,
                        "run-phone.sh create did not report the expected adb address"
                    );
                }
                sqlx::query(
                    "UPDATE cloud_phones SET status = 'running', last_error = NULL, \
                     updated_at = CURRENT_TIMESTAMP WHERE id = $1",
                )
                .bind(&id)
                .execute(&self.db)
                .await?;
            }
            Err(error) => {
                tracing::error!(phone_id = %id, host_id = %host.id, "cloud phone create failed: {error}");
                sqlx::query(
                    "UPDATE cloud_phones SET status = 'error', last_error = $2, \
                     updated_at = CURRENT_TIMESTAMP WHERE id = $1",
                )
                .bind(&id)
                .bind(error.to_string())
                .execute(&self.db)
                .await?;
                return Err(ApiError::ServiceUnavailable(
                    "The cloud phone could not be started; try again later".to_string(),
                ));
            }
        }
        self.get_for_user(&id, user_id).await
    }

    pub async fn get_for_user(&self, id: &str, user_id: &str) -> Result<CloudPhoneView, ApiError> {
        self.row_for_user(id, user_id).await.map(CloudPhoneView::from)
    }

    async fn row_for_user(&self, id: &str, user_id: &str) -> Result<PhoneRow, ApiError> {
        let row: Option<PhoneRow> = sqlx::query_as(&format!(
            "SELECT {PHONE_COLUMNS} FROM cloud_phones \
             WHERE id = $1 AND user_id = $2 AND status <> 'deleted'"
        ))
        .bind(id)
        .bind(user_id)
        .fetch_optional(&self.db)
        .await?;
        row.ok_or_else(|| ApiError::NotFound(format!("cloud phone {id}")))
    }

    /// Removes the container and its data, then marks the row deleted.
    pub async fn delete(&self, id: &str, user_id: &str) -> Result<(), ApiError> {
        let row = self.row_for_user(id, user_id).await?;
        if let Some(host_id) = &row.host_id {
            let target: Option<Option<String>> = sqlx::query_scalar(
                "SELECT phone_ssh_target FROM provisioned_hosts WHERE id = $1",
            )
            .bind(host_id)
            .fetch_optional(&self.db)
            .await?;
            if let Some(Some(target)) = target {
                let args = vec![
                    "delete".to_string(),
                    row.container_name.clone(),
                    "--purge".to_string(),
                ];
                self.exec.run_phone(&target, &args).await?;
            }
        }
        sqlx::query(
            "UPDATE cloud_phones SET status = 'deleted', deleted_at = CURRENT_TIMESTAMP, \
             updated_at = CURRENT_TIMESTAMP WHERE id = $1",
        )
        .bind(id)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// ACL rules for every live cloud phone, for operators to merge into
    /// infrastructure/mesh/headscale/policy.hujson (static file mode; there is
    /// no live policy API in use). Sorted for stable diffs.
    pub async fn acl_fragment(&self) -> Result<Vec<serde_json::Value>, ApiError> {
        let rows: Vec<(String, String, i32, i32)> = sqlx::query_as(
            "SELECT user_id, mesh_ip, adb_port, link_port FROM cloud_phones \
             WHERE status NOT IN ('deleted', 'error') AND mesh_ip IS NOT NULL \
               AND adb_port IS NOT NULL AND link_port IS NOT NULL \
             ORDER BY user_id, mesh_ip, adb_port",
        )
        .fetch_all(&self.db)
        .await?;
        Ok(rows
            .iter()
            .map(|(user_id, ip, adb, link)| acl_rule_for(user_id, ip, *adb, *link))
            .collect())
    }

    async fn select_phone_host(&self) -> Result<PhoneHostRow, ApiError> {
        let host: Option<PhoneHostRow> = sqlx::query_as(
            r#"
            SELECT h.id, h.phone_ssh_target, h.phone_mesh_ip, h.phone_slots
            FROM provisioned_hosts h
            LEFT JOIN cloud_phones p
              ON p.host_id = h.id AND p.status <> 'deleted'
            WHERE h.enabled AND h.phones_enabled
              AND h.phone_ssh_target IS NOT NULL AND h.phone_mesh_ip IS NOT NULL
              AND h.phone_slots > 0
            GROUP BY h.id, h.phone_ssh_target, h.phone_mesh_ip, h.phone_slots
            HAVING COUNT(p.id) < h.phone_slots
            ORDER BY h.phone_slots - COUNT(p.id) DESC, h.id
            LIMIT 1
            "#,
        )
        .fetch_optional(&self.db)
        .await?;
        host.ok_or_else(|| ApiError::ServiceUnavailable(NOT_AVAILABLE_MESSAGE.to_string()))
    }

    async fn free_port(&self, host: &PhoneHostRow) -> Result<i32, ApiError> {
        let used: Vec<i32> = sqlx::query_scalar(
            "SELECT adb_port FROM cloud_phones \
             WHERE host_id = $1 AND status <> 'deleted' AND adb_port IS NOT NULL",
        )
        .bind(&host.id)
        .fetch_all(&self.db)
        .await?;
        (0..host.phone_slots.max(0))
            .map(|slot| ADB_PORT_BASE + slot)
            .find(|port| !used.contains(port))
            .ok_or_else(|| ApiError::ServiceUnavailable(NOT_AVAILABLE_MESSAGE.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_name_matches_runner_pattern() {
        let name = container_name_for("3F2A9C1B7D4E8F00aa");
        assert_eq!(name, "allternit-phone-3f2a9c1b7d4e");
    }

    #[test]
    fn parses_adb_address_line() {
        assert_eq!(
            parse_adb_address("noise\nADB_ADDRESS=100.64.0.9:5556\n").as_deref(),
            Some("100.64.0.9:5556")
        );
        assert_eq!(parse_adb_address("STATUS=running"), None);
    }

    #[test]
    fn acl_rule_grants_only_the_phone_ports() {
        let rule = acl_rule_for("user_2abc", "100.64.0.9", 5556, 6556);
        assert_eq!(rule["src"][0], "clerk-user-2abc@");
        assert_eq!(rule["dst"][0], "100.64.0.9:5556,6556");
    }

    #[test]
    fn ssh_target_rejects_options_and_shell() {
        assert!(is_safe_ssh_target("root@100.64.0.9"));
        assert!(!is_safe_ssh_target("-oProxyCommand=x"));
        assert!(!is_safe_ssh_target("root@host;rm"));
    }
}
