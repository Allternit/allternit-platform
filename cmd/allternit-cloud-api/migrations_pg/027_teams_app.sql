-- 027_teams_app.sql
--
-- Microsoft Teams shared app (ao-teams, 2026-10-02). One single-tenant Azure
-- Bot in Allternit's tenant + a multi-tenant Entra app registration; one Teams
-- app package installed per customer tenant. Because Microsoft blocks new
-- multi-tenant Azure Bot registrations (2025-07-31), whether this bot can send
-- proactively into another tenant is UNVERIFIED — proactive send lives behind
-- the TEAMS_PROACTIVE_SEND env flag and is off by default. Reactive replies
-- are the default path.
--
-- teams_installs          : one per customer tenant that installed the Teams
--                           app. Maps a Bot Framework tenant id to the
--                           Allternit user who connected it, and (once the
--                           user's runtime registers) which runtime receives
--                           that tenant's activities.
-- teams_conversation_refs : one per Teams conversation the bot has seen, with
--                           the serviceUrl replies must be posted to
--                           (Bot Framework: serviceUrl is per-tenant and can
--                           change, so it is refreshed on every activity).
-- teams_app_queue         : inbound activities until delivered to the user's
--                           runtime over its relay (waking it if asleep),
--                           same retry contract as channel_inbound_queue.
-- teams_app_states        : short-lived OAuth states for the connect flow.

CREATE TABLE IF NOT EXISTS public.teams_installs (
    id text PRIMARY KEY,
    tenant_id text NOT NULL UNIQUE,
    user_id text NOT NULL,
    runtime_id text,
    installed_by text,
    team_name text,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_teams_installs_user ON public.teams_installs (user_id);

CREATE TABLE IF NOT EXISTS public.teams_conversation_refs (
    id bigserial PRIMARY KEY,
    tenant_id text NOT NULL,
    conversation_id text NOT NULL,
    service_url text NOT NULL,
    name text,
    conversation_type text,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, conversation_id)
);
CREATE INDEX IF NOT EXISTS idx_teams_conversation_refs_tenant ON public.teams_conversation_refs (tenant_id);

CREATE TABLE IF NOT EXISTS public.teams_app_queue (
    id bigserial PRIMARY KEY,
    tenant_id text NOT NULL,
    conversation_id text NOT NULL,
    activity jsonb NOT NULL,
    received_at timestamptz NOT NULL DEFAULT now(),
    attempts integer NOT NULL DEFAULT 0,
    next_attempt_at timestamptz NOT NULL DEFAULT now(),
    locked_until timestamptz,
    delivered_at timestamptz,
    dead_at timestamptz,
    last_status integer,
    last_error text
);
CREATE INDEX IF NOT EXISTS idx_teams_app_queue_due
    ON public.teams_app_queue (next_attempt_at)
    WHERE delivered_at IS NULL AND dead_at IS NULL;

CREATE TABLE IF NOT EXISTS public.teams_app_states (
    state text PRIMARY KEY,
    user_id text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL
);
