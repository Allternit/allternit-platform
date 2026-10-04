-- V234: the digital twin's persona and shared memory (layer 4).
--
-- `twin_profile` is one voice/persona per owner: name, how it speaks, signature, and what
-- every bot may say about the owner. `twin_memory` is the shared knowledge all the owner's bots
-- (and their vendor bots) can read. Every row carries provenance (which bot/channel learned it,
-- when, how sure) and a per-fact visibility. Writes from conversations arrive as `proposed`
-- and only the owner's accept makes them `active`; only `active` rows are ever injected.
CREATE TABLE IF NOT EXISTS twin_profile (
    owner TEXT PRIMARY KEY,
    display_name TEXT NOT NULL DEFAULT '',
    speaking_style TEXT NOT NULL DEFAULT '',
    signature TEXT NOT NULL DEFAULT '',
    owner_disclosure TEXT NOT NULL DEFAULT '',
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS twin_memory (
    id TEXT PRIMARY KEY,
    owner TEXT NOT NULL,
    kind TEXT NOT NULL DEFAULT 'fact',          -- fact | preference | schedule_rule | person | decision
    subject TEXT NOT NULL DEFAULT '',
    content TEXT NOT NULL,
    visibility TEXT NOT NULL DEFAULT 'all',     -- all | bot | owner
    bot_id TEXT,                                -- the one bot, when visibility = 'bot'
    status TEXT NOT NULL DEFAULT 'active',      -- active | proposed
    source TEXT NOT NULL DEFAULT 'owner',       -- owner | bot
    source_bot_id TEXT,
    source_channel TEXT,
    source_thread_id TEXT,
    confidence REAL,
    learned_at TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    reviewed_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_twin_memory_owner ON twin_memory(owner, status, updated_at);
