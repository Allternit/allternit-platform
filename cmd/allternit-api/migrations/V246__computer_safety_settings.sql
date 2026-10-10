-- Computer-use safety settings (computer_safety): per-computer and
-- owner-wide app/domain allow and deny lists, extra watch-mode entries, the
-- monitor switch, screenshot redaction mode and kinds, and rollback
-- snapshots. scope = 'default' (all of the owner's computers) or
-- 'computer:<id>'; the computer row overrides the default field by field.
CREATE TABLE IF NOT EXISTS computer_safety_settings (
    owner TEXT NOT NULL,
    scope TEXT NOT NULL,
    settings_json TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (owner, scope)
);
