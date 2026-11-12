-- 123_platform_decisions.sql
--
-- Platform API parity, lane 0: `POST /v1/decisions`. One row per request;
-- `questions` and `answers` are the typed question list and its answers (one
-- per question, in order). `outcomes` is keyed by question name (PATCH).
-- There is no live/test column: no earlier platform table has one; a project's key mode comes from its keys
-- (alt_live_ / alt_test_).
--
-- Idempotent. Apply by hand to prod as `postgres` (ALLTERNIT_SKIP_MIGRATIONS=1).

CREATE TABLE IF NOT EXISTS public.platform_decisions (
    id text NOT NULL PRIMARY KEY,
    project_id text NOT NULL REFERENCES public.platform_projects (id),
    account_id text REFERENCES public.platform_accounts (id),
    questions jsonb NOT NULL DEFAULT '[]'::jsonb,
    answers jsonb NOT NULL DEFAULT '[]'::jsonb,
    backend text NOT NULL DEFAULT '',
    latency_ms integer NOT NULL DEFAULT 0,
    usage jsonb NOT NULL DEFAULT '{}'::jsonb,
    metadata jsonb NOT NULL DEFAULT '{}'::jsonb,
    outcomes jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_platform_decisions_project
    ON public.platform_decisions (project_id, created_at DESC);

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'allternit') THEN
        GRANT SELECT, INSERT, UPDATE, DELETE ON public.platform_decisions TO allternit;
    END IF;
END $$;
