-- Saved bot and team templates (Bot Mode "Save as template", Templates →
-- Mine / Org). `template` is the recipe JSON: a BotTemplate, or {team, bots}
-- for a team. No credentials, memory or threads (stripped on save).
CREATE TABLE IF NOT EXISTS bot_templates (
    id          TEXT PRIMARY KEY,
    user_id     TEXT NOT NULL,
    kind        TEXT NOT NULL,          -- bot | team
    visibility  TEXT NOT NULL DEFAULT 'private',  -- private | org
    forked_from TEXT,                   -- attribution when copied from another template
    template    TEXT NOT NULL,
    created_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_bot_templates_user ON bot_templates(user_id, created_at);
