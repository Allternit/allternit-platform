-- 070_platform_calls.sql
--
-- Platform API P3: voice. One row per call a project's agent takes part in
-- (phone in, phone out, or an in-app realtime session), its transcript, and
-- the agent a project number answers with.
--
-- A live call's id is also its `voice_calls.call_id`, so the voice worker's
-- events and turns find it. Sandbox calls are `simulated` and never reach
-- LiveKit or a carrier.
--
-- Idempotent (IF NOT EXISTS). Apply by hand to prod (ALLTERNIT_SKIP_MIGRATIONS=1).

-- The agent that answers calls to a project number (NULL: calls aren't answered).
ALTER TABLE public.phone_numbers ADD COLUMN IF NOT EXISTS agent_id text;
-- LiveKit inbound trunk and dispatch rule created when a live number is bound.
ALTER TABLE public.phone_numbers ADD COLUMN IF NOT EXISTS livekit_trunk_id text;
ALTER TABLE public.phone_numbers ADD COLUMN IF NOT EXISTS livekit_dispatch_rule_id text;

CREATE TABLE IF NOT EXISTS public.platform_calls (
    id text NOT NULL PRIMARY KEY,
    project_id text NOT NULL REFERENCES public.platform_projects (id),
    account_id text NOT NULL,
    agent_id text NOT NULL,
    number_id text,
    direction text NOT NULL CHECK (direction IN ('inbound', 'outbound', 'realtime')),
    from_e164 text,
    to_e164 text,
    purpose text,
    status text NOT NULL CHECK (status IN ('queued', 'ringing', 'in_progress', 'completed', 'failed', 'no_answer', 'canceled')),
    end_reason text,
    simulated boolean NOT NULL DEFAULT false,
    room text,
    consent_ref text,
    recording boolean NOT NULL DEFAULT false,
    recording_ref text,
    -- The conversation whose hosted-runtime session the call's turns run in.
    conversation_id text,
    transferred_to text,
    slot_id text,
    -- Realtime: the participant identity the client token was minted for.
    client_identity text,
    duration_seconds integer,
    created_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP,
    answered_at timestamp with time zone,
    ended_at timestamp with time zone,
    updated_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_platform_calls_project
    ON public.platform_calls (project_id, created_at, id);
CREATE UNIQUE INDEX IF NOT EXISTS uq_platform_calls_consent_ref
    ON public.platform_calls (consent_ref) WHERE consent_ref IS NOT NULL;

CREATE TABLE IF NOT EXISTS public.platform_call_transcript (
    id bigserial PRIMARY KEY,
    call_id text NOT NULL REFERENCES public.platform_calls (id),
    speaker text NOT NULL CHECK (speaker IN ('caller', 'agent', 'human')),
    text text NOT NULL,
    segment_id text,
    created_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_platform_call_transcript_call
    ON public.platform_call_transcript (call_id, id);
CREATE UNIQUE INDEX IF NOT EXISTS uq_platform_call_transcript_segment
    ON public.platform_call_transcript (call_id, speaker, segment_id) WHERE segment_id IS NOT NULL;
