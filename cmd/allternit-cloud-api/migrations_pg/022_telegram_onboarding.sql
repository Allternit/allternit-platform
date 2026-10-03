-- 022_telegram_onboarding.sql
--
-- Telegram Managed Bots onboarding (Bot API 9.6, phase 1 backend): one row
-- per "create me a Telegram bot" wizard run. The cloud manager bot receives
-- the managed_bot update, fetches the child bot's token, and relays it to
-- the user's runtime — the token is relayed straight through and is NEVER
-- stored here (or anywhere at rest outside the user's runtime vault).
--
-- Matching the managed_bot update to a row: exact, case-insensitive match on
-- suggested_username (Telegram usernames are case-insensitive). Refused when
-- zero or more than one waiting row matches. The creator's Telegram user id
-- is recorded as corroboration, never as the primary key: one person can run
-- the wizard several times and the update carries no wizard-side id.
--
-- state: waiting (row created, deep link shown) → created (manager bot saw
-- the managed_bot update) → connected (token relayed to the runtime) →
-- paired (the user's Telegram /start <nonce> reached the runtime). failed /
-- expired are terminal. expires_at bounds the whole flow at 30 minutes.

CREATE TABLE IF NOT EXISTS public.telegram_onboarding (
    id                bigserial PRIMARY KEY,
    user_id           text NOT NULL,
    runtime_id        text NOT NULL,
    allternit_bot_id  text NOT NULL,
    bot_name          text NOT NULL,
    suggested_username text NOT NULL,
    nonce             text NOT NULL UNIQUE,
    state             text NOT NULL DEFAULT 'waiting'
                      CHECK (state IN ('waiting','created','connected','paired','failed','expired')),
    tg_bot_id         text,
    tg_bot_username   text,
    tg_creator_user_id text,
    error             text,
    created_at        timestamptz NOT NULL DEFAULT now(),
    updated_at        timestamptz NOT NULL DEFAULT now(),
    expires_at        timestamptz NOT NULL DEFAULT now() + interval '30 minutes'
);
CREATE INDEX IF NOT EXISTS idx_telegram_onboarding_match
    ON public.telegram_onboarding (lower(suggested_username), state, expires_at);
CREATE INDEX IF NOT EXISTS idx_telegram_onboarding_user
    ON public.telegram_onboarding (user_id, created_at);
