-- 023_voice_calls_cloud.sql
--
-- Cloud side of phone calls (HANDOFF-realtime-voice-2026-10-02.md §4.1, frozen
-- 2026-10-02). The voice worker (service token) starts calls here and gets an
-- immediate answer from the bot-config cache, so a caller never waits on a
-- sleeping runtime. call.* events are queued per call and delivered to the
-- owner's runtime through the same relay machinery as channel_inbound (wake on
-- demand, retry with backoff for 24h), in order, deduped by the frozen
-- idempotency key `call:<callId>:<type>:<n>`.
--
-- voice_bot_config  : cloud-side copy of a bot's voice settings, refreshed by
--                     the runtime with PUT /api/v1/voice/bot-config/:botId
--                     whenever a bot is saved. A missing row means safe
--                     defaults (generic persona, default voice, recording off).
-- voice_calls       : call registry; remembers owner + runtime so event
--                     delivery never re-resolves the number.
-- voice_call_events : one call.* event per row until delivered (or dead after
--                     24h). Kept 7 days after delivery/death for debugging.

CREATE TABLE IF NOT EXISTS public.voice_bot_config (
    bot_id text PRIMARY KEY,
    user_id text NOT NULL,
    persona text NOT NULL DEFAULT 'A helpful AI assistant.',
    voice_id text NOT NULL DEFAULT 'allternit-default',
    greeting text NOT NULL DEFAULT 'How can I help you today?',
    recording text NOT NULL DEFAULT 'off'
        CHECK (recording IN ('off', 'consented')),
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_voice_bot_config_user ON public.voice_bot_config (user_id);

CREATE TABLE IF NOT EXISTS public.voice_calls (
    call_id text PRIMARY KEY,
    user_id text NOT NULL,
    runtime_id text NOT NULL,
    number_id text NOT NULL,
    bot_id text NOT NULL,
    room text NOT NULL,
    direction text NOT NULL CHECK (direction IN ('inbound', 'outbound')),
    from_e164 text NOT NULL,
    to_e164 text NOT NULL,
    started_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_voice_calls_user ON public.voice_calls (user_id, started_at);

-- Per-call event queue. `event_key` is the frozen idempotency key
-- call:<callId>:<type>:<n>; the UNIQUE constraint is what makes a redelivered
-- event (worker retry, relay retry) a no-op.
CREATE TABLE IF NOT EXISTS public.voice_call_events (
    id bigserial PRIMARY KEY,
    call_id text NOT NULL REFERENCES public.voice_calls(call_id) ON DELETE CASCADE,
    event_type text NOT NULL,
    n integer NOT NULL,
    event_key text NOT NULL UNIQUE,
    payload jsonb NOT NULL,
    received_at timestamptz NOT NULL DEFAULT now(),
    attempts integer NOT NULL DEFAULT 0,
    next_attempt_at timestamptz NOT NULL DEFAULT now(),
    locked_until timestamptz,
    delivered_at timestamptz,
    dead_at timestamptz,
    last_status integer,
    last_error text
);
CREATE INDEX IF NOT EXISTS idx_voice_call_events_due
    ON public.voice_call_events (call_id, n)
    WHERE delivered_at IS NULL AND dead_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_voice_call_events_cleanup
    ON public.voice_call_events (delivered_at, dead_at);
