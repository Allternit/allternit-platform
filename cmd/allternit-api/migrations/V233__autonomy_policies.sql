-- Autonomy levels per place (digital-twin layer 3). The owner decides, per
-- bot x channel (optionally per person), how far a bot may go on its own:
-- draft | ask | tell | limits. '' in bot_id / channel / person means "all".
CREATE TABLE IF NOT EXISTS autonomy_policies (
    id          TEXT PRIMARY KEY,
    owner       TEXT NOT NULL,
    bot_id      TEXT NOT NULL DEFAULT '',
    channel     TEXT NOT NULL DEFAULT '',
    person      TEXT NOT NULL DEFAULT '',
    level       TEXT NOT NULL CHECK (level IN ('draft','ask','tell','limits')),
    limits_json TEXT NOT NULL DEFAULT '{}',
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    UNIQUE (owner, bot_id, channel, person)
);
CREATE INDEX IF NOT EXISTS idx_autonomy_policies_owner ON autonomy_policies(owner, bot_id);

-- What the bot actually did under the policy: feeds the daily limits and the
-- "what did my bot do" digest. outcome: sent | held | drafted | denied.
CREATE TABLE IF NOT EXISTS autonomy_actions (
    id           TEXT PRIMARY KEY,
    owner        TEXT NOT NULL,
    bot_id       TEXT NOT NULL,
    channel      TEXT NOT NULL,
    person       TEXT NOT NULL DEFAULT '',
    action       TEXT NOT NULL,
    amount_cents INTEGER NOT NULL DEFAULT 0,
    level        TEXT NOT NULL,
    outcome      TEXT NOT NULL,
    reason       TEXT NOT NULL DEFAULT '',
    created_at   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_autonomy_actions_day ON autonomy_actions(owner, bot_id, outcome, created_at);
