-- 062_bot_email_domains.sql
--
-- Customer domains for bot email (support@acme.com). Allternit Mail holds the
-- domain under the platform's admin account, so ownership lives here: one
-- Allternit user per domain. verified_at is set once every DNS record checks
-- on mx.allternit.com; bot mailboxes can be created on it from then on.
--
-- Idempotent (IF NOT EXISTS). Apply by hand to prod (ALLTERNIT_SKIP_MIGRATIONS=1).

CREATE TABLE IF NOT EXISTS public.bot_email_domains (
    domain text PRIMARY KEY,
    user_id text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    verified_at timestamptz
);
CREATE INDEX IF NOT EXISTS idx_bot_email_domains_user ON public.bot_email_domains (user_id);
