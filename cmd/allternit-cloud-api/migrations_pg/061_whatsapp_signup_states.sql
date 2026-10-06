-- 061_whatsapp_signup_states.sql
--
-- WhatsApp Embedded Signup from the Allternit app: the app asks for a signup
-- link (POST /api/v1/channels/whatsapp/connect {runtimeId}); the hosted page
-- (/channels/whatsapp/signup?state=) runs Meta's signup and completes it with
-- this one-time state, which names the user and the computer that receives the
-- number's messages. States expire after 30 minutes and are deleted on use.
--
-- Idempotent (IF NOT EXISTS).

CREATE TABLE IF NOT EXISTS public.whatsapp_signup_states (
    state text PRIMARY KEY,
    user_id text NOT NULL,
    runtime_id text NOT NULL,
    expires_at timestamptz NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_whatsapp_signup_states_expiry ON public.whatsapp_signup_states (expires_at);
