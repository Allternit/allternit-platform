-- 086_artifact_presence.sql
--
-- Artifacts v2: durable, instance-shared presence ("who else is viewing or
-- editing this artifact"). Replaces the in-process map in
-- src/routes/artifacts_comments.rs. A heartbeat upserts last_seen; readers
-- only see rows seen in the last 45 s; rows older than 10 min are swept
-- opportunistically by the route.
--
-- Idempotent (IF NOT EXISTS). Apply by hand to prod (ALLTERNIT_SKIP_MIGRATIONS=1).

CREATE TABLE IF NOT EXISTS artifact_presence (
  artifact_id  TEXT NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
  user_id      TEXT NOT NULL,
  name         TEXT,
  image_url    TEXT,
  state        TEXT NOT NULL DEFAULT 'viewing',   -- viewing | editing
  last_seen    TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (artifact_id, user_id)
);
CREATE INDEX IF NOT EXISTS artifact_presence_seen_idx ON artifact_presence(last_seen);
