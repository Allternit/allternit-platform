-- 038_inapp_calls.sql
--
-- Free messages and calls between Allternit users who are linked through an invite
-- (channels swarm, m-inapp-calls). Applied by hand to prod, like 020/021/024/035.
--
-- inapp_threads       : one direct thread per (owner, owner's bot, contact); both people read the same id.
-- inapp_thread_events : the thread's log. kind = message | call.ring | call.answered |
--                       call.declined | call.cancelled | call.missed. Both sides read the same rows (fan-out).
-- inapp_calls         : one row per ring, keyed by the LiveKit room. State machine:
--                       ringing -> answered | declined | cancelled | missed.
-- phone_presence      : last time the person's app asked for incoming calls (online = within 60 s).
-- phone_call_prefs    : a person may let one of their bots answer rings with the voice agent.

CREATE TABLE IF NOT EXISTS public.inapp_threads (
    id text PRIMARY KEY,
    owner_user_id text NOT NULL,
    bot_id text NOT NULL,
    contact_user_id text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (owner_user_id, bot_id, contact_user_id)
);
CREATE INDEX IF NOT EXISTS idx_inapp_threads_contact ON public.inapp_threads (contact_user_id);

CREATE TABLE IF NOT EXISTS public.inapp_thread_events (
    id bigserial PRIMARY KEY,
    thread_id text NOT NULL REFERENCES public.inapp_threads(id) ON DELETE CASCADE,
    kind text NOT NULL,
    sender_user_id text NOT NULL,
    body text NOT NULL DEFAULT '',
    call_room text,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_inapp_thread_events_thread ON public.inapp_thread_events (thread_id, id);

CREATE TABLE IF NOT EXISTS public.inapp_calls (
    room text PRIMARY KEY,
    thread_id text NOT NULL REFERENCES public.inapp_threads(id) ON DELETE CASCADE,
    caller_user_id text NOT NULL,
    callee_user_id text NOT NULL,
    bot_id text NOT NULL,
    mode text NOT NULL DEFAULT 'voice',
    state text NOT NULL DEFAULT 'ringing'
        CHECK (state IN ('ringing', 'answered', 'declined', 'cancelled', 'missed')),
    answered_by_agent boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL,
    ended_at timestamptz
);
CREATE INDEX IF NOT EXISTS idx_inapp_calls_callee ON public.inapp_calls (callee_user_id, state);
CREATE INDEX IF NOT EXISTS idx_inapp_calls_caller ON public.inapp_calls (caller_user_id, created_at);

CREATE TABLE IF NOT EXISTS public.phone_presence (
    user_id text PRIMARY KEY,
    last_seen_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS public.phone_call_prefs (
    user_id text PRIMARY KEY,
    auto_answer_bot_id text,
    updated_at timestamptz NOT NULL DEFAULT now()
);
