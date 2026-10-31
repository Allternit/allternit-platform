//! Records which skill a bot actually ran. The agent-chat bridge sees gizzi's
//! `skill` tool calls as `message.part.updated` events; `record_from_part`
//! stores one row per call. Best-effort: never affects the stream.

use crate::db::DbHandle;
use rusqlite::params;

/// The skill name of a gizzi tool part, when the part is a `skill` tool call.
pub fn skill_name_of_part(part: &serde_json::Value) -> Option<String> {
    if part.get("tool").and_then(|v| v.as_str()) != Some("skill") {
        return None;
    }
    part.pointer("/state/input/name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
}

pub async fn record_from_part(
    db: &DbHandle,
    user_id: &str,
    agent_id: Option<&str>,
    session_id: &str,
    call_id: &str,
    part: &serde_json::Value,
) {
    let (Some(agent_id), Some(skill)) = (agent_id, skill_name_of_part(part)) else {
        return;
    };
    let db = db.clone();
    let (user_id, agent_id, session_id, call_id) = (
        user_id.to_string(),
        agent_id.to_string(),
        session_id.to_string(),
        call_id.to_string(),
    );
    let _ = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "INSERT OR IGNORE INTO skill_use (user_id, agent_id, skill, session_id, call_id) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![user_id, agent_id, skill, session_id, call_id],
        )
    })
    .await;
}
