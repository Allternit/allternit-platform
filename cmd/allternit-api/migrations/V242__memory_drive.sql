-- Canonical personal drive pointers; git is truth, these tables are projections.
CREATE TABLE memory_drives (
    id TEXT PRIMARY KEY,
    -- Creating/owning account. For personal drives this is the owner; other
    -- kinds (Phase 3) check membership of scope_id on every request.
    user_id TEXT NOT NULL,
    kind TEXT NOT NULL DEFAULT 'personal' CHECK(kind IN ('personal','project','team','bot','swarm')),
    scope_id TEXT NOT NULL,
    name TEXT NOT NULL DEFAULT 'memory',
    directory_key TEXT NOT NULL,
    brain_id TEXT NOT NULL UNIQUE REFERENCES brains(id),
    repo_path TEXT NOT NULL UNIQUE,
    branch TEXT NOT NULL DEFAULT 'main',
    indexed_revision TEXT,
    dirty_revision TEXT,
    imported_at TEXT,
    import_revision TEXT,
    dreaming_enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(kind, scope_id)
);
CREATE INDEX idx_memory_drives_user ON memory_drives(user_id);
-- Per-database storage root, written at startup from config. A database with
-- no root (unit tests, tools) keeps the legacy row writers.
CREATE TABLE memory_drive_config (key TEXT PRIMARY KEY, value TEXT NOT NULL);
-- Writes whose git commit failed (not a CAS conflict). Retried on the next
-- write/read; visible in /memory/drive/health. Never silently dropped.
CREATE TABLE memory_drive_pending (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    payload TEXT NOT NULL,
    error TEXT,
    attempts INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX idx_memory_drive_pending_user ON memory_drive_pending(user_id);
CREATE TABLE memory_drive_entries (
    drive_id TEXT NOT NULL REFERENCES memory_drives(id),
    file_path TEXT NOT NULL,
    entry_id TEXT NOT NULL,
    fact_id TEXT NOT NULL UNIQUE REFERENCES memory_facts(id),
    content_hash TEXT NOT NULL,
    source TEXT NOT NULL,
    metadata TEXT NOT NULL,
    PRIMARY KEY(drive_id, entry_id)
);
CREATE TABLE memory_drive_import_rows (
    drive_id TEXT NOT NULL REFERENCES memory_drives(id),
    legacy_fact_id TEXT NOT NULL,
    entry_id TEXT,
    imported_at TEXT NOT NULL,
    PRIMARY KEY(drive_id, legacy_fact_id)
);
-- Converted rows are soft-retired (valid_until) at import so recall isn't
-- doubled; skipped rows stay visible for 30 days, then are soft-retired too
-- (memory_drive_service::hide_expired_archives). Rows are never deleted.
-- Repo-scoped git credentials. NULL brain_id = legacy owner token (all owned
-- brains, read+write, unchanged behavior). Memory Drive tokens are scoped to
-- one brain and are 'read' (clone/fetch only) or 'write' (push validated by
-- the drive's pre-receive hook).
ALTER TABLE git_tokens ADD COLUMN brain_id TEXT;
ALTER TABLE git_tokens ADD COLUMN access TEXT NOT NULL DEFAULT 'write' CHECK(access IN ('read','write'));
CREATE INDEX idx_git_tokens_brain ON git_tokens(brain_id);
-- Nightly Dream runs: one per drive per date (the UNIQUE key is the lease).
-- The report and summary describe exactly what the Dream commit changed.
CREATE TABLE memory_dreams (
    id TEXT PRIMARY KEY,
    drive_id TEXT NOT NULL REFERENCES memory_drives(id),
    user_id TEXT NOT NULL,
    date TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('running','applied','no_changes','failed','undone')),
    base_revision TEXT,
    revision TEXT,
    report TEXT,
    summary TEXT,
    error TEXT,
    undo_revision TEXT,
    attempts INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(drive_id, date)
);
CREATE INDEX idx_memory_dreams_user ON memory_dreams(user_id, date);
-- Bot/project memory rows rebuilt from a bot or project Memory Drive carry
-- that drive's id; rows without one are archive or unscoped entries.
ALTER TABLE cowork_memory_entries ADD COLUMN drive_id TEXT;
CREATE INDEX idx_cowork_memory_drive ON cowork_memory_entries(drive_id);
