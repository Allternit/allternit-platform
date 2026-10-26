-- V275: per-user subagent catalog rows. The 15 built-in subagent types live
-- client-side and in gizzi; this table holds only the caller's custom types
-- and overrides of a built-in (keyed by the same slug, is_custom = 0).
-- `tools` is a JSON array of strings; `look` is JSON {pack, face, accent?}.
CREATE TABLE IF NOT EXISTS subagent_types (
    id          TEXT PRIMARY KEY,
    user_id     TEXT NOT NULL,
    slug        TEXT NOT NULL,
    name        TEXT,
    description TEXT,
    prompt      TEXT,
    model_class TEXT,
    tools       TEXT,
    max_steps   INTEGER,
    look        TEXT,
    is_custom   INTEGER NOT NULL DEFAULT 1,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (user_id, slug)
);

CREATE INDEX IF NOT EXISTS idx_subagent_types_user ON subagent_types(user_id);
