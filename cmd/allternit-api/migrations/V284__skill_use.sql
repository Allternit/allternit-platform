-- Where a bot's skills actually ran. One row per skill-tool call seen in the
-- agent-chat stream (gizzi's `skill` tool), written by skill_use::record.
-- `call_id` is the tool callID so repeated "running" updates of one call count
-- once. Read by /bot-hub/:id/summary and /memory-map. Lives on allternit-api
-- (SQLite); cloud-api is not involved.
CREATE TABLE IF NOT EXISTS skill_use (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id    TEXT NOT NULL,
    agent_id   TEXT NOT NULL,
    skill      TEXT NOT NULL,
    session_id TEXT NOT NULL,
    call_id    TEXT NOT NULL,
    used_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now')),
    UNIQUE (session_id, call_id)
);
CREATE INDEX IF NOT EXISTS idx_skill_use_agent ON skill_use(user_id, agent_id, used_at);
CREATE INDEX IF NOT EXISTS idx_skill_use_session ON skill_use(session_id);
