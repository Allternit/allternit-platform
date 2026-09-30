-- MCP App directory backend (P3b). WRITTEN, NOT APPLIED to any production DB.
--
-- * developer_domain_tokens : per-developer, per-host domain-verification
--   challenge. Only the SHA-256 of the token is stored; the token itself is
--   returned once at issue time and is what the developer serves at
--   /.well-known/allternit-apps-challenge.
-- * directory_submissions   : one row per listing. `form_json` / `package_zip`
--   are the LIVE listing; `pending_*` hold an update to an approved/published
--   listing until an admin approves it (held update). `approved_snapshot_json`
--   is the approved tool-metadata snapshot (ListingSnapshot) so held updates
--   survive restarts; `candidate_snapshot_json` is the submitted one.
-- * directory_reviewer_credentials : reviewer credentials, kept apart from the
--   package and sealed with token_crypto. Never selected by any list query.
-- * mcp_app_installs        : user-scoped MCP App installs + permission mode.

CREATE TABLE IF NOT EXISTS developer_domain_tokens (
    id           TEXT PRIMARY KEY,
    user_id      TEXT NOT NULL,
    host         TEXT NOT NULL,
    token_hash   TEXT NOT NULL,
    created_at   DATETIME DEFAULT CURRENT_TIMESTAMP,
    verified_at  DATETIME,
    UNIQUE (user_id, host)
);

CREATE TABLE IF NOT EXISTS directory_submissions (
    id                       TEXT PRIMARY KEY,
    user_id                  TEXT NOT NULL,
    name                     TEXT NOT NULL,
    developer                TEXT NOT NULL,
    state                    TEXT NOT NULL DEFAULT 'in_review'
        CHECK (state IN ('draft', 'in_review', 'approved', 'rejected', 'published')),
    version                  INTEGER NOT NULL DEFAULT 1,
    form_json                TEXT NOT NULL,
    package_zip              TEXT NOT NULL,
    findings_json            TEXT NOT NULL DEFAULT '[]',
    rejection_reason         TEXT,
    approved_snapshot_json   TEXT,
    candidate_snapshot_json  TEXT,
    pending_form_json        TEXT,
    pending_package_zip      TEXT,
    created_at               DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at               DATETIME DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_directory_submissions_user ON directory_submissions(user_id);
CREATE INDEX IF NOT EXISTS idx_directory_submissions_state ON directory_submissions(state);

CREATE TABLE IF NOT EXISTS directory_reviewer_credentials (
    submission_id  TEXT PRIMARY KEY REFERENCES directory_submissions(id) ON DELETE CASCADE,
    sealed         TEXT NOT NULL,
    created_at     DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at     DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS mcp_app_installs (
    user_id          TEXT NOT NULL,
    app_id           TEXT NOT NULL,
    connector_id     TEXT NOT NULL,
    permission_mode  TEXT NOT NULL DEFAULT 'ask_before_changes'
        CHECK (permission_mode IN ('always_ask', 'ask_before_changes', 'ask_before_important_changes')),
    installed_at     DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at       DATETIME DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (user_id, app_id)
);
CREATE INDEX IF NOT EXISTS idx_mcp_app_installs_connector ON mcp_app_installs(connector_id);
