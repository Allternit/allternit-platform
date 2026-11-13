-- 129_platform_agents_cloud_model_resync.sql
--
-- Agents on the default model "allternit" are now sent to their hosted runtime
-- as a concrete Allternit Cloud model (`allternit/<model>`, see
-- routes/platform_v1/conversations.rs `runtime_model`). Runtimes that already
-- hold such an agent got the bare "allternit", which left them on the
-- computer's own default brain (an unsigned coding CLI), so every turn failed.
-- Clearing synced_at makes the next turn re-send the agent definition.
--
-- Data only, idempotent (re-running only causes one more re-send).

UPDATE public.platform_agents SET synced_at = NULL WHERE model = 'allternit' AND synced_at IS NOT NULL;
