-- V228: phone numbers sync to their runtime from the cloud.
--
-- `synced_at` is set on every number the runtime pulled from cloud-api
-- (`GET /api/v1/runtime-devices/me/phone-numbers`). A synced number the cloud no
-- longer lists was released there, and the sync removes it here; numbers it never
-- touched (created by hand) are left alone.
ALTER TABLE channel_phone_numbers ADD COLUMN synced_at TEXT;
