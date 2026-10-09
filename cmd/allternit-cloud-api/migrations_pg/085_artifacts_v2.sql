-- 085_artifacts_v2.sql
--
-- Artifacts v2 (Phase 1 foundation): one account-level store of typed
-- artifacts that every surface reads and writes (Desktop, ai.allternit.com,
-- the phone layout, the m.allternit.com PWA, cloud computers, gizzi).
-- Contract: docs/design/artifacts-v2.md §2; routes: src/routes/artifacts_v2.rs
-- under /api/v2/artifacts.
--
-- * artifacts            one row per artifact (kind, runtime_version, sharing)
-- * artifact_versions    immutable bodies, ≤ 16 MiB each (checked in the route)
-- * artifact_shares      user / email / group grants (view | comment | edit)
-- * artifact_storage     Phase 4 page runtime storage (schema only for now)
-- * artifact_consents    Phase 4 viewer consents (schema only for now)
-- * artifact_comments    Phase 4 comments (schema only for now)
-- * org_artifact_settings  per-Clerk-org admin switches
--
-- Idempotent (IF NOT EXISTS). Apply by hand to prod (ALLTERNIT_SKIP_MIGRATIONS=1).

CREATE TABLE IF NOT EXISTS artifacts (
  id               TEXT PRIMARY KEY,            -- 'art_' + ULID; clients may supply it (idempotent create)
  owner_id         TEXT NOT NULL,               -- Clerk user id
  org_id           TEXT,                        -- owner's Clerk org at creation (sharing follows it)
  kind             TEXT NOT NULL,
  runtime_version  INTEGER NOT NULL DEFAULT 2,
  title            TEXT NOT NULL,
  icon             TEXT,                        -- one generic word, e.g. 'chart'
  template_id      TEXT,
  origin           JSONB NOT NULL DEFAULT '{}', -- {surface, session_id, message_id, computer_id, legacy_source, legacy_id}
  capabilities     JSONB NOT NULL DEFAULT '{}', -- {storage:bool, ai:bool, connectors:[{connector, tools:[...]}]}
  current_version  INTEGER NOT NULL DEFAULT 1,
  shared_version   INTEGER,                     -- NULL = viewers always see the latest
  visibility       TEXT NOT NULL DEFAULT 'private', -- private | people | org | link
  link_level       TEXT NOT NULL DEFAULT 'view',    -- what 'anyone with the link' may do (view only in v1)
  thumbnail_url    TEXT,
  created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS artifacts_owner_updated_idx ON artifacts(owner_id, updated_at DESC);
CREATE INDEX IF NOT EXISTS artifacts_org_visibility_idx ON artifacts(org_id, visibility);

CREATE TABLE IF NOT EXISTS artifact_versions (
  artifact_id  TEXT NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
  version      INTEGER NOT NULL,
  body         TEXT NOT NULL,                   -- ≤ 16 MiB
  body_format  TEXT NOT NULL,
  meta         JSONB NOT NULL DEFAULT '{}',      -- {language, filename, note, ...}
  size_bytes   INTEGER NOT NULL,
  sha256       TEXT NOT NULL,
  author_id    TEXT NOT NULL,                    -- user id, or 'assistant'
  created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (artifact_id, version)
);

CREATE TABLE IF NOT EXISTS artifact_shares (
  artifact_id     TEXT NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
  principal_type  TEXT NOT NULL,                -- user | email | group
  principal_id    TEXT NOT NULL,                -- user id, lower-cased email, or Clerk group id
  level           TEXT NOT NULL,                -- view | comment | edit
  invited_by      TEXT NOT NULL,
  created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
  expires_at      TIMESTAMPTZ,                  -- outside email invites: +30 days until accepted
  accepted_at     TIMESTAMPTZ,
  PRIMARY KEY (artifact_id, principal_type, principal_id)
);

CREATE TABLE IF NOT EXISTS artifact_storage (       -- Phase 4 runtime; schema now
  artifact_id  TEXT NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
  scope        TEXT NOT NULL,                  -- personal | shared
  user_id      TEXT NOT NULL DEFAULT '',       -- '' for shared
  key          TEXT NOT NULL,
  value        TEXT NOT NULL,
  bytes        INTEGER NOT NULL,
  updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (artifact_id, scope, user_id, key)
);

CREATE TABLE IF NOT EXISTS artifact_consents (
  artifact_id  TEXT NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
  user_id      TEXT NOT NULL,
  capability   TEXT NOT NULL,                  -- storage_shared | ai | connectors
  granted      BOOLEAN NOT NULL,
  denied_tools JSONB NOT NULL DEFAULT '[]',
  updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (artifact_id, user_id, capability)
);

CREATE TABLE IF NOT EXISTS artifact_comments (      -- Phase 4 UI; schema now
  id           TEXT PRIMARY KEY,
  artifact_id  TEXT NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
  version      INTEGER,
  anchor       JSONB NOT NULL DEFAULT '{}',
  parent_id    TEXT,
  author_id    TEXT NOT NULL,
  body         TEXT NOT NULL,
  to_assistant BOOLEAN NOT NULL DEFAULT false,
  resolved_at  TIMESTAMPTZ,
  created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS org_artifact_settings (
  org_id             TEXT PRIMARY KEY,
  enabled            BOOLEAN NOT NULL DEFAULT true,
  templates          JSONB NOT NULL DEFAULT '{}',   -- {kind: bool}; missing = plan default
  external_sharing   BOOLEAN NOT NULL DEFAULT false,
  outside_invites    BOOLEAN NOT NULL DEFAULT false,
  presence           BOOLEAN NOT NULL DEFAULT true,
  connectors         BOOLEAN NOT NULL DEFAULT true,
  allowed_external   JSONB NOT NULL DEFAULT '[]',   -- artifact ids individually allowed outside
  updated_by         TEXT,
  updated_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);
