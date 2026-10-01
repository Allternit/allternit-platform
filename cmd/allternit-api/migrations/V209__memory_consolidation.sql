-- V209: memory consolidation, store adapters, notes + procedures in the
-- shared index (WP-M1d, decision M1).

-- Retrieval tracking + soft decay on facts. Decay never deletes: it marks a
-- stale, never-retrieved, low-confidence fact (`decayed_at`) and records the
-- score it decayed to. Recall bumps `retrieval_count` / `last_retrieved_at`.
ALTER TABLE memory_facts ADD COLUMN retrieval_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE memory_facts ADD COLUMN last_retrieved_at DATETIME;
ALTER TABLE memory_facts ADD COLUMN decay_score REAL;
ALTER TABLE memory_facts ADD COLUMN decayed_at DATETIME;

-- One row per consolidation run (per user), for idempotency/backoff and the
-- status route.
CREATE TABLE IF NOT EXISTS memory_consolidation_runs (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    started_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    finished_at DATETIME,
    scanned INTEGER NOT NULL DEFAULT 0,
    merged INTEGER NOT NULL DEFAULT 0,
    shadow_decisions INTEGER NOT NULL DEFAULT 0,
    decayed INTEGER NOT NULL DEFAULT 0,
    live_bank TEXT NOT NULL DEFAULT 'incumbent', -- incumbent | s1
    error TEXT
);
CREATE INDEX IF NOT EXISTS idx_memory_consolidation_runs_user
    ON memory_consolidation_runs(user_id, started_at DESC);

-- Store adapters: an external store's item (gizzi memdir file, memory-agent
-- row, ...) maps to one canonical fact. Re-sending an item is an update, so
-- imports are idempotent.
CREATE TABLE IF NOT EXISTS memory_adapter_links (
    user_id TEXT NOT NULL,
    source TEXT NOT NULL,
    external_id TEXT NOT NULL,
    fact_id TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (user_id, source, external_id)
);
CREATE INDEX IF NOT EXISTS idx_memory_adapter_links_fact ON memory_adapter_links(fact_id);

-- Notes (type code 5) and procedures (type code 6) join the shared keyword
-- index (rowid = base rowid * 8 + code, as in V206). The indexer embeds them.
CREATE TRIGGER IF NOT EXISTS memory_index_note_ai AFTER INSERT ON memory_notes BEGIN
    INSERT OR REPLACE INTO memory_index_fts(rowid, text, user_id, target_type, target_id)
    VALUES (new.rowid * 8 + 5, new.title || ': ' || new.content, new.user_id, 'note', new.id);
END;
CREATE TRIGGER IF NOT EXISTS memory_index_note_au AFTER UPDATE OF title, content, user_id ON memory_notes BEGIN
    DELETE FROM memory_index_fts WHERE rowid = old.rowid * 8 + 5 AND target_id = old.id;
    INSERT OR REPLACE INTO memory_index_fts(rowid, text, user_id, target_type, target_id)
    VALUES (new.rowid * 8 + 5, new.title || ': ' || new.content, new.user_id, 'note', new.id);
    DELETE FROM memory_embeddings WHERE target_type = 'note' AND target_id = old.id;
END;
CREATE TRIGGER IF NOT EXISTS memory_index_note_ad AFTER DELETE ON memory_notes BEGIN
    DELETE FROM memory_index_fts WHERE rowid = old.rowid * 8 + 5 AND target_id = old.id;
    DELETE FROM memory_embeddings WHERE target_type = 'note' AND target_id = old.id;
END;

CREATE TRIGGER IF NOT EXISTS memory_index_procedure_ai AFTER INSERT ON procedural_memory BEGIN
    INSERT OR REPLACE INTO memory_index_fts(rowid, text, user_id, target_type, target_id)
    VALUES (new.rowid * 8 + 6, new.name || ': ' || COALESCE(new.description, '') || ' ' || new.trigger_patterns || ' ' || new.steps,
            new.user_id, 'procedure', new.id);
END;
CREATE TRIGGER IF NOT EXISTS memory_index_procedure_au AFTER UPDATE OF name, description, trigger_patterns, steps, user_id ON procedural_memory BEGIN
    DELETE FROM memory_index_fts WHERE rowid = old.rowid * 8 + 6 AND target_id = old.id;
    INSERT OR REPLACE INTO memory_index_fts(rowid, text, user_id, target_type, target_id)
    VALUES (new.rowid * 8 + 6, new.name || ': ' || COALESCE(new.description, '') || ' ' || new.trigger_patterns || ' ' || new.steps,
            new.user_id, 'procedure', new.id);
    DELETE FROM memory_embeddings WHERE target_type = 'procedure' AND target_id = old.id;
END;
CREATE TRIGGER IF NOT EXISTS memory_index_procedure_ad AFTER DELETE ON procedural_memory BEGIN
    DELETE FROM memory_index_fts WHERE rowid = old.rowid * 8 + 6 AND target_id = old.id;
    DELETE FROM memory_embeddings WHERE target_type = 'procedure' AND target_id = old.id;
END;

INSERT OR REPLACE INTO memory_index_fts(rowid, text, user_id, target_type, target_id)
SELECT rowid * 8 + 5, title || ': ' || content, user_id, 'note', id FROM memory_notes;
INSERT OR REPLACE INTO memory_index_fts(rowid, text, user_id, target_type, target_id)
SELECT rowid * 8 + 6, name || ': ' || COALESCE(description, '') || ' ' || trigger_patterns || ' ' || steps, user_id, 'procedure', id
FROM procedural_memory;
