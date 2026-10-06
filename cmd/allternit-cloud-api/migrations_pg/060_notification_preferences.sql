-- 060_notification_preferences.sql
--
-- Push from the event backbone (SPEC-allternit-events-2026-10-05 §4 P5).
--
-- `routes/notifications.rs` runs one push sink: it reads user events from
-- the backbone (`platform_events`, subject = 'user', migration 057) and
-- decides, per user and event type, whether to send a Web Push notification
-- (039_push_subscriptions). This migration adds what it needs:
--
--   * notification_preferences: one row per (user, event type) the person
--     changed. No row = the event type's default (`notifications::DEFAULTS`).
--   * notification_push_log: one row per backbone event the sink has looked
--     at, claimed with INSERT … ON CONFLICT DO NOTHING so a restart, a second
--     replica or a re-scan never pushes the same event twice. Pruned after
--     two days by the sink.
--   * an index on platform_events for the sink's scan of recent user events.
--
-- Additive only, idempotent (safe to re-run). 059 is reserved for the MCP
-- Events client (P6).

CREATE TABLE IF NOT EXISTS public.notification_preferences (
    user_id text NOT NULL,
    event_type text NOT NULL,
    enabled boolean NOT NULL,
    updated_at timestamp with time zone NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, event_type)
);

CREATE TABLE IF NOT EXISTS public.notification_push_log (
    event_id text PRIMARY KEY,
    user_id text NOT NULL,
    event_type text NOT NULL,
    -- sent | off (the person turned this type off) | unreachable (no device
    -- took it) | skipped (stale, a duplicate of a direct push, or rate limited)
    outcome text NOT NULL DEFAULT 'pending',
    created_at timestamp with time zone NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_notification_push_log_created
    ON public.notification_push_log (created_at);

CREATE INDEX IF NOT EXISTS idx_platform_events_user_recent
    ON public.platform_events (created_at) WHERE subject = 'user';
