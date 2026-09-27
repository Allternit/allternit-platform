-- Local routine scheduler (`src/routine_local_scheduler.rs`).
--
-- Routines with execution_domain = 'local' were stored but never run: the
-- local scheduler the /automation/routines/:id/run handler pointed at did not
-- exist. The API now runs them itself (the desktop API is the user's own
-- machine). next_run_at is the claim cursor: a tick advances it with a
-- compare-and-set before running, so a routine never fires twice.
ALTER TABLE routines ADD COLUMN next_run_at TEXT;
ALTER TABLE routines ADD COLUMN last_run_at TEXT;
CREATE INDEX IF NOT EXISTS idx_routines_local_due ON routines(execution_domain, status, next_run_at);
