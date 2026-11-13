-- Allternit Self-Hosted Agents Server: initial schema.
-- Applied by `sqlx::migrate!("./migrations_selfhost_pg")` (selfhost::store::pg).
-- This database belongs to the Agents Server alone; it shares nothing with
-- the hosted allternit-api or cloud-api schemas.

CREATE TABLE IF NOT EXISTS agents (
    id           TEXT PRIMARY KEY,
    name         TEXT NOT NULL,
    instructions TEXT NOT NULL DEFAULT '',
    model        TEXT NOT NULL,
    tools        JSONB NOT NULL DEFAULT '[]'::jsonb,
    mcp_servers  JSONB NOT NULL DEFAULT '[]'::jsonb,
    skills       JSONB NOT NULL DEFAULT '[]'::jsonb,
    metadata     JSONB NOT NULL DEFAULT '{}'::jsonb,
    version      BIGINT NOT NULL DEFAULT 1,
    created_at   TIMESTAMPTZ NOT NULL,
    updated_at   TIMESTAMPTZ NOT NULL,
    deleted_at   TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS agents_created ON agents (created_at, id);

CREATE TABLE IF NOT EXISTS environments (
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL,
    network       TEXT NOT NULL DEFAULT 'none' CHECK (network IN ('none', 'limited', 'full')),
    allowed_hosts JSONB NOT NULL DEFAULT '[]'::jsonb,
    git           JSONB NOT NULL DEFAULT '{}'::jsonb,
    mcp           JSONB NOT NULL DEFAULT '{}'::jsonb,
    image         TEXT,
    packages      JSONB NOT NULL DEFAULT '{}'::jsonb,
    metadata      JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at    TIMESTAMPTZ NOT NULL,
    updated_at    TIMESTAMPTZ NOT NULL,
    deleted_at    TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS environments_created ON environments (created_at, id);

CREATE TABLE IF NOT EXISTS sessions (
    id               TEXT PRIMARY KEY,
    agent_id         TEXT NOT NULL REFERENCES agents (id),
    environment_id   TEXT NOT NULL REFERENCES environments (id),
    title            TEXT,
    status           TEXT NOT NULL DEFAULT 'idle'
        CHECK (status IN ('idle', 'running', 'waiting_for_worker', 'interrupted', 'error', 'archived')),
    status_reason    TEXT,
    last_seq         BIGINT NOT NULL DEFAULT 0,
    metadata         JSONB NOT NULL DEFAULT '{}'::jsonb,
    agent_snapshot   JSONB NOT NULL DEFAULT '{}'::jsonb,
    gizzi_session_id TEXT,
    created_at       TIMESTAMPTZ NOT NULL,
    updated_at       TIMESTAMPTZ NOT NULL,
    archived_at      TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS sessions_created ON sessions (created_at, id);
CREATE INDEX IF NOT EXISTS sessions_agent ON sessions (agent_id);
CREATE INDEX IF NOT EXISTS sessions_environment ON sessions (environment_id);

CREATE TABLE IF NOT EXISTS session_events (
    id         TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions (id),
    seq        BIGINT NOT NULL,
    type       TEXT NOT NULL CHECK (type IN ('message', 'tool_call', 'tool_result', 'status', 'error')),
    data       JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    UNIQUE (session_id, seq)
);

CREATE TABLE IF NOT EXISTS work_items (
    id                  TEXT PRIMARY KEY,
    environment_id      TEXT NOT NULL REFERENCES environments (id),
    session_id          TEXT NOT NULL REFERENCES sessions (id),
    status              TEXT NOT NULL DEFAULT 'ready'
        CHECK (status IN ('ready', 'leased', 'done', 'failed', 'cancelled')),
    lease_generation    BIGINT NOT NULL DEFAULT 0,
    lease_expires_at    TIMESTAMPTZ,
    worker_id           TEXT,
    session_secret_hash TEXT,
    error_message       TEXT,
    created_at          TIMESTAMPTZ NOT NULL,
    updated_at          TIMESTAMPTZ NOT NULL,
    completed_at        TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS work_items_claim ON work_items (environment_id, status, created_at, id);
CREATE INDEX IF NOT EXISTS work_items_session ON work_items (session_id, status);
CREATE UNIQUE INDEX IF NOT EXISTS work_items_secret ON work_items (session_secret_hash)
    WHERE session_secret_hash IS NOT NULL;

CREATE TABLE IF NOT EXISTS tool_calls (
    id                 TEXT PRIMARY KEY,
    session_id         TEXT NOT NULL REFERENCES sessions (id),
    work_id            TEXT,
    claimed_generation BIGINT,
    name               TEXT NOT NULL,
    input              JSONB NOT NULL,
    status             TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'claimed', 'done')),
    output             TEXT CHECK (output IS NULL OR octet_length(output) <= 102400),
    is_error           BOOLEAN NOT NULL DEFAULT FALSE,
    created_at         TIMESTAMPTZ NOT NULL,
    updated_at         TIMESTAMPTZ NOT NULL,
    completed_at       TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS tool_calls_session ON tool_calls (session_id, status, created_at);

CREATE TABLE IF NOT EXISTS workers (
    environment_id  TEXT NOT NULL,
    worker_id       TEXT NOT NULL,
    last_seen_at    TIMESTAMPTZ NOT NULL,
    current_work_id TEXT,
    hostname        TEXT,
    version         TEXT,
    PRIMARY KEY (environment_id, worker_id)
);
CREATE INDEX IF NOT EXISTS workers_seen ON workers (environment_id, last_seen_at);

CREATE TABLE IF NOT EXISTS api_keys (
    id           TEXT PRIMARY KEY,
    name         TEXT NOT NULL,
    role         TEXT NOT NULL CHECK (role IN ('admin', 'developer', 'viewer')),
    prefix       TEXT NOT NULL,
    key_hash     TEXT NOT NULL UNIQUE,
    created_at   TIMESTAMPTZ NOT NULL,
    expires_at   TIMESTAMPTZ,
    last_used_at TIMESTAMPTZ,
    revoked_at   TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS api_keys_created ON api_keys (created_at, id);

CREATE TABLE IF NOT EXISTS environment_keys (
    id             TEXT PRIMARY KEY,
    environment_id TEXT NOT NULL REFERENCES environments (id),
    name           TEXT,
    prefix         TEXT NOT NULL,
    key_hash       TEXT NOT NULL UNIQUE,
    created_at     TIMESTAMPTZ NOT NULL,
    expires_at     TIMESTAMPTZ,
    last_used_at   TIMESTAMPTZ,
    revoked_at     TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS environment_keys_env ON environment_keys (environment_id, created_at, id);

CREATE TABLE IF NOT EXISTS credentials (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL UNIQUE,
    sealed     BYTEA NOT NULL,
    key_id     TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);
