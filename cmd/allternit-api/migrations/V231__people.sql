-- V231: one person across every channel (digital twin layer 1, src/people.rs).
--
-- A `people` row is one human the owner talks to. `person_identities` are the ways
-- that human shows up (a Telegram user id, a phone number on SMS and WhatsApp, an
-- email, a Slack user...). An inbound message resolves its sender to a person; the
-- person has one thread per bot (`person_threads`, role 'main'), and the chat
-- threads that existed before people did hang under it as role 'sub'.
CREATE TABLE IF NOT EXISTS people (
    id TEXT PRIMARY KEY,
    owner TEXT NOT NULL,
    display_name TEXT NOT NULL,
    -- 'auto' (made up from the first message) | 'contact' (imported) | 'owner' (typed by the owner)
    name_source TEXT NOT NULL DEFAULT 'auto',
    avatar TEXT,
    notes TEXT,
    org TEXT,
    -- the provider the owner pinned replies to; NULL = wherever the person last wrote
    reply_pin TEXT,
    -- set when this person was merged into another; the row stays so old references resolve
    merged_into TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_people_owner ON people(owner, merged_into);

CREATE TABLE IF NOT EXISTS person_identities (
    id TEXT PRIMARY KEY,
    owner TEXT NOT NULL,
    person_id TEXT NOT NULL,
    -- the channel it was seen on (sms, whatsapp, telegram, slack, discord, teams, email, allternit)
    provider TEXT NOT NULL,
    -- what kind of handle it is: phone | email | telegram | slack | discord | teams | whatsapp | allternit
    kind TEXT NOT NULL,
    -- normalised: E.164 for phones, lower-case for emails
    external_id TEXT NOT NULL,
    label TEXT,
    confidence REAL NOT NULL DEFAULT 1.0,
    -- inbound | exact_match | contact | invite | manual | proposal
    source TEXT NOT NULL,
    last_seen_at TEXT,
    created_at TEXT NOT NULL,
    UNIQUE (owner, provider, external_id)
);
CREATE INDEX IF NOT EXISTS idx_person_identities_person ON person_identities(person_id);
CREATE INDEX IF NOT EXISTS idx_person_identities_key ON person_identities(owner, kind, external_id);

-- Likely matches the owner confirms or dismisses (same name and organisation).
CREATE TABLE IF NOT EXISTS person_link_proposals (
    id TEXT PRIMARY KEY,
    owner TEXT NOT NULL,
    person_a TEXT NOT NULL,
    person_b TEXT NOT NULL,
    reason TEXT NOT NULL,
    score REAL NOT NULL DEFAULT 0.6,
    -- open | accepted | dismissed
    status TEXT NOT NULL DEFAULT 'open',
    created_at TEXT NOT NULL,
    UNIQUE (owner, person_a, person_b)
);

CREATE TABLE IF NOT EXISTS person_merges (
    id TEXT PRIMARY KEY,
    owner TEXT NOT NULL,
    into_id TEXT NOT NULL,
    from_id TEXT NOT NULL,
    from_name TEXT NOT NULL,
    identity_ids TEXT NOT NULL,
    created_at TEXT NOT NULL
);

-- Which bot threads belong to which person. 'main' = the person's one thread for
-- that bot; 'sub' = a chat thread from before people existed (or a merged person's
-- old main thread), parented under the main thread.
CREATE TABLE IF NOT EXISTS person_threads (
    thread_id TEXT PRIMARY KEY,
    owner TEXT NOT NULL,
    person_id TEXT NOT NULL,
    bot_id TEXT NOT NULL,
    role TEXT NOT NULL DEFAULT 'main',
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_person_threads_person ON person_threads(owner, person_id, bot_id);

-- What a bot knows about a person, shared by every channel the person uses.
CREATE TABLE IF NOT EXISTS person_facts (
    id TEXT PRIMARY KEY,
    owner TEXT NOT NULL,
    person_id TEXT NOT NULL,
    bot_id TEXT,
    fact TEXT NOT NULL,
    source_thread_id TEXT,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_person_facts_person ON person_facts(owner, person_id, created_at);
