-- 121_platform_items.sql
--
-- Platform API parity, lane 0: Responses and conversation items.
-- `platform_responses` is one model/agent turn; `platform_items` is the unified
-- transcript (messages, function calls, hosted tool calls, approval requests)
-- that conversations and responses both read. Conversations no longer need an
-- agent (a `model` response can use one), so `agent_id` becomes nullable, and
-- `account_id` too (a project-level model conversation has no account).
-- `legacy_id` keeps a backfilled message's `cmsg_` id and `error` its failure
-- reason, so the legacy `conversation.message` projection stays byte-equal.
--
-- `seq` gives a stable cursor order independent of created_at ties.
-- The backfill turns every existing platform_conversation_messages row into one
-- `message` item (same created_at order, deterministic id, idempotent).
--
-- Idempotent. Applied automatically before the binary swap by deploy-contabo.sh
-- via the ops_applied_migrations ledger (platform #1892); never edit after it ships.
-- Afterwards has_sequence_privilege('allternit', 'public.platform_items_seq_seq', 'USAGE') must be true.

CREATE TABLE IF NOT EXISTS public.platform_responses (
    id text NOT NULL PRIMARY KEY,
    project_id text NOT NULL REFERENCES public.platform_projects (id),
    account_id text REFERENCES public.platform_accounts (id),
    agent_id text REFERENCES public.platform_agents (id),
    model text,
    conversation_id text REFERENCES public.platform_conversations (id) ON DELETE SET NULL,
    previous_response_id text,
    status text NOT NULL DEFAULT 'in_progress'
        CHECK (status IN ('in_progress', 'completed', 'failed', 'incomplete', 'requires_action')),
    output jsonb NOT NULL DEFAULT '[]'::jsonb,
    usage jsonb NOT NULL DEFAULT '{}'::jsonb,
    error jsonb,
    metadata jsonb NOT NULL DEFAULT '{}'::jsonb,
    store boolean NOT NULL DEFAULT true,
    created_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP,
    completed_at timestamp with time zone
);
CREATE INDEX IF NOT EXISTS idx_platform_responses_project
    ON public.platform_responses (project_id, created_at DESC, id);
CREATE INDEX IF NOT EXISTS idx_platform_responses_conversation
    ON public.platform_responses (conversation_id, created_at);

CREATE TABLE IF NOT EXISTS public.platform_items (
    id text NOT NULL PRIMARY KEY,
    project_id text NOT NULL REFERENCES public.platform_projects (id),
    conversation_id text REFERENCES public.platform_conversations (id) ON DELETE CASCADE,
    response_id text REFERENCES public.platform_responses (id) ON DELETE CASCADE,
    seq bigint GENERATED ALWAYS AS IDENTITY,
    type text NOT NULL
        CHECK (type IN ('message', 'function_call', 'function_call_output', 'tool_call', 'approval_request')),
    role text CHECK (role IS NULL OR role IN ('user', 'assistant', 'system', 'developer')),
    status text NOT NULL DEFAULT 'completed'
        CHECK (status IN ('in_progress', 'completed', 'failed', 'incomplete')),
    content jsonb NOT NULL DEFAULT '[]'::jsonb,
    legacy_id text,
    error text,
    created_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_platform_items_legacy_id
    ON public.platform_items (legacy_id) WHERE legacy_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_platform_items_conversation ON public.platform_items (conversation_id, seq);
CREATE INDEX IF NOT EXISTS idx_platform_items_response ON public.platform_items (response_id, seq);

ALTER TABLE public.platform_conversations ALTER COLUMN agent_id DROP NOT NULL;
ALTER TABLE public.platform_conversations ALTER COLUMN account_id DROP NOT NULL;
-- Conversation lookups by account and list-newest-first for GET /v1/conversations.
CREATE INDEX IF NOT EXISTS idx_platform_conversations_project
    ON public.platform_conversations (project_id, created_at DESC, id);

-- Backfill: one message item per legacy conversation message, in created_at order.
-- Content is [{type: input_text|output_text, text}] by role; a failed assistant
-- turn keeps status 'failed'. ON CONFLICT makes a re-run a no-op.
INSERT INTO public.platform_items (id, project_id, conversation_id, type, role, status, content, legacy_id, error, created_at)
SELECT 'item_' || substr(md5(m.id), 1, 24),
       c.project_id,
       m.conversation_id,
       'message',
       m.role,
       CASE WHEN m.status = 'failed' THEN 'failed' ELSE 'completed' END,
       jsonb_build_array(jsonb_build_object(
           'type', CASE WHEN m.role = 'user' THEN 'input_text' ELSE 'output_text' END,
           'text', m.content)),
       m.id,
       m.error,
       m.created_at
FROM public.platform_conversation_messages m
JOIN public.platform_conversations c ON c.id = m.conversation_id
ORDER BY m.created_at, m.id
ON CONFLICT (id) DO NOTHING;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'allternit') THEN
        GRANT SELECT, INSERT, UPDATE, DELETE ON public.platform_responses, public.platform_items TO allternit;
        GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO allternit;
    END IF;
END $$;
