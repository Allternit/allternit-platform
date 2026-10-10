-- 087_motion_render_jobs.sql
--
-- Artifacts v2 gap-motion: cloud MP4 render for Motion artifacts. One row per
-- render job; the renderer runs on the cloud-api host (a Node child process
-- that draws the composition and pipes frames to ffmpeg). Routes:
-- src/routes/artifact_render.rs under /api/v2/artifacts/:id/render.
--
-- * status     queued | running | done | failed | canceled | expired
-- * storage    'r2' (object in R2) or 'local' (file in the host temp dir);
--              NULL until the file exists. Files live 24 hours (expires_at).
--
-- Idempotent (IF NOT EXISTS). Apply by hand to prod (ALLTERNIT_SKIP_MIGRATIONS=1).

CREATE TABLE IF NOT EXISTS motion_render_jobs (
  id             TEXT PRIMARY KEY,                -- 'rj_' + ULID
  artifact_id    TEXT NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
  version        INTEGER NOT NULL,                -- the artifact version rendered
  user_id        TEXT NOT NULL,                   -- who asked (Clerk user id)
  status         TEXT NOT NULL DEFAULT 'queued',
  progress       REAL NOT NULL DEFAULT 0,         -- 0..1
  width          INTEGER NOT NULL,                -- encoded size (even)
  height         INTEGER NOT NULL,
  fps            INTEGER NOT NULL,
  frames         INTEGER NOT NULL,
  error_code     TEXT,
  error_message  TEXT,
  storage        TEXT,
  output_key     TEXT,                            -- R2 key, or the file name in the job's temp dir
  output_bytes   BIGINT,
  created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
  started_at     TIMESTAMPTZ,
  finished_at    TIMESTAMPTZ,
  expires_at     TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS motion_render_jobs_user_status_idx ON motion_render_jobs(user_id, status);
CREATE INDEX IF NOT EXISTS motion_render_jobs_artifact_idx ON motion_render_jobs(artifact_id, created_at DESC);
CREATE INDEX IF NOT EXISTS motion_render_jobs_expiry_idx ON motion_render_jobs(expires_at) WHERE output_key IS NOT NULL;
