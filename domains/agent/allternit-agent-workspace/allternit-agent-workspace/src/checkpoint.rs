//! Checkpoint System
//!
//! Saves and restores agent state for crash recovery and session continuity.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use anyhow::Result;
use chrono::Utc;

/// Checkpoint entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub id: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub session_id: String,
    pub agent_state: AgentState,
    pub task_state: TaskState,
    pub memory_refs: Vec<String>,
    pub context_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentState {
    pub current_goal: String,
    pub recent_decisions: Vec<String>,
    pub open_questions: Vec<String>,
    pub blockers: Vec<String>,
    pub mood: AgentMood,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentMood {
    Focused,
    Exploring,
    Blocked,
    Reviewing,
    Completing,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskState {
    pub active_task_id: Option<String>,
    pub progress_percent: u8,
    pub completed_steps: Vec<String>,
    pub pending_steps: Vec<String>,
    pub verification_status: VerificationStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VerificationStatus {
    NotStarted,
    InProgress,
    Passed,
    Failed,
}

/// Checkpoint manager
pub struct CheckpointManager {
    workspace: PathBuf,
    checkpoints_file: PathBuf,
}

impl CheckpointManager {
    pub fn new(workspace: &Path) -> Self {
        Self {
            workspace: workspace.to_path_buf(),
            checkpoints_file: workspace.join(".allternit/state/checkpoints.jsonl"),
        }
    }

    /// Create a new checkpoint
    pub fn create(&self, session_id: &str, agent_state: AgentState, task_state: TaskState) -> Result<Checkpoint> {
        let checkpoint = Checkpoint {
            id: format!("ckpt_{}", Utc::now().timestamp()),
            timestamp: Utc::now(),
            session_id: session_id.to_string(),
            agent_state,
            task_state,
            memory_refs: self.collect_memory_refs(),
            context_hash: self.compute_context_hash(),
        };
        
        // Append to checkpoints file
        let line = serde_json::to_string(&checkpoint)?;
        
        // Ensure directory exists
        if let Some(parent) = self.checkpoints_file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.checkpoints_file)?;
        
        writeln!(file, "{}", line)?;
        
        log::info!("Created checkpoint: {}", checkpoint.id);
        
        Ok(checkpoint)
    }

    /// Get the most recent checkpoint
    pub fn latest(&self) -> Result<Option<Checkpoint>> {
        if !self.checkpoints_file.exists() {
            return Ok(None);
        }
        
        let content = std::fs::read_to_string(&self.checkpoints_file)?;
        let mut latest: Option<Checkpoint> = None;
        
        for line in content.lines() {
            if let Ok(checkpoint) = serde_json::from_str::<Checkpoint>(line) {
                if latest.is_none() || checkpoint.timestamp > latest.as_ref().unwrap().timestamp {
                    latest = Some(checkpoint);
                }
            }
        }
        
        Ok(latest)
    }

    /// Get checkpoints from a specific session
    pub fn for_session(&self, session_id: &str) -> Result<Vec<Checkpoint>> {
        if !self.checkpoints_file.exists() {
            return Ok(Vec::new());
        }
        
        let content = std::fs::read_to_string(&self.checkpoints_file)?;
        let mut checkpoints = Vec::new();
        
        for line in content.lines() {
            if let Ok(checkpoint) = serde_json::from_str::<Checkpoint>(line) {
                if checkpoint.session_id == session_id {
                    checkpoints.push(checkpoint);
                }
            }
        }
        
        checkpoints.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
        Ok(checkpoints)
    }

    /// List all checkpoints
    pub fn list(&self, limit: usize) -> Result<Vec<Checkpoint>> {
        if !self.checkpoints_file.exists() {
            return Ok(Vec::new());
        }
        
        let content = std::fs::read_to_string(&self.checkpoints_file)?;
        let mut checkpoints: Vec<Checkpoint> = content
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        
        // Sort by timestamp (newest first)
        checkpoints.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        
        // Limit results
        checkpoints.truncate(limit);
        
        Ok(checkpoints)
    }

    /// Restore from a checkpoint
    pub fn restore(&self, checkpoint_id: &str) -> Result<Option<Checkpoint>> {
        let checkpoints = self.list(1000)?;
        
        for checkpoint in checkpoints {
            if checkpoint.id == checkpoint_id {
                log::info!("Restoring from checkpoint: {}", checkpoint_id);
                return Ok(Some(checkpoint));
            }
        }
        
        log::warn!("Checkpoint not found: {}", checkpoint_id);
        Ok(None)
    }

    /// Clean up old checkpoints
    pub fn cleanup(&self, keep_days: u32) -> Result<usize> {
        if !self.checkpoints_file.exists() {
            return Ok(0);
        }
        
        let cutoff = Utc::now() - chrono::Duration::days(keep_days as i64);
        let content = std::fs::read_to_string(&self.checkpoints_file)?;
        
        let mut kept = 0;
        let mut removed = 0;
        
        let filtered: Vec<String> = content
            .lines()
            .filter(|line| {
                if let Ok(checkpoint) = serde_json::from_str::<Checkpoint>(line) {
                    if checkpoint.timestamp > cutoff {
                        kept += 1;
                        true
                    } else {
                        removed += 1;
                        false
                    }
                } else {
                    false
                }
            })
            .map(|s| s.to_string())
            .collect();
        
        // Rewrite file with only kept checkpoints
        std::fs::write(&self.checkpoints_file, filtered.join("\n"))?;
        
        log::info!("Checkpoint cleanup: kept {}, removed {}", kept, removed);
        
        Ok(removed)
    }

    /// Collect references to memory files
    fn collect_memory_refs(&self) -> Vec<String> {
        let mut refs = Vec::new();
        
        // Add today's memory file
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let today_path = self.workspace.join("memory").join(format!("{}.md", today));
        if today_path.exists() {
            refs.push(today_path.to_string_lossy().to_string());
        }
        
        // Add active tasks
        let active_tasks = self.workspace.join("memory/active-tasks.md");
        if active_tasks.exists() {
            refs.push(active_tasks.to_string_lossy().to_string());
        }
        
        refs
    }

    /// Compute hash of current context
    fn compute_context_hash(&self) -> String {
        // In a real implementation, this would hash the context pack
        // For now, return a placeholder
        format!("hash_{}", Utc::now().timestamp())
    }
}

/// Auto-checkpoint on interval
pub struct AutoCheckpoint {
    manager: CheckpointManager,
    interval: std::time::Duration,
    last_checkpoint: Option<std::time::Instant>,
}

impl AutoCheckpoint {
    pub fn new(workspace: &Path, interval_minutes: u64) -> Self {
        Self {
            manager: CheckpointManager::new(workspace),
            interval: std::time::Duration::from_secs(interval_minutes * 60),
            last_checkpoint: None,
        }
    }

    /// Check if it's time to checkpoint
    pub fn should_checkpoint(&self) -> bool {
        match self.last_checkpoint {
            None => true,
            Some(last) => last.elapsed() >= self.interval,
        }
    }

    /// Create checkpoint if due
    pub fn maybe_checkpoint(&mut self, session_id: &str, agent_state: AgentState, task_state: TaskState) -> Result<Option<Checkpoint>> {
        if self.should_checkpoint() {
            let checkpoint = self.manager.create(session_id, agent_state, task_state)?;
            self.last_checkpoint = Some(std::time::Instant::now());
            Ok(Some(checkpoint))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_checkpoint_create_and_retrieve() {
        let temp = TempDir::new().unwrap();
        let manager = CheckpointManager::new(temp.path());

        let agent_state = AgentState {
            current_goal: "Test goal".to_string(),
            recent_decisions: vec![],
            open_questions: vec![],
            blockers: vec![],
            mood: AgentMood::Focused,
        };

        let task_state = TaskState {
            active_task_id: Some("T1".to_string()),
            progress_percent: 50,
            completed_steps: vec!["Step 1".to_string()],
            pending_steps: vec!["Step 2".to_string()],
            verification_status: VerificationStatus::NotStarted,
        };

        let checkpoint = manager.create("session_1", agent_state, task_state).unwrap();
        
        assert_eq!(checkpoint.session_id, "session_1");
        assert!(matches!(checkpoint.agent_state.mood, AgentMood::Focused));

        let latest = manager.latest().unwrap();
        assert!(latest.is_some());
        assert_eq!(latest.unwrap().id, checkpoint.id);
    }

    #[test]
    fn test_checkpoint_cleanup() {
        let temp = TempDir::new().unwrap();
        let manager = CheckpointManager::new(temp.path());

        // Create test checkpoint
        let agent_state = AgentState {
            current_goal: "Old goal".to_string(),
            recent_decisions: vec![],
            open_questions: vec![],
            blockers: vec![],
            mood: AgentMood::Focused,
        };

        let task_state = TaskState {
            active_task_id: None,
            progress_percent: 0,
            completed_steps: vec![],
            pending_steps: vec![],
            verification_status: VerificationStatus::NotStarted,
        };

        manager.create("old_session", agent_state, task_state).unwrap();

        // Cleanup with 0 days (should remove all)
        let removed = manager.cleanup(0).unwrap();
        assert_eq!(removed, 1);

        let latest = manager.latest().unwrap();
        assert!(latest.is_none());
    }
}
