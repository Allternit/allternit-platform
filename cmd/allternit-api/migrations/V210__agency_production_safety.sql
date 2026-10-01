-- V210: Agency kernel production safety (WP-P1, W6 / research memo A).
--
-- Three tables, all owned by `agency_api::safety`:
--
-- * agency_run_fences: one fencing token (epoch) per run. Every drive of a run
--   bumps the epoch; a worker holding an older epoch is stale and can neither
--   prepare nor commit an effect (two-phase fence).
-- * agency_effect_journal: prepare -> commit journal of every side-effecting
--   step and every journaled model output, keyed by a stable idempotency key
--   (`<run>:<node>:<tool>:<n>`). A resumed or replayed run serves committed
--   rows instead of re-applying the effect or re-calling the model.
-- * agency_org_safety: per-org policy (non-requester approval, run rate,
--   per-run caps). NULL = server default; values only ever tighten the
--   server ceilings, never raise them.

CREATE TABLE IF NOT EXISTS agency_run_fences (
    run_id      TEXT PRIMARY KEY,
    epoch       INTEGER NOT NULL,
    holder      TEXT NOT NULL,
    acquired_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS agency_effect_journal (
    idempotency_key TEXT PRIMARY KEY,
    run_id          TEXT NOT NULL,
    node_id         TEXT,
    tool            TEXT NOT NULL,
    args_hash       TEXT NOT NULL,
    epoch           INTEGER NOT NULL,
    chain_key       TEXT,
    status          TEXT NOT NULL CHECK (status IN ('prepared', 'committed', 'failed')),
    retry_safe      INTEGER NOT NULL DEFAULT 1,
    result          TEXT,
    error_class     TEXT CHECK (error_class IS NULL OR error_class IN ('retryable', 'non_retryable')),
    error           TEXT,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_agency_effect_journal_run ON agency_effect_journal(run_id);

CREATE TABLE IF NOT EXISTS agency_org_safety (
    org_id                         TEXT PRIMARY KEY,
    require_non_requester_approval INTEGER,
    runs_per_hour                  INTEGER,
    max_steps                      INTEGER,
    max_wall_secs                  INTEGER,
    max_usd                        REAL,
    updated_by                     TEXT,
    updated_at                     TEXT
);
