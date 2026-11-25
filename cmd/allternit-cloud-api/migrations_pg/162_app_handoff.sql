-- Apps + MCP 2026, plan v3 lane 32b (D15): app-origin sessions.
--
-- An app runs on its own origin (<slug>.allternitusercontent.com) and never
-- sees the Clerk session. The signed-in workspace mints a one-time handoff
-- (single use, 60 s) for one user + one app; the app origin trades it for a
-- short app session (1 h) that the page runtime accepts for that one
-- artifact only. Only SHA-256 hashes of the tokens are stored.
--
-- Additive only: two new tables, no change to existing ones.

CREATE TABLE IF NOT EXISTS app_handoffs (
  token_hash   TEXT PRIMARY KEY,               -- sha256 hex of the 'aph_' token
  user_id      TEXT NOT NULL,
  -- The minting caller's profile, copied to the session so the runtime's
  -- access checks (people shares by email, org rules) see the same person.
  email        TEXT,
  name         TEXT,
  org_id       TEXT,
  org_role     TEXT,
  artifact_id  TEXT NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
  app_slug     TEXT NOT NULL,
  expires_at   TIMESTAMPTZ NOT NULL,
  used_at      TIMESTAMPTZ,
  created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS app_handoffs_user_idx ON app_handoffs (user_id, expires_at);

CREATE TABLE IF NOT EXISTS app_sessions (
  token_hash   TEXT PRIMARY KEY,               -- sha256 hex of the 'aps_' token
  user_id      TEXT NOT NULL,
  email        TEXT,
  name         TEXT,
  org_id       TEXT,
  org_role     TEXT,
  artifact_id  TEXT NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
  app_slug     TEXT NOT NULL,
  expires_at   TIMESTAMPTZ NOT NULL,
  created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS app_sessions_user_idx ON app_sessions (user_id, expires_at);
