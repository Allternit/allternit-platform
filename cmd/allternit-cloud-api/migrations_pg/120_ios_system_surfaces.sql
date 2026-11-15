-- 120_ios_system_surfaces.sql
--
-- iPhone system surfaces (plan: ios-system-surfaces, step B1). Additive and idempotent: safe to re-run.
--
-- apns_device_tokens      : + read_token_hash  sha256 hex of the device read token's secret (routes/device_read.rs).
--                             The token itself is returned once per registration and never stored.
--                           + os_version / app_build  what the app said at registration.
--                           + capabilities  JSON string array the app supports: actions, nse, la_push, widget_push.
--                             Old clients send none, so the default is [] and they get today's alert payload.
--                           + focus / focus_at  the device's Focus filter (written by a later step; columns only here).
-- apns_activity_tokens    : Live Activity push-to-start / update tokens and widget push tokens, per device.
--                             Cleaned up like device tokens (410 / BadDeviceToken / too many failures), and removed
--                             with their device when the device token is unregistered.
-- status_snapshots        : the latest `status.snapshot` per runtime (one row per user + runtime).
-- notification_delivered  : which notification tag is on which phone for an inbox item, so it can be revoked.
-- push_subscriptions      : + platform  ios-pwa | android | desktop (Web Push subscribe).

ALTER TABLE apns_device_tokens ADD COLUMN IF NOT EXISTS read_token_hash text;
ALTER TABLE apns_device_tokens ADD COLUMN IF NOT EXISTS os_version text;
ALTER TABLE apns_device_tokens ADD COLUMN IF NOT EXISTS app_build text;
ALTER TABLE apns_device_tokens ADD COLUMN IF NOT EXISTS capabilities jsonb NOT NULL DEFAULT '[]'::jsonb;
ALTER TABLE apns_device_tokens ADD COLUMN IF NOT EXISTS focus jsonb;
ALTER TABLE apns_device_tokens ADD COLUMN IF NOT EXISTS focus_at timestamptz;

CREATE TABLE IF NOT EXISTS apns_activity_tokens (
    id text PRIMARY KEY,
    user_id text NOT NULL,
    device_id text NOT NULL DEFAULT '',
    kind text NOT NULL CHECK (kind IN ('la_start', 'la_update', 'widget')),
    attributes_type text,
    activity_key text,
    token text NOT NULL UNIQUE,
    environment text NOT NULL DEFAULT 'production',
    created_at timestamptz NOT NULL DEFAULT now(),
    last_ok_at timestamptz,
    failures integer NOT NULL DEFAULT 0,
    last_error text
);
CREATE INDEX IF NOT EXISTS idx_apns_activity_tokens_user ON apns_activity_tokens (user_id, kind);
CREATE INDEX IF NOT EXISTS idx_apns_activity_tokens_device ON apns_activity_tokens (user_id, device_id);

CREATE TABLE IF NOT EXISTS status_snapshots (
    user_id text NOT NULL,
    runtime_id text NOT NULL,
    snapshot jsonb NOT NULL,
    hash text NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, runtime_id)
);

CREATE TABLE IF NOT EXISTS notification_delivered (
    user_id text NOT NULL,
    inbox_id text NOT NULL,
    tag text NOT NULL,
    event_id text,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, inbox_id, tag)
);

ALTER TABLE push_subscriptions ADD COLUMN IF NOT EXISTS platform text;
