-- 140: brain runners, one sealed box per person for their own plan (2026-11-14).
--
-- PLAN-brain-runner-2026-11-14 step 1.1, CONTRACT-connections-2026-11 section 9.
-- A brain runner is a small Incus container on the runner host (`mail` by
-- default) that runs the vendor CLIs (Claude, ChatGPT/Codex, Grok, Kimi)
-- unmodified. The person signs in themselves; only chat messages they send
-- reach it. services/brain_runners.rs owns both tables.
--
-- Runners never use provisioned_hosts placement and never appear in
-- compute-status: their RAM budget is ALLTERNIT_RUNNER_RAM_BUDGET_MB, counted
-- from the rows below whose state is 'creating' or 'awake'.
--
-- Both tables are erased with the account (services/account_erase.rs ERASE).
-- No foreign key to users: the erase list orders deletes itself.
-- Idempotent (IF NOT EXISTS), like every file here.

CREATE TABLE IF NOT EXISTS brain_runners (
    id TEXT PRIMARY KEY,
    -- One box per account.
    user_id TEXT NOT NULL UNIQUE,
    -- Incus instance name (allternit-runner-<slug>-<hash>).
    instance_name TEXT NOT NULL UNIQUE,
    -- Custom volume mounted at /home/runner (the CLI homes, 256 MiB).
    home_volume TEXT NOT NULL,
    -- Host id (ALLTERNIT_RUNNER_HOST) and storage pool it was made on.
    host_id TEXT NOT NULL,
    storage_pool TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'creating'
        CHECK (state IN ('creating', 'awake', 'stopped', 'failed')),
    -- SHA-256 hex of the runner's socket token. The token itself lives only
    -- in the box's environment.
    token_hash TEXT NOT NULL UNIQUE,
    -- Per vendor sign-in state as the runner last reported it:
    -- {"claude": "signed_in", "chatgpt": "connecting", ...}
    -- Values: signed_out | connecting | signed_in | expired | limited.
    vendors JSONB NOT NULL DEFAULT '{}'::jsonb,
    -- Plan names the runner reported ({"claude": "Max"}), shown as `found`.
    vendor_plans JSONB NOT NULL DEFAULT '{}'::jsonb,
    last_error TEXT,
    last_used_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Idle stop (5 minutes) and the 90-day destroy scan by state and last use.
CREATE INDEX IF NOT EXISTS brain_runners_state_last_used_idx
    ON brain_runners (state, last_used_at);

-- The plan disclosure a person accepted (CONTRACT section 9.2, 9.8). Per
-- account, per vendor, per version: a new disclosure version needs a new
-- acceptance. Kept when a vendor is disconnected; erased with the account.
CREATE TABLE IF NOT EXISTS plan_runner_consents (
    user_id TEXT NOT NULL,
    vendor TEXT NOT NULL CHECK (vendor IN ('claude', 'chatgpt', 'grok', 'kimi')),
    disclosure_version TEXT NOT NULL,
    accepted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, vendor, disclosure_version)
);
