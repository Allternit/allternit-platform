-- 170_runtime_start_options.sql
-- Start anywhere (2026-11-27, D7). Additive only.
--
-- The folder snapshot a computer pushes while online (PUT
-- /api/v1/runtime-devices/:id/start-options, device credential): recent
-- folders (path, name, git remote/branch/head/dirty), permission modes per
-- coding tool, and the default tool. One row per runtime, replaced on every
-- push. GET /api/v1/start-options reads one owner's rows, so a phone can
-- offer an asleep computer's folders (and move a git folder to a cloud
-- computer).
CREATE TABLE IF NOT EXISTS runtime_start_options (
  runtime_id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL,
  options JSONB NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS runtime_start_options_user_idx
  ON runtime_start_options (user_id);
