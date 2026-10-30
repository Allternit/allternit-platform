-- 112_phone_minutes.sql
--
-- Platform API phone runs (routes/platform_v1/phones.rs). Phones stay on the owner's Desktop; the
-- cloud only keeps what it needs to meter, cap and report the runs a project's key started.
-- (111 is taken by 111_apns_device_tokens.sql, so this is 112.)
--
-- platform_phone_runs       : one row per task or skill run started through /v1/phones or the MCP phone
--                             tools. `ended_at` is set when the Desktop's phone.run.completed arrives;
--                             open rows count against the plan's concurrent phone-run limit (rows older
--                             than 2 hours stop counting, so a lost completion can't pin capacity).
-- platform_phone_fleet_runs : POST /v1/phones/runs, one run per phone. `failures` holds the phones whose
--                             run did not start ({serial, code, message}).
-- platform_projects.phone_run_cap_override : Enterprise (or any project) concurrent phone-run cap.
--
-- Meters `phone_minutes` and `linked_phone` are plain strings in platform_usage_events (no DDL).

CREATE TABLE IF NOT EXISTS platform_phone_fleet_runs (
    id text PRIMARY KEY,
    project_id text NOT NULL,
    key_id text,
    kind text NOT NULL,
    goal text,
    skill text,
    failures jsonb NOT NULL DEFAULT '[]'::jsonb,
    created_at timestamptz NOT NULL DEFAULT now(),
    completed_at timestamptz
);

CREATE INDEX IF NOT EXISTS platform_phone_fleet_runs_project ON platform_phone_fleet_runs (project_id, created_at DESC);

CREATE TABLE IF NOT EXISTS platform_phone_runs (
    project_id text NOT NULL,
    run_id text NOT NULL,
    owner_user_id text NOT NULL,
    key_id text,
    fleet_id text,
    serial text,
    kind text NOT NULL,
    status text NOT NULL DEFAULT 'running',
    ok boolean,
    started_at timestamptz NOT NULL DEFAULT now(),
    ended_at timestamptz,
    PRIMARY KEY (project_id, run_id)
);

CREATE INDEX IF NOT EXISTS platform_phone_runs_open ON platform_phone_runs (project_id) WHERE ended_at IS NULL;
CREATE INDEX IF NOT EXISTS platform_phone_runs_owner ON platform_phone_runs (owner_user_id, run_id);
CREATE INDEX IF NOT EXISTS platform_phone_runs_fleet ON platform_phone_runs (fleet_id) WHERE fleet_id IS NOT NULL;

ALTER TABLE platform_projects ADD COLUMN IF NOT EXISTS phone_run_cap_override integer;
