-- 076_platform_twin.sql
--
-- Platform API P4: the twin settings a developer sets per account and agent.
-- The cloud is the source of truth; the project's hosted runtime gets them
-- with the agent's definition (so a replaced runtime gets them back).
--
-- * `platform_agents.autonomy_rules`: per-channel / per-person autonomy rules
--   for one agent (`GET/PUT /v1/agents/{id}/autonomy`), a JSON array of
--   `{channel, person, level, limits}`. The agent-wide level stays in
--   `platform_agents.autonomy`.
-- * `platform_memory`: facts every agent of one account knows
--   (`/v1/accounts/{id}/memory`), with provenance (`source`, `source_ref`).
--
-- Idempotent (IF NOT EXISTS). Apply by hand to prod (ALLTERNIT_SKIP_MIGRATIONS=1).

ALTER TABLE public.platform_agents ADD COLUMN IF NOT EXISTS autonomy_rules jsonb NOT NULL DEFAULT '[]'::jsonb;

CREATE TABLE IF NOT EXISTS public.platform_memory (
    id text NOT NULL PRIMARY KEY,
    project_id text NOT NULL REFERENCES public.platform_projects (id),
    account_id text NOT NULL REFERENCES public.platform_accounts (id),
    kind text NOT NULL DEFAULT 'fact',
    subject text NOT NULL DEFAULT '',
    content text NOT NULL,
    source text NOT NULL DEFAULT 'api',
    source_ref text,
    created_by_key text,
    created_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP,
    deleted_at timestamp with time zone,
    CONSTRAINT platform_memory_kind_check CHECK (kind IN ('fact', 'preference', 'schedule_rule', 'person', 'decision'))
);

CREATE INDEX IF NOT EXISTS idx_platform_memory_account
    ON public.platform_memory (project_id, account_id, created_at)
    WHERE deleted_at IS NULL;
