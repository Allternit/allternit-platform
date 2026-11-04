use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, RwLock};
use tokio::time::sleep;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

// ── Types ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrainStatus {
    Idle,
    Running,
    Paused,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStage {
    CollectingEvidence,
    Ideating,
    Critiquing,
    Synthesizing,
    Dispatching,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    Sleep,
    Suggest,
    Dispatch,
    Defer,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackgroundRun {
    pub id: String,
    pub stage: RunStage,
    pub outcome: Option<RunOutcome>,
    pub evidence_summary: String,
    pub hypothesis: Option<String>,
    pub decision: Option<String>,
    pub error: Option<String>,
    pub started_at: u64,
    pub completed_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackgroundSettings {
    pub enabled: bool,
    pub cadence_minutes: u64,
    pub auto_dispatch: bool,
    pub journaling_enabled: bool,
    pub api_url: String,
}

impl Default for BackgroundSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            cadence_minutes: 60,
            auto_dispatch: false,
            journaling_enabled: true,
            api_url: "http://localhost:3013".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrainSummary {
    pub status: BrainStatus,
    pub enabled: bool,
    pub cadence_minutes: u64,
    pub last_run_at: Option<u64>,
    pub run_count: u64,
}

// ── Database ─────────────────────────────────────────────────────────────────

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn init_db(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "
        PRAGMA journal_mode=WAL;
        PRAGMA synchronous=NORMAL;

        CREATE TABLE IF NOT EXISTS background_runs (
            id TEXT PRIMARY KEY,
            stage TEXT NOT NULL,
            outcome TEXT,
            evidence_summary TEXT NOT NULL DEFAULT '',
            hypothesis TEXT,
            decision TEXT,
            error TEXT,
            started_at INTEGER NOT NULL,
            completed_at INTEGER
        );

        CREATE TABLE IF NOT EXISTS background_settings (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            enabled INTEGER NOT NULL DEFAULT 0,
            cadence_minutes INTEGER NOT NULL DEFAULT 60,
            auto_dispatch INTEGER NOT NULL DEFAULT 0,
            journaling_enabled INTEGER NOT NULL DEFAULT 1,
            api_url TEXT NOT NULL DEFAULT 'http://localhost:3013'
        );

        INSERT OR IGNORE INTO background_settings (id) VALUES (1);
    ",
    )
}

fn load_settings(conn: &Connection) -> rusqlite::Result<BackgroundSettings> {
    conn.query_row(
        "SELECT enabled, cadence_minutes, auto_dispatch, journaling_enabled, api_url FROM background_settings WHERE id = 1",
        [],
        |row| {
            Ok(BackgroundSettings {
                enabled: row.get::<_, i64>(0)? != 0,
                cadence_minutes: row.get::<_, i64>(1)? as u64,
                auto_dispatch: row.get::<_, i64>(2)? != 0,
                journaling_enabled: row.get::<_, i64>(3)? != 0,
                api_url: row.get(4)?,
            })
        },
    )
}

fn save_settings(conn: &Connection, s: &BackgroundSettings) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE background_settings SET enabled=?1, cadence_minutes=?2, auto_dispatch=?3, journaling_enabled=?4, api_url=?5 WHERE id=1",
        params![
            s.enabled as i64,
            s.cadence_minutes as i64,
            s.auto_dispatch as i64,
            s.journaling_enabled as i64,
            s.api_url,
        ],
    )?;
    Ok(())
}

fn insert_run(conn: &Connection, run: &BackgroundRun) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO background_runs (id, stage, outcome, evidence_summary, hypothesis, decision, error, started_at, completed_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            run.id,
            serde_json::to_string(&run.stage).unwrap_or_default(),
            run.outcome.as_ref().map(|o| serde_json::to_string(o).unwrap_or_default()),
            run.evidence_summary,
            run.hypothesis,
            run.decision,
            run.error,
            run.started_at as i64,
            run.completed_at.map(|t| t as i64),
        ],
    )?;
    Ok(())
}

fn update_run(conn: &Connection, run: &BackgroundRun) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE background_runs SET stage=?1, outcome=?2, hypothesis=?3, decision=?4, error=?5, completed_at=?6 WHERE id=?7",
        params![
            serde_json::to_string(&run.stage).unwrap_or_default(),
            run.outcome.as_ref().map(|o| serde_json::to_string(o).unwrap_or_default()),
            run.hypothesis,
            run.decision,
            run.error,
            run.completed_at.map(|t| t as i64),
            run.id,
        ],
    )?;
    Ok(())
}

fn recent_runs(conn: &Connection, limit: u32) -> rusqlite::Result<Vec<BackgroundRun>> {
    let mut stmt = conn.prepare(
        "SELECT id, stage, outcome, evidence_summary, hypothesis, decision, error, started_at, completed_at FROM background_runs ORDER BY started_at DESC LIMIT ?1"
    )?;
    let rows = stmt.query_map([limit], |row| {
        let stage_str: String = row.get(1)?;
        let outcome_str: Option<String> = row.get(2)?;
        Ok(BackgroundRun {
            id: row.get(0)?,
            stage: serde_json::from_str(&stage_str).unwrap_or(RunStage::Failed),
            outcome: outcome_str.and_then(|s| serde_json::from_str(&s).ok()),
            evidence_summary: row.get(3)?,
            hypothesis: row.get(4)?,
            decision: row.get(5)?,
            error: row.get(6)?,
            started_at: row.get::<_, i64>(7)? as u64,
            completed_at: row.get::<_, Option<i64>>(8)?.map(|t| t as u64),
        })
    })?;
    rows.collect()
}

fn run_count(conn: &Connection) -> rusqlite::Result<u64> {
    conn.query_row("SELECT COUNT(*) FROM background_runs", [], |r| {
        r.get::<_, i64>(0)
    })
    .map(|n| n as u64)
}

fn last_run_at(conn: &Connection) -> rusqlite::Result<Option<u64>> {
    conn.query_row(
        "SELECT MAX(started_at) FROM background_runs WHERE outcome IS NOT NULL",
        [],
        |r| r.get::<_, Option<i64>>(0),
    )
    .map(|v| v.map(|t| t as u64))
}

// ── Background Service ────────────────────────────────────────────────────────

pub struct CoworkBackgroundService {
    db: Arc<Mutex<Connection>>,
    status: Arc<RwLock<BrainStatus>>,
}

impl CoworkBackgroundService {
    pub fn new(db_path: &PathBuf) -> Result<Self, rusqlite::Error> {
        let conn = Connection::open(db_path)?;
        init_db(&conn)?;
        Ok(Self {
            db: Arc::new(Mutex::new(conn)),
            status: Arc::new(RwLock::new(BrainStatus::Idle)),
        })
    }

    /// Returns a shareable handle that the router can use.
    pub fn handle(&self) -> BackgroundServiceHandle {
        BackgroundServiceHandle {
            db: self.db.clone(),
            status: self.status.clone(),
        }
    }

    /// Starts the background loop as a detached tokio task.
    pub fn start(self: Arc<Self>) {
        let svc = self.clone();
        tokio::spawn(async move { svc.run_loop().await });
    }

    async fn run_loop(&self) {
        loop {
            let settings = {
                let conn = self.db.lock().await;
                match load_settings(&conn) {
                    Ok(s) => s,
                    Err(e) => {
                        error!("Background service: failed to load settings: {e}");
                        sleep(Duration::from_secs(60)).await;
                        continue;
                    }
                }
            };

            if !settings.enabled {
                debug!("Background service: disabled — sleeping 5 min");
                sleep(Duration::from_secs(300)).await;
                continue;
            }

            {
                let mut status = self.status.write().await;
                *status = BrainStatus::Running;
            }

            if let Err(e) = self.execute_cycle(&settings).await {
                warn!("Background service: cycle error: {e}");
            }

            {
                let mut status = self.status.write().await;
                *status = BrainStatus::Idle;
            }

            let cadence = Duration::from_secs(settings.cadence_minutes * 60);
            info!(
                "Background service: next cycle in {} min",
                settings.cadence_minutes
            );
            sleep(cadence).await;
        }
    }

    async fn execute_cycle(&self, settings: &BackgroundSettings) -> anyhow::Result<()> {
        let run_id = Uuid::new_v4().to_string();
        let mut run = BackgroundRun {
            id: run_id.clone(),
            stage: RunStage::CollectingEvidence,
            outcome: None,
            evidence_summary: String::new(),
            hypothesis: None,
            decision: None,
            error: None,
            started_at: now_unix(),
            completed_at: None,
        };

        {
            let conn = self.db.lock().await;
            insert_run(&conn, &run)?;
        }

        info!("Background service: starting cycle {run_id}");

        // Phase 1: Collect evidence from sessions/projects API
        run.stage = RunStage::CollectingEvidence;
        let evidence = self.collect_evidence(&settings.api_url).await;
        run.evidence_summary = evidence.clone();

        if evidence.is_empty() {
            run.stage = RunStage::Completed;
            run.outcome = Some(RunOutcome::Sleep);
            run.completed_at = Some(now_unix());
            let conn = self.db.lock().await;
            update_run(&conn, &run)?;
            info!("Background service: no evidence — sleeping");
            return Ok(());
        }

        // Phase 2: Ideate — generate hypotheses via platform LLM endpoint
        run.stage = RunStage::Ideating;
        {
            let conn = self.db.lock().await;
            update_run(&conn, &run)?;
        }
        let hypothesis = self.ideate(&settings.api_url, &evidence).await;
        run.hypothesis = Some(hypothesis.clone());

        // Phase 3: Critique
        run.stage = RunStage::Critiquing;
        {
            let conn = self.db.lock().await;
            update_run(&conn, &run)?;
        }
        let critique_passed = self
            .critique(&settings.api_url, &evidence, &hypothesis)
            .await;

        // Phase 4: Synthesize decision
        run.stage = RunStage::Synthesizing;
        {
            let conn = self.db.lock().await;
            update_run(&conn, &run)?;
        }
        let (outcome, decision) = if critique_passed {
            (RunOutcome::Suggest, format!("Suggestion: {hypothesis}"))
        } else {
            (
                RunOutcome::Defer,
                "Critique rejected hypothesis — deferring".to_string(),
            )
        };
        run.decision = Some(decision.clone());

        // Phase 5: Dispatch (post suggestion to platform if auto_dispatch)
        run.stage = RunStage::Dispatching;
        {
            let conn = self.db.lock().await;
            update_run(&conn, &run)?;
        }

        if settings.auto_dispatch && matches!(outcome, RunOutcome::Suggest) {
            if let Err(e) = self.dispatch_suggestion(&settings.api_url, &decision).await {
                warn!("Background service: dispatch failed: {e}");
            }
        }

        run.stage = RunStage::Completed;
        run.outcome = Some(outcome);
        run.completed_at = Some(now_unix());

        {
            let conn = self.db.lock().await;
            update_run(&conn, &run)?;
        }

        info!(
            "Background service: cycle {run_id} completed — {:?}",
            run.outcome
        );
        Ok(())
    }

    async fn collect_evidence(&self, api_url: &str) -> String {
        let client = crate::net_client::client();
        let mut parts = Vec::new();

        // Read from gizzi agent-sessions filtered by cowork surface
        let sessions_url = format!("{api_url}/api/v1/agent-sessions?surface=cowork&limit=10");
        match client.get(&sessions_url).send().await {
            Ok(res) if res.status().is_success() => {
                if let Ok(body) = res.text().await {
                    if body.len() > 20 {
                        parts.push(format!(
                            "Recent cowork sessions: {}",
                            &body[..body.len().min(500)]
                        ));
                    }
                }
            }
            _ => {}
        }

        // Read from cowork runs to incorporate DAG/WIH lifecycle state
        let runs_url = format!("{api_url}/api/v1/runs?limit=10");
        match client.get(&runs_url).send().await {
            Ok(res) if res.status().is_success() => {
                if let Ok(body) = res.text().await {
                    if body.len() > 20 {
                        parts.push(format!(
                            "Recent cowork runs: {}",
                            &body[..body.len().min(500)]
                        ));
                    }
                }
            }
            _ => {}
        }

        if parts.is_empty() {
            String::new()
        } else {
            parts.join("\n")
        }
    }

    async fn call_run_agent(
        &self,
        api_url: &str,
        agent_id: &str,
        role: &str,
        prompt: &str,
    ) -> Option<String> {
        let client = crate::net_client::client();
        let body = serde_json::json!({
            "spec": { "id": agent_id, "role": role, "prompt": prompt }
        });
        match client
            .post(format!("{api_url}/api/v1/cowork/run-agent"))
            .json(&body)
            .send()
            .await
        {
            Ok(res) if res.status().is_success() => {
                if let Ok(json) = res.json::<serde_json::Value>().await {
                    return json
                        .get("output")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                }
            }
            _ => {}
        }
        None
    }

    async fn ideate(&self, api_url: &str, evidence: &str) -> String {
        let prompt = format!(
            "You are an autonomous background agent. Review this evidence and propose one actionable improvement or task for the user:\n\n{evidence}\n\nRespond with a single concise suggestion."
        );
        self.call_run_agent(api_url, "bg-ideate", "strategist", &prompt)
            .await
            .unwrap_or_else(|| {
                format!(
                    "Consider reviewing recent cowork activity: {}",
                    &evidence[..evidence.len().min(100)]
                )
            })
    }

    async fn critique(&self, api_url: &str, evidence: &str, hypothesis: &str) -> bool {
        let prompt = format!(
            "Evidence: {evidence}\nProposed suggestion: {hypothesis}\n\nIs this suggestion useful and actionable? Reply with only 'yes' or 'no'."
        );
        match self
            .call_run_agent(api_url, "bg-critique", "critic", &prompt)
            .await
        {
            Some(text) => text.trim().to_lowercase().starts_with("yes"),
            None => true,
        }
    }

    async fn dispatch_suggestion(&self, api_url: &str, decision: &str) -> anyhow::Result<()> {
        let client = crate::net_client::client();
        client
            .post(format!("{api_url}/api/v1/cowork/suggestions"))
            .json(&serde_json::json!({ "content": decision, "source": "background_service" }))
            .send()
            .await?;
        Ok(())
    }
}

// ── Public Handle (cloneable, used by route handlers) ────────────────────────

#[derive(Clone)]
pub struct BackgroundServiceHandle {
    db: Arc<Mutex<Connection>>,
    status: Arc<RwLock<BrainStatus>>,
}

impl BackgroundServiceHandle {
    pub async fn summary(&self) -> anyhow::Result<BrainSummary> {
        let conn = self.db.lock().await;
        let settings = load_settings(&conn)?;
        let status = self.status.read().await.clone();
        let count = run_count(&conn)?;
        let last = last_run_at(&conn)?;
        Ok(BrainSummary {
            status,
            enabled: settings.enabled,
            cadence_minutes: settings.cadence_minutes,
            last_run_at: last,
            run_count: count,
        })
    }

    pub async fn recent_runs(&self, limit: u32) -> anyhow::Result<Vec<BackgroundRun>> {
        let conn = self.db.lock().await;
        Ok(recent_runs(&conn, limit)?)
    }

    pub async fn get_settings(&self) -> anyhow::Result<BackgroundSettings> {
        let conn = self.db.lock().await;
        Ok(load_settings(&conn)?)
    }

    pub async fn update_settings(&self, updates: BackgroundSettings) -> anyhow::Result<()> {
        let conn = self.db.lock().await;
        save_settings(&conn, &updates)?;
        Ok(())
    }
}
