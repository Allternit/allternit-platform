-- 117_managed_connector_auth.sql
--
-- Managed connector auth (docs/MANAGED_AUTH_MAP.md, 2026-11-03).
-- Renumber at merge if main has moved past 116.
--
-- managed_auth_configs : per Clerk organization and connector, the vendor
--                        authorization server that trusts the org's Allternit
--                        ID-JAG issuer (routes/managed_auth.rs). The vendor
--                        client secret is sealed with the credential cipher
--                        (ALLTERNIT_CREDENTIALS_KEY); it is never stored in
--                        plaintext and never returned to a browser.
-- idjag_issuance_log   : one row per ID-JAG signed (who, which org, which vendor
--                        AS and resource). No token is stored.
-- mcp_trusted_issuers  : outside issuers (a Claude org, Okta) an org admin trusts
--                        on the MCP edge, and for which edge servers (`targets`:
--                        'connectors' and/or 'agents'; bots stay per-user). An
--                        active row is also the org's consent for client
--                        `issuer:<issuer>` (routes/edge_auth.rs). Removing sets
--                        revoked_at.
-- edge_jti_seen        : `<iss>|<jti>` of every accepted ID-JAG, kept until it
--                        expires, so an assertion works once.
--
-- Idempotent like the rest of migrations_pg: init_db applies it at startup.

CREATE TABLE IF NOT EXISTS public.managed_auth_configs (
    id                   TEXT PRIMARY KEY,
    org_id               TEXT NOT NULL,
    connector_id         TEXT NOT NULL,
    as_issuer            TEXT NOT NULL,
    token_endpoint       TEXT NOT NULL,
    resource             TEXT NOT NULL,
    client_id            TEXT NOT NULL,
    client_secret_sealed TEXT,
    roles                TEXT[] NOT NULL DEFAULT '{}',
    scopes               TEXT[] NOT NULL DEFAULT '{}',
    enabled              BOOLEAN NOT NULL DEFAULT FALSE,
    created_by           TEXT NOT NULL,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, connector_id)
);
CREATE INDEX IF NOT EXISTS idx_managed_auth_configs_issuer ON public.managed_auth_configs (org_id, as_issuer) WHERE enabled;
CREATE INDEX IF NOT EXISTS idx_managed_auth_configs_connector ON public.managed_auth_configs (connector_id) WHERE enabled;

CREATE TABLE IF NOT EXISTS public.idjag_issuance_log (
    jti       TEXT PRIMARY KEY,
    org_id    TEXT NOT NULL,
    user_id   TEXT NOT NULL,
    aud       TEXT NOT NULL,
    resource  TEXT NOT NULL,
    issued_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_idjag_issuance_log_org ON public.idjag_issuance_log (org_id, issued_at DESC);

CREATE TABLE IF NOT EXISTS public.mcp_trusted_issuers (
    id         TEXT PRIMARY KEY,
    org_id     TEXT NOT NULL,
    issuer     TEXT NOT NULL,
    jwks_uri   TEXT,
    kind       TEXT NOT NULL CHECK (kind IN ('claude', 'okta', 'other')),
    label      TEXT NOT NULL DEFAULT '',
    targets    TEXT[] NOT NULL DEFAULT '{}',
    created_by TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at TIMESTAMPTZ
);
CREATE UNIQUE INDEX IF NOT EXISTS mcp_trusted_issuers_live ON public.mcp_trusted_issuers (org_id, issuer) WHERE revoked_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_mcp_trusted_issuers_issuer ON public.mcp_trusted_issuers (issuer) WHERE revoked_at IS NULL;

CREATE TABLE IF NOT EXISTS public.edge_jti_seen (
    jti        TEXT PRIMARY KEY,
    expires_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_edge_jti_seen_expires ON public.edge_jti_seen (expires_at);
