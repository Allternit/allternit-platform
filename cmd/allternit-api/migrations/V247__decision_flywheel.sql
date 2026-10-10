-- Decision Runtime flywheel (agency_api::decisions::flywheel, phase E5).
--
-- decision_heads: one row per (head, kind) with its state: shadow (runs on
-- live decisions off the latency path, never served), live (tried first in
-- the chain), retired (manual off). state_since bounds the metrics window, so
-- a head re-earns its bar after every transition (hysteresis).
CREATE TABLE IF NOT EXISTS decision_heads (
    head TEXT NOT NULL,
    kind TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'shadow',
    state_since TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (head, kind)
);

-- Every transition, automatic or manual, with the metrics that caused it.
CREATE TABLE IF NOT EXISTS decision_head_audit (
    id TEXT PRIMARY KEY,
    head TEXT NOT NULL,
    kind TEXT NOT NULL,
    from_state TEXT NOT NULL,
    to_state TEXT NOT NULL,
    reason TEXT NOT NULL,
    actor TEXT NOT NULL,
    metrics_json TEXT NOT NULL,
    at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_decision_head_audit_head ON decision_head_audit(head, kind, at);

-- A head's answer to a decision, next to the served choice (decisions.choice)
-- and, later, the outcome (decisions.outcome_*). choice NULL = no answer.
CREATE TABLE IF NOT EXISTS decision_shadow (
    decision_id TEXT NOT NULL,
    head TEXT NOT NULL,
    kind TEXT NOT NULL,
    head_state TEXT NOT NULL,
    choice TEXT,
    confidence REAL,
    latency_ms REAL NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (decision_id, head)
);
CREATE INDEX IF NOT EXISTS idx_decision_shadow_head ON decision_shadow(head, kind, created_at);

-- The lookup head's key per decision: sha256 of kind, context hash, question
-- and the ordered option ids and texts. Never the context itself. learned
-- = the first outcome was folded into decision_lookup (later re-patches of
-- the same decision are not counted twice).
CREATE TABLE IF NOT EXISTS decision_keys (
    decision_id TEXT PRIMARY KEY,
    owner TEXT NOT NULL,
    kind TEXT NOT NULL,
    key_hash TEXT NOT NULL,
    learned INTEGER NOT NULL DEFAULT 0
);

-- What the lookup head learned from outcomes, per owner: for one key, how
-- often a choice succeeded or failed.
CREATE TABLE IF NOT EXISTS decision_lookup (
    owner TEXT NOT NULL,
    kind TEXT NOT NULL,
    key_hash TEXT NOT NULL,
    choice TEXT NOT NULL,
    successes INTEGER NOT NULL DEFAULT 0,
    failures INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (owner, kind, key_hash, choice)
);
