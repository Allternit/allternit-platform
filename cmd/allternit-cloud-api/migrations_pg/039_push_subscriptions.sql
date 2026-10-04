-- 039_push_subscriptions.sql
--
-- Web Push so incoming Allternit calls and new channel messages reach a person whose app is
-- closed (channels swarm, p-push-ring). Applied by hand to prod, like 020/021/024/035/038.
-- Inert until ALLTERNIT_VAPID_PUBLIC_KEY / ALLTERNIT_VAPID_PRIVATE_KEY are set on cloud-api.
--
-- push_subscriptions : one browser/PWA push endpoint per (user, device). `device_id` is a random id the
--                      app keeps in localStorage, so re-subscribing from the same device replaces the row.
-- push_log           : one row per push that went out for a message-type event; drives the rate limit
--                      and the per-thread collapse. Calls are not logged (rings are already limited).

CREATE TABLE IF NOT EXISTS public.push_subscriptions (
    id text PRIMARY KEY,
    user_id text NOT NULL,
    device_id text NOT NULL,
    endpoint text NOT NULL,
    p256dh text NOT NULL,
    auth text NOT NULL,
    user_agent text NOT NULL DEFAULT '',
    created_at timestamptz NOT NULL DEFAULT now(),
    last_ok_at timestamptz,
    last_error text,
    failures integer NOT NULL DEFAULT 0,
    UNIQUE (user_id, device_id)
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_push_subscriptions_endpoint ON public.push_subscriptions (endpoint);
CREATE INDEX IF NOT EXISTS idx_push_subscriptions_user ON public.push_subscriptions (user_id);

CREATE TABLE IF NOT EXISTS public.push_log (
    id bigserial PRIMARY KEY,
    user_id text NOT NULL,
    collapse_key text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_push_log_user ON public.push_log (user_id, collapse_key, created_at);
