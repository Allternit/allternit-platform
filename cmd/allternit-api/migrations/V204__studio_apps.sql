-- Studio apps (W5). WRITTEN, NOT APPLIED to any production DB.
--
-- * studio_apps      : a Mini app made without code in Studio. `doc_json` holds
--   connectors + tool names, view mappings and look tokens (never tokens,
--   secrets or sample results). `visibility` decides who may read it:
--   private = owner only; link / workspace = members of the owner's org
--   (link is reachable only by id, workspace is also listed).
-- * studio_app_adds  : apps a person added from a share link. Adding copies
--   nothing; the person connects their OWN connector accounts.
-- Submission to the directory stays in directory_submissions; `submission_id`
-- only points at it (owner-checked).

CREATE TABLE IF NOT EXISTS studio_apps (
    id             TEXT PRIMARY KEY,
    owner_id       TEXT NOT NULL,
    org_id         TEXT,
    name           TEXT NOT NULL,
    doc_json       TEXT NOT NULL,
    visibility     TEXT NOT NULL DEFAULT 'private'
        CHECK (visibility IN ('private', 'link', 'workspace')),
    submission_id  TEXT,
    created_at     DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at     DATETIME DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_studio_apps_owner ON studio_apps(owner_id);
CREATE INDEX IF NOT EXISTS idx_studio_apps_org ON studio_apps(org_id, visibility);

CREATE TABLE IF NOT EXISTS studio_app_adds (
    user_id   TEXT NOT NULL,
    app_id    TEXT NOT NULL REFERENCES studio_apps(id) ON DELETE CASCADE,
    added_at  DATETIME DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (user_id, app_id)
);
