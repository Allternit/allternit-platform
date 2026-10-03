-- 029_voice_usage.sql
--
-- Cloud voice minutes metering and single-use voice tickets (track G,
-- ao/voice-tickets). Metering only: nothing here talks to Stripe. Applied by
-- hand to prod, like 020/021/024.
--
-- voice_usage        : one row per finished voice session or call. engine 'cloud'
--                      is Allternit Cloud Voice; 'phone' is a bot phone call.
--                      UNIQUE (engine, ref) makes reporting idempotent: the voice
--                      service can retry a usage report without double counting.
-- voice_tickets_used : nonces of voice tickets already redeemed. A ticket is valid
--                      for 60 s, so rows are only needed until expires_at; old
--                      rows are purged opportunistically on redeem.

CREATE TABLE IF NOT EXISTS public.voice_usage (
    id bigserial PRIMARY KEY,
    user_id text NOT NULL,
    engine text NOT NULL CHECK (engine IN ('cloud', 'phone')),
    seconds integer NOT NULL CHECK (seconds >= 0),
    ref text NOT NULL,
    occurred_at timestamptz NOT NULL DEFAULT now(),
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (engine, ref)
);
CREATE INDEX IF NOT EXISTS idx_voice_usage_user_time
    ON public.voice_usage (user_id, occurred_at);

CREATE TABLE IF NOT EXISTS public.voice_tickets_used (
    nonce text PRIMARY KEY,
    user_id text NOT NULL,
    expires_at timestamptz NOT NULL,
    used_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_voice_tickets_used_expires
    ON public.voice_tickets_used (expires_at);
