-- 112_mailboxes.sql
--
-- Platform API mailboxes and agent identities (`/v1/mailboxes`, `/v1/identities`).
--
-- A mailbox is an address on Allternit Mail (mx.allternit.com) owned by one
-- project account, optionally tied to one hosted agent. cloud-api creates it
-- with the Allternit Mail admin key (as `bot_email` does for runtimes) plus a
-- `message.inbound` webhook pointed at a `platform_mail` relay route
-- (`channel_inbound_routes`); inbound mail is kept here and sent to the
-- developer as `mail.received`. The webhook signing secret is stored sealed
-- with the credential cipher when the deployment has one.
--
-- An identity binds one number, one mailbox and, optionally, one of the
-- owner's linked phones (by serial), billed as one `identity_month` bundle.
--
-- Idempotent (IF NOT EXISTS). Apply by hand to prod if migrations are skipped.

CREATE TABLE IF NOT EXISTS public.platform_mailboxes (
    id text PRIMARY KEY,
    project_id text NOT NULL,
    account_id text NOT NULL,
    agent_id text,
    address text NOT NULL,
    display_name text,
    mail_mailbox_id text NOT NULL,
    route_id text,
    webhook_id text,
    webhook_secret_sealed text,
    created_at timestamptz NOT NULL DEFAULT now(),
    deleted_at timestamptz
);
CREATE INDEX IF NOT EXISTS idx_platform_mailboxes_project ON public.platform_mailboxes (project_id, created_at, id) WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS uq_platform_mailboxes_route ON public.platform_mailboxes (route_id) WHERE route_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS public.platform_mail_messages (
    id text PRIMARY KEY,
    mailbox_id text NOT NULL REFERENCES public.platform_mailboxes (id),
    provider_message_id text,
    from_address text NOT NULL DEFAULT '',
    to_address text NOT NULL DEFAULT '',
    subject text,
    snippet text,
    text_body text,
    html_body text,
    codes jsonb NOT NULL DEFAULT '[]'::jsonb,
    links jsonb NOT NULL DEFAULT '[]'::jsonb,
    is_verification boolean NOT NULL DEFAULT false,
    received_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_platform_mail_messages_mailbox ON public.platform_mail_messages (mailbox_id, received_at DESC, id);
CREATE UNIQUE INDEX IF NOT EXISTS uq_platform_mail_messages_provider ON public.platform_mail_messages (mailbox_id, provider_message_id) WHERE provider_message_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS public.platform_identities (
    id text PRIMARY KEY,
    project_id text NOT NULL,
    account_id text NOT NULL,
    agent_id text,
    number_id text NOT NULL,
    mailbox_id text NOT NULL,
    phone_serial text,
    created_at timestamptz NOT NULL DEFAULT now(),
    deleted_at timestamptz
);
CREATE INDEX IF NOT EXISTS idx_platform_identities_project ON public.platform_identities (project_id, created_at, id) WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS uq_platform_identities_number ON public.platform_identities (number_id) WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS uq_platform_identities_mailbox ON public.platform_identities (mailbox_id) WHERE deleted_at IS NULL;
