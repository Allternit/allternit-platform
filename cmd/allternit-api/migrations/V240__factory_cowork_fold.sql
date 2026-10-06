-- Allternit Factory: the cowork queue folds into workspace nodes (stream F9).
--
-- Cowork tasks and queue items are now nodes in the Factory ledger, written
-- through the Gate (src/cowork_nodes.rs). The `tasks` table stays as a read
-- model for comments, audit logs and older readers; `cowork_queue` keeps
-- done/failed history only.
--
-- The fold of existing open rows into nodes runs once, from code, at startup
-- (`cowork_nodes::fold_once`). It can't be SQL because nodes live in the
-- ledger. Each real run is recorded here with its counts, which is also what
-- makes it run only once. `allternit-api cowork-fold --dry-run` prints the
-- same report without writing anything.
CREATE TABLE IF NOT EXISTS factory_cowork_fold_runs (
    id       TEXT PRIMARY KEY,
    dry_run  INTEGER NOT NULL DEFAULT 0,
    skipped  INTEGER NOT NULL DEFAULT 0,
    report   TEXT NOT NULL,
    ran_at   DATETIME DEFAULT CURRENT_TIMESTAMP
);
