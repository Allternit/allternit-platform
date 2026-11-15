-- V285: loops with execution_domain = 'local' were stored and never run. The
-- API now runs them (routine_local_scheduler): next_run_at is the claim
-- cursor, the same as V185's on routines.
--
-- loop_runs has routine_runs' shape so the run routes and clients read both
-- the same way, but its own table: routine_runs.routine_id references routines.
-- The column keeps the name routine_id for that reason.
ALTER TABLE loops ADD COLUMN next_run_at TEXT;
ALTER TABLE loops ADD COLUMN last_run_at TEXT;
CREATE INDEX IF NOT EXISTS idx_loops_local_due ON loops(execution_domain, status, next_run_at);

CREATE TABLE IF NOT EXISTS loop_runs (
    id TEXT PRIMARY KEY,
    routine_id TEXT NOT NULL,
    gizzi_run_id TEXT,
    status TEXT NOT NULL,
    scheduled_at TEXT NOT NULL,
    started_at TEXT,
    finished_at TEXT,
    duration_ms INTEGER,
    output TEXT,
    error TEXT,
    attempt INTEGER NOT NULL DEFAULT 1,
    triggered_by TEXT NOT NULL DEFAULT 'schedule',
    metadata TEXT,
    outcome_reason TEXT,
    alerted_at TEXT,
    late_by_ms INTEGER,
    FOREIGN KEY (routine_id) REFERENCES loops(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_loop_runs_routine_id ON loop_runs(routine_id);
CREATE INDEX IF NOT EXISTS idx_loop_runs_status ON loop_runs(status);
