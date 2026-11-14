-- Several replicas on one database (selfhost 1.1).
-- Append-only: 001 and 002 are never edited. A 1.0 server cannot run on a
-- database after this migration.

-- Hold off writers (a 1.0 replica still running during a rolling upgrade)
-- between the clean-up below and the unique index.
LOCK TABLE work_items IN SHARE ROW EXCLUSIVE MODE;

-- One open work item per session. 1.0 enforced this only inside one process,
-- so a database used by two processes can hold duplicates. Keep one per
-- session (a leased item over a ready one, then the newest) and cancel the
-- rest. Tool calls claimed under a cancelled item go back to pending.
CREATE TEMP TABLE selfhost_003_dup_work ON COMMIT DROP AS
SELECT id FROM (
    SELECT id, row_number() OVER (
        PARTITION BY session_id
        ORDER BY (status = 'leased') DESC, created_at DESC, id DESC
    ) AS rn
    FROM work_items
    WHERE status IN ('ready', 'leased')
) ranked
WHERE rn > 1;

UPDATE tool_calls
SET status = 'pending', work_id = NULL, claimed_generation = NULL, updated_at = now()
WHERE status = 'claimed' AND work_id IN (SELECT id FROM selfhost_003_dup_work);

UPDATE work_items
SET status = 'cancelled',
    lease_expires_at = NULL,
    session_secret_hash = NULL,
    error_message = COALESCE(error_message, 'Closed by migration 003: the session had another open work item.'),
    completed_at = now(),
    updated_at = now()
WHERE id IN (SELECT id FROM selfhost_003_dup_work);

CREATE UNIQUE INDEX IF NOT EXISTS work_items_one_open ON work_items (session_id)
    WHERE status IN ('ready', 'leased');

-- Soft worker affinity: for a short while after a worker completes or
-- releases a session's item, a claim from that worker is preferred.
ALTER TABLE work_items ADD COLUMN IF NOT EXISTS preferred_worker_id TEXT;
ALTER TABLE work_items ADD COLUMN IF NOT EXISTS prefer_until TIMESTAMPTZ;

-- Turn ownership: the replica running the turn, and the replica whose gizzi
-- child holds gizzi_session_id.
ALTER TABLE sessions ADD COLUMN IF NOT EXISTS turn_replica TEXT;
ALTER TABLE sessions ADD COLUMN IF NOT EXISTS gizzi_replica TEXT;
CREATE INDEX IF NOT EXISTS sessions_turn_replica ON sessions (turn_replica)
    WHERE turn_replica IS NOT NULL;

-- Idempotency-Key records. One per (principal, method, path, key). Only 2xx
-- responses are stored; kept 24 hours.
CREATE TABLE IF NOT EXISTS idempotency_keys (
    principal        TEXT NOT NULL,
    method           TEXT NOT NULL,
    path             TEXT NOT NULL,
    idem_key         TEXT NOT NULL,
    request_hash     TEXT NOT NULL,
    state            TEXT NOT NULL CHECK (state IN ('in_progress', 'done')),
    status_code      INTEGER,
    response_headers JSONB,
    response_body    BYTEA,
    created_at       TIMESTAMPTZ NOT NULL,
    updated_at       TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (principal, method, path, idem_key)
);
CREATE INDEX IF NOT EXISTS idempotency_keys_created ON idempotency_keys (created_at);

-- Live server replicas (heartbeat rows).
CREATE TABLE IF NOT EXISTS server_replicas (
    replica_id   TEXT PRIMARY KEY,
    started_at   TIMESTAMPTZ NOT NULL,
    heartbeat_at TIMESTAMPTZ NOT NULL,
    version      TEXT
);
