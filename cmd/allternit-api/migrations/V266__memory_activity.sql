-- V266: Memory Drive activity log (`src/memory_activity.rs`). One row per
-- thing that touched a person's memory: a save, a recall, a forget, an
-- import, a Dream step or a vendor read. Shown in chat under the turn, in
-- Memory Drive → Activity and as live Dream steps. Rows hold short previews
-- only; the drive's git history stays the full record.
CREATE TABLE IF NOT EXISTS memory_activity (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT NOT NULL,
    at TEXT NOT NULL,
    kind TEXT NOT NULL,
    title TEXT NOT NULL,
    detail TEXT NOT NULL DEFAULT '{}',
    source TEXT,
    session_id TEXT,
    run_id TEXT
);
CREATE INDEX IF NOT EXISTS idx_memory_activity_user ON memory_activity(user_id, id);
CREATE INDEX IF NOT EXISTS idx_memory_activity_session ON memory_activity(user_id, session_id, id);
CREATE INDEX IF NOT EXISTS idx_memory_activity_run ON memory_activity(user_id, run_id, id);
