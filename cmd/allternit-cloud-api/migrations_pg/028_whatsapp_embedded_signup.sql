-- 028_whatsapp_embedded_signup.sql
--
-- WhatsApp Business via Embedded Signup. Allternit is the Tech Provider for
-- each user's own business number (never a shared assistant number).
--
-- whatsapp_es_accounts : one onboarded business number. The business-integration
--                        token and the registration PIN are sealed with the
--                        platform credential cipher; the verify token is kept
--                        hashed (Meta's GET handshake is answered here).
-- whatsapp_windows     : last inbound message per (business number, customer),
--                        which opens the 24-hour customer service window.

CREATE TABLE IF NOT EXISTS public.whatsapp_es_accounts (
    id text PRIMARY KEY,
    user_id text NOT NULL,
    runtime_id text NOT NULL,
    route_id text NOT NULL REFERENCES public.channel_inbound_routes(id) ON DELETE CASCADE,
    waba_id text NOT NULL,
    phone_number_id text NOT NULL,
    token_sealed text NOT NULL,
    pin_sealed text NOT NULL,
    verify_token_hash text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    revoked_at timestamptz
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_whatsapp_es_accounts_phone
    ON public.whatsapp_es_accounts (phone_number_id) WHERE revoked_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_whatsapp_es_accounts_route ON public.whatsapp_es_accounts (route_id);
CREATE INDEX IF NOT EXISTS idx_whatsapp_es_accounts_user ON public.whatsapp_es_accounts (user_id);

CREATE TABLE IF NOT EXISTS public.whatsapp_windows (
    phone_number_id text NOT NULL,
    wa_id text NOT NULL,
    last_inbound_at timestamptz NOT NULL,
    PRIMARY KEY (phone_number_id, wa_id)
);
