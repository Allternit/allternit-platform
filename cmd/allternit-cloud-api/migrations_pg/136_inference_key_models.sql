-- 136_inference_key_models.sql
--
-- Connect kit (CONTRACT-connections-2026-11): the chat models a saved key's
-- vendor listed at its last check (Anthropic, Google, xAI), so the key's row
-- in GET /api/v1/connections lists real models and the picker can offer
-- `<provider>/<model>`. Ids only, at most 50, replaced on every re-check.
-- The row is already erased with the account (user_inference_keys is in
-- ERASE); this column adds no person data.

ALTER TABLE public.user_inference_keys
    ADD COLUMN IF NOT EXISTS models jsonb NOT NULL DEFAULT '[]'::jsonb;
