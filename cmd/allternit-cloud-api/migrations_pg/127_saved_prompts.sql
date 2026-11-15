-- 127_saved_prompts.sql
--
-- Console playground saved prompts (Waves 1–6). A prompt belongs to one
-- Platform project; body holds the playground state (messages, model,
-- parameters, tools). share_token is set while a signed-in share link is on
-- and cleared when it is turned off. deleted_at soft-deletes.
--
-- Additive only, idempotent (safe to re-run).

CREATE TABLE IF NOT EXISTS public.platform_saved_prompts (
    id text PRIMARY KEY,
    project_id text NOT NULL REFERENCES public.platform_projects (id),
    owner_user_id text NOT NULL,
    name text NOT NULL,
    body jsonb NOT NULL DEFAULT '{}'::jsonb,
    share_token text UNIQUE,
    created_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP,
    deleted_at timestamp with time zone
);

CREATE INDEX IF NOT EXISTS idx_platform_saved_prompts_project
    ON public.platform_saved_prompts (project_id, updated_at DESC) WHERE deleted_at IS NULL;
