-- ── Per-skill on/off switches for skills from MCP servers (mcp-skills-app 2026-11-26) ──
-- Additive only. One new table; nothing existing changes.
--
-- A row overrides the skill's default switch (`defaultEnabled` in the catalog)
-- either way: enabled = 0 turns a default-on skill off, 1 turns a default-off
-- skill on. No row = the default. `skill_id` is the catalog id
-- `<connector tool prefix>:<skill>` (mcp_skills.rs).

CREATE TABLE IF NOT EXISTS mcp_skill_prefs (
    user_id    TEXT NOT NULL,
    skill_id   TEXT NOT NULL,
    enabled    INTEGER NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (user_id, skill_id)
);
