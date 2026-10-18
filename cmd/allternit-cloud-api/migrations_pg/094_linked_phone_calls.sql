-- 094_linked_phone_calls.sql
--
-- Bot voice on a linked phone through a wired cable
-- (CONTRACT-linked-phone-voice-2026-10-18.md C3). A bot's own Android/iPhone is
-- wired to the Mac running Allternit Desktop; Desktop answers the carrier call
-- and joins a LiveKit room `call-lp-<uuid>` as the caller. No phone number and
-- no SIP leg are involved, so:
--
-- voice_linked_call_rooms : one row per room Desktop opened with
--                           POST /api/v1/voice/linked-calls. The worker's
--                           start call (POST /api/v1/voice/calls with a
--                           linked_phone line) is trusted only for a room in
--                           this table; owner, runtime and bot come from it.
-- voice_calls.number_id   : nullable (a linked-phone call has none).
-- voice_calls.line        : the call's line (C1), {kind:"number",numberId} or
--                           {kind:"linked_phone",deviceId,serial?,label,platform}.
-- voice_calls.line_kind   : 'number' | 'linked_phone' (NULL on old rows = number).
-- voice_calls.ended_at    : set when call.ended is queued, so
--                           GET /api/v1/voice/calls/live never depends on the
--                           7-day event cleanup.
-- voice_calls.thread_id   : the runtime's thread for the call, from its reply to
--                           the relayed call.started; NULL until then.
--
-- Idempotent like every file here (IF NOT EXISTS / DROP NOT NULL is a no-op
-- when already nullable).

CREATE TABLE IF NOT EXISTS public.voice_linked_call_rooms (
    room text PRIMARY KEY,
    owner_id text NOT NULL,
    runtime_id text NOT NULL,
    bot_id text NOT NULL,
    line jsonb NOT NULL,
    direction text NOT NULL CHECK (direction IN ('inbound', 'outbound')),
    from_number text,
    to_number text,
    created_at timestamptz NOT NULL DEFAULT now(),
    ended_at timestamptz
);
CREATE INDEX IF NOT EXISTS idx_voice_linked_call_rooms_owner
    ON public.voice_linked_call_rooms (owner_id, created_at);

ALTER TABLE public.voice_calls ALTER COLUMN number_id DROP NOT NULL;
ALTER TABLE public.voice_calls ADD COLUMN IF NOT EXISTS line jsonb;
ALTER TABLE public.voice_calls ADD COLUMN IF NOT EXISTS line_kind text;
ALTER TABLE public.voice_calls ADD COLUMN IF NOT EXISTS ended_at timestamptz;
ALTER TABLE public.voice_calls ADD COLUMN IF NOT EXISTS thread_id text;
CREATE INDEX IF NOT EXISTS idx_voice_calls_live
    ON public.voice_calls (user_id, started_at)
    WHERE ended_at IS NULL;

-- registered_phones.call_bridge : the phone's "bot answers calls" setting as
--                                 the Desktop holds it, {enabled, botName}
--                                 (botName <= 80 chars). NULL = never set.
--                                 Device ids for the audio cable stay on the
--                                 Desktop and are never stored here.
ALTER TABLE public.registered_phones ADD COLUMN IF NOT EXISTS call_bridge jsonb;
