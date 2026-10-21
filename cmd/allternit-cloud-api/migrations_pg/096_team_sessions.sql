-- 096_team_sessions.sql
--
-- Team roster (agent-network phase 2, WP13). Runtimes report their live CLI
-- sessions in the presence heartbeat (`sessions: LiveSession[]`); the roster
-- shows a Clerk org's members -> machines -> sessions.
--
-- Additive only: two new tables, no change to existing data. Rows in
-- team_sessions are pruned when not seen for 24 h.

CREATE TABLE IF NOT EXISTS public.team_sessions (
    runtime_id text NOT NULL,
    user_id text NOT NULL,
    org_id text NOT NULL DEFAULT '',
    address text NOT NULL,
    payload jsonb NOT NULL DEFAULT '{}'::jsonb,
    updated_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (runtime_id, address)
);
CREATE INDEX IF NOT EXISTS team_sessions_org_idx ON public.team_sessions (org_id, updated_at);
CREATE INDEX IF NOT EXISTS team_sessions_user_idx ON public.team_sessions (user_id);

CREATE TABLE IF NOT EXISTS public.team_members_seen (
    org_id text NOT NULL,
    user_id text NOT NULL,
    display text NOT NULL DEFAULT '',
    last_seen timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (org_id, user_id)
);
