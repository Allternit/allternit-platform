-- 166_plan_artifacts.sql
-- Plans as artifacts (artifacts redesign 2026-11-25, D10-D12). Additive only.
--
-- A plan is an artifact of kind 'plan' (id art_plan_<session id>, body = the
-- plan as Markdown; a revised plan is a new version of the same row). Its
-- status lives here so a status change does not add a version. NULL reads as
-- 'open'. Gizzi sets it through PATCH /api/v2/artifacts/:id.
ALTER TABLE artifacts ADD COLUMN IF NOT EXISTS plan_status TEXT
  CHECK (plan_status IS NULL OR plan_status IN ('open', 'approved', 'done'));

-- The Plans filter (GET /api/v2/artifacts?kind=plan) reads one owner's plans
-- newest first; the default list leaves plans out.
CREATE INDEX IF NOT EXISTS artifacts_plan_owner_updated_idx
  ON artifacts (owner_id, updated_at DESC)
  WHERE kind = 'plan';
