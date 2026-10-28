-- Morning brief and first-day "gets to know you" (morning_brief.rs).
--
-- bot_briefs: one brief per user + bot + local day, however many devices ask.
-- The UNIQUE key is the idempotency lock: the first request inserts the row as
-- 'generating'; others see it and wait or read the stored brief.
CREATE TABLE IF NOT EXISTS bot_briefs (
    id            TEXT PRIMARY KEY,
    user_id       TEXT NOT NULL,
    bot_id        TEXT NOT NULL,
    local_date    TEXT NOT NULL,
    tz_offset_min INTEGER NOT NULL DEFAULT 0,
    status        TEXT NOT NULL DEFAULT 'generating', -- generating | ready
    content_json  TEXT,
    session_id    TEXT,
    posted_at     TEXT,
    claimed_at    TEXT NOT NULL,
    created_at    TEXT NOT NULL,
    UNIQUE (user_id, bot_id, local_date)
);

-- bot_intro: "intro done" per user + bot, so the first-day greeting shows once.
-- status: generating | posted (bubbles are in the chat) | answered | skipped
-- (a bot that existed before this feature; no intro is shown).
CREATE TABLE IF NOT EXISTS bot_intro (
    user_id       TEXT NOT NULL,
    bot_id        TEXT NOT NULL,
    local_date    TEXT NOT NULL,
    status        TEXT NOT NULL,
    content_json  TEXT,
    reply         TEXT, -- right | off
    claimed_at    TEXT NOT NULL,
    created_at    TEXT NOT NULL,
    PRIMARY KEY (user_id, bot_id)
);
