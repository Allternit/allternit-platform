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

CREATE TRIGGER IF NOT EXISTS memory_entity_fact_ai AFTER INSERT ON memory_facts BEGIN
    INSERT OR IGNORE INTO memory_entity_queue (user_id, target_type, target_id) VALUES (new.user_id, 'fact', new.id);
END;
CREATE TRIGGER IF NOT EXISTS memory_entity_fact_au AFTER UPDATE OF fact, valid_until, user_id ON memory_facts BEGIN
    INSERT OR IGNORE INTO memory_entity_queue (user_id, target_type, target_id) VALUES (old.user_id, 'fact', old.id);
    INSERT OR IGNORE INTO memory_entity_queue (user_id, target_type, target_id) VALUES (new.user_id, 'fact', new.id);
END;
CREATE TRIGGER IF NOT EXISTS memory_entity_fact_ad AFTER DELETE ON memory_facts BEGIN
    INSERT OR IGNORE INTO memory_entity_queue (user_id, target_type, target_id) VALUES (old.user_id, 'fact', old.id);
END;

CREATE TRIGGER IF NOT EXISTS memory_entity_obs_ai AFTER INSERT ON memory_observations BEGIN
    INSERT OR IGNORE INTO memory_entity_queue (user_id, target_type, target_id) VALUES (new.user_id, 'observation', new.id);
END;
CREATE TRIGGER IF NOT EXISTS memory_entity_obs_au AFTER UPDATE OF content, user_id ON memory_observations BEGIN
    INSERT OR IGNORE INTO memory_entity_queue (user_id, target_type, target_id) VALUES (old.user_id, 'observation', old.id);
    INSERT OR IGNORE INTO memory_entity_queue (user_id, target_type, target_id) VALUES (new.user_id, 'observation', new.id);
END;
CREATE TRIGGER IF NOT EXISTS memory_entity_obs_ad AFTER DELETE ON memory_observations BEGIN
    INSERT OR IGNORE INTO memory_entity_queue (user_id, target_type, target_id) VALUES (old.user_id, 'observation', old.id);
END;

CREATE TRIGGER IF NOT EXISTS memory_entity_doc_ai AFTER INSERT ON memory_documents BEGIN
    INSERT OR IGNORE INTO memory_entity_queue (user_id, target_type, target_id) VALUES (new.user_id, 'document', new.id);
END;
CREATE TRIGGER IF NOT EXISTS memory_entity_doc_au AFTER UPDATE OF title, content, user_id ON memory_documents BEGIN
    INSERT OR IGNORE INTO memory_entity_queue (user_id, target_type, target_id) VALUES (old.user_id, 'document', old.id);
    INSERT OR IGNORE INTO memory_entity_queue (user_id, target_type, target_id) VALUES (new.user_id, 'document', new.id);
END;
CREATE TRIGGER IF NOT EXISTS memory_entity_doc_ad AFTER DELETE ON memory_documents BEGIN
    INSERT OR IGNORE INTO memory_entity_queue (user_id, target_type, target_id) VALUES (old.user_id, 'document', old.id);
END;

CREATE TRIGGER IF NOT EXISTS memory_entity_note_ai AFTER INSERT ON memory_notes BEGIN
    INSERT OR IGNORE INTO memory_entity_queue (user_id, target_type, target_id) VALUES (new.user_id, 'note', new.id);
END;
CREATE TRIGGER IF NOT EXISTS memory_entity_note_au AFTER UPDATE OF title, content, user_id ON memory_notes BEGIN
    INSERT OR IGNORE INTO memory_entity_queue (user_id, target_type, target_id) VALUES (old.user_id, 'note', old.id);
    INSERT OR IGNORE INTO memory_entity_queue (user_id, target_type, target_id) VALUES (new.user_id, 'note', new.id);
END;
CREATE TRIGGER IF NOT EXISTS memory_entity_note_ad AFTER DELETE ON memory_notes BEGIN
    INSERT OR IGNORE INTO memory_entity_queue (user_id, target_type, target_id) VALUES (old.user_id, 'note', old.id);
END;

-- memory_entities rows seed names, types and aliases.
CREATE TRIGGER IF NOT EXISTS memory_entity_seed_ai AFTER INSERT ON memory_entities BEGIN
    INSERT OR IGNORE INTO memory_entity_queue (user_id, target_type, target_id) VALUES (new.user_id, 'entity', new.id);
END;
CREATE TRIGGER IF NOT EXISTS memory_entity_seed_au AFTER UPDATE ON memory_entities BEGIN
    INSERT OR IGNORE INTO memory_entity_queue (user_id, target_type, target_id) VALUES (old.user_id, 'entity', old.id);
    INSERT OR IGNORE INTO memory_entity_queue (user_id, target_type, target_id) VALUES (new.user_id, 'entity', new.id);
END;
CREATE TRIGGER IF NOT EXISTS memory_entity_seed_ad AFTER DELETE ON memory_entities BEGIN
    INSERT OR IGNORE INTO memory_entity_queue (user_id, target_type, target_id) VALUES (old.user_id, 'entity', old.id);
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
