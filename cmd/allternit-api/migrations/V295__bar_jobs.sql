-- ── Island bar jobs (Island bar + Team map program, PLAN.md §2.2, §2.6, D9) ──
-- Additive only. New tables; nothing existing changes.
--
-- A `bar_jobs` row is the unit of one Island bar run (D9): it exists from the
-- review card on, before any MCP task, and keeps the receipt after it. The MCP
-- task id is only a pointer (`task_id` = the proxy's
-- `<connector_id>:<upstream_task_id>`). The job's engine session is
-- `bar:<job_id>`.

CREATE TABLE IF NOT EXISTS bar_jobs (
    id               TEXT PRIMARY KEY,                -- 'bjb_…'
    user_id          TEXT NOT NULL,                   -- the person who runs it (requester)
    bot_id           TEXT,                            -- agents.id when a bot acts; NULL = the person
    app_kind         TEXT NOT NULL CHECK (app_kind IN ('connector', 'vendor', 'artifact')),
    app_ref          TEXT NOT NULL,
    tool             TEXT NOT NULL,                   -- upstream tool name (not namespaced)
    args_json        TEXT NOT NULL DEFAULT '{}',
    level            TEXT CHECK (level IS NULL OR level IN ('browser', 'app', 'full')),
    rule_id          TEXT,                            -- approval_rules.id that matched at review
    approver_user_id TEXT,
    approval_id      TEXT,                            -- cowork_approvals.id once an ask is open
    task_id          TEXT,                            -- '<connector_id>:<upstream_task_id>'
    session_id       TEXT NOT NULL,                   -- 'bar:<job_id>'
    state            TEXT NOT NULL DEFAULT 'review' CHECK (state IN (
                         'review', 'waiting_approval', 'working', 'input_required',
                         'completed', 'failed', 'cancelled')),
    result_json      TEXT,                            -- receipt: structuredContent first, text second
    error            TEXT,
    created_at       TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at       TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    started_at       TEXT,                            -- first left 'review'
    finished_at      TEXT                             -- reached completed / failed / cancelled
);
CREATE INDEX IF NOT EXISTS idx_bar_jobs_user_state ON bar_jobs(user_id, state, updated_at);
CREATE INDEX IF NOT EXISTS idx_bar_jobs_approver ON bar_jobs(approver_user_id, state);
CREATE INDEX IF NOT EXISTS idx_bar_jobs_approval ON bar_jobs(approval_id);
CREATE INDEX IF NOT EXISTS idx_bar_jobs_task ON bar_jobs(task_id);

-- ── Recent values per blank ────────────────────────────────────────────────
-- Written on run only (lane 17, `bar_routes::record_recent_values`). Never a
-- `format: password`, `writeOnly` or secret-named field. 8 newest per field.
CREATE TABLE IF NOT EXISTS bar_recent_values (
    user_id    TEXT NOT NULL,
    app_kind   TEXT NOT NULL CHECK (app_kind IN ('connector', 'vendor', 'artifact')),
    app_ref    TEXT NOT NULL,
    tool       TEXT NOT NULL,
    field      TEXT NOT NULL,
    value_json TEXT NOT NULL,
    label      TEXT,
    used_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
-- One row per value; a reuse moves `used_at`.
CREATE UNIQUE INDEX IF NOT EXISTS idx_bar_recent_values_value
    ON bar_recent_values(user_id, app_kind, app_ref, tool, field, value_json);
CREATE INDEX IF NOT EXISTS idx_bar_recent_values_field
    ON bar_recent_values(user_id, app_kind, app_ref, tool, field, used_at);
