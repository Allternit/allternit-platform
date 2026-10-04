-- 051_platform_numbers_messaging.sql
--
-- Platform API P1: phone numbers owned by a developer project/account, the
-- texts sent and received on them, events, and signed webhook delivery.
--
-- Idempotent (IF NOT EXISTS). The new phone_numbers columns are nullable; a
-- number bought in the Allternit app keeps project_id NULL and is untouched.

ALTER TABLE public.phone_numbers ADD COLUMN IF NOT EXISTS project_id text;
ALTER TABLE public.phone_numbers ADD COLUMN IF NOT EXISTS account_id text;
-- Sandbox numbers are simulated: no carrier, texts are recorded as `simulated`.
ALTER TABLE public.phone_numbers ADD COLUMN IF NOT EXISTS simulated boolean NOT NULL DEFAULT false;
CREATE INDEX IF NOT EXISTS idx_phone_numbers_project
    ON public.phone_numbers (project_id, account_id) WHERE project_id IS NOT NULL;

-- Texts on API-owned numbers, both directions.
CREATE TABLE IF NOT EXISTS public.platform_messages (
    id text NOT NULL PRIMARY KEY,
    project_id text NOT NULL REFERENCES public.platform_projects (id),
    account_id text,
    number_id text NOT NULL,
    direction text NOT NULL CHECK (direction IN ('inbound', 'outbound')),
    from_e164 text NOT NULL,
    to_e164 text NOT NULL,
    body text NOT NULL,
    segments integer NOT NULL DEFAULT 1,
    status text NOT NULL,
    error_code text,
    carrier_message_id text,
    media jsonb NOT NULL DEFAULT '[]'::jsonb,
    created_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_platform_messages_project
    ON public.platform_messages (project_id, created_at, id);
CREATE INDEX IF NOT EXISTS idx_platform_messages_number
    ON public.platform_messages (number_id, created_at);

-- Every event a project can receive (the webhook payloads), kept for replay.
CREATE TABLE IF NOT EXISTS public.platform_events (
    id text NOT NULL PRIMARY KEY,
    project_id text NOT NULL REFERENCES public.platform_projects (id),
    account_id text,
    type text NOT NULL,
    data jsonb NOT NULL,
    created_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_platform_events_project
    ON public.platform_events (project_id, created_at);

CREATE TABLE IF NOT EXISTS public.platform_webhooks (
    id text NOT NULL PRIMARY KEY,
    project_id text NOT NULL REFERENCES public.platform_projects (id),
    url text NOT NULL,
    events text[] NOT NULL DEFAULT '{}'::text[],
    secret text NOT NULL,
    description text,
    created_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP,
    deleted_at timestamp with time zone
);
CREATE INDEX IF NOT EXISTS idx_platform_webhooks_project
    ON public.platform_webhooks (project_id) WHERE deleted_at IS NULL;

-- One row per (endpoint, event); retried with backoff until delivered or given up.
CREATE TABLE IF NOT EXISTS public.platform_webhook_deliveries (
    id text NOT NULL PRIMARY KEY,
    webhook_id text NOT NULL REFERENCES public.platform_webhooks (id),
    event_id text NOT NULL REFERENCES public.platform_events (id),
    attempts integer NOT NULL DEFAULT 0,
    state text NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'delivered', 'failed')),
    next_attempt_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP,
    last_status integer,
    last_error text,
    delivered_at timestamp with time zone,
    created_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE UNIQUE INDEX IF NOT EXISTS uq_platform_webhook_deliveries
    ON public.platform_webhook_deliveries (webhook_id, event_id);
CREATE INDEX IF NOT EXISTS idx_platform_webhook_deliveries_due
    ON public.platform_webhook_deliveries (next_attempt_at) WHERE state = 'pending';
