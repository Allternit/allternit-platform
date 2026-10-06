-- Canonical personal drive pointers; git is truth, these tables are projections.
CREATE TABLE memory_drives (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL DEFAULT 'memory' CHECK(name = 'memory'),
    directory_key TEXT NOT NULL UNIQUE,
    brain_id TEXT NOT NULL UNIQUE REFERENCES brains(id),
    repo_path TEXT NOT NULL UNIQUE,
    branch TEXT NOT NULL DEFAULT 'main',
    indexed_revision TEXT,
    dirty_revision TEXT,
    imported_at TEXT,
    import_revision TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
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
-- Converted rows are immediately deduplicated; skipped archives remain visible
-- for 30 days. Nothing deletes or overwrites the historical source rows.
CREATE VIEW memory_drive_visible_facts AS
SELECT f.* FROM memory_facts f WHERE NOT EXISTS (
    SELECT 1 FROM memory_drive_import_rows i
    JOIN memory_drives d ON d.id = i.drive_id AND d.user_id = f.user_id
    WHERE i.legacy_fact_id = f.id
      AND (i.entry_id IS NOT NULL OR datetime(i.imported_at, '+30 days') <= datetime('now'))
);
