-- 064_platform_conversations.sql
--
-- Platform API P2: conversations with hosted agents. A conversation belongs to
-- one agent (and its account); its runtime session is opened on the first
-- message, on the project's hosted runtime (owner `platform:<project_id>`).
-- `platform_agents.synced_at` records when the runtime last received the
-- agent's definition, so an update reaches the runtime before the next turn.
--
-- Idempotent (IF NOT EXISTS). Apply by hand to prod (ALLTERNIT_SKIP_MIGRATIONS=1).

ALTER TABLE public.platform_agents ADD COLUMN IF NOT EXISTS synced_at timestamp with time zone;

CREATE TABLE IF NOT EXISTS public.platform_conversations (
    id text NOT NULL PRIMARY KEY,
    project_id text NOT NULL REFERENCES public.platform_projects (id),
    account_id text NOT NULL REFERENCES public.platform_accounts (id),
    agent_id text NOT NULL REFERENCES public.platform_agents (id),
    runtime_id text,
    runtime_session_id text,
    metadata jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_platform_conversations_agent
    ON public.platform_conversations (project_id, agent_id, created_at, id);

CREATE TABLE IF NOT EXISTS public.platform_conversation_messages (
    id text NOT NULL PRIMARY KEY,
    conversation_id text NOT NULL REFERENCES public.platform_conversations (id),
    role text NOT NULL CHECK (role IN ('user', 'assistant')),
    content text NOT NULL,
    status text NOT NULL DEFAULT 'completed' CHECK (status IN ('completed', 'failed')),
    error text,
    created_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_platform_conversation_messages_conv
    ON public.platform_conversation_messages (conversation_id, created_at, id);

-- A project's own model keys (spec §5, bring your own key). Encrypted with the
-- credential cipher (ALLTERNIT_CREDENTIALS_KEY); only a masked form is ever returned.
CREATE TABLE IF NOT EXISTS public.platform_model_keys (
    project_id text NOT NULL REFERENCES public.platform_projects (id),
    provider text NOT NULL CHECK (provider IN ('anthropic', 'openai', 'xai')),
    key_encrypted text NOT NULL,
    masked text NOT NULL,
    created_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (project_id, provider)
);
