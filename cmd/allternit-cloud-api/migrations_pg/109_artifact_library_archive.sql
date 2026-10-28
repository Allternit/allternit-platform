-- A viewer's library organization never changes ownership, sharing or links.
CREATE TABLE IF NOT EXISTS artifact_library_archives (
  user_id TEXT NOT NULL,
  artifact_id TEXT NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
  archived_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (user_id, artifact_id)
);
