-- Project chat with the coordinator (`src/coordinator_routes.rs`, spec P5).
-- The user talks to Al once per project; Al's replies carry what it did in
-- `payload` (threads it started, the thread it routed a follow-up to, a
-- thread that finished) so every surface renders the same receipts.
CREATE TABLE IF NOT EXISTS project_messages (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    -- user | coordinator
    role TEXT NOT NULL,
    text TEXT NOT NULL,
    payload TEXT,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_project_messages_project ON project_messages(project_id, created_at);
