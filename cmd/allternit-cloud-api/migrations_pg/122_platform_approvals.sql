-- 122_platform_approvals.sql
--
-- Platform API parity, lane 0: one approvals resource over the twin, computer
-- and phone hold systems. Each source keeps its own state; this table is the
-- Platform-facing record (`GET /v1/approvals`, `POST /v1/approvals/{id}`).
-- `UNIQUE (source, source_ref)` makes mirroring a source row idempotent.
--
-- Idempotent. Apply by hand to prod as `postgres` (ALLTERNIT_SKIP_MIGRATIONS=1).

CREATE TABLE IF NOT EXISTS public.platform_approvals (
    id text NOT NULL PRIMARY KEY,
    project_id text NOT NULL REFERENCES public.platform_projects (id),
    source text NOT NULL CHECK (source IN ('twin', 'computer', 'phone')),
    source_ref text NOT NULL,
    account_id text REFERENCES public.platform_accounts (id),
    conversation_id text REFERENCES public.platform_conversations (id) ON DELETE SET NULL,
    response_id text REFERENCES public.platform_responses (id) ON DELETE SET NULL,
    kind text NOT NULL,
    proposed_action jsonb NOT NULL DEFAULT '{}'::jsonb,
    status text NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'approved', 'denied', 'expired', 'consumed')),
    expires_at timestamp with time zone,
    created_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP,
    resolved_at timestamp with time zone,
    resolved_by text,
    UNIQUE (source, source_ref)
);
CREATE INDEX IF NOT EXISTS idx_platform_approvals_project
    ON public.platform_approvals (project_id, status, created_at DESC);
-- Expiry sweep: pending rows that have a deadline.
CREATE INDEX IF NOT EXISTS idx_platform_approvals_pending_expiry
    ON public.platform_approvals (expires_at) WHERE status = 'pending' AND expires_at IS NOT NULL;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'allternit') THEN
        GRANT SELECT, INSERT, UPDATE, DELETE ON public.platform_approvals TO allternit;
    END IF;
END $$;
