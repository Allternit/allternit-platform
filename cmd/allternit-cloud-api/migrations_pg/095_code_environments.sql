-- 095_code_environments.sql
--
-- Code mode cloud environments (CODE_PARITY Contract B). An environment is a
-- named, per-user recipe a cloud Code session starts in: network policy,
-- setup script, .env text, repos to clone, and write-only secrets.
--
-- Additive only: two new tables, no change to existing data.
--
-- Secrets are encrypted at rest with the platform AES-256-GCM credential
-- cipher (ALLTERNIT_CREDENTIALS_KEY), split into value_ciphertext and the
-- 12-byte GCM value_nonce exactly like user_inference_keys (010). Values are
-- returned only by GET /api/v1/code-environments/:id/resolve to a runtime
-- device owned by the environment's user.

CREATE TABLE IF NOT EXISTS public.code_environments (
    id text PRIMARY KEY,
    user_id text NOT NULL,
    name text NOT NULL,
    network_access text NOT NULL DEFAULT 'full'
        CHECK (network_access IN ('full', 'limited', 'none')),
    allowed_hosts jsonb NOT NULL DEFAULT '[]'::jsonb,
    setup_script text NOT NULL DEFAULT '',
    env_vars text NOT NULL DEFAULT '',
    repos jsonb NOT NULL DEFAULT '[]'::jsonb,
    is_default boolean NOT NULL DEFAULT false,
    archived_at timestamp with time zone,
    created_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_code_environments_user
    ON public.code_environments (user_id, created_at);

-- At most one default environment per user (archiving clears the flag).
CREATE UNIQUE INDEX IF NOT EXISTS idx_code_environments_one_default
    ON public.code_environments (user_id)
    WHERE is_default;

CREATE TABLE IF NOT EXISTS public.code_environment_secrets (
    environment_id text NOT NULL
        REFERENCES public.code_environments (id) ON DELETE CASCADE,
    name text NOT NULL,
    value_ciphertext bytea NOT NULL,
    value_nonce bytea NOT NULL,
    hosts jsonb NOT NULL DEFAULT '[]'::jsonb,
    created_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (environment_id, name)
);
