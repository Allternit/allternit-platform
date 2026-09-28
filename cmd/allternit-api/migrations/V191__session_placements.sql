-- P4.2: a thread placed on another Allternit (your own server, Allternit
-- cloud) keeps its session there; this maps the session to where it lives.
CREATE TABLE IF NOT EXISTS session_placements (
    session_id TEXT PRIMARY KEY,
    target_id TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_session_placements_target ON session_placements(target_id);
