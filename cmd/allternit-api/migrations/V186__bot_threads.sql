-- Durable bot threads (`src/thread_routes.rs`, spec BOT_THREAD_PARITY_SPEC P3.1).
--
-- A thread is a unit of work owned by a bot, not a chat session. It outlives
-- any one model context: each context window is a generation
-- (`bot_thread_sessions`), and a handoff writes a checkpoint and starts the
-- next one. Status uses the spec's grammar; the UI groups it (waiting /
-- working / queued / idle / resolved) from the server, never from transcript
-- text.
CREATE TABLE IF NOT EXISTS bot_threads (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    bot_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    project_id TEXT,
    parent_thread_id TEXT,
    -- 'standing' (long-lived: a bot's main chat, a routine's home) or 'task'
    kind TEXT NOT NULL DEFAULT 'task',
    incognito INTEGER NOT NULL DEFAULT 0,
    title TEXT NOT NULL,
    objective TEXT,
    success_criteria TEXT,
    -- queued | planning | working | blocked | needs_you | review | done | failed | paused | idle
    status TEXT NOT NULL DEFAULT 'idle',
    status_line TEXT,
    todo TEXT,
    summary TEXT,
    checkpoint TEXT,
    current_session_id TEXT,
    -- user | coordinator | bot | mention | routine | import
    created_by TEXT NOT NULL DEFAULT 'user',
    origin TEXT,
    started_at TEXT,
    last_activity_at TEXT NOT NULL,
    resolved_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_bot_threads_user ON bot_threads(user_id, last_activity_at);
CREATE INDEX IF NOT EXISTS idx_bot_threads_bot ON bot_threads(bot_id);
CREATE INDEX IF NOT EXISTS idx_bot_threads_project ON bot_threads(project_id);

CREATE TABLE IF NOT EXISTS bot_thread_sessions (
    thread_id TEXT NOT NULL REFERENCES bot_threads(id) ON DELETE CASCADE,
    generation INTEGER NOT NULL,
    session_id TEXT NOT NULL,
    model TEXT,
    context_window INTEGER,
    tokens_used INTEGER NOT NULL DEFAULT 0,
    -- start | budget | model_switch | routine_run | manual
    reason TEXT NOT NULL DEFAULT 'start',
    checkpoint_summary TEXT,
    started_at TEXT NOT NULL,
    ended_at TEXT,
    PRIMARY KEY (thread_id, generation)
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_bot_thread_sessions_session ON bot_thread_sessions(session_id);

CREATE TABLE IF NOT EXISTS bot_thread_deps (
    thread_id TEXT NOT NULL REFERENCES bot_threads(id) ON DELETE CASCADE,
    depends_on TEXT NOT NULL REFERENCES bot_threads(id) ON DELETE CASCADE,
    PRIMARY KEY (thread_id, depends_on)
);

-- A project's team: the bots the coordinator can assign threads to.
CREATE TABLE IF NOT EXISTS project_bots (
    project_id TEXT NOT NULL,
    bot_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    added_at TEXT NOT NULL,
    PRIMARY KEY (project_id, bot_id)
);

ALTER TABLE bot_events ADD COLUMN thread_id TEXT;
CREATE INDEX IF NOT EXISTS idx_bot_events_thread ON bot_events(thread_id);
