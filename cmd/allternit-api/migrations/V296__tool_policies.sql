-- ── Per-user tool permissions (Settings polish, Feature C, step C1) ──────────
-- Additive only. One new table; nothing existing changes.
--
-- One row per (user, connection, tool) the user set to Allow or Block. Ask is
-- the default and is stored as no row (`PUT /tool-policies` with `ask`
-- deletes). `owner_id` keeps the client's form: `connectors:<id>` or
-- `mcps:<id>`. Served by `tool_policy_routes.rs`; enforced by the bridge and
-- the runtime (C2, C3). Order: user Block > bot settings > user Allow >
-- Autopilot > prompt.

CREATE TABLE IF NOT EXISTS tool_policies (
    user_id    TEXT NOT NULL,
    owner_id   TEXT NOT NULL,                      -- 'connectors:<id>' | 'mcps:<id>'
    tool_id    TEXT NOT NULL,                      -- 1-200 characters
    policy     TEXT NOT NULL CHECK (policy IN ('allow', 'ask', 'block')),
    updated_at TEXT NOT NULL,                      -- RFC 3339, UTC
    PRIMARY KEY (user_id, owner_id, tool_id)
);
