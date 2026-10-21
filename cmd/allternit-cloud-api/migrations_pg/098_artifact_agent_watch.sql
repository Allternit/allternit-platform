-- 098_artifact_agent_watch.sql
--
-- Artifacts v2: "Watch your agents work" (SPEC-agent-watch-2026-10-20).
--
-- * artifact_presence     gains agent rows: kind = 'agent', the agent's
--                         session, task line, cursor anchor and running cost.
--                         Agent rows use user_id 'agent:<session|none>:<slug>'.
-- * artifact_comments     gains agent_name: a comment an agent posts is stored
--                         with author_id 'assistant' and the agent's name.
-- * artifact_suggestions  pending edits an agent proposes. The client applies
--                         an accepted change and saves a version; the server
--                         only stores and resolves them (never rewrites bodies).
--                         Routes: src/routes/artifacts_suggestions.rs.
--
-- Idempotent (IF NOT EXISTS). Apply by hand to prod (ALLTERNIT_SKIP_MIGRATIONS=1).

ALTER TABLE artifact_presence
  ADD COLUMN IF NOT EXISTS kind TEXT NOT NULL DEFAULT 'user',   -- 'user' | 'agent'
  ADD COLUMN IF NOT EXISTS session_id TEXT,
  ADD COLUMN IF NOT EXISTS task TEXT,
  ADD COLUMN IF NOT EXISTS anchor JSONB,
  ADD COLUMN IF NOT EXISTS cost_usd NUMERIC(12,4);

ALTER TABLE artifact_comments ADD COLUMN IF NOT EXISTS agent_name TEXT;

CREATE TABLE IF NOT EXISTS artifact_suggestions (
  id            TEXT PRIMARY KEY,                -- 'sug_' + ULID
  artifact_id   TEXT NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
  base_version  INTEGER NOT NULL,
  author_id     TEXT NOT NULL,                   -- the caller (user whose agent made it)
  agent_name    TEXT NOT NULL,
  session_id    TEXT,
  change        JSONB NOT NULL,                  -- {kind:'text'|'block'|'cell', ...}
  reason        TEXT,
  cost_usd      NUMERIC(12,4),
  status        TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','accepted','rejected','stale')),
  resolved_by   TEXT,
  resolved_at   TIMESTAMPTZ,
  created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS artifact_suggestions_open ON artifact_suggestions (artifact_id, status, created_at);
