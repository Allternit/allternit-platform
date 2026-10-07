-- 067_platform_agent_knowledge.sql
--
-- Platform API P2: knowledge files for hosted agents (`knowledge_search` tool).
-- A developer uploads text files to one agent (`POST /v1/agents/{id}/knowledge`).
-- The original bytes go to R2 (bucket allternit-user-files, key
-- platform/knowledge/<project>/<agent>/<file>); the text is split into chunks
-- here, with a generated English tsvector, so the agent's runtime can search
-- them during a conversation (ranked with ts_rank_cd).
--
-- Idempotent (IF NOT EXISTS). Apply by hand to prod (ALLTERNIT_SKIP_MIGRATIONS=1).

CREATE TABLE IF NOT EXISTS public.platform_knowledge_files (
    id text NOT NULL PRIMARY KEY,
    project_id text NOT NULL REFERENCES public.platform_projects (id),
    account_id text NOT NULL REFERENCES public.platform_accounts (id),
    agent_id text NOT NULL REFERENCES public.platform_agents (id),
    name text NOT NULL,
    content_type text NOT NULL,
    bytes integer NOT NULL,
    sha256 text NOT NULL,
    storage_key text NOT NULL,
    chunk_count integer NOT NULL DEFAULT 0,
    created_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP,
    deleted_at timestamp with time zone
);
CREATE INDEX IF NOT EXISTS idx_platform_knowledge_files_agent
    ON public.platform_knowledge_files (agent_id, created_at, id) WHERE deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS public.platform_knowledge_chunks (
    id bigserial PRIMARY KEY,
    file_id text NOT NULL REFERENCES public.platform_knowledge_files (id) ON DELETE CASCADE,
    agent_id text NOT NULL,
    ord integer NOT NULL,
    content text NOT NULL,
    tsv tsvector GENERATED ALWAYS AS (to_tsvector('english', content)) STORED
);
CREATE INDEX IF NOT EXISTS idx_platform_knowledge_chunks_agent
    ON public.platform_knowledge_chunks (agent_id);
CREATE INDEX IF NOT EXISTS idx_platform_knowledge_chunks_tsv
    ON public.platform_knowledge_chunks USING gin (tsv);
