-- 059_mcp_event_client_subscriptions.sql
--
-- MCP Events client (SPEC-allternit-events-2026-10-05 §2 item 6, P6):
-- Allternit subscribes to events from the external MCP servers a user has
-- connected (Gmail, GitHub, Linear, ...). The subscription itself lives on the
-- user's runtime (it holds the connector credentials); cloud-api is only the
-- public webhook receiver.
--
-- mcp_event_client_subscriptions : one subscription the runtime registered.
--     key          = the runtime's deterministic `sub_...` id (callback path
--                    `/mcp/events/callback/<key>`; not a credential: every
--                    delivery must carry a Standard Webhooks signature).
--     secret       = the `whsec_` secret the runtime minted and gave the
--                    external server (sealed with ALLTERNIT_CREDENTIALS_KEY
--                    when set). previous_secret stays valid until
--                    previous_secret_until after a rotation.
--     route_id     = the internal channel_inbound route its events queue on
--                    (provider `mcp_events`; same queue, worker and relay as
--                    the channels hybrid relay).
--     status       = active | terminated (the server ended it) | ended (the
--                    runtime removed it). Anything but active answers 410.
-- mcp_event_client_seen : eventIds already accepted, for dedupe (7 days).
--
-- Additive and idempotent (IF NOT EXISTS).

CREATE TABLE IF NOT EXISTS public.mcp_event_client_subscriptions (
    key text PRIMARY KEY,
    user_id text NOT NULL,
    runtime_id text NOT NULL,
    route_id text NOT NULL REFERENCES public.channel_inbound_routes(id),
    connector_id text NOT NULL,
    event_name text NOT NULL,
    secret text NOT NULL,
    previous_secret text,
    previous_secret_until timestamptz,
    status text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'terminated', 'ended')),
    verified_at timestamptz,
    last_event_at timestamptz,
    event_count bigint NOT NULL DEFAULT 0,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    ended_at timestamptz
);
CREATE INDEX IF NOT EXISTS idx_mcp_event_client_subscriptions_owner
    ON public.mcp_event_client_subscriptions (user_id, runtime_id);

CREATE TABLE IF NOT EXISTS public.mcp_event_client_seen (
    subscription_key text NOT NULL REFERENCES public.mcp_event_client_subscriptions(key) ON DELETE CASCADE,
    event_id text NOT NULL,
    seen_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (subscription_key, event_id)
);
CREATE INDEX IF NOT EXISTS idx_mcp_event_client_seen_at ON public.mcp_event_client_seen (seen_at);
