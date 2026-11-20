//! Wire types for the Sleep routes (docs: `surfaces/docs/api/memory-sleep.mdx`).
//!
//! Field names are snake_case on the wire, like the other `/memory` routes.

use serde::{Deserialize, Serialize};

/// Where a claim or fact came from. Stored in `memory_page_claims.source_type`
/// and `memory_facts.source_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceType {
    UserSaid,
    ToolObserved,
    ModelInferred,
    DreamGenerated,
    Imported,
}

impl SourceType {
    pub const ALL: [SourceType; 5] = [
        SourceType::UserSaid,
        SourceType::ToolObserved,
        SourceType::ModelInferred,
        SourceType::DreamGenerated,
        SourceType::Imported,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            SourceType::UserSaid => "user_said",
            SourceType::ToolObserved => "tool_observed",
            SourceType::ModelInferred => "model_inferred",
            SourceType::DreamGenerated => "dream_generated",
            SourceType::Imported => "imported",
        }
    }
}

/// A stage of Sleep. Each has its own flag in `sleep_config` (all off by
/// default); the key is `stage.<name>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    LabelsLive,
    Strengths,
    Pages,
    Router,
    Orchestrator,
    Disputes,
    Checks,
    Intentions,
    Rem,
    Track1,
}

impl Stage {
    pub const ALL: [Stage; 10] = [
        Stage::LabelsLive,
        Stage::Strengths,
        Stage::Pages,
        Stage::Router,
        Stage::Orchestrator,
        Stage::Disputes,
        Stage::Checks,
        Stage::Intentions,
        Stage::Rem,
        Stage::Track1,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Stage::LabelsLive => "labels_live",
            Stage::Strengths => "strengths",
            Stage::Pages => "pages",
            Stage::Router => "router",
            Stage::Orchestrator => "orchestrator",
            Stage::Disputes => "disputes",
            Stage::Checks => "checks",
            Stage::Intentions => "intentions",
            Stage::Rem => "rem",
            Stage::Track1 => "track1",
        }
    }

    pub fn parse(s: &str) -> Option<Stage> {
        Stage::ALL.into_iter().find(|st| st.as_str() == s)
    }
}

/// The user's (or arbiter's) choice on a dispute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisputeChoice {
    A,
    B,
    BothScoped,
    Neither,
}

/// A trigger sent by the runtime to `POST /memory/sleep/signal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalEvent {
    Idle,
    PreCompact,
    SessionEnd,
    Resume,
}

/// Events on the memory activity stream. Declared for the contract; nothing
/// emits them until the orchestrator (step 5.1) and disputes (step 6.1) land.
pub mod events {
    pub const CYCLE_STARTED: &str = "sleep.cycle.started";
    pub const CYCLE_STAGE: &str = "sleep.cycle.stage";
    pub const CYCLE_FINISHED: &str = "sleep.cycle.finished";
    pub const DISPUTE_OPENED: &str = "sleep.dispute.opened";
    pub const DISPUTE_NEEDS_USER: &str = "sleep.dispute.needs_user";
    pub const DEBT_CHANGED: &str = "sleep.debt.changed";

    pub const ALL: [&str; 6] = [
        CYCLE_STARTED,
        CYCLE_STAGE,
        CYCLE_FINISHED,
        DISPUTE_OPENED,
        DISPUTE_NEEDS_USER,
        DEBT_CHANGED,
    ];
}

// ── Requests ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct SignalRequest {
    pub session_id: String,
    pub event: SignalEvent,
    #[serde(default)]
    pub cache_ttl_s: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RunRequest {
    /// `daydream`, `nap`, `deep` or `rem`; the orchestrator picks when absent.
    #[serde(default)]
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ContextQuery {
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoadPageRequest {
    pub topic_id: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ResolveDisputeRequest {
    pub choice: DisputeChoice,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub limit: Option<i64>,
    /// Disputes only: `open`, `needs_user` or `resolved`.
    #[serde(default)]
    pub state: Option<String>,
}

/// `PUT /memory/sleep/settings`. Every field is optional; only the ones sent
/// are written (as `source = owner`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsUpdate {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub stages: Option<std::collections::BTreeMap<String, bool>>,
    #[serde(default)]
    pub daily_spend_cap_usd: Option<f64>,
}

/// `PUT /memory/sleep/config`: threshold key → JSON value.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigUpdate {
    pub values: std::collections::BTreeMap<String, serde_json::Value>,
}

// ── Responses ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct SleepStatus {
    pub available: bool,
    pub enabled: bool,
    pub stages: std::collections::BTreeMap<String, bool>,
    pub debt: f64,
    pub last_cycle: Option<CycleSummary>,
    pub spend_today_usd: f64,
    pub daily_spend_cap_usd: f64,
    pub manifest_id: Option<String>,
    pub open_disputes: i64,
    pub tracks: Tracks,
}

#[derive(Debug, Clone, Serialize)]
pub struct Tracks {
    /// Track 1 (adapters): `off` until migration B and its flag.
    pub track1: &'static str,
    /// Track 2 (pages): `off` until the `pages` flag.
    pub track2: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct CycleSummary {
    pub id: String,
    pub trigger: String,
    pub kind: String,
    pub state: String,
    pub stage: Option<String>,
    pub session_id: Option<String>,
    pub spend_usd: f64,
    pub manifest_before: Option<String>,
    pub manifest_after: Option<String>,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub undone_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PageSummary {
    pub id: String,
    pub topic_id: String,
    pub version: i64,
    pub sha256: String,
    pub manifest_id: Option<String>,
    pub provenance_tier: Option<String>,
    pub tokens: Option<i64>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClaimView {
    pub id: String,
    pub claim: String,
    pub source_type: String,
    pub observation_id: Option<String>,
    pub fact_id: Option<String>,
    pub as_of: String,
    pub superseded_by: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DisputeView {
    pub id: String,
    pub topic_id: Option<String>,
    pub side_a: DisputeSide,
    pub side_b: DisputeSide,
    pub scope: Option<String>,
    pub stakes: Option<f64>,
    pub margin: Option<f64>,
    pub pick: Option<String>,
    pub reason: Option<String>,
    pub state: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DisputeSide {
    pub kind: String,
    pub id: String,
}

/// A refused write: the stage it needs is not built or not switched on.
#[derive(Debug, Clone, Serialize)]
pub struct NotAccepted {
    pub accepted: bool,
    pub reason: &'static str,
}

impl NotAccepted {
    pub fn sleep_disabled() -> Self {
        NotAccepted { accepted: false, reason: "sleep_disabled" }
    }
}
