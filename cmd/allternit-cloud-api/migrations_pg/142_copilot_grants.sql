-- GitHub Copilot grants (CONTRACT-connections-2026-11 sections 9.5 and 9.11,
-- brain runner step 1.5). One row per account: the GitHub App
-- (`allternit-copilot-connect`) user token and its refresh token, both
-- sealed with CredentialCipher, the Copilot chat models the account can
-- pick, and the single-use sign-in nonce while a connect is pending.
-- `sealed_access` NULL = not connected (only a pending sign-in).
-- `needs_reason` 'login_expired' = the refresh failed; the person signs in again.
-- Revoked at GitHub and deleted on disconnect and on account erase.
CREATE TABLE IF NOT EXISTS copilot_grants (
    user_id             TEXT PRIMARY KEY,
    github_login        TEXT,
    sealed_access       TEXT,
    access_expires_at   TIMESTAMPTZ,
    sealed_refresh      TEXT,
    refresh_expires_at  TIMESTAMPTZ,
    models              JSONB NOT NULL DEFAULT '[]'::jsonb,
    needs_reason        TEXT,
    pending_nonce       TEXT,
    pending_return      TEXT,
    pending_expires_at  TIMESTAMPTZ,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);
