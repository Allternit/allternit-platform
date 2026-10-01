-- 019: free sleeping cloud computer (plan PLAN-cloud-computer-provisioning-2026-09-30,
-- decision 16).
--
-- * provisioned_instances.tier: 'paid' (per subscription, always on, the
--   Desktop image) or 'free' (one per account, runtime-only image, sleeps
--   when idle and wakes on a request or a scheduled job).
-- * Two new statuses: 'sleeping' (free computer stopped by the idle sweep;
--   costs only its disk) and 'waking' (start issued, not yet running).
-- * Activity stamps for the idle sweep and the free retention rule, the
--   runtime-reported next scheduled job, sleep/wake counters, and the paid
--   computer that replaced a free one on upgrade.
-- * provisioned_instance_wakes: one row per wake, for the per-user hourly
--   wake limit.
--
-- Idempotent: safe to apply twice. No DO blocks (the test harness splits on ';').

ALTER TABLE public.provisioned_instances ADD COLUMN IF NOT EXISTS tier TEXT NOT NULL DEFAULT 'paid';
ALTER TABLE public.provisioned_instances
    DROP CONSTRAINT IF EXISTS provisioned_instances_tier_check;
ALTER TABLE public.provisioned_instances
    ADD CONSTRAINT provisioned_instances_tier_check CHECK (tier IN ('free', 'paid'));

ALTER TABLE public.provisioned_instances
    DROP CONSTRAINT IF EXISTS provisioned_instances_status_check;
ALTER TABLE public.provisioned_instances
    ADD CONSTRAINT provisioned_instances_status_check
    CHECK (status IN ('provisioning', 'running', 'stopped', 'sleeping', 'waking', 'suspended', 'error', 'deleted'));

-- Last user-driven traffic through the relay (or a runtime-reported busy
-- signal). The idle sweep sleeps a free computer when this is older than the
-- idle window. Runtime heartbeats deliberately do not move it.
ALTER TABLE public.provisioned_instances ADD COLUMN IF NOT EXISTS last_activity_at TIMESTAMPTZ;
-- Last time the owner used the computer (relay traffic, a wake by the owner,
-- the create call). Free retention deletes after N days without it;
-- scheduled wakes do not count.
ALTER TABLE public.provisioned_instances ADD COLUMN IF NOT EXISTS last_owner_activity_at TIMESTAMPTZ;
-- Next scheduled job inside the runtime (reported by the runtime); the sweep
-- wakes the computer shortly before it.
ALTER TABLE public.provisioned_instances ADD COLUMN IF NOT EXISTS next_wake_at TIMESTAMPTZ;
ALTER TABLE public.provisioned_instances ADD COLUMN IF NOT EXISTS sleep_count BIGINT NOT NULL DEFAULT 0;
ALTER TABLE public.provisioned_instances ADD COLUMN IF NOT EXISTS wake_count BIGINT NOT NULL DEFAULT 0;
ALTER TABLE public.provisioned_instances ADD COLUMN IF NOT EXISTS last_slept_at TIMESTAMPTZ;
ALTER TABLE public.provisioned_instances ADD COLUMN IF NOT EXISTS last_woken_at TIMESTAMPTZ;
-- Set on a free computer when the account upgrades: the paid computer that
-- replaced it. The free one is kept sleeping until delete_after.
ALTER TABLE public.provisioned_instances ADD COLUMN IF NOT EXISTS replaced_by TEXT;

-- One live free computer per account (racing create calls).
CREATE UNIQUE INDEX IF NOT EXISTS idx_provisioned_instances_one_live_free
    ON public.provisioned_instances(user_id)
    WHERE tier = 'free'
      AND status IN ('provisioning', 'running', 'stopped', 'sleeping', 'waking', 'suspended');

CREATE INDEX IF NOT EXISTS idx_provisioned_instances_free_sweep
    ON public.provisioned_instances(tier, status, last_activity_at);
CREATE INDEX IF NOT EXISTS idx_provisioned_instances_device
    ON public.provisioned_instances(device_id);

CREATE TABLE IF NOT EXISTS public.provisioned_instance_wakes (
    id          TEXT PRIMARY KEY,
    instance_id TEXT NOT NULL REFERENCES public.provisioned_instances(id) ON DELETE CASCADE,
    user_id     TEXT NOT NULL,
    -- 'owner' (wake endpoint), 'relay' (request to the sleeping runtime),
    -- 'schedule' (scheduled job due).
    reason      TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_provisioned_instance_wakes_user_time
    ON public.provisioned_instance_wakes(user_id, created_at);
