-- 050_platform_api_foundation.sql
--
-- Allternit Platform API foundation (P0): developer projects, end-customer
-- accounts, project-bound API keys, usage events, idempotency records,
-- Postgres-backed rate-limit windows and per-project call slots.
--
-- Idempotent (IF NOT EXISTS). Existing `alt_` keys keep working exactly as
-- before: the new api_keys columns are nullable and legacy paths never read them.

CREATE TABLE IF NOT EXISTS public.platform_projects (
    id text NOT NULL PRIMARY KEY,
    owner_user_id text NOT NULL,
    org_id text,
    name text NOT NULL,
    env text NOT NULL DEFAULT 'sandbox' CHECK (env IN ('sandbox', 'live')),
    plan text NOT NULL DEFAULT 'sandbox' CHECK (plan IN ('sandbox', 'payg', 'growth', 'enterprise')),
    spend_cap_cents bigint NOT NULL DEFAULT 10000,
    rpm_override integer,
    call_cap_override integer,
    created_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP,
    archived_at timestamp with time zone
);

CREATE INDEX IF NOT EXISTS idx_platform_projects_owner
    ON public.platform_projects (owner_user_id, created_at);
CREATE INDEX IF NOT EXISTS idx_platform_projects_org
    ON public.platform_projects (org_id, created_at) WHERE org_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS public.platform_accounts (
    id text NOT NULL PRIMARY KEY,
    project_id text NOT NULL REFERENCES public.platform_projects (id),
    name text NOT NULL,
    external_ref text,
    metadata jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP,
    deleted_at timestamp with time zone
);

-- A deleted account releases its external_ref so the developer can re-create it.
CREATE UNIQUE INDEX IF NOT EXISTS uq_platform_accounts_external_ref
    ON public.platform_accounts (project_id, external_ref)
    WHERE external_ref IS NOT NULL AND deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_platform_accounts_project
    ON public.platform_accounts (project_id, created_at, id);

ALTER TABLE public.api_keys ADD COLUMN IF NOT EXISTS project_id text REFERENCES public.platform_projects (id);
ALTER TABLE public.api_keys ADD COLUMN IF NOT EXISTS account_id text REFERENCES public.platform_accounts (id);
ALTER TABLE public.api_keys ADD COLUMN IF NOT EXISTS env text;

CREATE INDEX IF NOT EXISTS idx_api_keys_project
    ON public.api_keys (project_id) WHERE project_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS public.platform_usage_events (
    id text NOT NULL PRIMARY KEY,
    project_id text NOT NULL,
    account_id text,
    key_id text,
    meter text NOT NULL,
    quantity numeric NOT NULL,
    unit text,
    ref_id text,
    idempotency text,
    created_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE UNIQUE INDEX IF NOT EXISTS uq_platform_usage_events_idempotency
    ON public.platform_usage_events (project_id, idempotency) WHERE idempotency IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_platform_usage_events_project_created
    ON public.platform_usage_events (project_id, created_at);

CREATE TABLE IF NOT EXISTS public.platform_idempotency (
    key_id text NOT NULL,
    idem_key text NOT NULL,
    request_hash text NOT NULL,
    -- 0 while the first request is still running
    status integer NOT NULL DEFAULT 0,
    body jsonb,
    created_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (key_id, idem_key)
);

CREATE INDEX IF NOT EXISTS idx_platform_idempotency_created
    ON public.platform_idempotency (created_at);

CREATE TABLE IF NOT EXISTS public.platform_rate_windows (
    bucket text NOT NULL,
    window_start timestamp with time zone NOT NULL,
    count integer NOT NULL DEFAULT 0,
    PRIMARY KEY (bucket, window_start)
);

CREATE TABLE IF NOT EXISTS public.platform_call_slots (
    project_id text NOT NULL,
    slot_id text NOT NULL,
    kind text NOT NULL DEFAULT 'call',
    acquired_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at timestamp with time zone NOT NULL,
    PRIMARY KEY (project_id, slot_id)
);

CREATE INDEX IF NOT EXISTS idx_platform_call_slots_expires
    ON public.platform_call_slots (expires_at);
