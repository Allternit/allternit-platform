-- Artifact sharing (promo parity step 6): an artifact is private to its owner,
-- or shared with the owner's organization — view-only or editable. org_id is
-- the owner's org at the time it was shared (sharing follows that org).
ALTER TABLE artifacts ADD COLUMN visibility TEXT NOT NULL DEFAULT 'private';
ALTER TABLE artifacts ADD COLUMN org_access TEXT NOT NULL DEFAULT 'view';
ALTER TABLE artifacts ADD COLUMN org_id TEXT;
CREATE INDEX IF NOT EXISTS idx_artifacts_org_shared ON artifacts(org_id, visibility);
