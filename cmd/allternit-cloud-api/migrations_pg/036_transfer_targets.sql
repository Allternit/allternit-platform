-- 036_transfer_targets.sql
--
-- Owner-approved warm-transfer targets for a bot's calls (i-transfer-consent).
-- voice_bot_config.transfer_targets : jsonb array of {e164, label}. A bot-initiated
--                                     warm transfer may only dial a number on this
--                                     list; the cloud then issues a consentRef with
--                                     basis 'owner_transfer_target'. Owner-initiated
--                                     transfers get basis 'owner_directed' and need
--                                     no list entry. call_consents.basis is free text,
--                                     so the new bases need no constraint change.
-- Applied by hand to prod, like 020/021.

ALTER TABLE public.voice_bot_config
    ADD COLUMN IF NOT EXISTS transfer_targets jsonb NOT NULL DEFAULT '[]'::jsonb;
