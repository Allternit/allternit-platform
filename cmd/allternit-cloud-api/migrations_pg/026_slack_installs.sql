-- 026: Slack shared app (one Allternit app, HTTP Events API at the cloud).
--
-- slack_installs: one row per Slack workspace (team) that installed the shared
-- Allternit app via OAuth v2. The bot token is sealed with the platform
-- credential cipher (empty string = plaintext-at-rest, dev only, matching
-- provider_tokens). runtime_id is the user's Allternit runtime (cloud computer
-- or Desktop) that events are relayed to; it is set when the runtime claims
-- the install (POST /api/v1/channels/slack/installs/:team_id/claim) and may be
-- NULL while events queue.
CREATE TABLE IF NOT EXISTS slack_installs (
    team_id        TEXT PRIMARY KEY,
    team_name      TEXT NOT NULL DEFAULT '',
    user_id        TEXT NOT NULL,
    bot_user_id    TEXT NOT NULL DEFAULT '',
    app_id         TEXT NOT NULL DEFAULT '',
    scopes         TEXT NOT NULL DEFAULT '',
    bot_token      TEXT NOT NULL DEFAULT '',
    refresh_token  TEXT NOT NULL DEFAULT '',
    expires_at     TIMESTAMPTZ,
    runtime_id     TEXT,
    installed_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_slack_installs_user ON slack_installs(user_id);

-- Slack Events API events that arrived at the cloud and wait for delivery to
-- the installing user's runtime (by team_id -> slack_installs). Same delivery
-- discipline as channel_inbound_queue: ack Slack within 3s, deliver in order,
-- retry with backoff for 24 hours.
CREATE TABLE IF NOT EXISTS slack_event_queue (
    id             BIGSERIAL PRIMARY KEY,
    team_id        TEXT NOT NULL,
    api_app_id     TEXT NOT NULL DEFAULT '',
    event_id       TEXT NOT NULL DEFAULT '',
    payload        JSONB NOT NULL,
    received_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    delivered_at   TIMESTAMPTZ,
    dead_at        TIMESTAMPTZ,
    locked_until   TIMESTAMPTZ,
    attempts       INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_status    INTEGER,
    last_error     TEXT
);
CREATE INDEX IF NOT EXISTS idx_slack_event_queue_team ON slack_event_queue(team_id, delivered_at);
