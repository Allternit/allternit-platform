-- Allternit Sleep, migration A: the contract tables (2026-11-19).
--
-- Additive only. New tables are CREATE ... IF NOT EXISTS. The new memory_facts and
-- memory_recall_logs columns are plain nullable ADD COLUMNs, like V209 and V289:
-- refinery runs each version once per database, and no earlier migration or runtime
-- code adds these names. Every stage that writes these tables is behind its own
-- sleep_config flag, all off by default (cmd/allternit-api/src/sleep/config.rs).
-- Track 1 (adapter) tables are not here; they come in migration B only if gate G3
-- passes. API reference: surfaces/docs/api/memory-sleep.mdx.

-- Settings and thresholds. One row per (user_id, key); user_id '' is the global row,
-- a user's own row wins over it, and a key with no row uses the code default.
-- source: default | fitted (written by the fitter) | owner (set by the user).
CREATE TABLE IF NOT EXISTS sleep_config (
    user_id    TEXT NOT NULL DEFAULT '',
    key        TEXT NOT NULL,
    value_json TEXT NOT NULL,
    source     TEXT NOT NULL DEFAULT 'default' CHECK (source IN ('default', 'fitted', 'owner')),
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (user_id, key)
);
CREATE INDEX IF NOT EXISTS idx_sleep_config_user ON sleep_config(user_id);

-- One Sleep cycle (daydream, nap, deep, rem) and its report.
-- trigger: idle | pre_compact | session_end | manual | clock
-- state:   running | finished | stopped | failed | undone
CREATE TABLE IF NOT EXISTS sleep_cycles (
    id              TEXT PRIMARY KEY,
    user_id         TEXT NOT NULL,
    session_id      TEXT,
    trigger         TEXT NOT NULL,
    kind            TEXT NOT NULL DEFAULT 'nap',
    state           TEXT NOT NULL DEFAULT 'running',
    stage           TEXT,
    manifest_before TEXT,
    manifest_after  TEXT,
    spend_usd       REAL NOT NULL DEFAULT 0,
    report_json     TEXT,
    error           TEXT,
    started_at      DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    finished_at     DATETIME,
    undone_at       DATETIME
);
CREATE INDEX IF NOT EXISTS idx_sleep_cycles_user ON sleep_cycles(user_id, started_at DESC);

-- The unit queue: small jobs with a lease and up to N attempts (Dream's pattern).
-- state: queued | leased | done | failed | cancelled
CREATE TABLE IF NOT EXISTS sleep_units (
    id               TEXT PRIMARY KEY,
    user_id          TEXT NOT NULL,
    cycle_id         TEXT,
    stage            TEXT NOT NULL,
    payload_json     TEXT,
    state            TEXT NOT NULL DEFAULT 'queued',
    attempts         INTEGER NOT NULL DEFAULT 0,
    lease_owner      TEXT,
    lease_expires_at DATETIME,
    last_error       TEXT,
    spend_usd        REAL NOT NULL DEFAULT 0,
    created_at       DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at       DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    finished_at      DATETIME
);
CREATE INDEX IF NOT EXISTS idx_sleep_units_user ON sleep_units(user_id, state);
CREATE INDEX IF NOT EXISTS idx_sleep_units_lease ON sleep_units(state, lease_expires_at);
CREATE INDEX IF NOT EXISTS idx_sleep_units_cycle ON sleep_units(cycle_id);

-- Topics the router knows. entity_id links a topic seeded from an entity page.
-- state: active | weak | gist | untethered
CREATE TABLE IF NOT EXISTS memory_topics (
    id            TEXT PRIMARY KEY,
    user_id       TEXT NOT NULL,
    name          TEXT NOT NULL,
    entity_id     TEXT,
    state         TEXT NOT NULL DEFAULT 'active',
    router_weight REAL,
    score         REAL,
    created_at    DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at    DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_memory_topics_user ON memory_topics(user_id, state);

-- Signed manifests: the set of page versions in force. Undo = the parent manifest.
CREATE TABLE IF NOT EXISTS memory_manifests (
    id         TEXT PRIMARY KEY,
    user_id    TEXT NOT NULL,
    parent_id  TEXT,
    cycle_id   TEXT,
    body_json  TEXT NOT NULL DEFAULT '[]',
    key_id     TEXT,
    signature  TEXT,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_memory_manifests_user ON memory_manifests(user_id, created_at DESC);

-- Page versions. The text lives content-addressed at <data dir>/sleep/pages/<sha256>.md.
-- provenance_tier is shown to the model on each page header.
CREATE TABLE IF NOT EXISTS memory_pages (
    id              TEXT PRIMARY KEY,
    user_id         TEXT NOT NULL,
    topic_id        TEXT NOT NULL,
    version         INTEGER NOT NULL,
    sha256          TEXT NOT NULL,
    manifest_id     TEXT,
    provenance_tier TEXT,
    tokens          INTEGER,
    score           REAL,
    created_at      DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (topic_id, version)
);
CREATE INDEX IF NOT EXISTS idx_memory_pages_user ON memory_pages(user_id, topic_id);
CREATE INDEX IF NOT EXISTS idx_memory_pages_sha ON memory_pages(sha256);

-- Atomic dated claims. A claim with no back-pointer is refused (D12).
CREATE TABLE IF NOT EXISTS memory_page_claims (
    id             TEXT PRIMARY KEY,
    user_id        TEXT NOT NULL,
    topic_id       TEXT NOT NULL,
    page_id        TEXT,
    claim          TEXT NOT NULL,
    source_type    TEXT NOT NULL CHECK (source_type IN ('user_said', 'tool_observed', 'model_inferred', 'dream_generated', 'imported')),
    observation_id TEXT,
    fact_id        TEXT,
    visibility     TEXT,
    as_of          DATETIME NOT NULL,
    superseded_by  TEXT,
    created_at     DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CHECK (observation_id IS NOT NULL OR fact_id IS NOT NULL)
);
CREATE INDEX IF NOT EXISTS idx_memory_page_claims_user ON memory_page_claims(user_id, topic_id);
CREATE INDEX IF NOT EXISTS idx_memory_page_claims_fact ON memory_page_claims(fact_id);
CREATE INDEX IF NOT EXISTS idx_memory_page_claims_obs ON memory_page_claims(observation_id);

-- Contradictions kept as records (D10). side_*_kind: fact | claim.
-- pick:  a | b | both_scoped | neither (null while undecided)
-- state: open | needs_user | resolved
CREATE TABLE IF NOT EXISTS memory_disputes (
    id          TEXT PRIMARY KEY,
    user_id     TEXT NOT NULL,
    topic_id    TEXT,
    side_a_kind TEXT NOT NULL DEFAULT 'fact',
    side_a_id   TEXT NOT NULL,
    side_b_kind TEXT NOT NULL DEFAULT 'fact',
    side_b_id   TEXT NOT NULL,
    scope       TEXT,
    stakes      REAL,
    margin      REAL,
    pick        TEXT CHECK (pick IS NULL OR pick IN ('a', 'b', 'both_scoped', 'neither')),
    reason      TEXT,
    picked_by   TEXT,
    state       TEXT NOT NULL DEFAULT 'open' CHECK (state IN ('open', 'needs_user', 'resolved')),
    created_at  DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at  DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    resolved_at DATETIME
);
CREATE INDEX IF NOT EXISTS idx_memory_disputes_user ON memory_disputes(user_id, state);

-- Dream and REM output waits here; kept only if its metric improves (D11).
-- state: pending | kept | rejected
CREATE TABLE IF NOT EXISTS sleep_hypotheses (
    id           TEXT PRIMARY KEY,
    user_id      TEXT NOT NULL,
    cycle_id     TEXT,
    kind         TEXT NOT NULL,
    content      TEXT NOT NULL,
    refs_json    TEXT,
    metric       TEXT,
    score_before REAL,
    score_after  REAL,
    state        TEXT NOT NULL DEFAULT 'pending',
    created_at   DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    decided_at   DATETIME
);
CREATE INDEX IF NOT EXISTS idx_sleep_hypotheses_user ON sleep_hypotheses(user_id, state);

-- A user correction of a paged claim: shown at once, applied at the next nap.
CREATE TABLE IF NOT EXISTS memory_overlays (
    id               TEXT PRIMARY KEY,
    user_id          TEXT NOT NULL,
    topic_id         TEXT,
    claim_id         TEXT,
    correction       TEXT NOT NULL,
    observation_id   TEXT,
    as_of            DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    applied_cycle_id TEXT,
    applied_at       DATETIME,
    created_at       DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_memory_overlays_user ON memory_overlays(user_id, applied_at);

-- Prospective memory. cue_kind: event | time. state: open | fired | expired | cancelled
CREATE TABLE IF NOT EXISTS memory_intentions (
    id                    TEXT PRIMARY KEY,
    user_id               TEXT NOT NULL,
    content               TEXT NOT NULL,
    cue_kind              TEXT NOT NULL DEFAULT 'event',
    cue                   TEXT,
    due_at                DATETIME,
    expires_at            DATETIME,
    state                 TEXT NOT NULL DEFAULT 'open',
    source_observation_id TEXT,
    fired_at              DATETIME,
    created_at            DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_memory_intentions_user ON memory_intentions(user_id, state);

-- Verbatim values (numbers, IDs, secrets) sealed with the token_crypto keyring.
-- Pages hold `vault:<id>` refs, never the value. kind: number | id | secret
CREATE TABLE IF NOT EXISTS memory_vault (
    id                    TEXT PRIMARY KEY,
    user_id               TEXT NOT NULL,
    kind                  TEXT NOT NULL,
    label                 TEXT,
    sealed                TEXT NOT NULL,
    source_observation_id TEXT,
    created_at            DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_memory_vault_user ON memory_vault(user_id);

-- Spaced checks on old memories and the bounded check set.
-- target_kind: fact | topic | router | skill
-- check_kind:  spaced | neighbour | router_sample | skill_probe
-- result:      pass | weak | gist | untethered | fail (null until run)
CREATE TABLE IF NOT EXISTS sleep_checks (
    id          TEXT PRIMARY KEY,
    user_id     TEXT NOT NULL,
    cycle_id    TEXT,
    target_kind TEXT NOT NULL,
    target_id   TEXT NOT NULL,
    check_kind  TEXT NOT NULL,
    due_at      DATETIME,
    result      TEXT,
    score       REAL,
    created_at  DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    checked_at  DATETIME
);
CREATE INDEX IF NOT EXISTS idx_sleep_checks_user ON sleep_checks(user_id, due_at);

-- A page the model pulled that the router did not preload (trains the router).
CREATE TABLE IF NOT EXISTS sleep_router_misses (
    id                 TEXT PRIMARY KEY,
    user_id            TEXT NOT NULL,
    session_id         TEXT,
    topic_id           TEXT NOT NULL,
    model              TEXT,
    query              TEXT,
    preloaded_json     TEXT,
    created_at         DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_sleep_router_misses_user ON sleep_router_misses(user_id, created_at DESC);

-- Two strengths, tether state, provenance and insight per fact (all nullable; null =
-- not yet scored, the old behavior). tether: tethered | gist | untethered
ALTER TABLE memory_facts ADD COLUMN storage_strength REAL;
ALTER TABLE memory_facts ADD COLUMN retrieval_strength REAL;
ALTER TABLE memory_facts ADD COLUMN tether TEXT;
ALTER TABLE memory_facts ADD COLUMN last_reconsolidated_at DATETIME;
ALTER TABLE memory_facts ADD COLUMN source_type TEXT CHECK (source_type IS NULL OR source_type IN ('user_said', 'tool_observed', 'model_inferred', 'dream_generated', 'imported'));
ALTER TABLE memory_facts ADD COLUMN insight_score REAL;
-- Write-time scores (step 3.3, no model call). attention: how much this deserves keeping
-- (remember markers, self-disclosure, repetition, use, surprise, stakes, goal link).
-- credence: how likely it is true, set by source, never by emphasis. flashbulb: 1 = the
-- user flagged it ("remember this"); it raises priority only, never credence.
ALTER TABLE memory_facts ADD COLUMN attention REAL;
ALTER TABLE memory_facts ADD COLUMN credence REAL;
ALTER TABLE memory_facts ADD COLUMN flashbulb INTEGER;

-- Answer-used labels on the incumbent recall (step 3.1).
-- label_used: JSON array of the result ids the reply used. label_source: answer | user | shadow
ALTER TABLE memory_recall_logs ADD COLUMN label_used TEXT;
ALTER TABLE memory_recall_logs ADD COLUMN label_source TEXT;
ALTER TABLE memory_recall_logs ADD COLUMN labeled_at DATETIME;
