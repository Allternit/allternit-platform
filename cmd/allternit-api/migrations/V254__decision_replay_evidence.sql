-- Replay evidence for decision-head promotion (agency_api::decisions::acceptance, B5).
--
-- One row per (batch replay, head, kind): what a candidate head did when it
-- re-decided the labelled decisions of N recent finished runs
-- (`POST /v1/replays/batch` with `overrides.decision_heads`), next to the
-- served baseline on the same decisions. The flywheel promotes a shadow head
-- only when its latest row passes the acceptance rule (accuracy >= baseline
-- - epsilon, and cost or latency improves). Aggregates only: no context,
-- no options. Written only for batches run by a decisions admin.
CREATE TABLE IF NOT EXISTS decision_replay_evidence (
    id TEXT NOT NULL,
    head TEXT NOT NULL,
    kind TEXT NOT NULL,
    head_version TEXT NOT NULL,
    owner TEXT NOT NULL,
    created_at TEXT NOT NULL,
    runs INTEGER NOT NULL,
    decisions INTEGER NOT NULL,
    labelled INTEGER NOT NULL,
    accuracy REAL,
    baseline_accuracy REAL,
    cost_usd REAL,
    baseline_cost_usd REAL,
    latency_ms REAL,
    baseline_latency_ms REAL,
    pass_at_1 REAL,
    pass_at_k REAL,
    k INTEGER NOT NULL,
    deterministic INTEGER NOT NULL,
    metrics_json TEXT NOT NULL,
    PRIMARY KEY (id, head, kind)
);
CREATE INDEX IF NOT EXISTS idx_decision_replay_evidence_head ON decision_replay_evidence(head, kind, created_at);
