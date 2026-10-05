-- 056_bot_email_mailboxes.sql
--
-- Bot email for every runtime (phase 2). Allternit Mail runs once, owned by
-- the platform; cloud-api holds its admin key and provisions each bot's
-- mailbox on behalf of the runtime that owns the bot: an `email` relay route
-- to that runtime, the mailbox, a send/read key scoped to it, and a webhook
-- scoped to that mailbox only. A runtime never holds the admin key.
--
-- Idempotent (IF NOT EXISTS).

CREATE TABLE IF NOT EXISTS public.bot_email_mailboxes (
    mailbox_id text NOT NULL PRIMARY KEY,
    user_id text NOT NULL,
    runtime_id text NOT NULL,
    agent_id text NOT NULL,
    address text NOT NULL,
    route_id text,
    webhook_id text,
    api_key_id text,
    created_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP,
    deleted_at timestamp with time zone
);
CREATE UNIQUE INDEX IF NOT EXISTS uq_bot_email_mailboxes_agent
    ON public.bot_email_mailboxes (runtime_id, agent_id) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_bot_email_mailboxes_user
    ON public.bot_email_mailboxes (user_id);
