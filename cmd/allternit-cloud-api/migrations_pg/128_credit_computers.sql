-- 128: credit computers (compute gate, CONTRACT-compute-gate-2026-11-13 section 6).
--
-- * provisioned_instances.tier gains 'credit': a hosted computer bought with
--   the USD credit wallet, billed per minute while it runs (stopped rate
--   while it is stopped or asleep). One live credit computer per account.
-- * credit_billed_through: the per-minute billing cursor. The tick debits
--   [credit_billed_through, now rounded down to the minute) and advances the
--   cursor in the same transaction, so a repeated tick or a restart never
--   charges the same minute twice.
-- * credit_warned_24h_at / credit_warned_2h_at: when the low-runway warning
--   last fired (cleared when the balance climbs back above the threshold).
-- * At $0 the computer stops: status 'suspended', delete_after = the end of
--   the retention window (14 days by default). credit_delete_warned: the
--   retention warnings already queued (day 7, day 13).
--
-- Idempotent: safe to apply twice. No DO blocks (the test harness splits on ';').

ALTER TABLE public.provisioned_instances
    DROP CONSTRAINT IF EXISTS provisioned_instances_tier_check;
ALTER TABLE public.provisioned_instances
    ADD CONSTRAINT provisioned_instances_tier_check CHECK (tier IN ('free', 'paid', 'credit'));

ALTER TABLE public.provisioned_instances ADD COLUMN IF NOT EXISTS credit_billed_through TIMESTAMPTZ;
ALTER TABLE public.provisioned_instances ADD COLUMN IF NOT EXISTS credit_warned_24h_at TIMESTAMPTZ;
ALTER TABLE public.provisioned_instances ADD COLUMN IF NOT EXISTS credit_warned_2h_at TIMESTAMPTZ;
ALTER TABLE public.provisioned_instances ADD COLUMN IF NOT EXISTS credit_delete_warned INTEGER NOT NULL DEFAULT 0;

CREATE UNIQUE INDEX IF NOT EXISTS idx_provisioned_instances_one_live_credit
    ON public.provisioned_instances(user_id)
    WHERE tier = 'credit'
      AND status IN ('provisioning', 'running', 'stopped', 'sleeping', 'waking', 'suspended');

CREATE INDEX IF NOT EXISTS idx_provisioned_instances_credit_billing
    ON public.provisioned_instances(tier, status)
    WHERE tier = 'credit';
