-- 018: cloud computer provisioning (plan PLAN-cloud-computer-provisioning-2026-09-30, A3/B/C1).
--
-- * plan_tiers gains the cloud computer base + burst size per subscription
--   plan. Super and Ultra share the 'team' quota tier, so sizing rows are
--   keyed by the subscription plan id (plus/super/ultra =
--   billing_subscriptions.plan_id), not by the quota tier.
-- * provisioned_instances gains the cancel lifecycle (suspended -> deleted
--   after 30 days, snapshot image kept 6 months) and one live instance per
--   subscription at the database level (Stripe retries race).
--
-- Idempotent: safe to apply twice. No DO blocks (the test harness splits on ';').

ALTER TABLE public.plan_tiers ADD COLUMN IF NOT EXISTS computer_base_vcpu INTEGER;
ALTER TABLE public.plan_tiers ADD COLUMN IF NOT EXISTS computer_base_memory_mb BIGINT;
ALTER TABLE public.plan_tiers ADD COLUMN IF NOT EXISTS computer_base_disk_gb BIGINT;
ALTER TABLE public.plan_tiers ADD COLUMN IF NOT EXISTS computer_burst_vcpu INTEGER;
ALTER TABLE public.plan_tiers ADD COLUMN IF NOT EXISTS computer_burst_memory_mb BIGINT;

INSERT INTO public.plan_tiers (
    id, display_name,
    computer_base_vcpu, computer_base_memory_mb, computer_base_disk_gb,
    computer_burst_vcpu, computer_burst_memory_mb
) VALUES
    ('plus',  'Plus',  2, 4096,  20, 4,  8192),
    ('super', 'Super', 4, 8192,  40, 8,  16384),
    ('ultra', 'Ultra', 8, 16384, 80, 16, 32768)
ON CONFLICT (id) DO UPDATE SET
    computer_base_vcpu = EXCLUDED.computer_base_vcpu,
    computer_base_memory_mb = EXCLUDED.computer_base_memory_mb,
    computer_base_disk_gb = EXCLUDED.computer_base_disk_gb,
    computer_burst_vcpu = EXCLUDED.computer_burst_vcpu,
    computer_burst_memory_mb = EXCLUDED.computer_burst_memory_mb;

-- 'suspended' = subscription cancelled: instance stopped, snapshot taken,
-- deleted at delete_after (cancel + 30 days).
ALTER TABLE public.provisioned_instances
    DROP CONSTRAINT IF EXISTS provisioned_instances_status_check;
ALTER TABLE public.provisioned_instances
    ADD CONSTRAINT provisioned_instances_status_check
    CHECK (status IN ('provisioning', 'running', 'stopped', 'suspended', 'error', 'deleted'));

ALTER TABLE public.provisioned_instances ADD COLUMN IF NOT EXISTS plan_id TEXT;
ALTER TABLE public.provisioned_instances ADD COLUMN IF NOT EXISTS cancelled_at TIMESTAMPTZ;
ALTER TABLE public.provisioned_instances ADD COLUMN IF NOT EXISTS delete_after TIMESTAMPTZ;
-- Incus image alias (on host_id's host) published from the cancel-time disk
-- snapshot. Images outlive the instance; instance snapshots do not.
ALTER TABLE public.provisioned_instances ADD COLUMN IF NOT EXISTS snapshot_image TEXT;
ALTER TABLE public.provisioned_instances ADD COLUMN IF NOT EXISTS snapshot_expires_at TIMESTAMPTZ;
ALTER TABLE public.provisioned_instances ADD COLUMN IF NOT EXISTS snapshot_deleted_at TIMESTAMPTZ;
ALTER TABLE public.provisioned_instances ADD COLUMN IF NOT EXISTS restored_from TEXT;

-- Instance names are now deterministic per (user, subscription), so a
-- retired row must not block re-creating the same name: uniqueness only
-- among non-deleted rows.
ALTER TABLE public.provisioned_instances
    DROP CONSTRAINT IF EXISTS provisioned_instances_host_id_incus_name_key;
CREATE UNIQUE INDEX IF NOT EXISTS idx_provisioned_instances_live_name
    ON public.provisioned_instances(host_id, incus_name)
    WHERE status <> 'deleted';

-- One live instance per subscription (concurrent webhook deliveries).
CREATE UNIQUE INDEX IF NOT EXISTS idx_provisioned_instances_one_live_per_subscription
    ON public.provisioned_instances(subscription_id)
    WHERE subscription_id IS NOT NULL
      AND status IN ('provisioning', 'running', 'stopped', 'suspended');

CREATE INDEX IF NOT EXISTS idx_provisioned_instances_lifecycle
    ON public.provisioned_instances(status, delete_after);
