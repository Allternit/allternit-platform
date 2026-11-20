-- 152_device_token_platform.sql
--
-- Native Android push (FCM) for the Allternit Android app (routes/fcm.rs). Android FCM registration tokens live
-- in the same table as iOS APNs tokens, so the device cap, the move-on-sign-in rule, unregister, account erase and
-- the device read token (routes/device_read.rs) cover both. Additive only: old code keeps reading every row as iOS
-- (the default), and the new code sends APNs only to `ios` rows and FCM only to `android` rows.
--
-- apns_device_tokens : + platform  `ios` (default, every existing row) or `android`.

ALTER TABLE apns_device_tokens ADD COLUMN IF NOT EXISTS platform text NOT NULL DEFAULT 'ios';
CREATE INDEX IF NOT EXISTS idx_apns_device_tokens_user_platform ON apns_device_tokens (user_id, platform);
