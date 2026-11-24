-- 155_platform_agent_runtime.sql
--
-- Harness & Bots 2.0 (contract CONTRACT-harness-bots-2.md, "Backend gaps 1-3",
-- "Cloud-API agents (console)"): a Platform agent names the runtime that runs
-- it, how its brain is paid for and how its thread is shown, beside `model`
-- (the brain).
--
-- * `agent_runtime`: a runtime id from the registry (`GET /api/v1/runtimes`).
--   NULL on rows written before this change; the API reads NULL as `allternit`.
--   (Not `runtime_id`: that column is the hosted runtime that holds the agent.)
-- * `access`: `plan` | `key` | `local` | `credits`, or NULL when not chosen.
-- * `agent_view`: `{ "view": "allternit"|"runtime"|"split", "look": "terminal"|"desktop" }`, or NULL.
--
-- Additive and idempotent: old code keeps running on the new schema.

ALTER TABLE public.platform_agents ADD COLUMN IF NOT EXISTS agent_runtime text;
ALTER TABLE public.platform_agents ADD COLUMN IF NOT EXISTS access text;
ALTER TABLE public.platform_agents ADD COLUMN IF NOT EXISTS agent_view jsonb;
