-- 024_phone_numbers.sql
--
-- Phone numbers and SMS for bots (channels swarm, c-phone-cloud). A bot gets a
-- real number from a carrier (Telnyx primary, Twilio fallback). Inbound texts
-- arrive on the number's relay address (provider 'sms') and are verified,
-- deduped and consent-filtered here at the edge before they are queued to the
-- user's runtime. Applied by hand to prod, like 020/021.
--
-- phone_numbers      : one row per number a user holds (or is porting in).
-- sms_registrations  : 10DLC brand/campaign or toll-free verification state.
-- sms_opt_outs       : per-number, per-sender STOP list. Opted-out senders never
--                      reach the runtime and are never texted.
-- sms_consent_log    : append-only record of consent changes and "texted first".
-- sms_inbound_seen   : carrier message ids already handled (dedupe).
-- sms_outbound_log   : every SMS sent through the cloud (audit + daily cap).
-- call_consents      : consentRef issued before an outbound call may be dialled.

CREATE TABLE IF NOT EXISTS public.phone_numbers (
    id text PRIMARY KEY,
    user_id text NOT NULL,
    runtime_id text NOT NULL,
    bot_id text NOT NULL,
    e164 text NOT NULL,
    carrier text NOT NULL,
    carrier_number_id text,
    messaging_ref text,
    type text NOT NULL DEFAULT 'local',
    sms_state text NOT NULL DEFAULT 'pending_registration'
        CHECK (sms_state IN ('pending_registration', 'active', 'rejected', 'blocked')),
    voice_state text NOT NULL DEFAULT 'inactive',
    inbound_route_id text,
    port_order_id text,
    port_state text,
    created_at timestamptz NOT NULL DEFAULT now(),
    released_at timestamptz
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_phone_numbers_e164_live
    ON public.phone_numbers (e164) WHERE released_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_phone_numbers_user ON public.phone_numbers (user_id);
CREATE INDEX IF NOT EXISTS idx_phone_numbers_route ON public.phone_numbers (inbound_route_id);

CREATE TABLE IF NOT EXISTS public.sms_registrations (
    id text PRIMARY KEY,
    number_id text NOT NULL REFERENCES public.phone_numbers(id) ON DELETE CASCADE,
    kind text NOT NULL CHECK (kind IN ('10dlc', 'tollfree')),
    carrier text NOT NULL,
    brand_id text,
    campaign_id text,
    tfv_id text,
    state text NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending', 'approved', 'rejected')),
    rejection_reason text,
    fields jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_sms_registrations_number ON public.sms_registrations (number_id);

CREATE TABLE IF NOT EXISTS public.sms_opt_outs (
    number_id text NOT NULL REFERENCES public.phone_numbers(id) ON DELETE CASCADE,
    e164 text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (number_id, e164)
);

CREATE TABLE IF NOT EXISTS public.sms_consent_log (
    id bigserial PRIMARY KEY,
    number_id text NOT NULL REFERENCES public.phone_numbers(id) ON DELETE CASCADE,
    e164 text NOT NULL,
    -- inbound_text | inbound_call | opt_in | explicit | opt_out
    kind text NOT NULL,
    source text,
    evidence text,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_sms_consent_log_lookup ON public.sms_consent_log (number_id, e164, id);

CREATE TABLE IF NOT EXISTS public.sms_inbound_seen (
    number_id text NOT NULL REFERENCES public.phone_numbers(id) ON DELETE CASCADE,
    message_id text NOT NULL,
    seen_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (number_id, message_id)
);

CREATE TABLE IF NOT EXISTS public.sms_outbound_log (
    id bigserial PRIMARY KEY,
    number_id text NOT NULL REFERENCES public.phone_numbers(id) ON DELETE CASCADE,
    to_e164 text NOT NULL,
    carrier_message_id text,
    chars integer NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_sms_outbound_log_number ON public.sms_outbound_log (number_id, created_at);

CREATE TABLE IF NOT EXISTS public.call_consents (
    id text PRIMARY KEY,
    number_id text NOT NULL REFERENCES public.phone_numbers(id) ON DELETE CASCADE,
    user_id text NOT NULL,
    bot_id text NOT NULL,
    to_e164 text NOT NULL,
    purpose text NOT NULL,
    basis text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_call_consents_number ON public.call_consents (number_id, to_e164);
