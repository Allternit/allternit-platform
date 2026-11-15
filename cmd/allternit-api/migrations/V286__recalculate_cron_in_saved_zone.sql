-- Loops read their cron line in a saved zone too; none saved means this runtime's.
ALTER TABLE loops ADD COLUMN timezone TEXT;

-- Cron lines now read in the task's saved timezone. Clear the cursor so the
-- scheduler works out each active local cron routine's next run again.
UPDATE routines SET next_run_at = NULL
 WHERE execution_domain = 'local' AND status = 'active' AND schedule_type = 'cron'
   AND timezone IS NOT NULL AND timezone <> '';
