-- ── Team map (Island bar + Team map program, PLAN.md §2.2) ─────────────────
-- Additive only. Old code keeps running on this schema: every new column is
-- NULL by default and NULL means today's behaviour.
--
-- Owner of a bot stays `agents.user_id`, a sub-bot's parent stays
-- `agents.parent_agent_id` (V1) and a Connected bot's parent stays
-- `vendor_bot_connectors.directing_bot_id` (V220). No agents column is added.

-- Who a member reports to inside the org. NULL = top of the tree.
ALTER TABLE organization_members ADD COLUMN reports_to_user_id TEXT;
CREATE INDEX IF NOT EXISTS idx_org_members_reports_to
    ON organization_members(organization_id, reports_to_user_id);

-- ── Per-bot app grants ─────────────────────────────────────────────────────
-- A bot with no live row cannot call the app (fail closed). A level change
-- revokes the live row and inserts a new one, so the table is the audit trail.
-- Levels reuse #2921's `app_permission_tier` ids.
CREATE TABLE IF NOT EXISTS bot_app_grants (
    id         TEXT PRIMARY KEY,                      -- 'bag_…'
    bot_id     TEXT NOT NULL,                         -- agents.id or vendor_bot_connectors.vendor_bot_id
    bot_kind   TEXT NOT NULL CHECK (bot_kind IN ('agent', 'vendor')),
    app_kind   TEXT NOT NULL CHECK (app_kind IN ('connector', 'vendor', 'artifact')),
    app_ref    TEXT NOT NULL,
    level      TEXT NOT NULL CHECK (level IN ('browser', 'app', 'full')),
    granted_by TEXT NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    revoked_at DATETIME
);
-- One live grant per bot + app; also serves "list by bot".
CREATE UNIQUE INDEX IF NOT EXISTS idx_bot_app_grants_live
    ON bot_app_grants(bot_kind, bot_id, app_kind, app_ref) WHERE revoked_at IS NULL;
-- "Who can use this" (list by app).
CREATE INDEX IF NOT EXISTS idx_bot_app_grants_app
    ON bot_app_grants(app_kind, app_ref) WHERE revoked_at IS NULL;

-- ── Approval rules ─────────────────────────────────────────────────────────
-- Structured rows shown as a sentence (D5). Rules only add approvals; they
-- never remove one (payments always need approval stays).
CREATE TABLE IF NOT EXISTS approval_rules (
    id               TEXT PRIMARY KEY,                -- 'arl_…'
    org_id           TEXT,                            -- NULL = a solo account's rule
    owner_user_id    TEXT NOT NULL,
    scope_kind       TEXT NOT NULL CHECK (scope_kind IN ('org', 'person', 'bot')),
    scope_ref        TEXT,                            -- person: user id; bot: 'b:<agent_id>' | 'v:<vendor_bot_id>'
    app_kind         TEXT CHECK (app_kind IS NULL OR app_kind IN ('connector', 'vendor', 'artifact')),
    app_ref          TEXT,
    tool             TEXT NOT NULL DEFAULT '*',       -- '*' or an exact tool name
    field            TEXT,                            -- tool input field compared by op
    op               TEXT NOT NULL DEFAULT 'always' CHECK (op IN ('always', 'gt', 'gte', 'eq', 'in')),
    value_json       TEXT,
    unit_label       TEXT,
    approver_kind    TEXT NOT NULL CHECK (approver_kind IN ('owner', 'manager', 'user')),
    approver_user_id TEXT,                            -- set when approver_kind = 'user'
    enabled          INTEGER NOT NULL DEFAULT 1,
    created_by       TEXT NOT NULL,
    created_at       DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at       DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_approval_rules_org ON approval_rules(org_id, enabled);
CREATE INDEX IF NOT EXISTS idx_approval_rules_owner ON approval_rules(owner_user_id, enabled);
CREATE INDEX IF NOT EXISTS idx_approval_rules_scope ON approval_rules(scope_kind, scope_ref);

-- ── Owner routing on cowork approvals ──────────────────────────────────────
-- All NULL = today's behaviour (the ask goes to `user_id`).
ALTER TABLE cowork_approvals ADD COLUMN approver_user_id TEXT;
ALTER TABLE cowork_approvals ADD COLUMN rule_id TEXT;
ALTER TABLE cowork_approvals ADD COLUMN bot_id TEXT;
ALTER TABLE cowork_approvals ADD COLUMN job_id TEXT;
CREATE INDEX IF NOT EXISTS idx_cowork_approvals_approver
    ON cowork_approvals(approver_user_id, dismissed);
CREATE INDEX IF NOT EXISTS idx_cowork_approvals_job ON cowork_approvals(job_id);
