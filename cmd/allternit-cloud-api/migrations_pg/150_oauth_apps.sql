-- Sign in with Allternit: OAuth apps, consent requests, grants, tokens,
-- per-grant monthly spend and the OAuth audit trail
-- (CONTRACT-oauth-linked-billing-2026-11.md, Part A, section A11).
--
-- Secrets, codes and tokens are stored only as SHA-256 hashes (D1).
-- Rows that hang off an app or a grant go with it (ON DELETE CASCADE).
-- oauth_audit_log has no foreign keys on purpose: the trail outlives the app.
-- Account erase (services/account_erase.rs) lists every table here that
-- names a person.
--
-- Prod runs cloud-api with ALLTERNIT_SKIP_MIGRATIONS=1; deploy-contabo.sh
-- applies this file with psql before the binary swap (ops_applied_migrations).
-- Additive only, every statement is re-runnable.

CREATE TABLE IF NOT EXISTS oauth_apps (
    id              UUID PRIMARY KEY,
    project_id      TEXT NOT NULL REFERENCES platform_projects (id),
    owner_user_id   TEXT NOT NULL,
    client_id       TEXT NOT NULL UNIQUE,
    name            TEXT NOT NULL,
    logo_url        TEXT,
    homepage_url    TEXT,
    privacy_url     TEXT,
    client_type     TEXT NOT NULL CHECK (client_type IN ('confidential', 'public')),
    secret_hash     TEXT,
    secret_prefix   TEXT,
    state           TEXT NOT NULL DEFAULT 'testing'
                    CHECK (state IN ('testing', 'in_review', 'live', 'suspended')),
    scopes          TEXT[] NOT NULL DEFAULT '{}',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    approved_at     TIMESTAMPTZ,
    approved_by     TEXT,
    -- A confidential app always has a secret; a public (PKCE-only) app never does.
    CHECK ((client_type = 'confidential') = (secret_hash IS NOT NULL))
);

CREATE INDEX IF NOT EXISTS oauth_apps_project ON oauth_apps (project_id);

-- At most 10 per app; the service checks the count under a row lock on the app.
CREATE TABLE IF NOT EXISTS oauth_app_redirect_uris (
    id          UUID PRIMARY KEY,
    app_id      UUID NOT NULL REFERENCES oauth_apps (id) ON DELETE CASCADE,
    uri         TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (app_id, uri)
);

-- Up to 100 per app (service). Emails are stored lowercased: the contract says
-- citext, but no migration here enables that extension, so a CHECK keeps the
-- same case-insensitive key without one.
CREATE TABLE IF NOT EXISTS oauth_app_test_users (
    app_id      UUID NOT NULL REFERENCES oauth_apps (id) ON DELETE CASCADE,
    email       TEXT NOT NULL CHECK (email = lower(email)),
    user_id     TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (app_id, email)
);

CREATE TABLE IF NOT EXISTS oauth_auth_requests (
    id              TEXT PRIMARY KEY,
    app_id          UUID NOT NULL REFERENCES oauth_apps (id) ON DELETE CASCADE,
    redirect_uri    TEXT NOT NULL,
    scopes          TEXT[] NOT NULL DEFAULT '{}',
    state           TEXT,
    code_challenge  TEXT NOT NULL,
    user_id         TEXT,
    status          TEXT NOT NULL DEFAULT 'pending'
                    CHECK (status IN ('pending', 'approved', 'denied', 'used')),
    code_hash       TEXT,
    code_expires_at TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at      TIMESTAMPTZ NOT NULL
);

CREATE INDEX IF NOT EXISTS oauth_auth_requests_code_hash ON oauth_auth_requests (code_hash);
CREATE INDEX IF NOT EXISTS oauth_auth_requests_expires_at ON oauth_auth_requests (expires_at);

CREATE TABLE IF NOT EXISTS oauth_grants (
    id                  UUID PRIMARY KEY,
    app_id              UUID NOT NULL REFERENCES oauth_apps (id) ON DELETE CASCADE,
    user_id             TEXT NOT NULL,
    scopes              TEXT[] NOT NULL DEFAULT '{}',
    monthly_cap_cents   INTEGER NOT NULL CHECK (monthly_cap_cents >= 0),
    sub                 TEXT NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at          TIMESTAMPTZ,
    last_used_at        TIMESTAMPTZ
);

-- One live grant per (app, user).
CREATE UNIQUE INDEX IF NOT EXISTS oauth_grants_live_app_user
    ON oauth_grants (app_id, user_id) WHERE revoked_at IS NULL;
CREATE INDEX IF NOT EXISTS oauth_grants_user ON oauth_grants (user_id);

CREATE TABLE IF NOT EXISTS oauth_tokens (
    id          UUID PRIMARY KEY,
    grant_id    UUID NOT NULL REFERENCES oauth_grants (id) ON DELETE CASCADE,
    kind        TEXT NOT NULL CHECK (kind IN ('access', 'refresh')),
    token_hash  TEXT NOT NULL UNIQUE,
    prefix      TEXT NOT NULL,
    expires_at  TIMESTAMPTZ NOT NULL,
    rotated_at  TIMESTAMPTZ,
    replaced_by UUID,
    revoked_at  TIMESTAMPTZ,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS oauth_tokens_grant ON oauth_tokens (grant_id);

-- One row per grant per UTC month ('YYYY-MM'); a new month starts a new row.
CREATE TABLE IF NOT EXISTS oauth_grant_spend (
    grant_id        UUID NOT NULL REFERENCES oauth_grants (id) ON DELETE CASCADE,
    month           TEXT NOT NULL CHECK (month ~ '^[0-9]{4}-[0-9]{2}$'),
    spent_cents     BIGINT NOT NULL DEFAULT 0 CHECK (spent_cents >= 0),
    reserved_cents  BIGINT NOT NULL DEFAULT 0 CHECK (reserved_cents >= 0),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (grant_id, month)
);

-- Events: contract A10. No secrets or tokens in detail.
CREATE TABLE IF NOT EXISTS oauth_audit_log (
    id          BIGSERIAL PRIMARY KEY,
    at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    actor       TEXT NOT NULL,
    app_id      UUID,
    grant_id    UUID,
    event       TEXT NOT NULL,
    ip          TEXT,
    ua          TEXT,
    detail      JSONB NOT NULL DEFAULT '{}'::jsonb
);

CREATE INDEX IF NOT EXISTS oauth_audit_log_app_at ON oauth_audit_log (app_id, at);
CREATE INDEX IF NOT EXISTS oauth_audit_log_grant_at ON oauth_audit_log (grant_id, at);
