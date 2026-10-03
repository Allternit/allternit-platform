-- V213__telegram_managed_pairing.sql
--
-- Telegram Managed Bots onboarding (Bot API 9.6, phase 1 backend): pairing
-- columns on provider_account_bindings. The cloud manager bot relays a child
-- bot's token to the runtime with a one-time pairNonce; the first Telegram
-- user who sends `/start <nonce>` to the child bot is recorded as the owner
-- of that Telegram account binding and the nonce is cleared. The cloud
-- onboarding id is kept for status correlation only.
--
-- The pair nonce is a capability (32 hex chars, single use, 30-minute cloud
-- expiry) — it is never stored on the cloud side and is cleared here the
-- moment a /start consumes it.

ALTER TABLE provider_account_bindings
    ADD COLUMN pair_nonce TEXT;
ALTER TABLE provider_account_bindings
    ADD COLUMN pair_onboarding_id TEXT;
ALTER TABLE provider_account_bindings
    ADD COLUMN tg_owner_user_id TEXT;
