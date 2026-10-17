-- Lane health canaries: one row per free, non-spending probe the gateway ran against a connected account
-- (every 3 min while no task holds it). GET /v1/lanes reads the latest row and the last passing row.
CREATE TABLE canary_runs (
  id TEXT PRIMARY KEY,
  account_id TEXT NOT NULL,
  adapter_id TEXT NOT NULL,
  kind TEXT NOT NULL DEFAULT 'probe',
  started_at TEXT NOT NULL,
  ended_at TEXT,
  outcome TEXT NOT NULL,
  health TEXT NOT NULL,
  latency_ms INTEGER,
  error TEXT,
  evidence_ref TEXT
);
CREATE INDEX canary_runs_account ON canary_runs (account_id, started_at);
