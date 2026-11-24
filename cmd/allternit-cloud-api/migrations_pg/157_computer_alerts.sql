-- 157_computer_alerts.sql
--
-- Computers console (surfaces/docs/dev/computers-console-contract.mdx, D4),
-- step 2.8. 155 is taken by 155_platform_agent_runtime.sql and 156 by
-- 156_computer_ssh_keys.sql, so this is 157. Snippets (step 2.9) take 158.
--
-- computer_alert_rules gains duration_minutes: how long CPU must stay over the
-- threshold before the rule fires (offline and disk ignore it).
-- computer_alert_state : one row per (rule, computer) while a breach is being
--   watched or an incident is open. fired_at NULL = breach seen, not yet long
--   enough; fired_at set = the alert went out and stays quiet until the
--   computer is back under the threshold (the row is then deleted).
-- computer_alert_events : what the Activity tab lists: one 'fired' and, later,
--   one 'resolved' row per incident.
--
-- Additive only, IF NOT EXISTS everywhere. No DO blocks: the test harness
-- splits statements.

ALTER TABLE public.computer_alert_rules
    ADD COLUMN IF NOT EXISTS duration_minutes INTEGER NOT NULL DEFAULT 5;

CREATE UNIQUE INDEX IF NOT EXISTS uq_computer_alert_rules_scope
    ON public.computer_alert_rules (user_id, (COALESCE(computer_id, '')), rule_type);

CREATE TABLE IF NOT EXISTS public.computer_alert_state (
    rule_id TEXT NOT NULL,
    computer_id TEXT NOT NULL,
    breach_since TIMESTAMPTZ NOT NULL DEFAULT now(),
    fired_at TIMESTAMPTZ,
    event_id TEXT,
    PRIMARY KEY (rule_id, computer_id)
);

CREATE TABLE IF NOT EXISTS public.computer_alert_events (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    rule_id TEXT NOT NULL,
    computer_id TEXT NOT NULL,
    computer_name TEXT NOT NULL DEFAULT '',
    rule_type TEXT NOT NULL CHECK (rule_type IN ('offline', 'cpu', 'disk')),
    state TEXT NOT NULL CHECK (state IN ('fired', 'resolved')),
    value DOUBLE PRECISION,
    message TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_computer_alert_events_user
    ON public.computer_alert_events (user_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_computer_alert_events_computer
    ON public.computer_alert_events (computer_id, created_at DESC);
