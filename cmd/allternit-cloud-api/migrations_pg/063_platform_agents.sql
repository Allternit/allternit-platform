-- 063_platform_agents.sql
--
-- Platform API P2: hosted agents. One row per agent a developer creates with
-- `POST /v1/agents`; every agent belongs to one end-customer account. The
-- hosted runtime that runs it (and the bot id there) is filled in when the
-- project's runtime takes the agent; until then `runtime_id` is NULL.
--
-- Idempotent (IF NOT EXISTS). Apply by hand to prod (ALLTERNIT_SKIP_MIGRATIONS=1).

CREATE TABLE IF NOT EXISTS public.platform_agents (
    id text NOT NULL PRIMARY KEY,
    project_id text NOT NULL REFERENCES public.platform_projects (id),
    account_id text NOT NULL REFERENCES public.platform_accounts (id),
    name text NOT NULL,
    instructions text NOT NULL DEFAULT '',
    greeting text NOT NULL,
    model text NOT NULL DEFAULT 'allternit',
    voice text NOT NULL DEFAULT 'af_heart',
    tools text[] NOT NULL DEFAULT '{}',
    autonomy text NOT NULL DEFAULT 'ask' CHECK (autonomy IN ('draft', 'ask', 'tell', 'limits')),
    transfer_targets text[] NOT NULL DEFAULT '{}',
    business_hours jsonb,
    metadata jsonb NOT NULL DEFAULT '{}'::jsonb,
    runtime_id text,
    runtime_bot_id text,
    created_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP,
    deleted_at timestamp with time zone
);

CREATE INDEX IF NOT EXISTS idx_platform_agents_project
    ON public.platform_agents (project_id, created_at, id) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_platform_agents_account
    ON public.platform_agents (account_id) WHERE deleted_at IS NULL;
