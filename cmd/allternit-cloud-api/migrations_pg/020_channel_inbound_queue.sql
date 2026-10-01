-- 020_channel_inbound_queue.sql
--
-- Channels hybrid relay. Slack, Telegram, WhatsApp, Teams and Discord need a
-- public HTTPS address, but a user's runtime (their Mac, VPS or a sleeping
-- cloud computer) has none. Each connected channel gets an unguessable public
-- address on cloud-api; inbound platform requests are acknowledged at once,
-- held here, and delivered to the runtime over its relay, waking it if it is
-- asleep. Signatures are verified by the runtime (it holds the secrets);
-- cloud-api stores the raw request untouched.
--
-- channel_inbound_routes : public address key -> (user, runtime, provider). The
--                          key is the credential, so it is stored hashed.
-- channel_inbound_queue  : one inbound platform request, until delivered (or
--                          given up after 24 hours). Kept 7 days after.

CREATE TABLE IF NOT EXISTS public.channel_inbound_routes (
    id text PRIMARY KEY,
    key_hash text NOT NULL UNIQUE,
    user_id text NOT NULL,
    runtime_id text NOT NULL,
    provider text NOT NULL,
    label text,
    created_at timestamptz NOT NULL DEFAULT now(),
    revoked_at timestamptz,
    last_inbound_at timestamptz
);
CREATE INDEX IF NOT EXISTS idx_channel_inbound_routes_user ON public.channel_inbound_routes (user_id);

CREATE TABLE IF NOT EXISTS public.channel_inbound_queue (
    id bigserial PRIMARY KEY,
    route_id text NOT NULL REFERENCES public.channel_inbound_routes(id) ON DELETE CASCADE,
    method text NOT NULL,
    query text NOT NULL DEFAULT '',
    headers jsonb NOT NULL,
    body text NOT NULL,
    received_at timestamptz NOT NULL DEFAULT now(),
    attempts integer NOT NULL DEFAULT 0,
    next_attempt_at timestamptz NOT NULL DEFAULT now(),
    locked_until timestamptz,
    delivered_at timestamptz,
    dead_at timestamptz,
    last_status integer,
    last_error text
);
CREATE INDEX IF NOT EXISTS idx_channel_inbound_queue_due
    ON public.channel_inbound_queue (next_attempt_at)
    WHERE delivered_at IS NULL AND dead_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_channel_inbound_queue_route ON public.channel_inbound_queue (route_id, id);
