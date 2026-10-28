-- 110_apns_device_tokens.sql
--
-- Native iOS push (APNs) for the Allternit iPhone app (routes/apns.rs). Inert until APNS_KEY_P8 /
-- APNS_KEY_ID / APNS_TEAM_ID are set on cloud-api. Every Web Push sender also delivers here, so the
-- notification preferences, quiet hours and rate limits in routes/notifications.rs apply to both.
--
-- apns_device_tokens : one APNs device token per row. A token belongs to one person (signing in as
--                      someone else on the same phone moves it). `environment` is `production` or
--                      `sandbox` (debug builds); `device_id` lets a reinstall replace its old token.

CREATE TABLE IF NOT EXISTS apns_device_tokens (
    id text PRIMARY KEY,
    user_id text NOT NULL,
    token text NOT NULL UNIQUE,
    environment text NOT NULL DEFAULT 'production',
    device_id text NOT NULL DEFAULT '',
    created_at timestamptz NOT NULL DEFAULT now(),
    last_ok_at timestamptz,
    last_error text,
    failures integer NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_apns_device_tokens_user ON apns_device_tokens (user_id);
