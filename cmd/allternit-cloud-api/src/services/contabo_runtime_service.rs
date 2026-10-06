//! Contabo-hosted runtime provisioning service.
//!
//! Provisions user workloads as Docker containers on the existing Contabo VPS.
//! Each container runs agent-daemon + gizzi-code serve and auto-enrolls into
//! Headscale via the wizard bootstrap contract.

use crate::error::ApiError;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::process::Stdio;
use tokio::process::Command;
use uuid::Uuid;

/// Runtime credentials returned after provisioning.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvisionedContaboRuntime {
    pub instance_id: String,
    pub container_id: String,
    pub runtime_device_id: String,
    pub mesh_ip: Option<String>,
    pub gateway_url: String,
    pub bootstrap_token: String,
}

/// Container status as reported by `docker inspect` on the Contabo VPS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContaboContainerState {
    /// Container is running.
    Running,
    /// Container exists but is not running (exited, created, paused, dead).
    Stopped,
    /// Container is restarting.
    Starting,
    /// Container no longer exists on the host.
    Removed,
    /// Unmapped docker state string.
    Other(String),
}

/// Stored instance of a hosted runtime.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct HostedInstanceRow {
    pub id: String,
    pub user_id: String,
    pub organization_id: Option<String>,
    pub name: String,
    pub runtime_device_id: Option<String>,
    pub billing_mode: String,
    pub provider: Option<String>,
    pub region: String,
    pub cpu_kind: String,
    pub cpus: i64,
    pub memory_mb: i64,
    pub status: String,
    pub idle_timeout_minutes: i64,
    pub last_activity_at: Option<chrono::DateTime<chrono::Utc>>,
    pub active_since: Option<chrono::DateTime<chrono::Utc>>,
    pub stop_reason: Option<String>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub stopped_at: Option<chrono::DateTime<chrono::Utc>>,
    pub destroyed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub monthly_cost_cap: Option<f64>,
    pub cost_rate_provider: Option<String>,
    pub cost_rate_region: Option<String>,
    pub cost_rate_instance_type: Option<String>,
    pub last_synced_at: Option<chrono::DateTime<chrono::Utc>>,
    pub error_message: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// The retail rate instance type for a container size (cost_rates provider
/// 'contabo', region 'hosted'; see migrations 027). Sizes above 2GB meter at
/// the largest tier — quota caps keep containers at or under 2GB today, and
/// over-sizing the rate beats silently metering $0.
pub fn hosted_instance_type(memory_mb: i64) -> &'static str {
    if memory_mb <= 512 {
        "hosted-512mb"
    } else if memory_mb <= 1024 {
        "hosted-1024mb"
    } else {
        "hosted-2048mb"
    }
}

/// Writable-layer disk quota per container size, in GB. Enforced by the
/// lifecycle sweep (stop_reason 'disk_quota_exceeded') — see
/// DOCKER_STORAGE_OPT_SUPPORTED for why docker's own flag is not used.
pub(crate) fn hosted_disk_quota_gb(memory_mb: i64) -> u64 {
    if memory_mb <= 512 {
        5
    } else if memory_mb <= 1024 {
        10
    } else {
        20
    }
}

/// `--storage-opt size=` hard-errors on the hosted nodes: they run overlay2 on
/// ext4 (checked on `mail` 2026-09: `df -T /var/lib/docker` → ext4,
/// `docker info --format '{{.Driver}}'` → overlayfs), and the flag needs
/// xfs+pquota. Disk is instead enforced by the lifecycle disk sweep. Flip to
/// true if the nodes move to xfs+pquota.
const DOCKER_STORAGE_OPT_SUPPORTED: bool = false;

/// Parse docker's human size ("0B", "12.3MB", "1.5GB") into bytes (decimal
/// units, as docker prints them). Unknown formats are None, never a panic.
fn parse_docker_size(size: &str) -> Option<u64> {
    let split = size.find(|c: char| !(c.is_ascii_digit() || c == '.'))?;
    let (number, unit) = size.split_at(split);
    let value: f64 = number.parse().ok()?;
    let multiplier = match unit {
        "B" => 1.0,
        "kB" | "KB" => 1e3,
        "MB" => 1e6,
        "GB" => 1e9,
        "TB" => 1e12,
        _ => return None,
    };
    Some((value * multiplier) as u64)
}

/// Service that provisions user workloads on the Contabo VPS.
pub struct ContaboRuntimeService {
    db: PgPool,
    headscale_api_key: Option<String>,
    cloud_api_url: String,
}

/// Placement decision for a new container: which node row records it (none
/// for legacy single-node deployments) and which docker host runs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodePlacement {
    pub node_id: Option<String>,
    pub docker_host: String,
}

impl ContaboRuntimeService {
    pub fn new(db: PgPool, headscale_api_key: Option<String>, cloud_api_url: String) -> Self {
        Self {
            db,
            headscale_api_key,
            cloud_api_url,
        }
    }

    /// Build a docker CLI invocation for the given host. "local" uses the
    /// control-plane daemon directly; anything else is passed via `-H`
    /// (e.g. `ssh://root@allternit-standby`), so steps like `docker cp` keep
    /// streaming from the local filesystem through the CLI unchanged.
    fn docker_command(docker_host: &str) -> Command {
        let mut command = Command::new("docker");
        if docker_host != "local" {
            command.arg("-H").arg(docker_host);
        }
        command
    }

    /// Pick the active node with the lowest allocated-memory ratio that still
    /// fits the request (allocated + requested <= 85% of total). Instances
    /// with `node_id` NULL are legacy control-plane containers and count
    /// against the node whose `docker_host` is 'local'. An empty nodes table
    /// means a legacy single-node deployment: local, no node row.
    async fn select_node(&self, memory_mb: i64) -> Result<NodePlacement, ApiError> {
        // An empty nodes table means a legacy single-node deployment: local,
        // no node row. Nodes that exist but are all draining/down are NOT
        // legacy — that is a capacity error.
        let node_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hosted_runtime_nodes")
            .fetch_one(&self.db)
            .await
            .map_err(ApiError::DatabaseError)?;
        if node_count == 0 {
            return Ok(NodePlacement {
                node_id: None,
                docker_host: "local".to_string(),
            });
        }

        let nodes: Vec<(String, String, i64)> = sqlx::query_as(
            "SELECT id, docker_host, total_memory_mb FROM hosted_runtime_nodes WHERE status = 'active'",
        )
        .fetch_all(&self.db)
        .await
        .map_err(ApiError::DatabaseError)?;

        let mut best: Option<NodePlacement> = None;
        let mut best_ratio = f64::MAX;
        for (node_id, docker_host, total_memory_mb) in nodes {
            let allocated: i64 = sqlx::query_scalar(
                r#"
                SELECT COALESCE(SUM(memory_mb), 0)::BIGINT
                FROM hosted_runtime_instances
                WHERE status NOT IN ('destroying', 'destroyed')
                  AND (node_id = $1 OR ($2 AND node_id IS NULL))
                "#,
            )
            .bind(&node_id)
            .bind(docker_host == "local")
            .fetch_one(&self.db)
            .await
            .map_err(ApiError::DatabaseError)?;

            if (allocated + memory_mb) as f64 > total_memory_mb as f64 * 0.85 {
                continue;
            }
            let ratio = allocated as f64 / total_memory_mb as f64;
            if ratio < best_ratio {
                best_ratio = ratio;
                best = Some(NodePlacement {
                    node_id: Some(node_id),
                    docker_host,
                });
            }
        }

        best.ok_or_else(|| {
            ApiError::ServiceUnavailable(format!(
                "No hosted runtime node has capacity for {} MB. Try a smaller size or again later.",
                memory_mb
            ))
        })
    }

    /// The docker host an instance's container lives on. Instances without a
    /// node assignment (legacy) live on the control-plane daemon.
    async fn docker_host_for_instance(&self, instance_id: &str) -> Result<String, ApiError> {
        let host: Option<String> = sqlx::query_scalar(
            r#"
            SELECT n.docker_host
            FROM hosted_runtime_instances i
            JOIN hosted_runtime_nodes n ON n.id = i.node_id
            WHERE i.id = $1
            "#,
        )
        .bind(instance_id)
        .fetch_optional(&self.db)
        .await
        .map_err(ApiError::DatabaseError)?;
        Ok(host.unwrap_or_else(|| "local".to_string()))
    }

    /// Docker container name for a hosted runtime instance. The container is
    /// always addressable from the instance id, so no external ID bookkeeping
    /// is needed.
    pub fn container_name(instance_id: &str) -> String {
        let prefix_len = instance_id.len().min(16);
        format!("allternit-rt-{}", &instance_id[..prefix_len])
    }

    /// Writable-layer sizes of the containers on one docker host, as
    /// (container name, bytes). One `docker ps -s` per host — the lifecycle
    /// disk sweep calls this only for hosts that currently run hosted
    /// containers.
    pub(crate) async fn container_writable_bytes(
        &self,
        docker_host: &str,
    ) -> Result<Vec<(String, u64)>, ApiError> {
        let output = Self::docker_command(docker_host)
            .args(["ps", "-s", "--format", "{{.Names}}\t{{.Size}}"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| ApiError::Internal(format!("docker ps failed: {}", e)))?;
        if !output.status.success() {
            return Err(ApiError::Internal(format!(
                "docker ps failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout
            .lines()
            .filter_map(|line| {
                let (name, size) = line.split_once('\t')?;
                // `{{.Size}}` is "12.3MB (virtual 1.2GB)": the writable layer
                // is the first token.
                let writable = size.split_whitespace().next()?;
                Some((name.to_string(), parse_docker_size(writable)?))
            })
            .collect())
    }

    /// Provision a new workload container for a user.
    ///
    /// The instance row is left in `starting` with the bootstrap token stored
    /// as a SHA-256 hash; the container's data plane completes the hosted
    /// pairing flow (`/api/v1/runtime-pairings` + exchange), which assigns the
    /// runtime device and flips the row to `running`.
    pub async fn provision(
        &self,
        user_id: &str,
        name: &str,
        memory_mb: i64,
    ) -> Result<ProvisionedContaboRuntime, ApiError> {
        let instance_id = format!("hr_contabo_{}", Uuid::new_v4().simple());
        let runtime_device_id = format!("rt_{}", Uuid::new_v4().simple());
        let bootstrap_token = Uuid::new_v4().to_string();
        let placement = self.select_node(memory_mb).await?;

        // 1. Insert hosted_runtime_instances row. The pairing endpoint
        // validates the presented token against the SHA-256 hash, so store
        // the hash — never the raw token. The cost_rate_* columns must point at
        // a real retail rate (migrations 027): leaving them NULL made
        // record_runtime_started's LEFT JOIN COALESCE to $0.00/hr, metering
        // every container for free.
        sqlx::query(
            r#"
            INSERT INTO hosted_runtime_instances (
                id, user_id, name, provider, region, cpu_kind, cpus, memory_mb,
                status, bootstrap_token_hash, node_id,
                cost_rate_provider, cost_rate_region, cost_rate_instance_type,
                created_at, updated_at
            ) VALUES ($1, $2, $3, 'contabo', 'local', 'shared', 1, $4, 'creating', $5, $6, 'contabo', 'hosted', $7, NOW(), NOW())
            "#,
        )
        .bind(&instance_id)
        .bind(user_id)
        .bind(name)
        .bind(memory_mb)
        .bind(sha256_hex(bootstrap_token.as_bytes()))
        .bind(&placement.node_id)
        .bind(hosted_instance_type(memory_mb))
        .execute(&self.db)
        .await
        .map_err(ApiError::DatabaseError)?;

        // 2. Create and set up the Docker container
        let container_id = self
            .create_container(
                &placement.docker_host,
                &instance_id,
                memory_mb,
                &bootstrap_token,
                &runtime_device_id,
                &[],
            )
            .await?;

        // 3. Mark instance as starting — but only if the pairing exchange has
        // not already flipped it to 'running'. The setup script pairs
        // synchronously inside the container, and the exchange owns the
        // 'running' transition; an unconditional update here would stomp it.
        sqlx::query(
            r#"
            UPDATE hosted_runtime_instances
            SET status = 'starting', last_synced_at = NOW(), updated_at = NOW()
            WHERE id = $1 AND runtime_device_id IS NULL AND status IN ('creating', 'starting')
            "#,
        )
        .bind(&instance_id)
        .execute(&self.db)
        .await
        .map_err(ApiError::DatabaseError)?;

        let gateway_url = format!("http://{}:8013", container_id);
        Ok(ProvisionedContaboRuntime {
            instance_id,
            container_id,
            runtime_device_id,
            mesh_ip: None,
            gateway_url,
            bootstrap_token,
        })
    }

    /// Provision a container for an existing `hosted_runtime_instances` row.
    ///
    /// Unlike [`provision`](Self::provision), the database row (quota
    /// reservation, bootstrap token hash, cost-rate columns) is managed by the
    /// caller; this only creates and sets up the container. Pairing completes
    /// through the bootstrap-token flow inside the container, which flips the
    /// row to `running` and assigns its runtime device.
    pub async fn provision_container(
        &self,
        instance_id: &str,
        memory_mb: i64,
        bootstrap_token: &str,
        extra_env: &[(String, String)],
    ) -> Result<ProvisionedContaboRuntime, ApiError> {
        let runtime_device_id = format!("rt_{}", Uuid::new_v4().simple());
        let placement = self.select_node(memory_mb).await?;
        sqlx::query("UPDATE hosted_runtime_instances SET node_id = $1 WHERE id = $2")
            .bind(&placement.node_id)
            .bind(instance_id)
            .execute(&self.db)
            .await
            .map_err(ApiError::DatabaseError)?;

        let container_id = self
            .create_container(
                &placement.docker_host,
                instance_id,
                memory_mb,
                bootstrap_token,
                &runtime_device_id,
                extra_env,
            )
            .await?;

        let gateway_url = format!("http://{}:8013", container_id);
        Ok(ProvisionedContaboRuntime {
            instance_id: instance_id.to_string(),
            container_id,
            runtime_device_id,
            mesh_ip: None,
            gateway_url,
            bootstrap_token: bootstrap_token.to_string(),
        })
    }

    /// Create the workload container on the selected docker host and install
    /// the data plane. Returns the docker container id.
    async fn create_container(
        &self,
        docker_host: &str,
        instance_id: &str,
        memory_mb: i64,
        bootstrap_token: &str,
        runtime_device_id: &str,
        extra_env: &[(String, String)],
    ) -> Result<String, ApiError> {
        let container_name = Self::container_name(instance_id);

        let mut args: Vec<String> = vec![
            "run".to_string(),
            "-d".to_string(),
            "--name".to_string(),
            container_name.clone(),
            "--memory".to_string(),
            format!("{}m", memory_mb),
            "--cpus".to_string(),
            "1".to_string(),
            "-e".to_string(),
            format!("ALLTERNIT_BOOTSTRAP_TOKEN={}", bootstrap_token),
            "-e".to_string(),
            format!("ALLTERNIT_CLOUD_API_URL={}", self.cloud_api_url),
            "-e".to_string(),
            format!("ALLTERNIT_RUNTIME_DEVICE_ID={}", runtime_device_id),
            "-e".to_string(),
            format!("ALLTERNIT_INSTANCE_NAME={}", instance_id),
        ];
        // Disk quota via docker itself only where the storage driver supports
        // it (xfs+pquota); today the nodes are overlay2-on-ext4, so the
        // lifecycle disk sweep is the enforcement (see the const's comment).
        if DOCKER_STORAGE_OPT_SUPPORTED {
            args.push("--storage-opt".to_string());
            args.push(format!("size={}g", hosted_disk_quota_gb(memory_mb)));
        }
        for (key, value) in extra_env {
            args.push("-e".to_string());
            args.push(format!("{}={}", key, value));
        }
        args.extend([
            "ubuntu:24.04".to_string(),
            "sleep".to_string(),
            "infinity".to_string(),
        ]);

        let output = Self::docker_command(docker_host)
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| ApiError::Internal(format!("docker run failed: {}", e)))?;

        if !output.status.success() {
            return Err(ApiError::Internal(format!(
                "docker run failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        let container_id = String::from_utf8_lossy(&output.stdout).trim().to_string();

        // Copy agent-daemon files into container. `docker cp` over -H ssh://
        // still streams from this machine's filesystem through the CLI.
        let agent_daemon_src = std::path::Path::new("/opt/allternit-build/cmd/agent-daemon/dist");
        if agent_daemon_src.exists() {
            let _ = Self::docker_command(docker_host)
                .args([
                    "cp",
                    agent_daemon_src.to_str().unwrap(),
                    &format!("{}:/opt/agent-daemon", container_name),
                ])
                .output()
                .await;
        }

        // Install data plane inside container
        let setup_script = self.generate_setup_script(bootstrap_token, runtime_device_id, instance_id);
        let setup_output = Self::docker_command(docker_host)
            .args([
                "exec",
                "-i",
                &container_name,
                "bash",
                "-c",
                &setup_script,
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| ApiError::Internal(format!("docker exec failed: {}", e)))?;

        if !setup_output.status.success() {
            tracing::warn!(
                "Data plane setup had errors: {}",
                String::from_utf8_lossy(&setup_output.stderr)
            );
        }
        tracing::info!(
            instance_id = %instance_id,
            exit_ok = %setup_output.status.success(),
            stdout = %String::from_utf8_lossy(&setup_output.stdout),
            "data plane setup finished"
        );

        Ok(container_id)
    }

    /// Generate the data plane setup script for the container.
    fn generate_setup_script(&self, bootstrap_token: &str, runtime_device_id: &str, instance_name: &str) -> String {
        let mesh_block = if let Some(ref _key) = self.headscale_api_key {
            r#"
# Install tailscale
if ! command -v tailscale >/dev/null 2>&1; then
    curl -fsSL https://tailscale.com/install.sh | sh
fi

# Join tailnet (Headscale)
if ! tailscale ip -4 >/dev/null 2>&1; then
    tailscale up \
        --login-server "https://mail.news.allternit.com:8444" \
        --auth-key "$ALLTERNIT_HEADSCALE_PREAUTH_KEY" \
        --hostname "$ALLTERNIT_INSTANCE_NAME" \
        --timeout 60s || true
fi
MESH_IP="$(tailscale ip -4 2>/dev/null | head -n1 || true)"
"#
        } else {
            r#"
MESH_IP=""
"#
        };

        format!(
            r#"#!/bin/bash
set -e

export DEBIAN_FRONTEND=noninteractive

# Install dependencies
apt-get update -qq
apt-get install -y -qq curl jq openssl ca-certificates nodejs npm
echo "setup-stage: deps installed"

# Install gizzi-code
GIZZI_RELEASE="hosted-runtime-2026.07.16"
GIZZI_SHA256="f1d29bad0b3903d77261e7706ff80fd292fefece3ebeaa4bb7f08a51ad2fc694"
mkdir -p /opt/gizzi/bin
curl -fsSL "https://github.com/Allternit/gizzi-code/releases/download/${{GIZZI_RELEASE}}/gizzi-code-linux-x64-native" -o /opt/gizzi/bin/gizzi-code
echo "${{GIZZI_SHA256}}  /opt/gizzi/bin/gizzi-code" | sha256sum -c -
chmod +x /opt/gizzi/bin/gizzi-code
echo "setup-stage: gizzi-code installed"

# Install agent-daemon
mkdir -p /opt/agent-daemon
cd /opt/agent-daemon
npm install ws@8 --production 2>/dev/null || true

{mesh_block}

# Pair runtime device (if cloud API URL is set)
if [ -n "$ALLTERNIT_CLOUD_API_URL" ] && [ -n "$ALLTERNIT_BOOTSTRAP_TOKEN" ]; then
    # Generate ephemeral Ed25519 identity. The API expects base64url WITHOUT
    # padding; openssl emits standard base64, so translate the alphabet and
    # strip '='.
    PRIVATE_KEY="$(openssl genpkey -algorithm ed25519 2>/dev/null | openssl base64 -A)"
    PUBLIC_KEY="$(echo "$PRIVATE_KEY" | openssl base64 -d -A 2>/dev/null | openssl pkey -pubout -outform DER 2>/dev/null | tail -c 32 | openssl base64 -A | tr '+/' '-_' | tr -d '=')"
    # pkeyutl's Ed25519 oneshot must read the key from a real file (it seeks),
    # so process substitution does not work here.
    echo "$PRIVATE_KEY" | openssl base64 -d -A > /tmp/rt-identity-key.pem
    chmod 600 /tmp/rt-identity-key.pem
    
    # Create pairing
    PAIRING_RESPONSE="$(curl -fsSL -X POST "$ALLTERNIT_CLOUD_API_URL/api/v1/runtime-pairings" \
        -H "Content-Type: application/json" \
        -d "{{\"runtimeType\": \"hosted\", \"hostedInstanceId\": \"$ALLTERNIT_INSTANCE_NAME\", \"hostedBootstrapToken\": \"$ALLTERNIT_BOOTSTRAP_TOKEN\", \"publicKey\": \"$PUBLIC_KEY\", \"name\": \"$ALLTERNIT_INSTANCE_NAME\", \"capabilities\": [\"runtime:connect\", \"runtime:execute\", \"runtime:files\", \"runtime:terminal\", \"runtime:remote_control\", \"providers:connect\", \"providers:use\"]}}")"
    
    PAIRING_ID="$(echo "$PAIRING_RESPONSE" | jq -r '.pairingId // empty')"
    DEVICE_CODE="$(echo "$PAIRING_RESPONSE" | jq -r '.deviceCode // empty')"
    CHALLENGE="$(echo "$PAIRING_RESPONSE" | jq -r '.challenge // empty')"
    echo "setup-stage: pairing create answered (pairing id present: $([ -n "$PAIRING_ID" ] && echo yes || echo no))"
    
    if [ -n "$PAIRING_ID" ] && [ -n "$DEVICE_CODE" ] && [ -n "$CHALLENGE" ]; then
        # Sign the challenge. Ed25519 is a oneshot operation: pkeyutl needs
        # both the key and the message as real files (it seeks), not pipes.
        MESSAGE="allternit-runtime-pairing:${{PAIRING_ID}}:${{CHALLENGE}}"
        echo -n "$MESSAGE" > /tmp/rt-pairing-message.txt
        SIGNATURE="$(openssl pkeyutl -sign -inkey /tmp/rt-identity-key.pem -in /tmp/rt-pairing-message.txt -rawin 2>/dev/null | openssl base64 -A | tr '+/' '-_' | tr -d '=')"
        rm -f /tmp/rt-pairing-message.txt
        
        # Exchange for device token
        EXCHANGE_RESPONSE="$(curl -fsSL -X POST "$ALLTERNIT_CLOUD_API_URL/api/v1/runtime-pairings/exchange" \
            -H "Content-Type: application/json" \
            -d "{{\"pairingId\": \"$PAIRING_ID\", \"deviceCode\": \"$DEVICE_CODE\", \"signature\": \"$SIGNATURE\"}}")"
        
        DEVICE_TOKEN="$(echo "$EXCHANGE_RESPONSE" | jq -r '.deviceToken // empty')"
        PAIRED_RUNTIME_ID="$(echo "$EXCHANGE_RESPONSE" | jq -r '.runtimeId // empty')"
        echo "setup-stage: exchange answered (device token present: $([ -n "$DEVICE_TOKEN" ] && echo yes || echo no))"
        if [ -n "$DEVICE_TOKEN" ] && [ -n "$PAIRED_RUNTIME_ID" ]; then
            # Write the full RuntimeIdentity document agent-daemon expects at
            # ALLTERNIT_RUNTIME_IDENTITY_PATH (camelCase, includes the Ed25519
            # keypair), so the daemon loads it directly and starts heartbeating
            # instead of falling back to interactive pairing.
            jq -n \
                --arg runtimeId "$PAIRED_RUNTIME_ID" \
                --arg userId "$(echo "$EXCHANGE_RESPONSE" | jq -r '.userId // empty')" \
                --arg userEmail "$(echo "$EXCHANGE_RESPONSE" | jq -r '.userEmail // empty')" \
                --arg deviceToken "$DEVICE_TOKEN" \
                --arg expiresAt "$(echo "$EXCHANGE_RESPONSE" | jq -r '.expiresAt // empty')" \
                --arg privateKeyPem "$(cat /tmp/rt-identity-key.pem)" \
                --arg publicKey "$PUBLIC_KEY" \
                --argjson capabilities "$(echo "$EXCHANGE_RESPONSE" | jq -c '.capabilities // []')" \
                '{{runtimeId: $runtimeId, userId: $userId, userEmail: $userEmail, deviceToken: $deviceToken, expiresAt: $expiresAt, capabilities: $capabilities, privateKeyPem: $privateKeyPem, publicKey: $publicKey}}' \
                > /tmp/runtime-device.json
            rm -f /tmp/rt-identity-key.pem
            mkdir -p /data/.local/share/gizzi-code
            mv /tmp/runtime-device.json /data/.local/share/gizzi-code/runtime-device.json
            chmod 600 /data/.local/share/gizzi-code/runtime-device.json
            mkdir -p /root/.config/allternit
            cp /data/.local/share/gizzi-code/runtime-device.json /root/.config/allternit/runtime-identity.json
            chmod 600 /root/.config/allternit/runtime-identity.json
        fi
    fi
    if [ ! -f /data/.local/share/gizzi-code/runtime-device.json ]; then
        echo "setup-stage: hosted pairing failed — refusing to mark this box online without a device token" >&2
        exit 1
    fi
fi

# Start gizzi-code serve in background
nohup /opt/gizzi/bin/gizzi-code serve --hostname 0.0.0.0 --port 8013 > /var/log/gizzi-code.log 2>&1 &

# Start agent-daemon in background (if identity exists)
if [ -f /data/.local/share/gizzi-code/runtime-device.json ]; then
    export ALLTERNIT_HOSTED_INSTANCE_ID="$ALLTERNIT_INSTANCE_NAME"
    export ALLTERNIT_HOSTED_BOOTSTRAP_TOKEN="$ALLTERNIT_BOOTSTRAP_TOKEN"
    export ALLTERNIT_CLOUD_API_URL="$ALLTERNIT_CLOUD_API_URL"
    export ALLTERNIT_RUNTIME_IDENTITY_PATH="/data/.local/share/gizzi-code/runtime-device.json"
    export ALLTERNIT_GATEWAY_URL="http://127.0.0.1:8013"
    printf 'ALLTERNIT_RUNTIME_IDENTITY_PATH=%s\nALLTERNIT_CLOUD_API_URL=%s\nALLTERNIT_GATEWAY_URL=%s\n' \
      "$ALLTERNIT_RUNTIME_IDENTITY_PATH" "$ALLTERNIT_CLOUD_API_URL" "$ALLTERNIT_GATEWAY_URL" \
      > /etc/allternit-runtime.env
    cd /opt/agent-daemon
    nohup node index.js > /var/log/agent-daemon.log 2>&1 &
fi

echo "Data plane setup complete"
"#,
            mesh_block = mesh_block
        )
    }

    /// Stop and remove a workload container.
    pub async fn destroy(&self, instance_id: &str) -> Result<(), ApiError> {
        let container_name = Self::container_name(instance_id);
        let docker_host = self.docker_host_for_instance(instance_id).await?;

        let _ = Self::docker_command(&docker_host)
            .args(["rm", "-f", &container_name])
            .output()
            .await;

        sqlx::query(
            r#"
            UPDATE hosted_runtime_instances
            SET status = 'destroyed', destroyed_at = NOW(), updated_at = NOW()
            WHERE id = $1
            "#,
        )
        .bind(instance_id)
        .execute(&self.db)
        .await
        .map_err(ApiError::DatabaseError)?;

        Ok(())
    }

    /// Start a stopped workload container. No-op when it is already running.
    /// If the container was removed, mint a new bootstrap token and recreate it
    /// so a catalog row without a box can come back.
    pub async fn start(&self, instance_id: &str) -> Result<(), ApiError> {
        let container_name = Self::container_name(instance_id);
        let docker_host = self.docker_host_for_instance(instance_id).await?;
        let output = Self::docker_command(&docker_host)
            .args(["start", &container_name])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| ApiError::Internal(format!("docker start failed: {}", e)))?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !docker_reports_missing(&stderr) {
            return Err(ApiError::Internal(format!("docker start failed: {}", stderr)));
        }
        self.recreate_missing_container(instance_id).await
    }

    async fn recreate_missing_container(&self, instance_id: &str) -> Result<(), ApiError> {
        let memory_mb: i64 = sqlx::query_scalar(
            "SELECT memory_mb FROM hosted_runtime_instances WHERE id = $1 AND status != 'destroyed'",
        )
        .bind(instance_id)
        .fetch_optional(&self.db)
        .await
        .map_err(ApiError::DatabaseError)?
        .ok_or_else(|| ApiError::NotFound("Hosted runtime not found".to_string()))?;
        // This instance was stored at 24 GB, which is larger than mail.
        // Cap the replacement box so recreate cannot OOM the node.
        let memory_mb = memory_mb.clamp(512, 2048);
        let bootstrap_token = Uuid::new_v4().to_string();
        sqlx::query(
            r#"
            UPDATE hosted_runtime_instances
            SET bootstrap_token_hash = $1,
                runtime_device_id = NULL,
                status = 'starting',
                error_message = NULL,
                stop_reason = NULL,
                updated_at = NOW()
            WHERE id = $2
            "#,
        )
        .bind(sha256_hex(bootstrap_token.as_bytes()))
        .bind(instance_id)
        .execute(&self.db)
        .await
        .map_err(ApiError::DatabaseError)?;
        tracing::info!(instance_id, "Recreating missing hosted runtime container");
        self.provision_container(instance_id, memory_mb, &bootstrap_token, &[])
            .await?;
        Ok(())
    }

    /// Stop a running workload container. No-op when it is already stopped.
    pub async fn stop(&self, instance_id: &str) -> Result<(), ApiError> {
        let container_name = Self::container_name(instance_id);
        let docker_host = self.docker_host_for_instance(instance_id).await?;
        let output = Self::docker_command(&docker_host)
            .args(["stop", &container_name])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| ApiError::Internal(format!("docker stop failed: {}", e)))?;
        if !output.status.success() {
            return Err(ApiError::Internal(format!(
                "docker stop failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(())
    }

    /// Current container status on the VPS. A container that no longer exists
    /// reports [`ContaboContainerState::Removed`].
    pub async fn status(&self, instance_id: &str) -> Result<ContaboContainerState, ApiError> {
        let container_name = Self::container_name(instance_id);
        let docker_host = self.docker_host_for_instance(instance_id).await?;
        let output = Self::docker_command(&docker_host)
            .args([
                "inspect",
                "--format",
                "{{.State.Status}}",
                &container_name,
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| ApiError::Internal(format!("docker inspect failed: {}", e)))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if docker_reports_missing(&stderr) {
                return Ok(ContaboContainerState::Removed);
            }
            return Err(ApiError::Internal(format!(
                "docker inspect failed: {}",
                stderr
            )));
        }
        let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(match state.as_str() {
            "running" => ContaboContainerState::Running,
            "created" | "restarting" => ContaboContainerState::Starting,
            "exited" | "paused" | "dead" => ContaboContainerState::Stopped,
            other => ContaboContainerState::Other(other.to_string()),
        })
    }
}

fn docker_reports_missing(stderr: &str) -> bool {
    stderr.to_ascii_lowercase().contains("no such")
}

fn sha256_hex(value: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosted_instance_type_maps_memory_to_retail_tiers() {
        assert_eq!(hosted_instance_type(256), "hosted-512mb");
        assert_eq!(hosted_instance_type(512), "hosted-512mb");
        assert_eq!(hosted_instance_type(768), "hosted-1024mb");
        assert_eq!(hosted_instance_type(1024), "hosted-1024mb");
        assert_eq!(hosted_instance_type(2048), "hosted-2048mb");
        assert_eq!(hosted_instance_type(4096), "hosted-2048mb");
    }

    #[test]
    fn hosted_disk_quota_maps_memory_to_gb() {
        assert_eq!(hosted_disk_quota_gb(512), 5);
        assert_eq!(hosted_disk_quota_gb(1024), 10);
        assert_eq!(hosted_disk_quota_gb(2048), 20);
        assert_eq!(hosted_disk_quota_gb(4096), 20);
    }

    #[test]
    fn parse_docker_size_handles_docker_human_units() {
        assert_eq!(parse_docker_size("0B"), Some(0));
        assert_eq!(parse_docker_size("12.3MB"), Some(12_300_000));
        assert_eq!(parse_docker_size("1.5GB"), Some(1_500_000_000));
        assert_eq!(parse_docker_size("2kB"), Some(2_000));
        assert_eq!(parse_docker_size("42B"), Some(42));
        assert_eq!(parse_docker_size("garbage"), None);
        assert_eq!(parse_docker_size("12XB"), None);
        assert_eq!(parse_docker_size(""), None);
    }

    async fn test_db() -> PgPool {
        let url = "postgres://allternit:allternit_pg_2026@localhost:5432/allternit_test";
        let schema = format!("test_{}", Uuid::new_v4().simple());
        let schema_for_hook = schema.clone();
        sqlx::postgres::PgPoolOptions::new()
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
            .unwrap()
    }

    #[tokio::test]
    async fn provision_creates_container_and_instance_record() {
        let db = test_db().await;
        sqlx::query(
            r#"
            CREATE TABLE hosted_runtime_nodes (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                docker_host TEXT NOT NULL,
                tailnet_ip TEXT,
                total_memory_mb BIGINT NOT NULL,
                status TEXT NOT NULL DEFAULT 'active',
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query(
            r#"
            CREATE TABLE hosted_runtime_instances (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                name TEXT NOT NULL,
                provider TEXT NOT NULL,
                region TEXT NOT NULL,
                cpu_kind TEXT NOT NULL,
                cpus BIGINT NOT NULL,
                memory_mb BIGINT NOT NULL,
                status TEXT NOT NULL,
                runtime_device_id TEXT,
                bootstrap_token_hash TEXT,
                node_id TEXT,
                cost_rate_provider TEXT,
                cost_rate_region TEXT,
                cost_rate_instance_type TEXT,
                last_synced_at TIMESTAMPTZ,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                destroyed_at TIMESTAMPTZ
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();

        let service = ContaboRuntimeService::new(
            db.clone(),
            None,
            "https://api.allternit.com".to_string(),
        );

        let runtime = service
            .provision("user_test", "test-workload", 512)
            .await
            .unwrap();

        assert!(runtime.instance_id.starts_with("hr_contabo_"));
        assert!(!runtime.container_id.is_empty());
        assert!(runtime.runtime_device_id.starts_with("rt_"));

        // Verify instance record: provision leaves the row 'starting' with the
        // bootstrap hash stored (never the raw token) and no runtime device
        // assigned — the pairing exchange owns that transition.
        let (status, runtime_device_id, stored_hash): (String, Option<String>, Option<String>) =
            sqlx::query_as(
                "SELECT status, runtime_device_id, bootstrap_token_hash FROM hosted_runtime_instances WHERE id = $1",
            )
        .bind(&runtime.instance_id)
        .fetch_one(&db)
        .await
        .unwrap();
        assert_eq!(status, "starting");
        assert!(runtime_device_id.is_none());
        assert_eq!(
            stored_hash.as_deref(),
            Some(sha256_hex(runtime.bootstrap_token.as_bytes()).as_str()),
            "bootstrap token must be stored hashed"
        );

        // Verify container exists
        let output = Command::new("docker")
            .args(["ps", "--filter", &format!("id={}", runtime.container_id), "--format", "{{.ID}}"])
            .output()
            .await
            .unwrap();
        assert!(!String::from_utf8_lossy(&output.stdout).trim().is_empty());

        // Cleanup
        service.destroy(&runtime.instance_id).await.unwrap();
    }

    /// Minimal placement schema: the nodes table plus the instance columns
    /// select_node reads.
    async fn placement_db() -> PgPool {
        let db = test_db().await;
        sqlx::query(
            r#"
            CREATE TABLE hosted_runtime_nodes (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                docker_host TEXT NOT NULL,
                tailnet_ip TEXT,
                total_memory_mb BIGINT NOT NULL,
                status TEXT NOT NULL DEFAULT 'active',
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query(
            r#"
            CREATE TABLE hosted_runtime_instances (
                id TEXT PRIMARY KEY,
                memory_mb BIGINT NOT NULL,
                status TEXT NOT NULL,
                node_id TEXT
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();
        db
    }

    async fn insert_node(db: &PgPool, id: &str, docker_host: &str, total_memory_mb: i64, status: &str) {
        sqlx::query(
            "INSERT INTO hosted_runtime_nodes (id, name, docker_host, total_memory_mb, status) VALUES ($1, $1, $2, $3, $4)",
        )
        .bind(id)
        .bind(docker_host)
        .bind(total_memory_mb)
        .bind(status)
        .execute(db)
        .await
        .unwrap();
    }

    async fn insert_instance_on(
        db: &PgPool,
        id: &str,
        memory_mb: i64,
        status: &str,
        node_id: Option<&str>,
    ) {
        sqlx::query(
            "INSERT INTO hosted_runtime_instances (id, memory_mb, status, node_id) VALUES ($1, $2, $3, $4)",
        )
        .bind(id)
        .bind(memory_mb)
        .bind(status)
        .bind(node_id)
        .execute(db)
        .await
        .unwrap();
    }

    fn placement_service(db: &PgPool) -> ContaboRuntimeService {
        ContaboRuntimeService::new(db.clone(), None, "https://api.allternit.com".to_string())
    }

    #[tokio::test]
    async fn select_node_with_empty_nodes_table_is_legacy_local() {
        let db = placement_db().await;
        let placement = placement_service(&db).select_node(1024).await.unwrap();
        assert_eq!(placement.node_id, None);
        assert_eq!(placement.docker_host, "local");
    }

    #[tokio::test]
    async fn select_node_picks_the_emptier_active_node() {
        let db = placement_db().await;
        insert_node(&db, "node-busy", "ssh://root@busy", 10240, "active").await;
        insert_node(&db, "node-idle", "ssh://root@idle", 10240, "active").await;
        // node-busy at 60% allocated; node-idle empty.
        insert_instance_on(&db, "hr_1", 6144, "running", Some("node-busy")).await;

        let placement = placement_service(&db).select_node(1024).await.unwrap();
        assert_eq!(placement.node_id.as_deref(), Some("node-idle"));
        assert_eq!(placement.docker_host, "ssh://root@idle");
    }

    #[tokio::test]
    async fn select_node_skips_nodes_over_85_percent_and_errors_when_all_full() {
        let db = placement_db().await;
        insert_node(&db, "node-full", "local", 10240, "active").await;
        insert_node(&db, "node-fits", "ssh://root@fits", 10240, "active").await;
        // node-full at 86% (8800 MB) would exceed the 85% ceiling with 1024
        // more; legacy NULL-node_id instances count against the local node.
        insert_instance_on(&db, "hr_legacy", 8800, "running", None).await;
        insert_instance_on(&db, "hr_2", 5120, "running", Some("node-fits")).await;

        let placement = placement_service(&db).select_node(1024).await.unwrap();
        assert_eq!(placement.node_id.as_deref(), Some("node-fits"));

        // A request that fits nowhere is a clear capacity error. Destroyed
        // instances must NOT count as allocated.
        insert_instance_on(&db, "hr_3", 3000, "running", Some("node-fits")).await;
        insert_instance_on(&db, "hr_gone", 9000, "destroyed", Some("node-full")).await;
        let result = placement_service(&db).select_node(1024).await;
        assert!(
            matches!(result, Err(ApiError::ServiceUnavailable(_))),
            "all nodes full must be a capacity error: {result:?}"
        );

        // Marking the legacy instance destroyed frees its allocation, so the
        // local node fits again.
        sqlx::query("UPDATE hosted_runtime_instances SET status = 'destroyed' WHERE id = 'hr_legacy'")
            .execute(&db)
            .await
            .unwrap();
        let placement = placement_service(&db).select_node(1024).await.unwrap();
        assert_eq!(placement.node_id.as_deref(), Some("node-full"));
        assert_eq!(placement.docker_host, "local");
    }

    #[tokio::test]
    async fn select_node_never_picks_draining_or_down_nodes() {
        let db = placement_db().await;
        insert_node(&db, "node-draining", "ssh://root@draining", 10240, "draining").await;
        insert_node(&db, "node-down", "ssh://root@down", 10240, "down").await;

        let result = placement_service(&db).select_node(512).await;
        assert!(
            matches!(result, Err(ApiError::ServiceUnavailable(_))),
            "draining/down nodes must never be selected: {result:?}"
        );
    }

    #[test]
    fn docker_reports_missing_is_case_insensitive() {
        assert!(docker_reports_missing(
            "error: no such object: allternit-rt-hr_contabo_mail_"
        ));
        assert!(docker_reports_missing(
            "Error response from daemon: No such container: allternit-rt-hr_contabo_mail_"
        ));
        assert!(!docker_reports_missing("permission denied"));
    }

    #[test]
    fn docker_command_shapes_args_for_local_and_ssh_hosts() {
        let local = ContaboRuntimeService::docker_command("local");
        let std = local.as_std();
        assert_eq!(std.get_program(), "docker");
        assert_eq!(std.get_args().count(), 0, "local adds no -H flag");

        let remote = ContaboRuntimeService::docker_command("ssh://root@allternit-standby");
        let std = remote.as_std();
        assert_eq!(std.get_program(), "docker");
        let args: Vec<_> = std.get_args().collect();
        assert_eq!(args, ["-H", "ssh://root@allternit-standby"]);
    }
}

