use allternit_context_router::ContextRouter;
use allternit_history::{HistoryError, HistoryLedger};
use allternit_memory::MemoryFabric;
use allternit_messaging::{EventEnvelope, MessagingSystem};
use allternit_policy::{PolicyEffect, PolicyEngine, PolicyRequest};
use allternit_providers::ProviderRouter;
use allternit_runtime_core::SessionManager;
use allternit_skills::SkillRegistry;
use allternit_workflows::WorkflowEngine;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbodimentConfig {
    pub embodiment_id: String,
    pub embodiment_type: EmbodimentType,
    pub name: String,
    pub description: String,
    pub capabilities: Vec<EmbodimentCapability>,
    pub safety_envelope: SafetyEnvelope,
    pub physical_actuation_enabled: bool,
    pub connection_config: ConnectionConfig,
    pub status: EmbodimentStatus,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EmbodimentType {
    Robot,
    IoTDevice,
    Simulator,
    VirtualAgent,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbodimentCapability {
    pub capability_id: String,
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub output_schema: serde_json::Value,
    pub safety_tier: u8,
    pub execution_time_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SafetyEnvelope {
    pub max_speed_m_s: f64,
    pub max_rotation_deg_s: f64,
    pub workspace_bounds: Option<WorkspaceBounds>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceBounds {
    pub min_x: f64,
    pub max_x: f64,
    pub min_y: f64,
    pub max_y: f64,
    pub min_z: f64,
    pub max_z: f64,
}

impl Default for SafetyEnvelope {
    fn default() -> Self {
        SafetyEnvelope {
            max_speed_m_s: 2.0,
            max_rotation_deg_s: 180.0,
            workspace_bounds: Some(WorkspaceBounds {
                min_x: -10.0,
                max_x: 10.0,
                min_y: -10.0,
                max_y: 10.0,
                min_z: -2.0,
                max_z: 5.0,
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionConfig {
    pub connection_type: ConnectionType,
    pub adapter_type: Option<AdapterType>,
    pub endpoint: String,
    pub authentication: AuthenticationConfig,
    pub heartbeat_interval_ms: u64,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConnectionType {
    WebSocket,
    HTTP,
    MQTT,
    ROS2,
    BLE,
    TCP,
    UDP,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize, Hash, PartialEq, Eq)]
pub enum AdapterType {
    Ros2,
    Mqtt,
    Ble,
    Http,
    Simulator,
    Custom(String),
}

impl AdapterType {
    pub fn from_connection(connection_type: &ConnectionType) -> AdapterType {
        match connection_type {
            ConnectionType::HTTP | ConnectionType::WebSocket => AdapterType::Http,
            ConnectionType::MQTT => AdapterType::Mqtt,
            ConnectionType::ROS2 => AdapterType::Ros2,
            ConnectionType::BLE => AdapterType::Ble,
            ConnectionType::TCP => AdapterType::Custom("tcp".to_string()),
            ConnectionType::UDP => AdapterType::Custom("udp".to_string()),
            ConnectionType::Custom => AdapterType::Custom("custom".to_string()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthenticationConfig {
    pub auth_type: AuthType,
    pub credentials: HashMap<String, String>,
    pub certificate_path: Option<String>,
    pub token_expiry: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuthType {
    APIKey,
    OAuth2,
    Certificate,
    JWT,
    Basic,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EmbodimentStatus {
    Online,
    Offline,
    Maintenance,
    Error,
    Initializing,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbodimentCommand {
    pub command_id: String,
    pub embodiment_id: String,
    pub session_id: String,
    pub tenant_id: String,
    pub agent_id: String,
    pub command_type: CommandType,
    pub parameters: serde_json::Value,
    pub priority: u8,
    pub deadline: Option<u64>,
    pub trace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CommandType {
    Move(MoveCommand),
    Speak(SpeakCommand),
    Sense(SenseCommand),
    Execute(ExecuteCommand),
    Configure(ConfigureCommand),
    Custom(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveCommand {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub rotation: Option<(f64, f64, f64)>,
    pub speed: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeakCommand {
    pub text: String,
    pub voice: Option<String>,
    pub volume: Option<f64>,
    pub language: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SenseCommand {
    pub sensor_type: String,
    pub duration_ms: u64,
    pub sample_rate: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecuteCommand {
    pub skill_name: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigureCommand {
    pub setting: String,
    pub value: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbodimentResponse {
    pub command_id: String,
    pub embodiment_id: String,
    pub response: serde_json::Value,
    pub status: ExecutionStatus,
    pub execution_time_ms: u64,
    pub safety_rating: u8,
    pub trace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ExecutionStatus {
    Success,
    PartialSuccess,
    Failure,
    Timeout,
    Cancelled,
    SafetyViolation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbodimentState {
    pub embodiment_id: String,
    pub position: Option<(f64, f64, f64)>,
    pub rotation: Option<(f64, f64, f64)>,
    pub battery_level: Option<f64>,
    pub sensors: HashMap<String, serde_json::Value>,
    pub last_heartbeat: Option<u64>,
    pub connected: bool,
    pub capabilities: Vec<EmbodimentCapability>,
    pub active_commands: Vec<String>,
    pub estop_engaged: bool,
    pub estop_reason: Option<String>,
    pub estop_updated_at: Option<u64>,
    pub firmware_version: Option<String>,
    pub pending_firmware_version: Option<String>,
    pub last_updated: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum EmbodimentError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("History error: {0}")]
    History(#[from] HistoryError),
    #[error("SQLX error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("Policy error: {0}")]
    Policy(#[from] allternit_policy::PolicyError),
    #[error("Provider error: {0}")]
    Provider(#[from] allternit_providers::ProviderError),
    #[error("Embodiment not found: {0}")]
    EmbodimentNotFound(String),
    #[error("Embodiment offline: {0}")]
    EmbodimentOffline(String),
    #[error("Command failed: {0}")]
    CommandFailed(String),
    #[error("Adapter not found: {0}")]
    AdapterNotFound(String),
    #[error("OTA update not found: {0}")]
    OtaUpdateNotFound(String),
    #[error("OTA update failed: {0}")]
    OtaUpdateFailed(String),
    #[error("Authentication failed: {0}")]
    AuthenticationFailed(String),
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),
    #[error("Invalid command: {0}")]
    InvalidCommand(String),
    #[error("Safety violation: {0}")]
    SafetyViolation(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtaUpdateRequest {
    pub embodiment_id: String,
    pub artifact_id: String,
    pub version: String,
    pub signature: Option<String>,
    pub rollout: RolloutStrategy,
    pub session_id: String,
    pub tenant_id: String,
    pub actor_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtaUpdate {
    pub update_id: String,
    pub embodiment_id: String,
    pub artifact_id: String,
    pub version: String,
    pub signature: Option<String>,
    pub status: OtaStatus,
    pub rollout: RolloutStrategy,
    pub session_id: String,
    pub tenant_id: String,
    pub actor_id: String,
    pub staged_at: u64,
    pub applied_at: Option<u64>,
    pub verified_at: Option<u64>,
    pub rolled_back_at: Option<u64>,
    pub previous_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OtaStatus {
    Staged,
    Applied,
    Verified,
    RolledBack,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RolloutStrategy {
    Immediate,
    Canary { percent: u8 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtaUpdateResult {
    pub success: bool,
    pub details: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryFrame {
    pub embodiment_id: String,
    pub captured_at: u64,
    pub payload: serde_json::Value,
}

#[async_trait::async_trait]
pub trait DeviceAdapter: Send + Sync {
    fn adapter_type(&self) -> AdapterType;
    async fn connect(&self, _config: &EmbodimentConfig) -> Result<(), EmbodimentError> {
        Ok(())
    }
    async fn disconnect(&self, _embodiment_id: &str) -> Result<(), EmbodimentError> {
        Ok(())
    }
    async fn send_command(
        &self,
        config: &EmbodimentConfig,
        command: &EmbodimentCommand,
    ) -> Result<serde_json::Value, EmbodimentError>;
    async fn read_telemetry(&self, embodiment_id: &str) -> Result<TelemetryFrame, EmbodimentError>;
    async fn perform_ota(
        &self,
        config: &EmbodimentConfig,
        update: &OtaUpdate,
    ) -> Result<OtaUpdateResult, EmbodimentError>;
}

// Storage trait for embodiment configurations
#[async_trait::async_trait]
pub trait EmbodimentStorage: Send + Sync {
    async fn store_embodiment(&self, config: &EmbodimentConfig) -> Result<(), EmbodimentError>;
    async fn get_embodiment(
        &self,
        embodiment_id: &str,
    ) -> Result<Option<EmbodimentConfig>, EmbodimentError>;
    async fn update_embodiment(&self, config: &EmbodimentConfig) -> Result<(), EmbodimentError>;
    async fn delete_embodiment(&self, embodiment_id: &str) -> Result<(), EmbodimentError>;

    async fn store_state(&self, state: &EmbodimentState) -> Result<(), EmbodimentError>;
    async fn get_state(
        &self,
        embodiment_id: &str,
    ) -> Result<Option<EmbodimentState>, EmbodimentError>;
    async fn update_state(&self, state: &EmbodimentState) -> Result<(), EmbodimentError>;

    async fn store_command(&self, command: &EmbodimentCommand) -> Result<(), EmbodimentError>;
    async fn get_command(
        &self,
        command_id: &str,
    ) -> Result<Option<EmbodimentCommand>, EmbodimentError>;
    async fn update_command_status(
        &self,
        command_id: &str,
        status: ExecutionStatus,
    ) -> Result<(), EmbodimentError>;

    async fn get_embodiment_stats(
        &self,
        embodiment_id: &str,
    ) -> Result<EmbodimentStats, EmbodimentError>;
    async fn update_embodiment_stats(&self, stats: &EmbodimentStats)
        -> Result<(), EmbodimentError>;
}

pub struct SqliteEmbodimentStorage {
    pool: SqlitePool,
}

impl SqliteEmbodimentStorage {
    pub async fn new(pool: SqlitePool) -> Result<Self, EmbodimentError> {
        // Create the embodiments table if it doesn't exist
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS embodiments (
                embodiment_id TEXT PRIMARY KEY,
                embodiment_type TEXT NOT NULL,
                name TEXT NOT NULL,
                description TEXT NOT NULL,
                capabilities TEXT NOT NULL,
                safety_envelope TEXT NOT NULL,
                physical_actuation_enabled INTEGER NOT NULL,
                connection_config TEXT NOT NULL,
                status TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .map_err(EmbodimentError::Sqlx)?;

        // Create the embodiment_states table if it doesn't exist
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS embodiment_states (
                embodiment_id TEXT PRIMARY KEY,
                position TEXT,
                rotation TEXT,
                battery_level REAL,
                sensors TEXT NOT NULL,
                last_heartbeat INTEGER,
                connected BOOLEAN NOT NULL,
                capabilities TEXT NOT NULL,
                active_commands TEXT NOT NULL,
                estop_engaged INTEGER NOT NULL,
                estop_reason TEXT,
                estop_updated_at INTEGER,
                firmware_version TEXT,
                pending_firmware_version TEXT,
                last_updated INTEGER NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .map_err(EmbodimentError::Sqlx)?;

        // Create the commands table if it doesn't exist
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS commands (
                command_id TEXT PRIMARY KEY,
                embodiment_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                tenant_id TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                command_type TEXT NOT NULL,
                parameters TEXT NOT NULL,
                priority INTEGER NOT NULL,
                deadline INTEGER,
                status TEXT NOT NULL,
                trace_id TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .map_err(EmbodimentError::Sqlx)?;

        // Create indexes for better query performance
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_embodiments_status ON embodiments(status)")
            .execute(&pool)
            .await
            .map_err(EmbodimentError::Sqlx)?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_commands_embodiment ON commands(embodiment_id)",
        )
        .execute(&pool)
        .await
        .map_err(EmbodimentError::Sqlx)?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_commands_status ON commands(status)")
            .execute(&pool)
            .await
            .map_err(EmbodimentError::Sqlx)?;

        Self::ensure_column(&pool, "embodiments", "safety_envelope", "TEXT").await?;
        Self::ensure_column(
            &pool,
            "embodiments",
            "physical_actuation_enabled",
            "INTEGER DEFAULT 0",
        )
        .await?;
        Self::ensure_column(
            &pool,
            "embodiment_states",
            "estop_engaged",
            "INTEGER DEFAULT 0",
        )
        .await?;
        Self::ensure_column(&pool, "embodiment_states", "estop_reason", "TEXT").await?;
        Self::ensure_column(&pool, "embodiment_states", "estop_updated_at", "INTEGER").await?;
        Self::ensure_column(&pool, "embodiment_states", "firmware_version", "TEXT").await?;
        Self::ensure_column(
            &pool,
            "embodiment_states",
            "pending_firmware_version",
            "TEXT",
        )
        .await?;

        Ok(SqliteEmbodimentStorage { pool })
    }

    async fn ensure_column(
        pool: &SqlitePool,
        table: &str,
        column: &str,
        definition: &str,
    ) -> Result<(), EmbodimentError> {
        let pragma = format!("PRAGMA table_info({})", table);
        let rows = sqlx::query(&pragma)
            .fetch_all(pool)
            .await
            .map_err(EmbodimentError::Sqlx)?;
        let exists = rows.iter().any(|row| {
            let name: String = row.get("name");
            name == column
        });
        if !exists {
            let statement = format!("ALTER TABLE {} ADD COLUMN {} {}", table, column, definition);
            sqlx::query(&statement)
                .execute(pool)
                .await
                .map_err(EmbodimentError::Sqlx)?;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl EmbodimentStorage for SqliteEmbodimentStorage {
    async fn store_embodiment(&self, config: &EmbodimentConfig) -> Result<(), EmbodimentError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let capabilities_json =
            serde_json::to_string(&config.capabilities).map_err(EmbodimentError::Json)?;
        let safety_envelope_json =
            serde_json::to_string(&config.safety_envelope).map_err(EmbodimentError::Json)?;
        let actuation_enabled = if config.physical_actuation_enabled {
            1
        } else {
            0
        };
        let connection_config_json =
            serde_json::to_string(&config.connection_config).map_err(EmbodimentError::Json)?;
        let status_str = format!("{:?}", config.status);

        sqlx::query(
            "INSERT OR REPLACE INTO embodiments (
                embodiment_id, embodiment_type, name, description, capabilities,
                safety_envelope, physical_actuation_enabled, connection_config,
                status, created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&config.embodiment_id)
        .bind(format!("{:?}", config.embodiment_type))
        .bind(&config.name)
        .bind(&config.description)
        .bind(&capabilities_json)
        .bind(&safety_envelope_json)
        .bind(actuation_enabled)
        .bind(&connection_config_json)
        .bind(&status_str)
        .bind(now as i64)
        .bind(now as i64)
        .execute(&self.pool)
        .await
        .map_err(EmbodimentError::Sqlx)?;

        Ok(())
    }

    async fn get_embodiment(
        &self,
        embodiment_id: &str,
    ) -> Result<Option<EmbodimentConfig>, EmbodimentError> {
        let row = sqlx::query(
            "SELECT embodiment_id, embodiment_type, name, description, capabilities, safety_envelope, physical_actuation_enabled, connection_config, status, created_at, updated_at
             FROM embodiments WHERE embodiment_id = ?"
        )
        .bind(embodiment_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(EmbodimentError::Sqlx)?;

        if let Some(row) = row {
            let capabilities: Vec<EmbodimentCapability> =
                serde_json::from_str(row.get::<&str, _>("capabilities"))
                    .map_err(EmbodimentError::Json)?;
            let safety_envelope = match row.try_get::<Option<String>, _>("safety_envelope") {
                Ok(Some(json)) => serde_json::from_str(&json).unwrap_or_default(),
                Ok(None) => SafetyEnvelope::default(),
                Err(_) => SafetyEnvelope::default(),
            };
            let physical_actuation_enabled =
                match row.try_get::<Option<i64>, _>("physical_actuation_enabled") {
                    Ok(Some(value)) => value != 0,
                    Ok(None) => false,
                    Err(_) => false,
                };
            let connection_config: ConnectionConfig =
                serde_json::from_str(row.get::<&str, _>("connection_config"))
                    .map_err(EmbodimentError::Json)?;

            let embodiment_type_str: String = row.get("embodiment_type");
            let embodiment_type = match embodiment_type_str.as_str() {
                "Robot" => EmbodimentType::Robot,
                "IoTDevice" => EmbodimentType::IoTDevice,
                "Simulator" => EmbodimentType::Simulator,
                "VirtualAgent" => EmbodimentType::VirtualAgent,
                "Custom" => EmbodimentType::Custom,
                _ => {
                    return Err(EmbodimentError::CommandFailed(
                        "Invalid embodiment type".to_string(),
                    ))
                }
            };

            let status_str: String = row.get("status");
            let status = match status_str.as_str() {
                "Online" => EmbodimentStatus::Online,
                "Offline" => EmbodimentStatus::Offline,
                "Maintenance" => EmbodimentStatus::Maintenance,
                "Error" => EmbodimentStatus::Error,
                "Initializing" => EmbodimentStatus::Initializing,
                _ => {
                    return Err(EmbodimentError::CommandFailed(
                        "Invalid embodiment status".to_string(),
                    ))
                }
            };

            let config = EmbodimentConfig {
                embodiment_id: row.get("embodiment_id"),
                embodiment_type,
                name: row.get("name"),
                description: row.get("description"),
                capabilities,
                safety_envelope,
                physical_actuation_enabled,
                connection_config,
                status,
                created_at: row.get::<i64, _>("created_at") as u64,
                updated_at: row.get::<i64, _>("updated_at") as u64,
            };

            Ok(Some(config))
        } else {
            Ok(None)
        }
    }

    async fn update_embodiment(&self, config: &EmbodimentConfig) -> Result<(), EmbodimentError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let capabilities_json =
            serde_json::to_string(&config.capabilities).map_err(EmbodimentError::Json)?;
        let safety_envelope_json =
            serde_json::to_string(&config.safety_envelope).map_err(EmbodimentError::Json)?;
        let actuation_enabled = if config.physical_actuation_enabled {
            1
        } else {
            0
        };
        let connection_config_json =
            serde_json::to_string(&config.connection_config).map_err(EmbodimentError::Json)?;
        let status_str = format!("{:?}", config.status);

        sqlx::query(
            "UPDATE embodiments SET
                embodiment_type = ?, name = ?, description = ?, capabilities = ?,
                safety_envelope = ?, physical_actuation_enabled = ?, connection_config = ?,
                status = ?, updated_at = ?
             WHERE embodiment_id = ?",
        )
        .bind(format!("{:?}", config.embodiment_type))
        .bind(&config.name)
        .bind(&config.description)
        .bind(&capabilities_json)
        .bind(&safety_envelope_json)
        .bind(actuation_enabled)
        .bind(&connection_config_json)
        .bind(&status_str)
        .bind(now as i64)
        .bind(&config.embodiment_id)
        .execute(&self.pool)
        .await
        .map_err(EmbodimentError::Sqlx)?;

        Ok(())
    }

    async fn delete_embodiment(&self, embodiment_id: &str) -> Result<(), EmbodimentError> {
        sqlx::query("DELETE FROM embodiments WHERE embodiment_id = ?")
            .bind(embodiment_id)
            .execute(&self.pool)
            .await
            .map_err(EmbodimentError::Sqlx)?;

        Ok(())
    }

    async fn store_state(&self, state: &EmbodimentState) -> Result<(), EmbodimentError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let position_json = state
            .position
            .map(|p| serde_json::to_string(&p))
            .transpose()
            .map_err(EmbodimentError::Json)?;
        let rotation_json = state
            .rotation
            .map(|r| serde_json::to_string(&r))
            .transpose()
            .map_err(EmbodimentError::Json)?;
        let sensors_json = serde_json::to_string(&state.sensors).map_err(EmbodimentError::Json)?;
        let capabilities_json =
            serde_json::to_string(&state.capabilities).map_err(EmbodimentError::Json)?;
        let active_commands_json =
            serde_json::to_string(&state.active_commands).map_err(EmbodimentError::Json)?;
        let estop_engaged = if state.estop_engaged { 1 } else { 0 };

        sqlx::query(
            "INSERT OR REPLACE INTO embodiment_states (
                embodiment_id, position, rotation, battery_level, sensors,
                last_heartbeat, connected, capabilities, active_commands,
                estop_engaged, estop_reason, estop_updated_at,
                firmware_version, pending_firmware_version, last_updated
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&state.embodiment_id)
        .bind(position_json)
        .bind(rotation_json)
        .bind(state.battery_level)
        .bind(&sensors_json)
        .bind(state.last_heartbeat.map(|t| t as i64))
        .bind(state.connected)
        .bind(&capabilities_json)
        .bind(&active_commands_json)
        .bind(estop_engaged)
        .bind(&state.estop_reason)
        .bind(state.estop_updated_at.map(|value| value as i64))
        .bind(&state.firmware_version)
        .bind(&state.pending_firmware_version)
        .bind(now as i64)
        .execute(&self.pool)
        .await
        .map_err(EmbodimentError::Sqlx)?;

        Ok(())
    }

    async fn get_state(
        &self,
        embodiment_id: &str,
    ) -> Result<Option<EmbodimentState>, EmbodimentError> {
        let row = sqlx::query(
            "SELECT embodiment_id, position, rotation, battery_level, sensors, last_heartbeat, connected, capabilities, active_commands,
                    estop_engaged, estop_reason, estop_updated_at, firmware_version, pending_firmware_version, last_updated
             FROM embodiment_states WHERE embodiment_id = ?"
        )
        .bind(embodiment_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(EmbodimentError::Sqlx)?;

        if let Some(row) = row {
            let position: Option<(f64, f64, f64)> =
                if let Some(pos_str) = row.get::<Option<String>, _>("position") {
                    Some(serde_json::from_str(&pos_str).map_err(EmbodimentError::Json)?)
                } else {
                    None
                };

            let rotation: Option<(f64, f64, f64)> =
                if let Some(rot_str) = row.get::<Option<String>, _>("rotation") {
                    Some(serde_json::from_str(&rot_str).map_err(EmbodimentError::Json)?)
                } else {
                    None
                };

            let sensors: HashMap<String, serde_json::Value> =
                serde_json::from_str(row.get::<&str, _>("sensors"))
                    .map_err(EmbodimentError::Json)?;
            let capabilities: Vec<EmbodimentCapability> =
                serde_json::from_str(row.get::<&str, _>("capabilities"))
                    .map_err(EmbodimentError::Json)?;
            let active_commands: Vec<String> =
                serde_json::from_str(row.get::<&str, _>("active_commands"))
                    .map_err(EmbodimentError::Json)?;
            let estop_engaged = match row.try_get::<Option<i64>, _>("estop_engaged") {
                Ok(Some(value)) => value != 0,
                Ok(None) => false,
                Err(_) => false,
            };
            let estop_reason = row
                .try_get::<Option<String>, _>("estop_reason")
                .ok()
                .flatten();
            let estop_updated_at = row
                .try_get::<Option<i64>, _>("estop_updated_at")
                .ok()
                .flatten()
                .map(|value| value as u64);
            let firmware_version = row
                .try_get::<Option<String>, _>("firmware_version")
                .ok()
                .flatten();
            let pending_firmware_version = row
                .try_get::<Option<String>, _>("pending_firmware_version")
                .ok()
                .flatten();

            let state = EmbodimentState {
                embodiment_id: row.get("embodiment_id"),
                position,
                rotation,
                battery_level: row.get("battery_level"),
                sensors,
                last_heartbeat: row
                    .get::<Option<i64>, _>("last_heartbeat")
                    .map(|t| t as u64),
                connected: row.get("connected"),
                capabilities,
                active_commands,
                estop_engaged,
                estop_reason,
                estop_updated_at,
                firmware_version,
                pending_firmware_version,
                last_updated: row.get::<i64, _>("last_updated") as u64,
            };

            Ok(Some(state))
        } else {
            Ok(None)
        }
    }

    async fn update_state(&self, state: &EmbodimentState) -> Result<(), EmbodimentError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let position_json = state
            .position
            .map(|p| serde_json::to_string(&p))
            .transpose()
            .map_err(EmbodimentError::Json)?;
        let rotation_json = state
            .rotation
            .map(|r| serde_json::to_string(&r))
            .transpose()
            .map_err(EmbodimentError::Json)?;
        let sensors_json = serde_json::to_string(&state.sensors).map_err(EmbodimentError::Json)?;
        let capabilities_json =
            serde_json::to_string(&state.capabilities).map_err(EmbodimentError::Json)?;
        let active_commands_json =
            serde_json::to_string(&state.active_commands).map_err(EmbodimentError::Json)?;
        let estop_engaged = if state.estop_engaged { 1 } else { 0 };

        sqlx::query(
            "UPDATE embodiment_states SET
                position = ?, rotation = ?, battery_level = ?, sensors = ?,
                last_heartbeat = ?, connected = ?, capabilities = ?, active_commands = ?,
                estop_engaged = ?, estop_reason = ?, estop_updated_at = ?,
                firmware_version = ?, pending_firmware_version = ?, last_updated = ?
             WHERE embodiment_id = ?",
        )
        .bind(position_json)
        .bind(rotation_json)
        .bind(state.battery_level)
        .bind(&sensors_json)
        .bind(state.last_heartbeat.map(|t| t as i64))
        .bind(state.connected)
        .bind(&capabilities_json)
        .bind(&active_commands_json)
        .bind(estop_engaged)
        .bind(&state.estop_reason)
        .bind(state.estop_updated_at.map(|value| value as i64))
        .bind(&state.firmware_version)
        .bind(&state.pending_firmware_version)
        .bind(now as i64)
        .bind(&state.embodiment_id)
        .execute(&self.pool)
        .await
        .map_err(EmbodimentError::Sqlx)?;

        Ok(())
    }

    async fn store_command(&self, command: &EmbodimentCommand) -> Result<(), EmbodimentError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let command_type_json =
            serde_json::to_string(&command.command_type).map_err(EmbodimentError::Json)?;
        let parameters_json =
            serde_json::to_string(&command.parameters).map_err(EmbodimentError::Json)?;
        let status_str = format!("{:?}", ExecutionStatus::Success); // Default status

        sqlx::query(
            "INSERT OR REPLACE INTO commands (
                command_id, embodiment_id, session_id, tenant_id, agent_id,
                command_type, parameters, priority, deadline, status, trace_id, created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(&command.command_id)
        .bind(&command.embodiment_id)
        .bind(&command.session_id)
        .bind(&command.tenant_id)
        .bind(&command.agent_id)
        .bind(&command_type_json)
        .bind(&parameters_json)
        .bind(command.priority as i32)
        .bind(command.deadline.map(|t| t as i64))
        .bind(&status_str)
        .bind(&command.trace_id)
        .bind(now as i64)
        .bind(now as i64)
        .execute(&self.pool)
        .await
        .map_err(EmbodimentError::Sqlx)?;

        Ok(())
    }

    async fn get_command(
        &self,
        command_id: &str,
    ) -> Result<Option<EmbodimentCommand>, EmbodimentError> {
        let row = sqlx::query(
            "SELECT command_id, embodiment_id, session_id, tenant_id, agent_id, command_type, parameters, priority, deadline, trace_id, created_at, updated_at
             FROM commands WHERE command_id = ?"
        )
        .bind(command_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(EmbodimentError::Sqlx)?;

        if let Some(row) = row {
            let command_type: CommandType =
                serde_json::from_str(row.get::<&str, _>("command_type"))
                    .map_err(EmbodimentError::Json)?;
            let parameters: serde_json::Value =
                serde_json::from_str(row.get::<&str, _>("parameters"))
                    .map_err(EmbodimentError::Json)?;

            let command = EmbodimentCommand {
                command_id: row.get("command_id"),
                embodiment_id: row.get("embodiment_id"),
                session_id: row.get("session_id"),
                tenant_id: row.get("tenant_id"),
                agent_id: row.get("agent_id"),
                command_type,
                parameters,
                priority: row.get::<i32, _>("priority") as u8,
                deadline: row.get::<Option<i64>, _>("deadline").map(|t| t as u64),
                trace_id: row.get("trace_id"),
            };

            Ok(Some(command))
        } else {
            Ok(None)
        }
    }

    async fn update_command_status(
        &self,
        command_id: &str,
        status: ExecutionStatus,
    ) -> Result<(), EmbodimentError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let status_str = format!("{:?}", status);

        sqlx::query("UPDATE commands SET status = ?, updated_at = ? WHERE command_id = ?")
            .bind(&status_str)
            .bind(now as i64)
            .bind(command_id)
            .execute(&self.pool)
            .await
            .map_err(EmbodimentError::Sqlx)?;

        Ok(())
    }

    async fn get_embodiment_stats(
        &self,
        embodiment_id: &str,
    ) -> Result<EmbodimentStats, EmbodimentError> {
        // In a real implementation, this would fetch from a stats table
        // For now, return default stats
        Ok(EmbodimentStats {
            embodiment_id: embodiment_id.to_string(),
            total_commands: 0,
            total_execution_time: 0,
            avg_execution_time: 0.0,
            success_rate: 1.0, // Initially assume 100% success
            last_command: None,
            error_count: 0,
            uptime_percentage: 100.0,
        })
    }

    async fn update_embodiment_stats(
        &self,
        stats: &EmbodimentStats,
    ) -> Result<(), EmbodimentError> {
        // In a real implementation, this would update a stats table
        // For now, we'll just return Ok
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbodimentStats {
    pub embodiment_id: String,
    pub total_commands: u64,
    pub total_execution_time: u64,
    pub avg_execution_time: f64,
    pub success_rate: f64,
    pub last_command: Option<u64>,
    pub error_count: u64,
    pub uptime_percentage: f64,
}

fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn render_command_result(command: &EmbodimentCommand) -> serde_json::Value {
    match &command.command_type {
        CommandType::Move(move_cmd) => serde_json::json!({
            "status": "executed",
            "position": [move_cmd.x, move_cmd.y, move_cmd.z],
            "rotation": move_cmd.rotation,
            "speed": move_cmd.speed,
        }),
        CommandType::Speak(speak_cmd) => serde_json::json!({
            "status": "spoken",
            "text": speak_cmd.text,
            "voice": speak_cmd.voice,
            "volume": speak_cmd.volume,
            "language": speak_cmd.language,
        }),
        CommandType::Sense(sense_cmd) => serde_json::json!({
            "status": "sensed",
            "sensor_type": sense_cmd.sensor_type,
            "duration_ms": sense_cmd.duration_ms,
            "sample_rate": sense_cmd.sample_rate,
            "data": "simulated_sensor_data",
        }),
        CommandType::Execute(exec_cmd) => serde_json::json!({
            "status": "executed",
            "skill_name": exec_cmd.skill_name,
            "result": "simulated_skill_result",
        }),
        CommandType::Configure(config_cmd) => serde_json::json!({
            "status": "configured",
            "setting": config_cmd.setting,
            "value": config_cmd.value,
        }),
        CommandType::Custom(custom_cmd) => serde_json::json!({
            "status": "executed",
            "command": custom_cmd,
            "result": "custom_command_result",
        }),
    }
}

fn adapter_telemetry(embodiment_id: &str, adapter: &str) -> TelemetryFrame {
    TelemetryFrame {
        embodiment_id: embodiment_id.to_string(),
        captured_at: now_unix_secs(),
        payload: serde_json::json!({
            "adapter": adapter,
            "status": "telemetry_snapshot",
        }),
    }
}

pub struct Ros2Adapter;
pub struct MqttAdapter;
pub struct BleAdapter;
pub struct HttpAdapter;

#[async_trait::async_trait]
impl DeviceAdapter for Ros2Adapter {
    fn adapter_type(&self) -> AdapterType {
        AdapterType::Ros2
    }

    async fn send_command(
        &self,
        _config: &EmbodimentConfig,
        command: &EmbodimentCommand,
    ) -> Result<serde_json::Value, EmbodimentError> {
        Ok(render_command_result(command))
    }

    async fn read_telemetry(&self, embodiment_id: &str) -> Result<TelemetryFrame, EmbodimentError> {
        Ok(adapter_telemetry(embodiment_id, "ros2"))
    }

    async fn perform_ota(
        &self,
        _config: &EmbodimentConfig,
        _update: &OtaUpdate,
    ) -> Result<OtaUpdateResult, EmbodimentError> {
        Ok(OtaUpdateResult {
            success: true,
            details: Some("simulated ros2 ota".to_string()),
        })
    }
}

#[async_trait::async_trait]
impl DeviceAdapter for MqttAdapter {
    fn adapter_type(&self) -> AdapterType {
        AdapterType::Mqtt
    }

    async fn send_command(
        &self,
        _config: &EmbodimentConfig,
        command: &EmbodimentCommand,
    ) -> Result<serde_json::Value, EmbodimentError> {
        Ok(render_command_result(command))
    }

    async fn read_telemetry(&self, embodiment_id: &str) -> Result<TelemetryFrame, EmbodimentError> {
        Ok(adapter_telemetry(embodiment_id, "mqtt"))
    }

    async fn perform_ota(
        &self,
        _config: &EmbodimentConfig,
        _update: &OtaUpdate,
    ) -> Result<OtaUpdateResult, EmbodimentError> {
        Ok(OtaUpdateResult {
            success: true,
            details: Some("simulated mqtt ota".to_string()),
        })
    }
}

#[async_trait::async_trait]
impl DeviceAdapter for BleAdapter {
    fn adapter_type(&self) -> AdapterType {
        AdapterType::Ble
    }

    async fn send_command(
        &self,
        _config: &EmbodimentConfig,
        command: &EmbodimentCommand,
    ) -> Result<serde_json::Value, EmbodimentError> {
        Ok(render_command_result(command))
    }

    async fn read_telemetry(&self, embodiment_id: &str) -> Result<TelemetryFrame, EmbodimentError> {
        Ok(adapter_telemetry(embodiment_id, "ble"))
    }

    async fn perform_ota(
        &self,
        _config: &EmbodimentConfig,
        _update: &OtaUpdate,
    ) -> Result<OtaUpdateResult, EmbodimentError> {
        Ok(OtaUpdateResult {
            success: true,
            details: Some("simulated ble ota".to_string()),
        })
    }
}

#[async_trait::async_trait]
impl DeviceAdapter for HttpAdapter {
    fn adapter_type(&self) -> AdapterType {
        AdapterType::Http
    }

    async fn send_command(
        &self,
        _config: &EmbodimentConfig,
        command: &EmbodimentCommand,
    ) -> Result<serde_json::Value, EmbodimentError> {
        Ok(render_command_result(command))
    }

    async fn read_telemetry(&self, embodiment_id: &str) -> Result<TelemetryFrame, EmbodimentError> {
        Ok(adapter_telemetry(embodiment_id, "http"))
    }

    async fn perform_ota(
        &self,
        _config: &EmbodimentConfig,
        _update: &OtaUpdate,
    ) -> Result<OtaUpdateResult, EmbodimentError> {
        Ok(OtaUpdateResult {
            success: true,
            details: Some("simulated http ota".to_string()),
        })
    }
}

pub struct EmbodimentControlPlane {
    embodiments: Arc<RwLock<HashMap<String, EmbodimentConfig>>>,
    states: Arc<RwLock<HashMap<String, EmbodimentState>>>,
    history_ledger: Arc<Mutex<HistoryLedger>>,
    messaging_system: Arc<MessagingSystem>,
    policy_engine: Arc<PolicyEngine>,
    context_router: Arc<ContextRouter>,
    memory_fabric: Arc<MemoryFabric>,
    provider_router: Arc<ProviderRouter>,
    skill_registry: Arc<SkillRegistry>,
    workflow_engine: Arc<WorkflowEngine>,
    session_manager: Arc<SessionManager>,
    storage: Arc<dyn EmbodimentStorage>,
    simulators: Arc<RwLock<HashMap<String, Arc<dyn EmbodimentSimulator>>>>,
    adapters: Arc<RwLock<HashMap<AdapterType, Arc<dyn DeviceAdapter>>>>,
    ota_updates: Arc<RwLock<HashMap<String, OtaUpdate>>>,
}

impl EmbodimentControlPlane {
    pub async fn new_with_storage(
        history_ledger: Arc<Mutex<HistoryLedger>>,
        messaging_system: Arc<MessagingSystem>,
        policy_engine: Arc<PolicyEngine>,
        context_router: Arc<ContextRouter>,
        memory_fabric: Arc<MemoryFabric>,
        provider_router: Arc<ProviderRouter>,
        skill_registry: Arc<SkillRegistry>,
        workflow_engine: Arc<WorkflowEngine>,
        session_manager: Arc<SessionManager>,
        pool: SqlitePool,
    ) -> Result<Self, EmbodimentError> {
        let storage = Arc::new(SqliteEmbodimentStorage::new(pool).await?);

        // Load existing embodiments and states from storage
        let embodiments_map = HashMap::new();
        let states_map = HashMap::new();

        // In a real implementation, we would load from storage
        // For now, we'll initialize empty maps
        let mut adapters_map: HashMap<AdapterType, Arc<dyn DeviceAdapter>> = HashMap::new();
        adapters_map.insert(AdapterType::Ros2, Arc::new(Ros2Adapter));
        adapters_map.insert(AdapterType::Mqtt, Arc::new(MqttAdapter));
        adapters_map.insert(AdapterType::Ble, Arc::new(BleAdapter));
        adapters_map.insert(AdapterType::Http, Arc::new(HttpAdapter));

        Ok(EmbodimentControlPlane {
            embodiments: Arc::new(RwLock::new(embodiments_map)),
            states: Arc::new(RwLock::new(states_map)),
            history_ledger,
            messaging_system,
            policy_engine,
            context_router,
            memory_fabric,
            provider_router,
            skill_registry,
            workflow_engine,
            session_manager,
            storage,
            simulators: Arc::new(RwLock::new(HashMap::new())),
            adapters: Arc::new(RwLock::new(adapters_map)),
            ota_updates: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    pub async fn register_embodiment(
        &self,
        config: EmbodimentConfig,
    ) -> Result<String, EmbodimentError> {
        // Validate embodiment config
        self.validate_embodiment_config(&config).await?;

        // Store in durable storage
        self.storage.store_embodiment(&config).await?;

        // Update in-memory cache
        let mut embodiments = self.embodiments.write().await;
        embodiments.insert(config.embodiment_id.clone(), config.clone());

        // Initialize state for the embodiment
        let initial_state = EmbodimentState {
            embodiment_id: config.embodiment_id.clone(),
            position: None,
            rotation: None,
            battery_level: None,
            sensors: HashMap::new(),
            last_heartbeat: None,
            connected: false,
            capabilities: config.capabilities.clone(),
            active_commands: Vec::new(),
            estop_engaged: false,
            estop_reason: None,
            estop_updated_at: None,
            firmware_version: None,
            pending_firmware_version: None,
            last_updated: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        };

        self.storage.store_state(&initial_state).await?;
        {
            let mut states = self.states.write().await;
            states.insert(config.embodiment_id.clone(), initial_state);
        }

        // Log the event
        let event = EventEnvelope {
            event_id: Uuid::new_v4().to_string(),
            event_type: "EmbodimentRegistered".to_string(),
            session_id: "system".to_string(), // System event
            tenant_id: "system".to_string(),
            actor_id: "system".to_string(), // Would be the actual creator in real implementation
            role: "embodiment_control_plane".to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            trace_id: None,
            payload: serde_json::json!({
                "embodiment_id": config.embodiment_id,
                "embodiment_type": format!("{:?}", config.embodiment_type),
                "name": config.name,
                "capabilities": config.capabilities,
            }),
        };

        // Log to history ledger
        {
            let mut history = self.history_ledger.lock().unwrap();
            let content = serde_json::to_value(&event)?;
            history.append(content)?;
        }

        // Emit event asynchronously
        tokio::spawn({
            let event_bus = self.messaging_system.event_bus.clone();
            let event_to_send = event.clone();
            async move {
                let _ = event_bus.publish(event_to_send).await;
            }
        });

        Ok(config.embodiment_id)
    }

    async fn validate_embodiment_config(
        &self,
        config: &EmbodimentConfig,
    ) -> Result<(), EmbodimentError> {
        // Validate embodiment ID
        if config.embodiment_id.is_empty() {
            return Err(EmbodimentError::CommandFailed(
                "Embodiment ID cannot be empty".to_string(),
            ));
        }

        // Validate name
        if config.name.is_empty() {
            return Err(EmbodimentError::CommandFailed(
                "Embodiment name cannot be empty".to_string(),
            ));
        }

        // Validate connection config
        if config.connection_config.endpoint.is_empty() {
            return Err(EmbodimentError::CommandFailed(
                "Embodiment endpoint cannot be empty".to_string(),
            ));
        }

        // Validate capabilities
        if config.capabilities.is_empty() {
            return Err(EmbodimentError::CommandFailed(
                "Embodiment must have at least one capability".to_string(),
            ));
        }

        // Validate each capability
        for cap in &config.capabilities {
            if cap.capability_id.is_empty() {
                return Err(EmbodimentError::CommandFailed(
                    "Capability ID cannot be empty".to_string(),
                ));
            }

            if cap.name.is_empty() {
                return Err(EmbodimentError::CommandFailed(
                    "Capability name cannot be empty".to_string(),
                ));
            }
        }

        Ok(())
    }

    fn is_physical_embodiment(embodiment_type: &EmbodimentType) -> bool {
        matches!(
            embodiment_type,
            EmbodimentType::Robot | EmbodimentType::IoTDevice | EmbodimentType::Custom
        )
    }

    fn command_simulation_passed(command: &EmbodimentCommand) -> bool {
        command
            .parameters
            .get("simulation_passed")
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
    }

    fn validate_safety_envelope(
        &self,
        envelope: &SafetyEnvelope,
        command: &EmbodimentCommand,
    ) -> Result<(), EmbodimentError> {
        if let CommandType::Move(move_cmd) = &command.command_type {
            if let Some(speed) = move_cmd.speed {
                if speed > envelope.max_speed_m_s {
                    return Err(EmbodimentError::SafetyViolation(format!(
                        "Speed {} exceeds safety envelope max {}",
                        speed, envelope.max_speed_m_s
                    )));
                }
            }

            if let Some(rotation) = move_cmd.rotation {
                let max_rotation = envelope.max_rotation_deg_s;
                if rotation.0.abs() > max_rotation
                    || rotation.1.abs() > max_rotation
                    || rotation.2.abs() > max_rotation
                {
                    return Err(EmbodimentError::SafetyViolation(
                        "Rotation exceeds safety envelope".to_string(),
                    ));
                }
            }

            if let Some(bounds) = &envelope.workspace_bounds {
                if move_cmd.x < bounds.min_x
                    || move_cmd.x > bounds.max_x
                    || move_cmd.y < bounds.min_y
                    || move_cmd.y > bounds.max_y
                    || move_cmd.z < bounds.min_z
                    || move_cmd.z > bounds.max_z
                {
                    return Err(EmbodimentError::SafetyViolation(
                        "Movement outside safety envelope bounds".to_string(),
                    ));
                }
            }
        }

        Ok(())
    }

    pub async fn execute_command(
        &self,
        command: EmbodimentCommand,
    ) -> Result<EmbodimentResponse, EmbodimentError> {
        let start_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Get the embodiment
        let embodiment = {
            let embodiments = self.embodiments.read().await;
            embodiments
                .get(&command.embodiment_id)
                .cloned()
                .ok_or_else(|| EmbodimentError::EmbodimentNotFound(command.embodiment_id.clone()))?
        };

        let is_physical = Self::is_physical_embodiment(&embodiment.embodiment_type);
        let requested_tier = if is_physical {
            allternit_policy::SafetyTier::T4
        } else {
            allternit_policy::SafetyTier::T2
        };

        // Validate access through policy
        let policy_request = PolicyRequest {
            identity_id: command.agent_id.clone(),
            resource: format!("embodiment_command:{}", command.embodiment_id),
            action: "execute".to_string(),
            context: serde_json::json!({
                "command": &command,
                "session_id": &command.session_id,
                "tenant_id": &command.tenant_id,
            }),
            requested_tier,
        };

        let policy_decision = self.policy_engine.evaluate(policy_request).await?;
        if matches!(policy_decision.decision, PolicyEffect::Deny) {
            return Err(EmbodimentError::SafetyViolation(format!(
                "Policy denied embodiment command: {}",
                policy_decision.reason
            )));
        }

        // Check estop state
        let state = {
            let states = self.states.read().await;
            states
                .get(&command.embodiment_id)
                .cloned()
                .ok_or_else(|| EmbodimentError::EmbodimentNotFound(command.embodiment_id.clone()))?
        };

        if state.estop_engaged {
            return Err(EmbodimentError::SafetyViolation(
                "Emergency stop engaged".to_string(),
            ));
        }

        // Enforce safety envelope (simulation + physical)
        self.validate_safety_envelope(&embodiment.safety_envelope, &command)?;

        if is_physical {
            if !embodiment.physical_actuation_enabled {
                return Err(EmbodimentError::SafetyViolation(
                    "Physical actuation is disabled".to_string(),
                ));
            }
            if !Self::command_simulation_passed(&command) {
                return Err(EmbodimentError::SafetyViolation(
                    "Simulation pass required for physical actuation".to_string(),
                ));
            }
        }

        // Check if embodiment is online
        if !matches!(embodiment.status, EmbodimentStatus::Online) {
            return Err(EmbodimentError::EmbodimentOffline(
                command.embodiment_id.clone(),
            ));
        }

        // Execute the command based on embodiment type
        let response = match embodiment.embodiment_type {
            EmbodimentType::Simulator => {
                // Use simulator for this embodiment
                self.execute_command_with_simulator(&command).await?
            }
            _ => {
                // Execute on actual embodiment
                self.execute_command_on_embodiment(&command, &embodiment)
                    .await?
            }
        };

        // Update state after command execution
        self.update_state_after_command(&command, &response).await?;

        // Log the command execution
        let event = EventEnvelope {
            event_id: Uuid::new_v4().to_string(),
            event_type: "EmbodimentCommandExecuted".to_string(),
            session_id: command.session_id.clone(),
            tenant_id: command.tenant_id.clone(),
            actor_id: command.agent_id.clone(),
            role: "embodiment_control_plane".to_string(),
            timestamp: start_time,
            trace_id: command.trace_id.clone(),
            payload: serde_json::json!({
                "command_id": command.command_id,
                "embodiment_id": command.embodiment_id,
                "command_type": format!("{:?}", command.command_type),
                "status": format!("{:?}", response.status),
                "execution_time_ms": response.execution_time_ms,
                "safety_rating": response.safety_rating,
            }),
        };

        // Log to history ledger
        {
            let mut history = self.history_ledger.lock().unwrap();
            let content = serde_json::to_value(&event)?;
            history.append(content)?;
        }

        tokio::spawn({
            let event_bus = self.messaging_system.event_bus.clone();
            let event_to_send = event.clone();
            async move {
                let _ = event_bus.publish(event_to_send).await;
            }
        });

        Ok(response)
    }

    async fn execute_command_with_simulator(
        &self,
        command: &EmbodimentCommand,
    ) -> Result<EmbodimentResponse, EmbodimentError> {
        // Get the simulator for this embodiment
        let simulator = {
            let simulators = self.simulators.read().await;
            simulators
                .get(&command.embodiment_id)
                .cloned()
                .ok_or_else(|| EmbodimentError::EmbodimentNotFound(command.embodiment_id.clone()))?
        };

        // Execute the command using the simulator
        let start_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();

        let result = simulator.execute_command(command.clone()).await?;

        let end_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();
        let execution_time = end_time.as_millis() as u64 - start_time.as_millis() as u64;

        Ok(EmbodimentResponse {
            command_id: command.command_id.clone(),
            embodiment_id: command.embodiment_id.clone(),
            response: result,
            status: ExecutionStatus::Success,
            execution_time_ms: execution_time,
            safety_rating: 4, // Simulated embodiment has high safety rating
            trace_id: command.trace_id.clone(),
        })
    }

    async fn execute_command_on_embodiment(
        &self,
        command: &EmbodimentCommand,
        embodiment: &EmbodimentConfig,
    ) -> Result<EmbodimentResponse, EmbodimentError> {
        let start_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();
        let adapter_type = embodiment
            .connection_config
            .adapter_type
            .clone()
            .unwrap_or_else(|| {
                AdapterType::from_connection(&embodiment.connection_config.connection_type)
            });
        let adapter = {
            let adapters = self.adapters.read().await;
            adapters
                .get(&adapter_type)
                .cloned()
                .ok_or_else(|| EmbodimentError::AdapterNotFound(format!("{:?}", adapter_type)))?
        };
        let result = adapter.send_command(embodiment, command).await?;

        let end_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();
        let execution_time = end_time.as_millis() as u64 - start_time.as_millis() as u64;

        Ok(EmbodimentResponse {
            command_id: command.command_id.clone(),
            embodiment_id: command.embodiment_id.clone(),
            response: result,
            status: ExecutionStatus::Success,
            execution_time_ms: execution_time,
            safety_rating: 3, // Default safety rating
            trace_id: command.trace_id.clone(),
        })
    }

    async fn update_state_after_command(
        &self,
        command: &EmbodimentCommand,
        response: &EmbodimentResponse,
    ) -> Result<(), EmbodimentError> {
        // Update the state of the embodiment after command execution
        let mut state = {
            let states = self.states.read().await;
            states
                .get(&command.embodiment_id)
                .cloned()
                .ok_or_else(|| EmbodimentError::EmbodimentNotFound(command.embodiment_id.clone()))?
        };

        // Update state based on command type
        match &command.command_type {
            CommandType::Move(move_cmd) => {
                state.position = Some((move_cmd.x, move_cmd.y, move_cmd.z));
                if let Some(rot) = move_cmd.rotation {
                    state.rotation = Some(rot);
                }
            }
            CommandType::Sense(sense_cmd) => {
                // Update sensor data in the state
                state
                    .sensors
                    .insert(sense_cmd.sensor_type.clone(), response.response.clone());
            }
            _ => {
                // Other command types may update state differently
            }
        }

        // Update last updated time
        state.last_updated = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Remove the command from active commands
        state
            .active_commands
            .retain(|cmd_id| cmd_id != &command.command_id);

        // Update the state in storage and cache
        self.storage.update_state(&state).await?;
        {
            let mut states = self.states.write().await;
            states.insert(command.embodiment_id.clone(), state);
        }

        Ok(())
    }

    pub async fn register_simulator(
        &self,
        embodiment_id: String,
        simulator: Arc<dyn EmbodimentSimulator>,
    ) -> Result<(), EmbodimentError> {
        let mut simulators = self.simulators.write().await;
        simulators.insert(embodiment_id, simulator);
        Ok(())
    }

    pub async fn register_adapter(
        &self,
        adapter: Arc<dyn DeviceAdapter>,
    ) -> Result<(), EmbodimentError> {
        let adapter_type = adapter.adapter_type();
        let mut adapters = self.adapters.write().await;
        adapters.insert(adapter_type, adapter);
        Ok(())
    }

    pub async fn engage_estop(
        &self,
        embodiment_id: &str,
        actor_id: &str,
        session_id: &str,
        tenant_id: &str,
        reason: Option<String>,
    ) -> Result<(), EmbodimentError> {
        let mut state = {
            let states = self.states.read().await;
            states
                .get(embodiment_id)
                .cloned()
                .ok_or_else(|| EmbodimentError::EmbodimentNotFound(embodiment_id.to_string()))?
        };
        state.estop_engaged = true;
        state.estop_reason = reason.clone();
        state.estop_updated_at = Some(now_unix_secs());
        self.storage.update_state(&state).await?;
        {
            let mut states = self.states.write().await;
            states.insert(embodiment_id.to_string(), state);
        }

        let event = EventEnvelope {
            event_id: Uuid::new_v4().to_string(),
            event_type: "EmbodimentEStopEngaged".to_string(),
            session_id: session_id.to_string(),
            tenant_id: tenant_id.to_string(),
            actor_id: actor_id.to_string(),
            role: "embodiment_control_plane".to_string(),
            timestamp: now_unix_secs(),
            trace_id: None,
            payload: serde_json::json!({
                "embodiment_id": embodiment_id,
                "reason": reason,
            }),
        };

        {
            let mut history = self.history_ledger.lock().unwrap();
            let content = serde_json::to_value(&event)?;
            history.append(content)?;
        }

        tokio::spawn({
            let event_bus = self.messaging_system.event_bus.clone();
            async move {
                let _ = event_bus.publish(event).await;
            }
        });

        Ok(())
    }

    pub async fn clear_estop(
        &self,
        embodiment_id: &str,
        actor_id: &str,
        session_id: &str,
        tenant_id: &str,
    ) -> Result<(), EmbodimentError> {
        let policy_request = PolicyRequest {
            identity_id: actor_id.to_string(),
            resource: format!("embodiment_estop:{}", embodiment_id),
            action: "clear".to_string(),
            context: serde_json::json!({
                "session_id": session_id,
                "tenant_id": tenant_id,
            }),
            requested_tier: allternit_policy::SafetyTier::T4,
        };
        let policy_decision = self.policy_engine.evaluate(policy_request).await?;
        if matches!(policy_decision.decision, PolicyEffect::Deny) {
            return Err(EmbodimentError::SafetyViolation(format!(
                "Policy denied e-stop clear: {}",
                policy_decision.reason
            )));
        }

        let mut state = {
            let states = self.states.read().await;
            states
                .get(embodiment_id)
                .cloned()
                .ok_or_else(|| EmbodimentError::EmbodimentNotFound(embodiment_id.to_string()))?
        };
        state.estop_engaged = false;
        state.estop_reason = None;
        state.estop_updated_at = Some(now_unix_secs());
        self.storage.update_state(&state).await?;
        {
            let mut states = self.states.write().await;
            states.insert(embodiment_id.to_string(), state);
        }

        let event = EventEnvelope {
            event_id: Uuid::new_v4().to_string(),
            event_type: "EmbodimentEStopCleared".to_string(),
            session_id: session_id.to_string(),
            tenant_id: tenant_id.to_string(),
            actor_id: actor_id.to_string(),
            role: "embodiment_control_plane".to_string(),
            timestamp: now_unix_secs(),
            trace_id: None,
            payload: serde_json::json!({
                "embodiment_id": embodiment_id,
            }),
        };

        {
            let mut history = self.history_ledger.lock().unwrap();
            let content = serde_json::to_value(&event)?;
            history.append(content)?;
        }

        tokio::spawn({
            let event_bus = self.messaging_system.event_bus.clone();
            async move {
                let _ = event_bus.publish(event).await;
            }
        });

        Ok(())
    }

    pub async fn set_physical_actuation_enabled(
        &self,
        embodiment_id: &str,
        enabled: bool,
        actor_id: &str,
        session_id: &str,
        tenant_id: &str,
    ) -> Result<(), EmbodimentError> {
        let action = if enabled { "enable" } else { "disable" };
        let policy_request = PolicyRequest {
            identity_id: actor_id.to_string(),
            resource: format!("embodiment_actuation:{}", embodiment_id),
            action: action.to_string(),
            context: serde_json::json!({
                "session_id": session_id,
                "tenant_id": tenant_id,
                "enabled": enabled,
            }),
            requested_tier: allternit_policy::SafetyTier::T4,
        };
        let policy_decision = self.policy_engine.evaluate(policy_request).await?;
        if matches!(policy_decision.decision, PolicyEffect::Deny) {
            return Err(EmbodimentError::SafetyViolation(format!(
                "Policy denied actuation toggle: {}",
                policy_decision.reason
            )));
        }

        let mut config = {
            let embodiments = self.embodiments.read().await;
            embodiments
                .get(embodiment_id)
                .cloned()
                .ok_or_else(|| EmbodimentError::EmbodimentNotFound(embodiment_id.to_string()))?
        };
        config.physical_actuation_enabled = enabled;
        config.updated_at = now_unix_secs();
        self.storage.update_embodiment(&config).await?;
        {
            let mut embodiments = self.embodiments.write().await;
            embodiments.insert(embodiment_id.to_string(), config);
        }

        let event = EventEnvelope {
            event_id: Uuid::new_v4().to_string(),
            event_type: if enabled {
                "EmbodimentActuationEnabled".to_string()
            } else {
                "EmbodimentActuationDisabled".to_string()
            },
            session_id: session_id.to_string(),
            tenant_id: tenant_id.to_string(),
            actor_id: actor_id.to_string(),
            role: "embodiment_control_plane".to_string(),
            timestamp: now_unix_secs(),
            trace_id: None,
            payload: serde_json::json!({
                "embodiment_id": embodiment_id,
                "enabled": enabled,
            }),
        };

        {
            let mut history = self.history_ledger.lock().unwrap();
            let content = serde_json::to_value(&event)?;
            history.append(content)?;
        }

        tokio::spawn({
            let event_bus = self.messaging_system.event_bus.clone();
            async move {
                let _ = event_bus.publish(event).await;
            }
        });

        Ok(())
    }

    pub async fn stage_ota_update(
        &self,
        request: OtaUpdateRequest,
    ) -> Result<String, EmbodimentError> {
        let embodiment = {
            let embodiments = self.embodiments.read().await;
            embodiments
                .get(&request.embodiment_id)
                .cloned()
                .ok_or_else(|| EmbodimentError::EmbodimentNotFound(request.embodiment_id.clone()))?
        };

        if Self::is_physical_embodiment(&embodiment.embodiment_type) {
            return Err(EmbodimentError::SafetyViolation(
                "OTA updates are disabled for physical devices by default".to_string(),
            ));
        }

        let policy_request = PolicyRequest {
            identity_id: request.actor_id.clone(),
            resource: format!("embodiment_ota:{}", request.embodiment_id),
            action: "stage".to_string(),
            context: serde_json::json!({
                "session_id": request.session_id,
                "tenant_id": request.tenant_id,
            }),
            requested_tier: allternit_policy::SafetyTier::T4,
        };
        let policy_decision = self.policy_engine.evaluate(policy_request).await?;
        if matches!(policy_decision.decision, PolicyEffect::Deny) {
            return Err(EmbodimentError::SafetyViolation(format!(
                "Policy denied OTA staging: {}",
                policy_decision.reason
            )));
        }

        let update_id = Uuid::new_v4().to_string();
        let update = OtaUpdate {
            update_id: update_id.clone(),
            embodiment_id: request.embodiment_id.clone(),
            artifact_id: request.artifact_id.clone(),
            version: request.version.clone(),
            signature: request.signature.clone(),
            status: OtaStatus::Staged,
            rollout: request.rollout.clone(),
            session_id: request.session_id.clone(),
            tenant_id: request.tenant_id.clone(),
            actor_id: request.actor_id.clone(),
            staged_at: now_unix_secs(),
            applied_at: None,
            verified_at: None,
            rolled_back_at: None,
            previous_version: None,
        };

        {
            let mut updates = self.ota_updates.write().await;
            updates.insert(update_id.clone(), update.clone());
        }

        let event = EventEnvelope {
            event_id: Uuid::new_v4().to_string(),
            event_type: "EmbodimentOtaStaged".to_string(),
            session_id: request.session_id,
            tenant_id: request.tenant_id,
            actor_id: request.actor_id,
            role: "embodiment_control_plane".to_string(),
            timestamp: now_unix_secs(),
            trace_id: None,
            payload: serde_json::json!({
                "update_id": update_id,
                "embodiment_id": update.embodiment_id,
                "artifact_id": update.artifact_id,
                "version": update.version,
            }),
        };

        {
            let mut history = self.history_ledger.lock().unwrap();
            let content = serde_json::to_value(&event)?;
            history.append(content)?;
        }

        tokio::spawn({
            let event_bus = self.messaging_system.event_bus.clone();
            async move {
                let _ = event_bus.publish(event).await;
            }
        });

        Ok(update_id)
    }

    pub async fn rollout_ota_update(&self, update_id: &str) -> Result<(), EmbodimentError> {
        let mut update = {
            let updates = self.ota_updates.read().await;
            updates
                .get(update_id)
                .cloned()
                .ok_or_else(|| EmbodimentError::OtaUpdateNotFound(update_id.to_string()))?
        };

        if !matches!(update.status, OtaStatus::Staged) {
            return Err(EmbodimentError::OtaUpdateFailed(
                "OTA update is not staged".to_string(),
            ));
        }

        let embodiment = {
            let embodiments = self.embodiments.read().await;
            embodiments
                .get(&update.embodiment_id)
                .cloned()
                .ok_or_else(|| EmbodimentError::EmbodimentNotFound(update.embodiment_id.clone()))?
        };

        if Self::is_physical_embodiment(&embodiment.embodiment_type) {
            return Err(EmbodimentError::SafetyViolation(
                "OTA updates are disabled for physical devices by default".to_string(),
            ));
        }

        let policy_request = PolicyRequest {
            identity_id: update.actor_id.clone(),
            resource: format!("embodiment_ota:{}", update.embodiment_id),
            action: "rollout".to_string(),
            context: serde_json::json!({
                "session_id": update.session_id,
                "tenant_id": update.tenant_id,
            }),
            requested_tier: allternit_policy::SafetyTier::T4,
        };
        let policy_decision = self.policy_engine.evaluate(policy_request).await?;
        if matches!(policy_decision.decision, PolicyEffect::Deny) {
            return Err(EmbodimentError::SafetyViolation(format!(
                "Policy denied OTA rollout: {}",
                policy_decision.reason
            )));
        }

        let mut state = {
            let states = self.states.read().await;
            states
                .get(&update.embodiment_id)
                .cloned()
                .ok_or_else(|| EmbodimentError::EmbodimentNotFound(update.embodiment_id.clone()))?
        };

        update.previous_version = state.firmware_version.clone();
        state.pending_firmware_version = Some(update.version.clone());
        state.firmware_version = Some(update.version.clone());
        state.pending_firmware_version = None;
        state.last_updated = now_unix_secs();
        self.storage.update_state(&state).await?;
        {
            let mut states = self.states.write().await;
            states.insert(update.embodiment_id.clone(), state);
        }

        update.status = OtaStatus::Applied;
        update.applied_at = Some(now_unix_secs());

        {
            let mut updates = self.ota_updates.write().await;
            updates.insert(update_id.to_string(), update.clone());
        }

        let event = EventEnvelope {
            event_id: Uuid::new_v4().to_string(),
            event_type: "EmbodimentOtaApplied".to_string(),
            session_id: update.session_id.clone(),
            tenant_id: update.tenant_id.clone(),
            actor_id: update.actor_id.clone(),
            role: "embodiment_control_plane".to_string(),
            timestamp: now_unix_secs(),
            trace_id: None,
            payload: serde_json::json!({
                "update_id": update_id,
                "embodiment_id": update.embodiment_id,
                "version": update.version,
            }),
        };

        {
            let mut history = self.history_ledger.lock().unwrap();
            let content = serde_json::to_value(&event)?;
            history.append(content)?;
        }

        tokio::spawn({
            let event_bus = self.messaging_system.event_bus.clone();
            async move {
                let _ = event_bus.publish(event).await;
            }
        });

        Ok(())
    }

    pub async fn verify_ota_update(&self, update_id: &str) -> Result<(), EmbodimentError> {
        let mut update = {
            let updates = self.ota_updates.read().await;
            updates
                .get(update_id)
                .cloned()
                .ok_or_else(|| EmbodimentError::OtaUpdateNotFound(update_id.to_string()))?
        };

        if !matches!(update.status, OtaStatus::Applied) {
            return Err(EmbodimentError::OtaUpdateFailed(
                "OTA update is not applied".to_string(),
            ));
        }

        let policy_request = PolicyRequest {
            identity_id: update.actor_id.clone(),
            resource: format!("embodiment_ota:{}", update.embodiment_id),
            action: "verify".to_string(),
            context: serde_json::json!({
                "session_id": update.session_id,
                "tenant_id": update.tenant_id,
            }),
            requested_tier: allternit_policy::SafetyTier::T4,
        };
        let policy_decision = self.policy_engine.evaluate(policy_request).await?;
        if matches!(policy_decision.decision, PolicyEffect::Deny) {
            return Err(EmbodimentError::SafetyViolation(format!(
                "Policy denied OTA verification: {}",
                policy_decision.reason
            )));
        }

        update.status = OtaStatus::Verified;
        update.verified_at = Some(now_unix_secs());

        {
            let mut updates = self.ota_updates.write().await;
            updates.insert(update_id.to_string(), update.clone());
        }

        let event = EventEnvelope {
            event_id: Uuid::new_v4().to_string(),
            event_type: "EmbodimentOtaVerified".to_string(),
            session_id: update.session_id.clone(),
            tenant_id: update.tenant_id.clone(),
            actor_id: update.actor_id.clone(),
            role: "embodiment_control_plane".to_string(),
            timestamp: now_unix_secs(),
            trace_id: None,
            payload: serde_json::json!({
                "update_id": update_id,
                "embodiment_id": update.embodiment_id,
                "version": update.version,
            }),
        };

        {
            let mut history = self.history_ledger.lock().unwrap();
            let content = serde_json::to_value(&event)?;
            history.append(content)?;
        }

        tokio::spawn({
            let event_bus = self.messaging_system.event_bus.clone();
            async move {
                let _ = event_bus.publish(event).await;
            }
        });

        Ok(())
    }

    pub async fn rollback_ota_update(&self, update_id: &str) -> Result<(), EmbodimentError> {
        let mut update = {
            let updates = self.ota_updates.read().await;
            updates
                .get(update_id)
                .cloned()
                .ok_or_else(|| EmbodimentError::OtaUpdateNotFound(update_id.to_string()))?
        };

        if !matches!(update.status, OtaStatus::Applied | OtaStatus::Verified) {
            return Err(EmbodimentError::OtaUpdateFailed(
                "OTA update cannot be rolled back from current state".to_string(),
            ));
        }

        let policy_request = PolicyRequest {
            identity_id: update.actor_id.clone(),
            resource: format!("embodiment_ota:{}", update.embodiment_id),
            action: "rollback".to_string(),
            context: serde_json::json!({
                "session_id": update.session_id,
                "tenant_id": update.tenant_id,
            }),
            requested_tier: allternit_policy::SafetyTier::T4,
        };
        let policy_decision = self.policy_engine.evaluate(policy_request).await?;
        if matches!(policy_decision.decision, PolicyEffect::Deny) {
            return Err(EmbodimentError::SafetyViolation(format!(
                "Policy denied OTA rollback: {}",
                policy_decision.reason
            )));
        }

        let mut state = {
            let states = self.states.read().await;
            states
                .get(&update.embodiment_id)
                .cloned()
                .ok_or_else(|| EmbodimentError::EmbodimentNotFound(update.embodiment_id.clone()))?
        };

        if let Some(previous) = update.previous_version.clone() {
            state.firmware_version = Some(previous);
        } else {
            state.firmware_version = None;
        }
        state.pending_firmware_version = None;
        state.last_updated = now_unix_secs();
        self.storage.update_state(&state).await?;
        {
            let mut states = self.states.write().await;
            states.insert(update.embodiment_id.clone(), state);
        }

        update.status = OtaStatus::RolledBack;
        update.rolled_back_at = Some(now_unix_secs());

        {
            let mut updates = self.ota_updates.write().await;
            updates.insert(update_id.to_string(), update.clone());
        }

        let event = EventEnvelope {
            event_id: Uuid::new_v4().to_string(),
            event_type: "EmbodimentOtaRolledBack".to_string(),
            session_id: update.session_id.clone(),
            tenant_id: update.tenant_id.clone(),
            actor_id: update.actor_id.clone(),
            role: "embodiment_control_plane".to_string(),
            timestamp: now_unix_secs(),
            trace_id: None,
            payload: serde_json::json!({
                "update_id": update_id,
                "embodiment_id": update.embodiment_id,
                "version": update.version,
            }),
        };

        {
            let mut history = self.history_ledger.lock().unwrap();
            let content = serde_json::to_value(&event)?;
            history.append(content)?;
        }

        tokio::spawn({
            let event_bus = self.messaging_system.event_bus.clone();
            async move {
                let _ = event_bus.publish(event).await;
            }
        });

        Ok(())
    }

    pub async fn get_embodiment_state(
        &self,
        embodiment_id: String,
    ) -> Result<Option<EmbodimentState>, EmbodimentError> {
        // Try to get from cache first
        {
            let states = self.states.read().await;
            if let Some(state) = states.get(&embodiment_id) {
                return Ok(Some(state.clone()));
            }
        }

        // If not in cache, get from storage
        let state = self.storage.get_state(&embodiment_id).await?;
        if let Some(state) = &state {
            // Update cache
            let mut states = self.states.write().await;
            states.insert(embodiment_id.clone(), state.clone());
        }

        Ok(state)
    }

    pub async fn get_embodiment_stats(
        &self,
        embodiment_id: String,
    ) -> Result<EmbodimentStats, EmbodimentError> {
        self.storage.get_embodiment_stats(&embodiment_id).await
    }
}

// Trait for embodiment simulators
#[async_trait::async_trait]
pub trait EmbodimentSimulator: Send + Sync {
    async fn execute_command(
        &self,
        command: EmbodimentCommand,
    ) -> Result<serde_json::Value, EmbodimentError>;
    async fn get_state(&self) -> Result<EmbodimentState, EmbodimentError>;
    async fn update_state(&self, state: EmbodimentState) -> Result<(), EmbodimentError>;
}

// Basic simulator implementation for testing
pub struct BasicSimulator {
    state: Arc<RwLock<EmbodimentState>>,
}

impl BasicSimulator {
    pub fn new(embodiment_id: String) -> Self {
        BasicSimulator {
            state: Arc::new(RwLock::new(EmbodimentState {
                embodiment_id,
                position: Some((0.0, 0.0, 0.0)),
                rotation: Some((0.0, 0.0, 0.0)),
                battery_level: Some(100.0),
                sensors: HashMap::new(),
                last_heartbeat: Some(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                ),
                connected: true,
                capabilities: vec![],
                active_commands: Vec::new(),
                estop_engaged: false,
                estop_reason: None,
                estop_updated_at: None,
                firmware_version: None,
                pending_firmware_version: None,
                last_updated: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            })),
        }
    }
}

#[async_trait::async_trait]
impl EmbodimentSimulator for BasicSimulator {
    async fn execute_command(
        &self,
        command: EmbodimentCommand,
    ) -> Result<serde_json::Value, EmbodimentError> {
        // Simulate command execution
        match command.command_type {
            CommandType::Move(move_cmd) => {
                // Update position in state
                {
                    let mut state = self.state.write().await;
                    state.position = Some((move_cmd.x, move_cmd.y, move_cmd.z));
                    if let Some(rot) = move_cmd.rotation {
                        state.rotation = Some(rot);
                    }
                    state.last_updated = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs();
                }

                Ok(serde_json::json!({
                    "status": "moved",
                    "position": [move_cmd.x, move_cmd.y, move_cmd.z],
                    "rotation": move_cmd.rotation,
                }))
            }
            CommandType::Speak(speak_cmd) => Ok(serde_json::json!({
                "status": "spoken",
                "text": speak_cmd.text,
            })),
            CommandType::Sense(sense_cmd) => {
                // Generate simulated sensor data
                let sensor_data = match sense_cmd.sensor_type.as_str() {
                    "camera" => {
                        serde_json::json!({"type": "image", "data": "simulated_camera_data"})
                    }
                    "lidar" => {
                        serde_json::json!({"type": "point_cloud", "data": "simulated_lidar_data"})
                    }
                    "temperature" => serde_json::json!({"type": "temperature", "value": 22.5}),
                    "humidity" => serde_json::json!({"type": "humidity", "value": 45.0}),
                    _ => serde_json::json!({"type": "generic", "data": "simulated_sensor_data"}),
                };

                // Update sensors in state
                {
                    let mut state = self.state.write().await;
                    state
                        .sensors
                        .insert(sense_cmd.sensor_type, sensor_data.clone());
                    state.last_updated = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs();
                }

                Ok(serde_json::json!({
                    "status": "sensed",
                    "data": sensor_data,
                }))
            }
            CommandType::Execute(exec_cmd) => Ok(serde_json::json!({
                "status": "executed",
                "skill": exec_cmd.skill_name,
                "parameters": exec_cmd.parameters,
            })),
            CommandType::Configure(config_cmd) => Ok(serde_json::json!({
                "status": "configured",
                "setting": config_cmd.setting,
                "value": config_cmd.value,
            })),
            CommandType::Custom(custom_cmd) => Ok(serde_json::json!({
                "status": "executed",
                "command": custom_cmd,
            })),
        }
    }

    async fn get_state(&self) -> Result<EmbodimentState, EmbodimentError> {
        Ok(self.state.read().await.clone())
    }

    async fn update_state(&self, new_state: EmbodimentState) -> Result<(), EmbodimentError> {
        let mut state = self.state.write().await;
        *state = new_state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::Mutex;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_embodiment_control_plane_basic_functionality() {
        // Create temporary database
        let temp_db = NamedTempFile::new().unwrap();
        let db_url = format!("sqlite://{}", temp_db.path().to_string_lossy());
        let pool = SqlitePool::connect(&db_url).await.unwrap();

        // Create temporary history ledger
        let temp_path = format!("/tmp/test_embodiment_{}.jsonl", Uuid::new_v4());
        let history_ledger = Arc::new(Mutex::new(
            allternit_history::HistoryLedger::new(&temp_path).unwrap(),
        ));

        // Create messaging system
        let messaging_system = Arc::new(
            allternit_messaging::MessagingSystem::new_with_storage(
                history_ledger.clone(),
                pool.clone(),
            )
            .await
            .unwrap(),
        );

        // Create policy engine
        let policy_engine = Arc::new(allternit_policy::PolicyEngine::new(
            history_ledger.clone(),
            messaging_system.clone(),
        ));
        let agent_identity = allternit_policy::Identity {
            id: "agent-001".to_string(),
            identity_type: allternit_policy::IdentityType::AgentIdentity,
            name: "Test Agent".to_string(),
            tenant_id: "tenant-001".to_string(),
            created_at: 0,
            active: true,
            roles: vec!["agent".to_string()],
            permissions: vec!["perm_t0_read".to_string()],
        };
        policy_engine
            .register_identity(agent_identity)
            .await
            .unwrap();
        policy_engine.create_default_permissions().await.unwrap();
        policy_engine.create_default_rules().await.unwrap();
        let execute_rule = allternit_policy::PolicyRule {
            id: "rule_allow_execute".to_string(),
            name: "Allow Execute Operations".to_string(),
            description: "Allow embodiment command execution in tests".to_string(),
            condition: "identity.active".to_string(),
            effect: allternit_policy::PolicyEffect::Allow,
            resource: "*".to_string(),
            actions: vec![
                "execute".to_string(),
                "stage".to_string(),
                "rollout".to_string(),
                "verify".to_string(),
                "rollback".to_string(),
                "clear".to_string(),
                "enable".to_string(),
                "disable".to_string(),
            ],
            priority: 150,
            enabled: true,
        };
        policy_engine.add_rule(execute_rule).await.unwrap();

        // Create tool gateway
        let tool_gateway = Arc::new(allternit_tools_gateway::ToolGateway::new(
            policy_engine.clone(),
            history_ledger.clone(),
            messaging_system.clone(),
        ));

        // Create context router
        let context_router = Arc::new(allternit_context_router::ContextRouter::new(
            history_ledger.clone(),
            messaging_system.clone(),
            policy_engine.clone(),
            Arc::new(allternit_runtime_core::SessionManager::new(
                history_ledger.clone(),
                messaging_system.clone(),
            )),
        ));

        // Create memory fabric
        let memory_fabric = Arc::new(
            allternit_memory::MemoryFabric::new_with_storage(
                history_ledger.clone(),
                messaging_system.clone(),
                policy_engine.clone(),
                context_router.clone(),
                pool.clone(),
                Arc::new(allternit_memory::v2::memory_policy::DefaultMemoryPolicy),
            )
            .await
            .unwrap(),
        );

        // Create provider router
        let provider_router = Arc::new(
            allternit_providers::ProviderRouter::new_with_storage(
                history_ledger.clone(),
                messaging_system.clone(),
                policy_engine.clone(),
                context_router.clone(),
                memory_fabric.clone(),
                Arc::new(allternit_runtime_core::SessionManager::new(
                    history_ledger.clone(),
                    messaging_system.clone(),
                )),
                pool.clone(),
            )
            .await
            .unwrap(),
        );

        // Create skill registry
        let skill_registry = Arc::new(
            allternit_skills::SkillRegistry::new_with_storage(
                history_ledger.clone(),
                messaging_system.clone(),
                policy_engine.clone(),
                tool_gateway.clone(),
                pool.clone(),
            )
            .await
            .unwrap(),
        );

        // Create workflow engine
        // Create task queue
        let task_queue = Arc::new(
            allternit_messaging::TaskQueue::new(history_ledger.clone(), pool.clone())
                .await
                .unwrap(),
        );

        let workflow_engine = Arc::new(allternit_workflows::WorkflowEngine::new(
            history_ledger.clone(),
            messaging_system.clone(),
            policy_engine.clone(),
            tool_gateway,
            skill_registry.clone(),
            task_queue,
            pool.clone(),
        ));

        // Create session manager
        let session_manager = Arc::new(allternit_runtime_core::SessionManager::new(
            history_ledger.clone(),
            messaging_system.clone(),
        ));

        // Create embodiment control plane
        let embodiment_control_plane = Arc::new(
            EmbodimentControlPlane::new_with_storage(
                history_ledger,
                messaging_system,
                policy_engine,
                context_router,
                memory_fabric,
                provider_router,
                skill_registry,
                workflow_engine,
                session_manager,
                pool,
            )
            .await
            .unwrap(),
        );

        // Create a test embodiment
        let embodiment_config = EmbodimentConfig {
            embodiment_id: "test-robot-001".to_string(),
            embodiment_type: EmbodimentType::Robot,
            name: "Test Robot".to_string(),
            description: "A test robot embodiment".to_string(),
            capabilities: vec![EmbodimentCapability {
                capability_id: "move".to_string(),
                name: "Movement".to_string(),
                description: "Ability to move in 3D space".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "x": {"type": "number"},
                        "y": {"type": "number"},
                        "z": {"type": "number"}
                    }
                }),
                output_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "status": {"type": "string"},
                        "position": {"type": "array", "items": {"type": "number"}}
                    }
                }),
                safety_tier: 3,
                execution_time_ms: 1000,
            }],
            safety_envelope: SafetyEnvelope::default(),
            physical_actuation_enabled: true,
            connection_config: ConnectionConfig {
                connection_type: ConnectionType::WebSocket,
                adapter_type: None,
                endpoint: "ws://localhost:8080".to_string(),
                authentication: AuthenticationConfig {
                    auth_type: AuthType::APIKey,
                    credentials: HashMap::from([("api_key".to_string(), "test-key".to_string())]),
                    certificate_path: None,
                    token_expiry: None,
                },
                heartbeat_interval_ms: 5000,
                timeout_ms: 10000,
            },
            status: EmbodimentStatus::Online,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            updated_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        };

        let mut blocked_config = embodiment_config.clone();
        blocked_config.embodiment_id = "test-robot-002".to_string();
        blocked_config.name = "Blocked Robot".to_string();
        blocked_config.physical_actuation_enabled = false;

        // Register the embodiment
        let embodiment_id = embodiment_control_plane
            .register_embodiment(embodiment_config)
            .await
            .unwrap();
        assert_eq!(embodiment_id, "test-robot-001");

        // Create a test command
        let command = EmbodimentCommand {
            command_id: "cmd-001".to_string(),
            embodiment_id: "test-robot-001".to_string(),
            session_id: "session-001".to_string(),
            tenant_id: "tenant-001".to_string(),
            agent_id: "agent-001".to_string(),
            command_type: CommandType::Move(MoveCommand {
                x: 1.0,
                y: 2.0,
                z: 3.0,
                rotation: Some((0.0, 0.0, 0.0)),
                speed: Some(1.0),
            }),
            parameters: serde_json::json!({"simulation_passed": true}),
            priority: 5,
            deadline: None,
            trace_id: None,
        };

        // Execute the command
        let response = embodiment_control_plane
            .execute_command(command)
            .await
            .unwrap();
        assert_eq!(response.status, ExecutionStatus::Success);

        let blocked_id = embodiment_control_plane
            .register_embodiment(blocked_config)
            .await
            .unwrap();
        assert_eq!(blocked_id, "test-robot-002");

        let blocked_command = EmbodimentCommand {
            command_id: "cmd-002".to_string(),
            embodiment_id: "test-robot-002".to_string(),
            session_id: "session-002".to_string(),
            tenant_id: "tenant-001".to_string(),
            agent_id: "agent-001".to_string(),
            command_type: CommandType::Move(MoveCommand {
                x: 1.0,
                y: 2.0,
                z: 3.0,
                rotation: Some((0.0, 0.0, 0.0)),
                speed: Some(1.0),
            }),
            parameters: serde_json::json!({"simulation_passed": true}),
            priority: 5,
            deadline: None,
            trace_id: None,
        };
        let blocked_result = embodiment_control_plane
            .execute_command(blocked_command)
            .await;
        assert!(matches!(
            blocked_result,
            Err(EmbodimentError::SafetyViolation(_))
        ));

        let simulator_config = EmbodimentConfig {
            embodiment_id: "sim-001".to_string(),
            embodiment_type: EmbodimentType::Simulator,
            name: "Sim Device".to_string(),
            description: "Simulated embodiment".to_string(),
            capabilities: vec![EmbodimentCapability {
                capability_id: "sim_move".to_string(),
                name: "Sim Movement".to_string(),
                description: "Simulated movement capability".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "x": {"type": "number"},
                        "y": {"type": "number"},
                        "z": {"type": "number"}
                    }
                }),
                output_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "status": {"type": "string"}
                    }
                }),
                safety_tier: 1,
                execution_time_ms: 500,
            }],
            safety_envelope: SafetyEnvelope::default(),
            physical_actuation_enabled: false,
            connection_config: ConnectionConfig {
                connection_type: ConnectionType::HTTP,
                adapter_type: None,
                endpoint: "http://localhost:9999".to_string(),
                authentication: AuthenticationConfig {
                    auth_type: AuthType::APIKey,
                    credentials: HashMap::from([("api_key".to_string(), "sim-key".to_string())]),
                    certificate_path: None,
                    token_expiry: None,
                },
                heartbeat_interval_ms: 1000,
                timeout_ms: 2000,
            },
            status: EmbodimentStatus::Online,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            updated_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        };

        embodiment_control_plane
            .register_embodiment(simulator_config)
            .await
            .unwrap();

        let update_id = embodiment_control_plane
            .stage_ota_update(OtaUpdateRequest {
                embodiment_id: "sim-001".to_string(),
                artifact_id: "artifact-001".to_string(),
                version: "1.0.1".to_string(),
                signature: Some("sig".to_string()),
                rollout: RolloutStrategy::Immediate,
                session_id: "session-ota".to_string(),
                tenant_id: "tenant-001".to_string(),
                actor_id: "agent-001".to_string(),
            })
            .await
            .unwrap();

        embodiment_control_plane
            .rollout_ota_update(&update_id)
            .await
            .unwrap();
        embodiment_control_plane
            .verify_ota_update(&update_id)
            .await
            .unwrap();
        embodiment_control_plane
            .rollback_ota_update(&update_id)
            .await
            .unwrap();

        let sim_state = embodiment_control_plane
            .get_embodiment_state("sim-001".to_string())
            .await
            .unwrap()
            .unwrap();
        assert!(sim_state.firmware_version.is_none());

        let updates = embodiment_control_plane.ota_updates.read().await;
        let update = updates.get(&update_id).unwrap();
        assert!(matches!(update.status, OtaStatus::RolledBack));

        // Verify the embodiment was registered
        let embodiments = embodiment_control_plane.embodiments.read().await;
        assert_eq!(embodiments.len(), 3);
        assert!(embodiments.contains_key("test-robot-001"));
        drop(embodiments);

        // Clean up
        std::fs::remove_file(&temp_path).unwrap();
    }
}
