-- 107_quiet_hours_levels.sql
--
-- Quiet hours as a pager (`routes/notifications.rs`).
--
--   * notification_quiet_hours.let_through: which alerts still reach the person
--     during quiet hours. 'urgent' (default: critical problems and anything a bot
--     marked urgent), 'needs_you' (also approvals, threads waiting on them,
--     sign-in needed) or 'none' (hold everything; calls follow allow_calls).
--   * notification_repeat: one row per urgent push that went out during quiet
--     hours. Until the person opens it (POST /api/v1/notifications/ack with the
--     row's ack_token, sent by the service worker) or the item resolves (same
--     tag), the sink sends it again every 5 minutes, at most 3 more times.
--     Pruned after two days by the sink.
--
-- Additive only, idempotent (safe to re-run).

ALTER TABLE public.notification_quiet_hours
    ADD COLUMN IF NOT EXISTS let_through text NOT NULL DEFAULT 'urgent';

CREATE TABLE IF NOT EXISTS public.notification_repeat (
    id bigserial PRIMARY KEY,
    user_id text NOT NULL,
    tag text NOT NULL,
    kind text NOT NULL DEFAULT 'event',
    title text NOT NULL,
    body text NOT NULL,
    url text NOT NULL DEFAULT '/',
    data jsonb NOT NULL DEFAULT '{}'::jsonb,
    ack_token text NOT NULL UNIQUE,
    sent_count integer NOT NULL DEFAULT 0,
    next_at timestamp with time zone NOT NULL,
    acked_at timestamp with time zone,
    created_at timestamp with time zone NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_notification_repeat_due
    ON public.notification_repeat (next_at) WHERE acked_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_notification_repeat_user_tag
    ON public.notification_repeat (user_id, tag) WHERE acked_at IS NULL;
