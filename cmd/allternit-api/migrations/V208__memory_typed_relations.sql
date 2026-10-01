-- V208: memory write path, typed relations + memory types (WP-M1b, decision M1).
--
-- Closed sets (kept in sync with memory_relations.rs):
--   relation_type: same | updates | contradicts | causes | caused_by | part_of
--                  | about_entity | follows_in_time | unrelated
--   memory_type:   fact | preference | event | procedure | entity | relationship
--                  | task_state | not_memory
-- SQLite checks the CHECK constraints on every new or updated row.

-- What kind of memory a fact / observation is (NULL = not typed yet: rows
-- written before V208 or by the rule-based fallback).
ALTER TABLE memory_facts ADD COLUMN memory_type TEXT CHECK (memory_type IS NULL OR memory_type IN
    ('fact','preference','event','procedure','entity','relationship','task_state','not_memory'));
ALTER TABLE memory_observations ADD COLUMN memory_type TEXT CHECK (memory_type IS NULL OR memory_type IN
    ('fact','preference','event','procedure','entity','relationship','task_state','not_memory'));

-- memory_relationships becomes the canonical typed edge table. It was
-- entity↔entity only (and had no writer); the endpoint kinds make it carry
-- fact↔fact, observation→fact and fact→entity edges too. `relation` keeps the
-- free-text label for old rows; `relation_type` is the closed set.
ALTER TABLE memory_relationships ADD COLUMN relation_type TEXT CHECK (relation_type IS NULL OR relation_type IN
    ('same','updates','contradicts','causes','caused_by','part_of','about_entity','follows_in_time','unrelated'));
ALTER TABLE memory_relationships ADD COLUMN source_kind TEXT NOT NULL DEFAULT 'entity';
ALTER TABLE memory_relationships ADD COLUMN target_kind TEXT NOT NULL DEFAULT 'entity';
ALTER TABLE memory_relationships ADD COLUMN valid_until DATETIME;
-- incumbent_llm | s1 | user | heuristic
ALTER TABLE memory_relationships ADD COLUMN origin TEXT;
-- S1 shadow decision that judged this edge, when there was one.
ALTER TABLE memory_relationships ADD COLUMN decision_id TEXT;

CREATE INDEX IF NOT EXISTS idx_memory_relationships_user_target
    ON memory_relationships(user_id, target_kind, target_entity_id);
CREATE INDEX IF NOT EXISTS idx_memory_relationships_user_source
    ON memory_relationships(user_id, source_kind, source_entity_id);
CREATE INDEX IF NOT EXISTS idx_memory_facts_type ON memory_facts(user_id, memory_type);

-- S1 shadow decisions made on the memory write path, so later truth (the
-- incumbent's op, a user edit or delete) can be reported against the right
-- decision id via /v1/decision/outcome.
CREATE TABLE IF NOT EXISTS memory_s1_decisions (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    bank TEXT NOT NULL,               -- memory.type | memory.relation
    decision_id TEXT NOT NULL,        -- x-decision_id from the runtime
    observation_id TEXT,
    candidate_fact_id TEXT,           -- RELATION: the existing memory judged
    produced_fact_id TEXT,            -- the fact this turn wrote (if any)
    s1_answer TEXT,
    incumbent_label TEXT,
    user_label TEXT,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_memory_s1_decisions_produced ON memory_s1_decisions(user_id, produced_fact_id);
CREATE INDEX IF NOT EXISTS idx_memory_s1_decisions_obs ON memory_s1_decisions(observation_id);
