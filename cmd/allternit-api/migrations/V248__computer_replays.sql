-- Replay cache for run_subtask (driver spec D4, computer_replay.rs).
-- One row per recorded subtask, keyed per owner by
-- hash(goal, app + window shape, input names). Steps hold semantic
-- locators (role, name, tree path, window-relative box, pixel hash) and a
-- verify-syntax check each. Input VALUES are never stored: every place one
-- appeared holds a {{name}} placeholder, substituted at replay time.
-- A row with a name is a saved skill (run_skill / skills).
CREATE TABLE IF NOT EXISTS computer_replays (
    id TEXT PRIMARY KEY,
    owner TEXT NOT NULL,
    cache_key TEXT NOT NULL,
    name TEXT,
    goal TEXT NOT NULL,
    app TEXT NOT NULL DEFAULT '',
    window TEXT NOT NULL DEFAULT '',
    variables_json TEXT NOT NULL,
    success_json TEXT NOT NULL,
    steps_json TEXT NOT NULL,
    hits INTEGER NOT NULL DEFAULT 0,
    heals INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    last_used_at TEXT
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_computer_replays_owner_key ON computer_replays(owner, cache_key);
CREATE UNIQUE INDEX IF NOT EXISTS idx_computer_replays_owner_name ON computer_replays(owner, name) WHERE name IS NOT NULL;
