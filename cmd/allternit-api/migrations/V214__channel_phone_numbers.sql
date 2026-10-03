-- V214: phone numbers on this runtime (SMS + calls).
--
-- The cloud owns the carrier side (buying, registration, opt-outs; cloud-api
-- `phone_numbers`). This is the runtime's view of the numbers that reach its
-- bots, so an inbound text or a call can find its bot and one conversation
-- key (`phone:<botE164>:<callerE164>`) shared by calls and texts.
CREATE TABLE IF NOT EXISTS channel_phone_numbers (
    number_id   TEXT PRIMARY KEY,           -- cloud phone_numbers.id
    owner       TEXT NOT NULL,
    bot_id      TEXT NOT NULL,
    e164        TEXT NOT NULL,
    account_id  TEXT,                       -- provider_account_bindings.id (vendor 'sms') that sends and verifies for it
    created_at  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_channel_phone_numbers_e164 ON channel_phone_numbers(owner, e164);
CREATE INDEX IF NOT EXISTS idx_channel_phone_numbers_bot ON channel_phone_numbers(bot_id);
