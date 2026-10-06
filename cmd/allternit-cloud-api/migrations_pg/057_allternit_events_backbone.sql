-- 057_allternit_events_backbone.sql
--
-- One event backbone (SPEC-allternit-events-2026-10-05 §2, P2 + P4).
--
-- The Platform API's durable queue (051: platform_events, platform_webhooks,
-- platform_webhook_deliveries) becomes the one store for every outbound
-- event, instead of a second table:
--
--   * platform_events gains a subject: `project` (Platform API, unchanged),
--     `user` (events forwarded by a user's runtime or raised in the cloud for
--     one user) or `account` (reserved). A user event has no project.
--   * platform_webhooks gains an endpoint kind (`platform_webhook` |
--     `mcp_subscription`) and a signer (`allternit` Stripe-style, the 051
--     behaviour | `standard_webhooks`). MCP Events subscriptions made at the
--     mcp.allternit.com edge are rows of kind `mcp_subscription`, keyed by the
--     deterministic `sub_…` id of (principal, url, name, arguments).
--
-- Additive only. Every new column is nullable or has a default that keeps
-- 051 rows behaving exactly as before (kind platform_webhook, signer
-- allternit, subject project). Idempotent: safe to re-run.

-- ── events ───────────────────────────────────────────────────────────────────

ALTER TABLE public.platform_events ADD COLUMN IF NOT EXISTS subject text NOT NULL DEFAULT 'project';
ALTER TABLE public.platform_events ADD COLUMN IF NOT EXISTS user_id text;
-- Where the event came from (`runtime:<runtime id>`, `cloud`) and the
-- producer's own id for it; together they make runtime ingest idempotent.
ALTER TABLE public.platform_events ADD COLUMN IF NOT EXISTS source text;
ALTER TABLE public.platform_events ADD COLUMN IF NOT EXISTS source_id text;
-- When it happened at the producer (delivery `timestamp`); created_at is when we stored it.
ALTER TABLE public.platform_events ADD COLUMN IF NOT EXISTS occurred_at timestamp with time zone;
-- A user event belongs to no project.
ALTER TABLE public.platform_events ALTER COLUMN project_id DROP NOT NULL;

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'platform_events_subject_check' AND conrelid = 'public.platform_events'::regclass) THEN
        ALTER TABLE public.platform_events ADD CONSTRAINT platform_events_subject_check CHECK (
            (subject = 'project' AND project_id IS NOT NULL)
            OR (subject = 'user' AND user_id IS NOT NULL)
            OR (subject = 'account' AND account_id IS NOT NULL)
        );
    END IF;
END $$;

CREATE UNIQUE INDEX IF NOT EXISTS uq_platform_events_source
    ON public.platform_events (source, source_id) WHERE source_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_platform_events_user
    ON public.platform_events (user_id, created_at) WHERE user_id IS NOT NULL;

-- ── endpoints (Platform API webhooks + MCP Events subscriptions) ─────────────

ALTER TABLE public.platform_webhooks ADD COLUMN IF NOT EXISTS kind text NOT NULL DEFAULT 'platform_webhook';
ALTER TABLE public.platform_webhooks ADD COLUMN IF NOT EXISTS signer text NOT NULL DEFAULT 'allternit';
-- MCP subscriptions belong to a user, not a project.
ALTER TABLE public.platform_webhooks ALTER COLUMN project_id DROP NOT NULL;
-- MCP principal: verified user, OAuth client (or `cli-key:<id>`) and edge
-- target (`agents` | `bot:<vendorBotId>`).
ALTER TABLE public.platform_webhooks ADD COLUMN IF NOT EXISTS user_id text;
ALTER TABLE public.platform_webhooks ADD COLUMN IF NOT EXISTS client_id text;
ALTER TABLE public.platform_webhooks ADD COLUMN IF NOT EXISTS target text;
-- Subscription arguments; an event is delivered when its data contains them (jsonb @>).
ALTER TABLE public.platform_webhooks ADD COLUMN IF NOT EXISTS arguments jsonb NOT NULL DEFAULT '{}'::jsonb;
-- Secret rotation on refresh: deliveries are signed with both keys until previous_secret_until.
ALTER TABLE public.platform_webhooks ADD COLUMN IF NOT EXISTS previous_secret text;
ALTER TABLE public.platform_webhooks ADD COLUMN IF NOT EXISTS previous_secret_until timestamp with time zone;
-- Granted TTL (NULL = no expiry; the edge never grants that today).
ALTER TABLE public.platform_webhooks ADD COLUMN IF NOT EXISTS refresh_before timestamp with time zone;
-- When the callback last echoed a verification challenge.
ALTER TABLE public.platform_webhooks ADD COLUMN IF NOT EXISTS verified_at timestamp with time zone;
-- Why delivery stopped (`gone` = the receiver answered 410, `approval_revoked`, `unsubscribed`).
ALTER TABLE public.platform_webhooks ADD COLUMN IF NOT EXISTS deactivated_reason text;
ALTER TABLE public.platform_webhooks ADD COLUMN IF NOT EXISTS updated_at timestamp with time zone;

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'platform_webhooks_kind_check' AND conrelid = 'public.platform_webhooks'::regclass) THEN
        ALTER TABLE public.platform_webhooks ADD CONSTRAINT platform_webhooks_kind_check CHECK (
            (kind = 'platform_webhook' AND project_id IS NOT NULL)
            OR (kind = 'mcp_subscription' AND user_id IS NOT NULL AND client_id IS NOT NULL AND target IS NOT NULL)
        );
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'platform_webhooks_signer_check' AND conrelid = 'public.platform_webhooks'::regclass) THEN
        ALTER TABLE public.platform_webhooks ADD CONSTRAINT platform_webhooks_signer_check CHECK (
            signer IN ('allternit', 'standard_webhooks')
        );
    END IF;
END $$;

-- Fan-out for a user event looks up that user's live subscriptions.
CREATE INDEX IF NOT EXISTS idx_platform_webhooks_mcp_user
    ON public.platform_webhooks (user_id) WHERE kind = 'mcp_subscription' AND deleted_at IS NULL;
-- Revoking an app approval drops that principal's subscriptions.
CREATE INDEX IF NOT EXISTS idx_platform_webhooks_mcp_principal
    ON public.platform_webhooks (user_id, client_id, target) WHERE kind = 'mcp_subscription';
