//! Records which skill a bot actually ran. The agent-chat bridge sees gizzi's
//! `skill` tool calls as `message.part.updated` events; `record_from_part`
//! stores one row per call. Best-effort: never affects the stream.

use crate::db::DbHandle;
use rusqlite::params;

/// The skill name of a gizzi tool part, when the part is a `skill` tool call.
pub fn skill_name_of_part(part: &serde_json::Value) -> Option<String> {
    // Native gizzi calls it `skill`; a CLI brain sees it as `mcp__allternit__skill`
    // and Claude's own tool is `Skill`, whose input names it `skill`.
    let tool = part.get("tool").and_then(|v| v.as_str())?;
    let is_skill = tool.eq_ignore_ascii_case("skill") || tool.ends_with("__skill");
    if !is_skill {
        return None;
    }
    part.pointer("/state/input/name")
        .or_else(|| part.pointer("/state/input/skill"))
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

#[cfg(test)]
mod tests {
    use super::skill_name_of_part;
    use serde_json::json;

    /// 2026-11-01: a Claude-CLI bot's skill call never counted, because only the
    /// native tool name matched, so skills never reached the memory map.
    #[test]
    fn counts_the_skill_tool_under_every_brain_name() {
        for tool in ["skill", "Skill", "mcp__allternit__skill"] {
            let part = json!({"tool": tool, "state": {"input": {"name": "artifacts"}}});
            assert_eq!(skill_name_of_part(&part).as_deref(), Some("artifacts"), "{tool}");
        }
        assert_eq!(skill_name_of_part(&json!({"tool": "Skill", "state": {"input": {"skill": "x"}}})).as_deref(), Some("x"));
        assert_eq!(skill_name_of_part(&json!({"tool": "bash", "state": {"input": {"name": "x"}}})), None);
    }
}
