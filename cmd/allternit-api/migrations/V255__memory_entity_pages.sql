-- V255: entity layer for agentic search (C3, CorpusMap pattern).
--
-- Mentions of the same entity are resolved across a user's memory (facts,
-- observations, documents, notes; memory_entities rows seed names and
-- aliases) without LLM calls, and every recurring entity gets a page that
-- aggregates what is known about it and links to everything that mentions
-- it. `memory_entity_pages.rs` keeps these tables; everything is per user.

-- Targets whose text changed since they were last read (kept by the
-- triggers below, drained by the entity pass of the background indexer).
CREATE TABLE IF NOT EXISTS memory_entity_queue (
    user_id TEXT NOT NULL,
    target_type TEXT NOT NULL,   -- fact | observation | document | note | entity
    target_id TEXT NOT NULL,
    queued_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (user_id, target_type, target_id)
);

-- Mentions by normalized surface key (not by entity id), so resolution can
-- change without re-reading any text.
CREATE TABLE IF NOT EXISTS memory_entity_mentions (
    user_id TEXT NOT NULL,
    key TEXT NOT NULL,
    kind TEXT NOT NULL,          -- name | email | url | handle | code | <seed type>
    surface TEXT NOT NULL,
    target_type TEXT NOT NULL,
    target_id TEXT NOT NULL,
    count INTEGER NOT NULL,
    first_pos INTEGER NOT NULL,  -- byte offset of the first mention
    PRIMARY KEY (user_id, key, target_type, target_id)
);
CREATE INDEX IF NOT EXISTS idx_memory_entity_mentions_target
    ON memory_entity_mentions(user_id, target_type, target_id);

-- Alias evidence: "Allternit Platform (AP)", "X aka Y", a seed's aliases.
CREATE TABLE IF NOT EXISTS memory_entity_links (
    user_id TEXT NOT NULL,
    key_a TEXT NOT NULL,
    key_b TEXT NOT NULL,
    target_type TEXT NOT NULL,
    target_id TEXT NOT NULL,
    PRIMARY KEY (user_id, key_a, key_b, target_type, target_id)
);
CREATE INDEX IF NOT EXISTS idx_memory_entity_links_target
    ON memory_entity_links(user_id, target_type, target_id);

-- Resolution: surface key -> entity id.
CREATE TABLE IF NOT EXISTS memory_entity_keys (
    user_id TEXT NOT NULL,
    key TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    PRIMARY KEY (user_id, key)
);
CREATE INDEX IF NOT EXISTS idx_memory_entity_keys_entity ON memory_entity_keys(user_id, entity_id);

-- Keys whose mentions or links changed since pages were last rebuilt.
CREATE TABLE IF NOT EXISTS memory_entity_dirty (
    user_id TEXT NOT NULL,
    key TEXT NOT NULL,
    PRIMARY KEY (user_id, key)
);

CREATE TABLE IF NOT EXISTS memory_entity_pages (
    id TEXT PRIMARY KEY,         -- ent_<sha256(user, canonical key)>
    user_id TEXT NOT NULL,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    aliases TEXT NOT NULL DEFAULT '[]',
    memory_entity_id TEXT,
    mention_count INTEGER NOT NULL DEFAULT 0,
    target_count INTEGER NOT NULL DEFAULT 0,
    page_json TEXT NOT NULL,
    page_text TEXT NOT NULL,
    page_hash TEXT NOT NULL,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_memory_entity_pages_user ON memory_entity_pages(user_id, target_count DESC);

-- No conflict clause (OR IGNORE) inside these triggers: an outer UPSERT or
-- OR REPLACE statement overrides a trigger's conflict policy, so a duplicate
-- queue row would abort the caller's write. NOT EXISTS never conflicts.
CREATE TRIGGER IF NOT EXISTS memory_entity_fact_ai AFTER INSERT ON memory_facts BEGIN
    INSERT INTO memory_entity_queue (user_id, target_type, target_id)
    SELECT s.u, 'fact', s.i FROM (SELECT new.user_id AS u, new.id AS i) s
    WHERE NOT EXISTS (SELECT 1 FROM memory_entity_queue e WHERE e.user_id = s.u AND e.target_type = 'fact' AND e.target_id = s.i);
END;
CREATE TRIGGER IF NOT EXISTS memory_entity_fact_au AFTER UPDATE OF fact, valid_until, user_id ON memory_facts BEGIN
    INSERT INTO memory_entity_queue (user_id, target_type, target_id)
    SELECT s.u, 'fact', s.i FROM (SELECT old.user_id AS u, old.id AS i UNION SELECT new.user_id AS u, new.id AS i) s
    WHERE NOT EXISTS (SELECT 1 FROM memory_entity_queue e WHERE e.user_id = s.u AND e.target_type = 'fact' AND e.target_id = s.i);
END;
CREATE TRIGGER IF NOT EXISTS memory_entity_fact_ad AFTER DELETE ON memory_facts BEGIN
    INSERT INTO memory_entity_queue (user_id, target_type, target_id)
    SELECT s.u, 'fact', s.i FROM (SELECT old.user_id AS u, old.id AS i) s
    WHERE NOT EXISTS (SELECT 1 FROM memory_entity_queue e WHERE e.user_id = s.u AND e.target_type = 'fact' AND e.target_id = s.i);
END;

CREATE TRIGGER IF NOT EXISTS memory_entity_obs_ai AFTER INSERT ON memory_observations BEGIN
    INSERT INTO memory_entity_queue (user_id, target_type, target_id)
    SELECT s.u, 'observation', s.i FROM (SELECT new.user_id AS u, new.id AS i) s
    WHERE NOT EXISTS (SELECT 1 FROM memory_entity_queue e WHERE e.user_id = s.u AND e.target_type = 'observation' AND e.target_id = s.i);
END;
CREATE TRIGGER IF NOT EXISTS memory_entity_obs_au AFTER UPDATE OF content, user_id ON memory_observations BEGIN
    INSERT INTO memory_entity_queue (user_id, target_type, target_id)
    SELECT s.u, 'observation', s.i FROM (SELECT old.user_id AS u, old.id AS i UNION SELECT new.user_id AS u, new.id AS i) s
    WHERE NOT EXISTS (SELECT 1 FROM memory_entity_queue e WHERE e.user_id = s.u AND e.target_type = 'observation' AND e.target_id = s.i);
END;
CREATE TRIGGER IF NOT EXISTS memory_entity_obs_ad AFTER DELETE ON memory_observations BEGIN
    INSERT INTO memory_entity_queue (user_id, target_type, target_id)
    SELECT s.u, 'observation', s.i FROM (SELECT old.user_id AS u, old.id AS i) s
    WHERE NOT EXISTS (SELECT 1 FROM memory_entity_queue e WHERE e.user_id = s.u AND e.target_type = 'observation' AND e.target_id = s.i);
END;

CREATE TRIGGER IF NOT EXISTS memory_entity_doc_ai AFTER INSERT ON memory_documents BEGIN
    INSERT INTO memory_entity_queue (user_id, target_type, target_id)
    SELECT s.u, 'document', s.i FROM (SELECT new.user_id AS u, new.id AS i) s
    WHERE NOT EXISTS (SELECT 1 FROM memory_entity_queue e WHERE e.user_id = s.u AND e.target_type = 'document' AND e.target_id = s.i);
END;
CREATE TRIGGER IF NOT EXISTS memory_entity_doc_au AFTER UPDATE OF title, content, user_id ON memory_documents BEGIN
    INSERT INTO memory_entity_queue (user_id, target_type, target_id)
    SELECT s.u, 'document', s.i FROM (SELECT old.user_id AS u, old.id AS i UNION SELECT new.user_id AS u, new.id AS i) s
    WHERE NOT EXISTS (SELECT 1 FROM memory_entity_queue e WHERE e.user_id = s.u AND e.target_type = 'document' AND e.target_id = s.i);
END;
CREATE TRIGGER IF NOT EXISTS memory_entity_doc_ad AFTER DELETE ON memory_documents BEGIN
    INSERT INTO memory_entity_queue (user_id, target_type, target_id)
    SELECT s.u, 'document', s.i FROM (SELECT old.user_id AS u, old.id AS i) s
    WHERE NOT EXISTS (SELECT 1 FROM memory_entity_queue e WHERE e.user_id = s.u AND e.target_type = 'document' AND e.target_id = s.i);
END;

CREATE TRIGGER IF NOT EXISTS memory_entity_note_ai AFTER INSERT ON memory_notes BEGIN
    INSERT INTO memory_entity_queue (user_id, target_type, target_id)
    SELECT s.u, 'note', s.i FROM (SELECT new.user_id AS u, new.id AS i) s
    WHERE NOT EXISTS (SELECT 1 FROM memory_entity_queue e WHERE e.user_id = s.u AND e.target_type = 'note' AND e.target_id = s.i);
END;
CREATE TRIGGER IF NOT EXISTS memory_entity_note_au AFTER UPDATE OF title, content, user_id ON memory_notes BEGIN
    INSERT INTO memory_entity_queue (user_id, target_type, target_id)
    SELECT s.u, 'note', s.i FROM (SELECT old.user_id AS u, old.id AS i UNION SELECT new.user_id AS u, new.id AS i) s
    WHERE NOT EXISTS (SELECT 1 FROM memory_entity_queue e WHERE e.user_id = s.u AND e.target_type = 'note' AND e.target_id = s.i);
END;
CREATE TRIGGER IF NOT EXISTS memory_entity_note_ad AFTER DELETE ON memory_notes BEGIN
    INSERT INTO memory_entity_queue (user_id, target_type, target_id)
    SELECT s.u, 'note', s.i FROM (SELECT old.user_id AS u, old.id AS i) s
    WHERE NOT EXISTS (SELECT 1 FROM memory_entity_queue e WHERE e.user_id = s.u AND e.target_type = 'note' AND e.target_id = s.i);
END;

-- memory_entities rows seed names, types and aliases.
CREATE TRIGGER IF NOT EXISTS memory_entity_seed_ai AFTER INSERT ON memory_entities BEGIN
    INSERT INTO memory_entity_queue (user_id, target_type, target_id)
    SELECT s.u, 'entity', s.i FROM (SELECT new.user_id AS u, new.id AS i) s
    WHERE NOT EXISTS (SELECT 1 FROM memory_entity_queue e WHERE e.user_id = s.u AND e.target_type = 'entity' AND e.target_id = s.i);
END;
CREATE TRIGGER IF NOT EXISTS memory_entity_seed_au AFTER UPDATE ON memory_entities BEGIN
    INSERT INTO memory_entity_queue (user_id, target_type, target_id)
    SELECT s.u, 'entity', s.i FROM (SELECT old.user_id AS u, old.id AS i UNION SELECT new.user_id AS u, new.id AS i) s
    WHERE NOT EXISTS (SELECT 1 FROM memory_entity_queue e WHERE e.user_id = s.u AND e.target_type = 'entity' AND e.target_id = s.i);
END;
CREATE TRIGGER IF NOT EXISTS memory_entity_seed_ad AFTER DELETE ON memory_entities BEGIN
    INSERT INTO memory_entity_queue (user_id, target_type, target_id)
    SELECT s.u, 'entity', s.i FROM (SELECT old.user_id AS u, old.id AS i) s
    WHERE NOT EXISTS (SELECT 1 FROM memory_entity_queue e WHERE e.user_id = s.u AND e.target_type = 'entity' AND e.target_id = s.i);
END;

-- Backfill: everything that exists now is read once.
INSERT OR IGNORE INTO memory_entity_queue (user_id, target_type, target_id)
    SELECT user_id, 'entity', id FROM memory_entities;
INSERT OR IGNORE INTO memory_entity_queue (user_id, target_type, target_id)
    SELECT user_id, 'fact', id FROM memory_facts WHERE valid_until IS NULL;
INSERT OR IGNORE INTO memory_entity_queue (user_id, target_type, target_id)
    SELECT user_id, 'observation', id FROM memory_observations;
INSERT OR IGNORE INTO memory_entity_queue (user_id, target_type, target_id)
    SELECT user_id, 'document', id FROM memory_documents;
INSERT OR IGNORE INTO memory_entity_queue (user_id, target_type, target_id)
    SELECT user_id, 'note', id FROM memory_notes;
