-- Office floor Autopilot, digest and last-seen (2026-11-20, workspace overhaul step 4).
--
-- Additive only: three new tables, CREATE ... IF NOT EXISTS. Read and written by
-- cmd/allternit-api/src/floor_autopilot.rs (`/api/v1/floor/autopilot|digest|seen`).
-- This migration stores the switch and the rules; nothing acts on a need yet (the
-- evaluator is a later step), so an enabled switch changes no behaviour on its own.
-- API reference: surfaces/docs/api/floor.mdx.

-- The user's Autopilot switch. No row = off. main_bot_id NULL = the floor's main bot.
CREATE TABLE IF NOT EXISTS autopilot_state (
    user_id     TEXT PRIMARY KEY,
    enabled     INTEGER NOT NULL DEFAULT 0,
    main_bot_id TEXT,
    updated_at  TEXT NOT NULL
);

-- One rule per (scope, need). PUT /floor/autopilot replaces a user's whole set in one
-- transaction. The key is (user_id, id) so a client-chosen id can never collide with
-- another user's rule. position keeps the order the user saved.
-- scope_type: all | bot | pod (scope_id is NULL for all)
-- need:       suggestion | permission | question
-- level:      draft | ask | tell | limits
CREATE TABLE IF NOT EXISTS autopilot_rules (
    id                   TEXT NOT NULL,
    user_id              TEXT NOT NULL,
    position             INTEGER NOT NULL DEFAULT 0,
    scope_type           TEXT NOT NULL CHECK (scope_type IN ('all', 'bot', 'pod')),
    scope_id             TEXT,
    need                 TEXT NOT NULL CHECK (need IN ('suggestion', 'permission', 'question')),
    level                TEXT NOT NULL DEFAULT 'ask' CHECK (level IN ('draft', 'ask', 'tell', 'limits')),
    max_per_day          INTEGER,
    allowed_actions_json TEXT,
    note                 TEXT,
    created_at           TEXT NOT NULL,
    updated_at           TEXT NOT NULL,
    PRIMARY KEY (user_id, id)
);
CREATE INDEX IF NOT EXISTS idx_autopilot_rules_user ON autopilot_rules(user_id);

-- When the user last looked at the floor, on any device (drives the digest window).
CREATE TABLE IF NOT EXISTS floor_seen (
    user_id      TEXT PRIMARY KEY,
    last_seen_at TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);
