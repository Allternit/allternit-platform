-- 082_platform_computers.sql
--
-- Computer toolset P6: the hosted computer driver (`/v1/computers`).
--
-- * platform_projects.hosted_driver_enabled: the launch flag. Allternit sets it
--   per project (by hand until Eoj opens the public launch); developers can't.
-- * platform_projects.computer_settings: the project's safety defaults
--   (approval_mode, per_key_concurrency, browser_toolset).
-- * platform_computers: one row per developer computer. Each is a cloud
--   computer (provisioned_instances) owned by the synthetic user in
--   owner_user_id (`platform-drv:<computer id>`), never a person's account.
-- * platform_computer_events: `computer.action` events for the poll endpoint.

ALTER TABLE public.platform_projects ADD COLUMN IF NOT EXISTS hosted_driver_enabled boolean NOT NULL DEFAULT false;
ALTER TABLE public.platform_projects ADD COLUMN IF NOT EXISTS computer_settings jsonb NOT NULL DEFAULT '{}'::jsonb;

CREATE TABLE IF NOT EXISTS public.platform_computers (
    id               text PRIMARY KEY,
    project_id       text NOT NULL REFERENCES public.platform_projects(id),
    account_id       text,
    key_id           text NOT NULL,
    name             text NOT NULL,
    owner_user_id    text NOT NULL UNIQUE,
    instance_id      text,
    status           text NOT NULL DEFAULT 'provisioning',
    metadata         jsonb NOT NULL DEFAULT '{}'::jsonb,
    started_at       timestamptz,
    last_metered_at  timestamptz,
    created_at       timestamptz NOT NULL DEFAULT now(),
    deleted_at       timestamptz
);
CREATE INDEX IF NOT EXISTS idx_platform_computers_project
    ON public.platform_computers (project_id, created_at, id) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_platform_computers_key_running
    ON public.platform_computers (key_id) WHERE deleted_at IS NULL AND status IN ('provisioning', 'starting', 'running');

CREATE TABLE IF NOT EXISTS public.platform_computer_events (
    id           text PRIMARY KEY,
    computer_id  text NOT NULL REFERENCES public.platform_computers(id),
    project_id   text NOT NULL,
    type         text NOT NULL,
    data         jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_platform_computer_events_computer
    ON public.platform_computer_events (computer_id, created_at, id);
