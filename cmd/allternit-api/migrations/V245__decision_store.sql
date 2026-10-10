-- Decision Runtime store (POST /v1/decisions, agency_api::decisions).
-- One row per typed decision, in open-systemone's record shape: what was
-- asked (context hash, options), what answered (choice, backend, latency,
-- every escalation attempt) and, later via PATCH, the outcome. The context
-- itself is never stored, only its sha256.
CREATE TABLE IF NOT EXISTS decisions (
    id TEXT PRIMARY KEY,
    owner TEXT NOT NULL,
    created_at TEXT NOT NULL,
    kind TEXT NOT NULL,
    session_id TEXT,
    task TEXT,
    context_hash TEXT NOT NULL,
    has_image INTEGER NOT NULL DEFAULT 0,
    options_json TEXT NOT NULL,
    choice TEXT,
    probs_json TEXT NOT NULL,
    confidence REAL NOT NULL,
    abstained INTEGER NOT NULL DEFAULT 0,
    backend TEXT NOT NULL,
    latency_ms REAL NOT NULL,
    attempts_json TEXT NOT NULL,
    outcome_status TEXT,
    outcome_label TEXT,
    outcome_detail TEXT,
    outcome_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_decisions_owner_created ON decisions(owner, created_at);
CREATE INDEX IF NOT EXISTS idx_decisions_kind_created ON decisions(kind, created_at);
