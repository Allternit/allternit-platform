-- V206: memory-plane index (WP-M1a, decisions O6 + M1).
-- One shared retrieval index for memory facts, entities, observations and
-- document / vector-store chunks: FTS5 keyword index + per-row embeddings
-- tagged with model id and dimension (so a model change re-embeds instead of
-- mixing vectors). Hybrid search fuses both with reciprocal rank fusion.

-- Embedding rows record which model made them and its dimension.
ALTER TABLE memory_embeddings ADD COLUMN dim INTEGER;
UPDATE memory_embeddings
   SET model = COALESCE(model, 'local-hash-384'),
       dim = length(embedding) / 4;
CREATE INDEX IF NOT EXISTS idx_memory_embeddings_user_model
    ON memory_embeddings(user_id, model, target_type);

-- Chunks of memory documents and vector-store files. `scope` is the owner
-- (user id for documents, '__files__' for gateway files).
CREATE TABLE IF NOT EXISTS memory_index_chunks (
    id TEXT PRIMARY KEY,
    scope TEXT NOT NULL,
    source_type TEXT NOT NULL,  -- document | file
    source_id TEXT NOT NULL,
    chunk_index INTEGER NOT NULL,
    text TEXT NOT NULL,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_memory_index_chunks_source
    ON memory_index_chunks(source_type, source_id);

-- Sources that have been chunked (resumable chunking).
CREATE TABLE IF NOT EXISTS memory_index_sources (
    source_type TEXT NOT NULL,
    source_id TEXT NOT NULL,
    chunk_count INTEGER NOT NULL DEFAULT 0,
    indexed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (source_type, source_id)
);

-- Keyword index. rowid = base-table rowid * 8 + type code
-- (1 fact, 2 entity, 3 observation, 4 chunk), so triggers delete by rowid.
-- Deletes also check target_id: if a VACUUM renumbered base rowids the
-- trigger leaves a stale row (filtered out at load time) rather than
-- deleting the wrong one.
CREATE VIRTUAL TABLE IF NOT EXISTS memory_index_fts USING fts5(
    text,
    user_id UNINDEXED,
    target_type UNINDEXED,
    target_id UNINDEXED,
    tokenize = 'porter unicode61'
);

CREATE TRIGGER IF NOT EXISTS memory_index_fact_ai AFTER INSERT ON memory_facts
WHEN new.valid_until IS NULL BEGIN
    INSERT OR REPLACE INTO memory_index_fts(rowid, text, user_id, target_type, target_id)
    VALUES (new.rowid * 8 + 1, new.fact, new.user_id, 'fact', new.id);
END;
CREATE TRIGGER IF NOT EXISTS memory_index_fact_au AFTER UPDATE OF fact, valid_until, user_id ON memory_facts BEGIN
    DELETE FROM memory_index_fts WHERE rowid = old.rowid * 8 + 1 AND target_id = old.id;
    INSERT OR REPLACE INTO memory_index_fts(rowid, text, user_id, target_type, target_id)
    SELECT new.rowid * 8 + 1, new.fact, new.user_id, 'fact', new.id WHERE new.valid_until IS NULL;
END;
CREATE TRIGGER IF NOT EXISTS memory_index_fact_ad AFTER DELETE ON memory_facts BEGIN
    DELETE FROM memory_index_fts WHERE rowid = old.rowid * 8 + 1 AND target_id = old.id;
    DELETE FROM memory_embeddings WHERE target_type = 'fact' AND target_id = old.id;
END;

CREATE TRIGGER IF NOT EXISTS memory_index_entity_ai AFTER INSERT ON memory_entities BEGIN
    INSERT OR REPLACE INTO memory_index_fts(rowid, text, user_id, target_type, target_id)
    VALUES (new.rowid * 8 + 2, new.name || ' ' || new.type || ' ' || COALESCE(new.summary, ''),
            new.user_id, 'entity', new.id);
END;
CREATE TRIGGER IF NOT EXISTS memory_index_entity_au AFTER UPDATE OF name, type, summary, user_id ON memory_entities BEGIN
    DELETE FROM memory_index_fts WHERE rowid = old.rowid * 8 + 2 AND target_id = old.id;
    INSERT OR REPLACE INTO memory_index_fts(rowid, text, user_id, target_type, target_id)
    VALUES (new.rowid * 8 + 2, new.name || ' ' || new.type || ' ' || COALESCE(new.summary, ''),
            new.user_id, 'entity', new.id);
    -- Text changed: drop the stale vector; the indexer re-embeds it.
    DELETE FROM memory_embeddings WHERE target_type = 'entity' AND target_id = old.id;
END;
CREATE TRIGGER IF NOT EXISTS memory_index_entity_ad AFTER DELETE ON memory_entities BEGIN
    DELETE FROM memory_index_fts WHERE rowid = old.rowid * 8 + 2 AND target_id = old.id;
    DELETE FROM memory_embeddings WHERE target_type = 'entity' AND target_id = old.id;
END;

CREATE TRIGGER IF NOT EXISTS memory_index_obs_ai AFTER INSERT ON memory_observations BEGIN
    INSERT OR REPLACE INTO memory_index_fts(rowid, text, user_id, target_type, target_id)
    VALUES (new.rowid * 8 + 3, new.content, new.user_id, 'observation', new.id);
END;
CREATE TRIGGER IF NOT EXISTS memory_index_obs_ad AFTER DELETE ON memory_observations BEGIN
    DELETE FROM memory_index_fts WHERE rowid = old.rowid * 8 + 3 AND target_id = old.id;
    DELETE FROM memory_embeddings WHERE target_type = 'observation' AND target_id = old.id;
END;

CREATE TRIGGER IF NOT EXISTS memory_index_chunk_ai AFTER INSERT ON memory_index_chunks BEGIN
    INSERT OR REPLACE INTO memory_index_fts(rowid, text, user_id, target_type, target_id)
    VALUES (new.rowid * 8 + 4, new.text, new.scope, 'chunk', new.id);
END;
CREATE TRIGGER IF NOT EXISTS memory_index_chunk_ad AFTER DELETE ON memory_index_chunks BEGIN
    DELETE FROM memory_index_fts WHERE rowid = old.rowid * 8 + 4 AND target_id = old.id;
    DELETE FROM memory_embeddings WHERE target_type = 'chunk' AND target_id = old.id;
END;

-- Documents: deleting one drops its chunks and lets it be re-chunked.
CREATE TRIGGER IF NOT EXISTS memory_index_document_ad AFTER DELETE ON memory_documents BEGIN
    DELETE FROM memory_index_chunks WHERE source_type = 'document' AND source_id = old.id;
    DELETE FROM memory_index_sources WHERE source_type = 'document' AND source_id = old.id;
END;
CREATE TRIGGER IF NOT EXISTS memory_index_document_au AFTER UPDATE OF content, title ON memory_documents BEGIN
    DELETE FROM memory_index_chunks WHERE source_type = 'document' AND source_id = old.id;
    DELETE FROM memory_index_sources WHERE source_type = 'document' AND source_id = old.id;
END;

-- Keyword-index backfill of existing rows. Embeddings are backfilled by the
-- background indexer (batched, resumable: it picks rows whose embedding is
-- missing or from another model).
INSERT OR REPLACE INTO memory_index_fts(rowid, text, user_id, target_type, target_id)
SELECT rowid * 8 + 1, fact, user_id, 'fact', id FROM memory_facts WHERE valid_until IS NULL;
INSERT OR REPLACE INTO memory_index_fts(rowid, text, user_id, target_type, target_id)
SELECT rowid * 8 + 2, name || ' ' || type || ' ' || COALESCE(summary, ''), user_id, 'entity', id
FROM memory_entities;
INSERT OR REPLACE INTO memory_index_fts(rowid, text, user_id, target_type, target_id)
SELECT rowid * 8 + 3, content, user_id, 'observation', id FROM memory_observations;
