-- 163: cloud store for user and org Agent Skills (plan v3 lane 47b).
--
-- mcp.allternit.com serves skill://user/... and skill://org/<org_id>/... from
-- this table (routes/mcp_edge.rs, ProdBackend::skills). 161 is in #2921, 162 is app_handoff (#2943).
--
-- Exactly one of owner_user_id / owner_org_id is set. name is the SKILL.md
-- frontmatter `name` (a slug), unique per owner. skill_md is the whole
-- SKILL.md text, frontmatter included; files maps a supporting-file path
-- (references/x.md) to its text. The route caps skill_md + files at 256 KB.
--
-- Additive only, IF NOT EXISTS everywhere, no back-fill, no DO blocks (the
-- test harness splits statements). Account erase deletes the user's own rows.

CREATE TABLE IF NOT EXISTS public.cloud_skills (
    id TEXT PRIMARY KEY,
    owner_user_id TEXT,
    owner_org_id TEXT,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    skill_md TEXT NOT NULL,
    files JSONB NOT NULL DEFAULT '{}'::jsonb,
    version INTEGER NOT NULL DEFAULT 1,
    created_by TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT cloud_skills_one_owner CHECK ((owner_user_id IS NULL) <> (owner_org_id IS NULL))
);

CREATE UNIQUE INDEX IF NOT EXISTS uq_cloud_skills_user_name
    ON public.cloud_skills (owner_user_id, name) WHERE owner_user_id IS NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS uq_cloud_skills_org_name
    ON public.cloud_skills (owner_org_id, name) WHERE owner_org_id IS NOT NULL;
