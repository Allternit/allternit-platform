-- D16 (owner, 2026-09-28): every fabric task is human-initiated. The task
-- records who and which human action started it ({kind:"human", user_id,
-- action_id}); POST /v1/tasks rejects submissions without it. NULL only on
-- rows written before this migration.
ALTER TABLE tasks ADD COLUMN initiated_by TEXT;
