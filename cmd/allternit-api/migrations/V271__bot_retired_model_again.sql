-- Repeat of V161 (bot default model → Kimi).
--
-- V161 repointed bot rows that carried the retired catalog default
-- `openai/gpt-5-mini`, but rows created afterwards by Desktop builds that
-- predated #439 (and rows merged back from stale local registries) still
-- carry it. The packaged Gizzi assistant was one of them: every Gizzi turn
-- failed with ProviderModelNotFoundError. The readers now ignore the value
-- (resolveModelRef in allternit-ai, parse_model_ref in gizzi_chat_stream.rs);
-- this cleans the stored rows the same way V161 did.
--
-- IRREVERSIBLE for the same reason as V161: no down migration.
UPDATE agents
SET model = 'kimi/kimi-for-coding',
    provider = 'custom'
WHERE is_bot = 1
  AND (model = 'openai/gpt-5-mini' OR (provider = 'openai' AND model = 'gpt-5-mini'));
