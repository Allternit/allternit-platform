-- Link your Claude organization: the person's Anthropic user profile, the
-- pending browser sign-ins, and the linked connection
-- (CONTRACT-oauth-linked-billing-2026-11.md, Part B, section B5).
--
-- Secrets are sealed with the platform AES-256-GCM credential cipher
-- (ALLTERNIT_CREDENTIALS_KEY), split like 010_user_inference_keys.sql:
-- *_ciphertext is the sealed bytes, *_nonce the 12-byte GCM nonce. The
-- Anthropic access token is never stored. Account erase
-- (services/account_erase.rs) lists every table here.
--
-- inference_usage gains who paid for each call: billed_to is
-- 'allternit_credits', 'user_wallet' (an app using Sign in with Allternit)
-- or 'claude_org' (a linked Claude organization); oauth_app_id and
-- oauth_grant_id name the app and grant of an app's call. NULL on rows
-- written before this migration and on plain credits calls.
--
-- Prod runs cloud-api with ALLTERNIT_SKIP_MIGRATIONS=1; deploy-contabo.sh
-- applies this file with psql before the binary swap (ops_applied_migrations).
-- Additive only, every statement is re-runnable.

-- One Anthropic user profile (uprof_...) per account, reused on every re-link.
CREATE TABLE IF NOT EXISTS anthropic_link_profiles (
    user_id          TEXT PRIMARY KEY,
    user_profile_id  TEXT NOT NULL UNIQUE,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- A browser sign-in in flight: the single-use OAuth state and its sealed
-- PKCE verifier. Lives 15 minutes; used_at is set on the callback.
CREATE TABLE IF NOT EXISTS anthropic_link_requests (
    state                    TEXT PRIMARY KEY,
    user_id                  TEXT NOT NULL,
    code_verifier_ciphertext BYTEA NOT NULL,
    code_verifier_nonce      BYTEA NOT NULL,
    return_to                TEXT,
    created_at               TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at               TIMESTAMPTZ NOT NULL DEFAULT (now() + INTERVAL '15 minutes'),
    used_at                  TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS anthropic_link_requests_user ON anthropic_link_requests (user_id);
CREATE INDEX IF NOT EXISTS anthropic_link_requests_expires ON anthropic_link_requests (expires_at);

-- The linked organization. The refresh token is kept only for re-link and is
-- never refreshed on a timer (contract B3). Both halves of the sealed token
-- are set together or not at all.
CREATE TABLE IF NOT EXISTS anthropic_connections (
    user_id                  TEXT PRIMARY KEY,
    user_profile_id          TEXT NOT NULL,
    connection_id            TEXT,
    refresh_token_ciphertext BYTEA,
    refresh_token_nonce      BYTEA,
    org_name                 TEXT,
    status                   TEXT NOT NULL DEFAULT 'linked'
                             CHECK (status IN ('linked', 'relink_needed', 'disconnected')),
    linked_at                TIMESTAMPTZ NOT NULL DEFAULT now(),
    refresh_expires_at       TIMESTAMPTZ NOT NULL DEFAULT (now() + INTERVAL '1 year'),
    last_checked_at          TIMESTAMPTZ,
    last_error               TEXT,
    CHECK ((refresh_token_ciphertext IS NULL) = (refresh_token_nonce IS NULL))
);

CREATE INDEX IF NOT EXISTS anthropic_connections_connection ON anthropic_connections (connection_id);

ALTER TABLE public.inference_usage ADD COLUMN IF NOT EXISTS billed_to text;
ALTER TABLE public.inference_usage ADD COLUMN IF NOT EXISTS oauth_app_id uuid;
ALTER TABLE public.inference_usage ADD COLUMN IF NOT EXISTS oauth_grant_id uuid;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'inference_usage_billed_to_check' AND conrelid = 'public.inference_usage'::regclass
    ) THEN
        ALTER TABLE public.inference_usage ADD CONSTRAINT inference_usage_billed_to_check
            CHECK (billed_to IS NULL OR billed_to IN ('allternit_credits', 'user_wallet', 'claude_org'));
    END IF;
END $$;

CREATE INDEX IF NOT EXISTS idx_inference_usage_oauth_grant_created
    ON public.inference_usage (oauth_grant_id, created_at) WHERE oauth_grant_id IS NOT NULL;
