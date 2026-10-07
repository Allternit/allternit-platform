-- 075_platform_channel_connections.sql
--
-- Platform API P4: channel connections an end-customer account makes for one
-- of its agents (`/v1/accounts/{id}/channels`). One row per connection.
--
-- This is THE table to read for "which channels does this agent have"
-- (e.g. the hosted-agent `channel_post` tool): only rows with
-- `status = 'connected' AND deleted_at IS NULL` are live. `external_id` is the
-- vendor's id for the place connected (Slack team id, email address);
-- `display_name` is what people see (team name, address).
--
-- status: pending (connect started, waiting for the end customer: an OAuth
-- consent page) | connected | failed (`error` says why). A deleted row stays
-- for audit with `deleted_at` set.
--
-- `connect_nonce_hash` is the SHA-256 of the one-time nonce carried in the
-- vendor OAuth `state`, so a callback can only finish the connection it was
-- started for, once.
--
-- Idempotent (IF NOT EXISTS). Apply by hand to prod (ALLTERNIT_SKIP_MIGRATIONS=1).

CREATE TABLE IF NOT EXISTS public.platform_channel_connections (
    id text NOT NULL PRIMARY KEY,
    project_id text NOT NULL REFERENCES public.platform_projects (id),
    account_id text NOT NULL REFERENCES public.platform_accounts (id),
    agent_id text NOT NULL REFERENCES public.platform_agents (id),
    kind text NOT NULL,
    status text NOT NULL DEFAULT 'pending',
    external_id text,
    display_name text,
    -- Where `channel_post` posts when the agent names no target (a Slack channel id); nullable.
    default_target text,
    return_url text,
    connect_nonce_hash text,
    error text,
    metadata jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP,
    connected_at timestamp with time zone,
    deleted_at timestamp with time zone,
    CONSTRAINT platform_channel_connections_status_check CHECK (status IN ('pending', 'connected', 'failed'))
);

CREATE INDEX IF NOT EXISTS idx_platform_channel_connections_account
    ON public.platform_channel_connections (project_id, account_id, created_at)
    WHERE deleted_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_platform_channel_connections_agent
    ON public.platform_channel_connections (agent_id)
    WHERE deleted_at IS NULL;

-- One live connection per vendor place inside a project: two accounts can never
-- share a Slack team (or an address), so an inbound message has one account.
CREATE UNIQUE INDEX IF NOT EXISTS uq_platform_channel_connections_external
    ON public.platform_channel_connections (project_id, kind, external_id)
    WHERE deleted_at IS NULL AND external_id IS NOT NULL;
