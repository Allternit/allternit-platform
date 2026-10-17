-- Failure evidence from the browser lanes (ui_bridge): one row per failed step, written by the adapter
-- when a selector or the composer fails. The screenshot lives under ~/.allternit/subscriptions/evidence/
-- (kept 7 days); selector_key, url and the redacted DOM excerpt are stored here.
-- TaskError.evidence_ref carries the row id; GET /v1/evidence/:id reads it.
CREATE TABLE attempt_evidence (
  id TEXT PRIMARY KEY,
  task_id TEXT,
  adapter_id TEXT,
  selector_key TEXT,
  url TEXT,
  dom_excerpt TEXT,
  screenshot_file TEXT,
  detail TEXT,
  created_at TEXT NOT NULL
);
CREATE INDEX attempt_evidence_created ON attempt_evidence (created_at);
CREATE INDEX attempt_evidence_adapter ON attempt_evidence (adapter_id, created_at);
