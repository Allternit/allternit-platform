-- 102_notification_quiet_hours.sql
--
-- Quiet hours for push notifications (`routes/notifications.rs`).
--
--   * notification_quiet_hours: one row per person who set quiet hours. No row
--     = off, 22:00 to 07:00 UTC, calls still ring (`QuietHours::default`).
--     Times are minutes after local midnight in `time_zone` (IANA), so a
--     window follows daylight saving time. start_min > end_min wraps midnight;
--     start_min = end_min means no quiet hours.
--   * notification_held: one row per push that quiet hours kept back, from the
--     push sink (event_id = the backbone event, unique so a restart or a second
--     replica never holds it twice) or from a direct push (in-app messages,
--     relayed channel messages, missed calls; event_id NULL). When the window
--     ends the sink claims a person's open rows (summarized_at) in one UPDATE
--     and sends one "While you were away" summary, so a restart never loses or
--     doubles it. Pruned after seven days by the sink.
--   * notification_push_log.outcome gains the value 'held' (text column, no
--     constraint to change).
--
-- Additive only, idempotent (safe to re-run).

CREATE TABLE IF NOT EXISTS public.notification_quiet_hours (
    user_id text PRIMARY KEY,
    enabled boolean NOT NULL DEFAULT false,
    start_min integer NOT NULL DEFAULT 1320 CHECK (start_min BETWEEN 0 AND 1439),
    end_min integer NOT NULL DEFAULT 420 CHECK (end_min BETWEEN 0 AND 1439),
    time_zone text NOT NULL DEFAULT 'UTC',
    allow_calls boolean NOT NULL DEFAULT true,
    updated_at timestamp with time zone NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS public.notification_held (
    id bigserial PRIMARY KEY,
    user_id text NOT NULL,
    -- the registry event type the push was for (message.received, call.ended, approval.requested, ...)
    event_type text NOT NULL,
    -- the push tag: re-sends of the same approval or problem share it and are counted once
    tag text NOT NULL DEFAULT '',
    -- attention.opened severity (info, warn, critical), else NULL
    severity text,
    -- sink | direct
    source text NOT NULL DEFAULT 'sink',
    event_id text UNIQUE,
    held_at timestamp with time zone NOT NULL DEFAULT now(),
    summarized_at timestamp with time zone
);

CREATE INDEX IF NOT EXISTS idx_notification_held_open
    ON public.notification_held (user_id) WHERE summarized_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_notification_held_held_at
    ON public.notification_held (held_at);
