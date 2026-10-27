-- 108_platform_conversation_thread_ids.sql
--
-- Platform API AG-UI endpoint (`POST /v1/agents/{id}/ag-ui`, routes/platform_v1/ag_ui.rs).
-- An AG-UI frontend names its conversation with its own `threadId`. When that
-- isn't one of the agent's conversation ids, the conversation the run opens
-- keeps it here, so later runs with the same threadId continue it. Unique per
-- agent, so two first runs of one thread land on the same conversation.
--
-- Additive only, idempotent (safe to re-run).

ALTER TABLE public.platform_conversations
    ADD COLUMN IF NOT EXISTS external_thread_id text;

CREATE UNIQUE INDEX IF NOT EXISTS idx_platform_conversations_external_thread
    ON public.platform_conversations (project_id, agent_id, external_thread_id)
    WHERE external_thread_id IS NOT NULL;
