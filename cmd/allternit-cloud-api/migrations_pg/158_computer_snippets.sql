-- 158_computer_snippets.sql
--
-- Computers console (surfaces/docs/dev/computers-console-contract.mdx, D3),
-- step 2.9. 155 is 155_platform_agent_runtime.sql, 156 is
-- 156_computer_ssh_keys.sql, 157 is 157_computer_alerts.sql.
--
-- computer_snippets : a user's saved commands, owner-only. `command` is the
--   shell (Linux, macOS) or PowerShell (Windows) text the node runs.
-- computer_snippet_runs : one row per computer a snippet was run on, so the
--   Activity tab can list snippet runs. The command text is not copied here;
--   the node keeps the output in its change record (change_id).
--
-- Additive only, IF NOT EXISTS everywhere. No DO blocks: the test harness
-- splits statements.

CREATE TABLE IF NOT EXISTS public.computer_snippets (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    name TEXT NOT NULL,
    command TEXT NOT NULL,
    description TEXT,
    elevated BOOLEAN NOT NULL DEFAULT false,
    timeout_sec INTEGER,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_computer_snippets_user
    ON public.computer_snippets (user_id, created_at);

CREATE TABLE IF NOT EXISTS public.computer_snippet_runs (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    snippet_id TEXT NOT NULL,
    snippet_name TEXT NOT NULL,
    computer_id TEXT NOT NULL,
    approval_id TEXT,
    change_id TEXT,
    status TEXT NOT NULL,
    error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_computer_snippet_runs_user
    ON public.computer_snippet_runs (user_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_computer_snippet_runs_computer
    ON public.computer_snippet_runs (user_id, computer_id, created_at DESC);
