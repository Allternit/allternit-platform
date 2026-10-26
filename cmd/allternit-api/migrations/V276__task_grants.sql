-- One-task grants (`src/task_grants.rs`): the owner's "you may do X for this
-- task" on one bot thread. While a grant is live, an autonomy hold for an
-- action it covers becomes "send and tell me" (`autonomy::evaluate_ctx`), and
-- a channel post that needs Allternit approval goes out without a new card
-- (`channel_gateway::send`). Guards (taint floor, secrets, PII) still apply.
-- payment, sign and delete are never covered.
--
-- actions_json: JSON array of action names (send | upload | submit | booking |
-- call, or a gizzi permission class for a card grant), same style as the
-- other *_json columns. source: 'card' (the "Allow for this task" button or
-- plan approval) | 'words' (the owner said so in the thread). source_ref: the
-- approval id, or the owner message the grant came from.
-- Ends at expires_at (7 days), or revoked_at (Revoke, or the thread closed).
CREATE TABLE IF NOT EXISTS task_grants (
    id           TEXT PRIMARY KEY,
    owner        TEXT NOT NULL,
    bot_id       TEXT NOT NULL,
    thread_id    TEXT NOT NULL,
    actions_json TEXT NOT NULL DEFAULT '[]',
    source       TEXT NOT NULL CHECK (source IN ('card', 'words')),
    source_ref   TEXT NOT NULL DEFAULT '',
    created_at   TEXT NOT NULL,
    expires_at   TEXT NOT NULL,
    revoked_at   TEXT
);
CREATE INDEX IF NOT EXISTS idx_task_grants_thread ON task_grants(thread_id, revoked_at);
CREATE INDEX IF NOT EXISTS idx_task_grants_owner ON task_grants(owner, bot_id);

-- Which thread an autonomy outcome belongs to (the "Sent 3 things under your
-- OK" digest counts outcome = 'granted' rows per thread).
ALTER TABLE autonomy_actions ADD COLUMN thread_id TEXT;
