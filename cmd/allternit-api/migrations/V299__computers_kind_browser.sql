-- Add 'browser' to the computers.kind CHECK (V99:6) so the Rust enum
-- (ComputerKind::Browser) and the database agree. A browser computer has no
-- server row today (create returns 400), but any path that writes or
-- round-trips the value must not fail on the CHECK.
--
-- SQLite cannot alter a CHECK in place, so the table is rebuilt. Two traps
-- make this different from the V267/V278 rebuilds:
--
-- 1. The bundled SQLite turns foreign keys ON by default, and refinery runs
--    each migration inside a transaction, where `PRAGMA foreign_keys=OFF` is
--    a no-op. `DROP TABLE computers` then runs an implicit DELETE that
--    cascades into the four side tables (computer_cloud_desktop,
--    computer_managed, computer_byo_vps, computer_byoc; all ON DELETE
--    CASCADE). Their rows are copied to temp tables first and put back after.
-- 2. Renaming the old table out of the way would rewrite the side tables'
--    REFERENCES to the old name (foreign keys are on). So the order is the
--    one SQLite documents: create new, copy, drop old, rename new.
--
-- Column order matches the live table: V99, then V135 (idle_timeout_secs,
-- last_activity_at), V136 (group_id), V139 (role).

CREATE TEMP TABLE v299_cloud_desktop AS SELECT * FROM computer_cloud_desktop;
CREATE TEMP TABLE v299_managed AS SELECT * FROM computer_managed;
CREATE TEMP TABLE v299_byo_vps AS SELECT * FROM computer_byo_vps;
CREATE TEMP TABLE v299_byoc AS SELECT * FROM computer_byoc;

CREATE TABLE computers_v299 (
    id              TEXT PRIMARY KEY,
    kind            TEXT NOT NULL CHECK (kind IN ('local', 'byo_vps', 'managed', 'byoc', 'cloud_desktop', 'browser')),
    provider        TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT 'creating' CHECK (status IN ('creating', 'running', 'stopped', 'error', 'deleted')),
    owner_type      TEXT NOT NULL CHECK (owner_type IN ('user', 'org', 'bot')),
    owner_id        TEXT NOT NULL,
    bot_id          TEXT REFERENCES agents(id) ON DELETE SET NULL,
    session_id      TEXT,
    name            TEXT NOT NULL,
    os              TEXT,
    cpu_cores       INTEGER,
    memory_mb       INTEGER,
    disk_mb         INTEGER,
    region          TEXT,
    host            TEXT,
    native_id       TEXT,
    credential_id   TEXT,
    template_id     TEXT,
    billing_source  TEXT NOT NULL DEFAULT 'credits' CHECK (billing_source IN ('free', 'credits', 'provider_direct', 'platform_fee')),
    created_at      DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at      DATETIME DEFAULT CURRENT_TIMESTAMP,
    idle_timeout_secs INTEGER,
    last_activity_at DATETIME,
    group_id        TEXT REFERENCES computer_groups(id) ON DELETE SET NULL,
    role            TEXT NOT NULL DEFAULT 'user'
);

-- A dangling bot_id / group_id would fail the insert and stop the API from
-- booting. Clear it, which is what ON DELETE SET NULL would have done.
INSERT INTO computers_v299
    (id, kind, provider, status, owner_type, owner_id, bot_id, session_id, name,
     os, cpu_cores, memory_mb, disk_mb, region, host, native_id, credential_id,
     template_id, billing_source, created_at, updated_at, idle_timeout_secs,
     last_activity_at, group_id, role)
SELECT id, kind, provider, status, owner_type, owner_id,
       CASE WHEN bot_id IS NOT NULL AND EXISTS (SELECT 1 FROM agents a WHERE a.id = c.bot_id)
            THEN bot_id END,
       session_id, name, os, cpu_cores, memory_mb, disk_mb, region, host,
       native_id, credential_id, template_id, billing_source, created_at,
       updated_at, idle_timeout_secs, last_activity_at,
       CASE WHEN group_id IS NOT NULL AND EXISTS (SELECT 1 FROM computer_groups g WHERE g.id = c.group_id)
            THEN group_id END,
       role
FROM computers c;

DROP TABLE computers;
ALTER TABLE computers_v299 RENAME TO computers;

INSERT INTO computer_cloud_desktop SELECT * FROM temp.v299_cloud_desktop;
INSERT INTO computer_managed SELECT * FROM temp.v299_managed;
INSERT INTO computer_byo_vps SELECT * FROM temp.v299_byo_vps;
INSERT INTO computer_byoc SELECT * FROM temp.v299_byoc;

DROP TABLE temp.v299_cloud_desktop;
DROP TABLE temp.v299_managed;
DROP TABLE temp.v299_byo_vps;
DROP TABLE temp.v299_byoc;

-- Indexes and trigger from V99, V135 and V139 (dropped with the old table).
CREATE INDEX IF NOT EXISTS idx_computers_owner ON computers(owner_type, owner_id);
CREATE INDEX IF NOT EXISTS idx_computers_bot ON computers(bot_id);
CREATE INDEX IF NOT EXISTS idx_computers_session ON computers(session_id);
CREATE INDEX IF NOT EXISTS idx_computers_status ON computers(status);
CREATE INDEX IF NOT EXISTS idx_computers_kind_provider ON computers(kind, provider);
CREATE INDEX IF NOT EXISTS idx_computers_idle ON computers(status, idle_timeout_secs);
CREATE INDEX IF NOT EXISTS idx_computers_role ON computers(role);

CREATE TRIGGER IF NOT EXISTS computers_updated_at
AFTER UPDATE ON computers
BEGIN
    UPDATE computers SET updated_at = CURRENT_TIMESTAMP WHERE id = NEW.id;
END;
