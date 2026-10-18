-- Project Delegate board (`src/delegate_routes.rs`).
--
-- A project's to-do list you hand to its bots. A task waits here with no bot
-- until you drop it on one (or Auto does); assigning it creates a project
-- thread for that bot and records it in `thread_id`. The row stays after
-- assignment so the thread keeps the user's time estimate and the board
-- knows which threads it queued.
CREATE TABLE IF NOT EXISTS project_tasks (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    title TEXT NOT NULL,
    notes TEXT,
    -- The user's own estimate in minutes; NULL when they gave none.
    estimate_min INTEGER,
    position REAL NOT NULL,
    assigned_bot_id TEXT,
    assigned_at TEXT,
    thread_id TEXT REFERENCES bot_threads(id) ON DELETE SET NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_project_tasks_project ON project_tasks(user_id, project_id, position);
CREATE INDEX IF NOT EXISTS idx_project_tasks_thread ON project_tasks(thread_id);

-- Auto: new tasks go to whichever bot on the team is free first.
CREATE TABLE IF NOT EXISTS project_delegate_settings (
    project_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    auto INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (project_id, user_id)
);
