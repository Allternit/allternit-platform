-- 161_apps_mcp_2026.sql
--
-- Apps + MCP 2026 (plan v3 lane 7, the App contract): the app package on an
-- artifact, vendor app records, installs, the share-link registry and edge
-- task state. Contract: handoffs/artifact-apps-2026-11-20/CONTRACT-apps-mcp-2026.md;
-- routes: src/routes/artifacts_v2.rs (PUT /api/v2/artifacts/:id/app,
-- GET /api/v2/public/apps/:slug) and src/routes/share_links.rs
-- (/api/v1/share-links). Migration 109 (library archive) is not reused.
--
-- * artifacts (+ app_*)  an artifact made into an app: name, icon, slug, pinned
--                        version, capability snapshot, permission tier
-- * vendor_apps          an outside MCP server's ui:// app, kept as a record
--                        (never copied into artifacts; tier capped at 'app')
-- * app_installs         one row per user + app: what they granted and pinned
-- * share_links          hosted and live links (artifact, port or MCP server)
-- * mcp_tasks            MCP Tasks state at the edge (D28; the proxy is stateless)
--
-- Additive only, idempotent (IF NOT EXISTS). Prod runs with
-- ALLTERNIT_SKIP_MIGRATIONS=1: deploy-contabo.sh applies this file through
-- the ops_applied_migrations ledger before the binary swap.

-- ---------------------------------------------------------------------------
-- The app package on an artifact. All NULL until the owner makes it an app.
-- ---------------------------------------------------------------------------
ALTER TABLE artifacts ADD COLUMN IF NOT EXISTS app_name TEXT;
ALTER TABLE artifacts ADD COLUMN IF NOT EXISTS app_icon TEXT;              -- one generic word, like icon
ALTER TABLE artifacts ADD COLUMN IF NOT EXISTS app_slug TEXT;              -- random, 16 chars [a-z0-9], minted once, never changes
ALTER TABLE artifacts ADD COLUMN IF NOT EXISTS app_pinned_version INTEGER; -- the version installs run; NULL = not an app
ALTER TABLE artifacts ADD COLUMN IF NOT EXISTS app_capability_snapshot JSONB; -- capabilities at the time it was made/re-published as an app
ALTER TABLE artifacts ADD COLUMN IF NOT EXISTS app_permission_tier TEXT
  CHECK (app_permission_tier IS NULL OR app_permission_tier IN ('browser', 'app', 'full'));
ALTER TABLE artifacts ADD COLUMN IF NOT EXISTS app_install_scope TEXT     -- who may install: same words as visibility
  CHECK (app_install_scope IS NULL OR app_install_scope IN ('private', 'people', 'org', 'link'));
ALTER TABLE artifacts ADD COLUMN IF NOT EXISTS app_tools JSONB;            -- the app's own tools (MCP tool definitions)
ALTER TABLE artifacts ADD COLUMN IF NOT EXISTS app_skill_ref TEXT
  CHECK (app_skill_ref IS NULL OR app_skill_ref LIKE 'skill://%');
CREATE UNIQUE INDEX IF NOT EXISTS artifacts_app_slug_key ON artifacts(app_slug) WHERE app_slug IS NOT NULL;

-- ---------------------------------------------------------------------------
-- Vendor apps: a connected MCP server's ui:// resource. Owned by a user or an
-- org. Tier is never 'full' (D27: we cannot vouch for vendor code).
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS vendor_apps (
  id              TEXT PRIMARY KEY,              -- 'vap_' + hex
  owner_user_id   TEXT,                          -- Clerk user id (personal record)
  owner_org_id    TEXT,                          -- Clerk org id (org record)
  connector_id    TEXT NOT NULL,                 -- the user's/org's connector for this server
  server_url      TEXT NOT NULL,
  ui_resource     TEXT NOT NULL CHECK (ui_resource LIKE 'ui://%'),
  name            TEXT NOT NULL,
  icon            TEXT,
  tool_allowlist  JSONB NOT NULL DEFAULT '[]',   -- tool names the app may call
  tier            TEXT NOT NULL DEFAULT 'browser' CHECK (tier IN ('browser', 'app')),
  created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
  CHECK (owner_user_id IS NOT NULL OR owner_org_id IS NOT NULL)
);
CREATE UNIQUE INDEX IF NOT EXISTS vendor_apps_owner_resource_key
  ON vendor_apps(COALESCE(owner_org_id, ''), COALESCE(owner_user_id, ''), connector_id, ui_resource);
CREATE INDEX IF NOT EXISTS vendor_apps_user_idx ON vendor_apps(owner_user_id) WHERE owner_user_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS vendor_apps_org_idx ON vendor_apps(owner_org_id) WHERE owner_org_id IS NOT NULL;

-- ---------------------------------------------------------------------------
-- Installs: one row per user + app. The consent rule (lane 8) compares the
-- grant here with the app's tier and capability snapshot; it asks again only
-- when the tier rises or the snapshot adds a capability.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS app_installs (
  id                    TEXT PRIMARY KEY,        -- 'ain_' + hex
  user_id               TEXT NOT NULL,
  org_id                TEXT,                    -- the installer's org at install time
  app_kind              TEXT NOT NULL CHECK (app_kind IN ('artifact', 'vendor')),
  app_ref               TEXT NOT NULL,           -- artifacts.id or vendor_apps.id
  pinned_version        INTEGER,                 -- artifact version installed; NULL for vendor apps
  granted_tier          TEXT NOT NULL CHECK (granted_tier IN ('browser', 'app', 'full')),
  granted_capabilities  JSONB NOT NULL DEFAULT '{}',
  pinned                BOOLEAN NOT NULL DEFAULT false, -- on the shelf
  created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (user_id, app_kind, app_ref)
);
CREATE INDEX IF NOT EXISTS app_installs_app_idx ON app_installs(app_kind, app_ref);

-- ---------------------------------------------------------------------------
-- Share links (plan v2 step 9 + app_kind). hosted = served by the app host
-- Worker; live = through the relay to the owner's computer. Caps, kill and
-- the name rule come in lane 8 (share_links.rs enforce_caps / check_kill).
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS share_links (
  id                 TEXT PRIMARY KEY,           -- 'shl_' + hex
  user_id            TEXT NOT NULL,              -- owner (Clerk user id)
  org_id             TEXT,                       -- owner's org at creation
  kind               TEXT NOT NULL CHECK (kind IN ('hosted', 'live')),
  subject            TEXT NOT NULL CHECK (subject IN ('artifact', 'port', 'mcp')),
  artifact_id        TEXT REFERENCES artifacts(id) ON DELETE SET NULL,
  runtime_device_id  TEXT,                       -- live links: the owner's computer
  label              TEXT,
  subdomain          TEXT NOT NULL UNIQUE,       -- DNS label, 3-63 chars [a-z0-9-]
  target_port        INTEGER CHECK (target_port IS NULL OR target_port BETWEEN 1 AND 65535),
  auth_mode          TEXT NOT NULL DEFAULT 'public' CHECK (auth_mode IN ('public', 'signed_in', 'org')),
  status             TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'paused', 'revoked', 'killed')),
  abuse_flag         BOOLEAN NOT NULL DEFAULT false,
  abuse_note         TEXT,
  bytes_month        BIGINT NOT NULL DEFAULT 0,  -- fed by the usage route (lane 29)
  app_kind           TEXT CHECK (app_kind IS NULL OR app_kind IN ('artifact', 'vendor')),
  vendor_app_id      TEXT REFERENCES vendor_apps(id) ON DELETE SET NULL,
  created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
  last_seen_at       TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS share_links_user_created_idx ON share_links(user_id, created_at DESC);
CREATE INDEX IF NOT EXISTS share_links_artifact_idx ON share_links(artifact_id) WHERE artifact_id IS NOT NULL;

-- ---------------------------------------------------------------------------
-- MCP Tasks at the edge (D28). Status words follow the MCP Tasks spec.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS mcp_tasks (
  id                TEXT PRIMARY KEY,
  owner_id          TEXT NOT NULL,               -- the user the edge session acts for
  tool              TEXT NOT NULL,
  status            TEXT NOT NULL DEFAULT 'working'
                    CHECK (status IN ('working', 'input_required', 'completed', 'cancelled', 'failed')),
  input_requests    JSONB NOT NULL DEFAULT '[]', -- pending elicitation asks
  result            JSONB,
  error             TEXT,
  poll_interval_ms  INTEGER NOT NULL DEFAULT 1000,
  ttl_ms            BIGINT,                      -- MCP `ttl`; NULL = no expiry
  created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS mcp_tasks_owner_updated_idx ON mcp_tasks(owner_id, updated_at DESC);
