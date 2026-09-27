-- One-time memory maintenance steps that have already run (e.g. the prune of
-- facts the old extractor copied verbatim from chat turns).
CREATE TABLE IF NOT EXISTS memory_maintenance (
    key TEXT PRIMARY KEY,
    done_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
