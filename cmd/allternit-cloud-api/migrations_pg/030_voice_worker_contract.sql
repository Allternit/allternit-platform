-- 030_voice_worker_contract.sql
--
-- Lines cloud-api up with the call worker's wire contract
-- (services/voice/src/call_worker/cloud_client.rs):
--
-- voice_bot_config.name     : the bot's spoken name for the call disclosure
--                             ("you've reached {name}"); NULL until the
--                             runtime saves one.
-- voice_calls.sip_call_id   : carrier-side SIP Call-ID the worker reports.
-- voice_calls.consent_ref   : outbound only, the consent gate's reference.
-- voice_call_events.seq     : the worker's call-wide order. `n` counts one
--                             event type, so delivery orders by seq. The
--                             cloud's own call.started row is seq 0 (first).
-- voice_call_events.at_ms   : worker wall clock, relayed as occurredAt.

ALTER TABLE public.voice_bot_config ADD COLUMN IF NOT EXISTS name text;

ALTER TABLE public.voice_calls ADD COLUMN IF NOT EXISTS sip_call_id text;
ALTER TABLE public.voice_calls ADD COLUMN IF NOT EXISTS consent_ref text;

ALTER TABLE public.voice_call_events ADD COLUMN IF NOT EXISTS seq bigint NOT NULL DEFAULT 0;
ALTER TABLE public.voice_call_events ADD COLUMN IF NOT EXISTS at_ms bigint;

CREATE INDEX IF NOT EXISTS idx_voice_call_events_due_seq
    ON public.voice_call_events (call_id, seq, id)
    WHERE delivered_at IS NULL AND dead_at IS NULL;
