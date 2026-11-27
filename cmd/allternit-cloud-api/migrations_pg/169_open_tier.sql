-- 169_open_tier.sql
--
-- Allternit Open (CONTRACT-open-tier-2026-11-26.md §7): the money columns
-- open models need. Additive only; old code keeps running on this schema.
--
--   * user_credits.bought_usd: the part of balance_usd the user paid for
--     outside a plan (packs, auto top-up, store packs, transfers in, dispute
--     releases, comps). A sub-count of balance_usd, never a second balance.
--     Read it as LEAST(bought_usd, GREATEST(balance_usd, 0)); the plan part
--     is GREATEST(balance_usd, 0) minus that (contract D1). A plan grant's
--     rollover cap clips only the plan part (D3).
--   * user_open_allowance: one row per user, reset by each plan grant in the
--     same transaction. allowance_usd = 50% of the grant on Max (`super`) and
--     Ultra, 0 on Plus (D4); used_usd = plan credits open models spent this
--     period (D5). grant_key = the credit_transactions key of that grant.
--   * user_open_lane research_*: the admin-set Open Research flag (D10).
--   * inference_usage lane / open_plan_usd / open_bought_usd: which lane a
--     settled request ran on and how an open request's cost split between
--     plan allowance and bought credits.
--
-- Money columns are double precision, the same type as balance_usd and
-- inference_usage.cost_usd (002 widened `real` to float8), so the sub-count
-- and the balance never round differently and the service reads them as f64.

ALTER TABLE public.user_open_lane ADD COLUMN IF NOT EXISTS research_enabled_at timestamp with time zone;
ALTER TABLE public.user_open_lane ADD COLUMN IF NOT EXISTS research_set_by text;
ALTER TABLE public.user_open_lane ADD COLUMN IF NOT EXISTS research_ref text;

ALTER TABLE public.inference_usage ADD COLUMN IF NOT EXISTS lane text;
ALTER TABLE public.inference_usage ADD COLUMN IF NOT EXISTS open_plan_usd double precision;
ALTER TABLE public.inference_usage ADD COLUMN IF NOT EXISTS open_bought_usd double precision;

CREATE TABLE IF NOT EXISTS public.user_open_allowance (
    user_id text PRIMARY KEY,
    allowance_usd double precision NOT NULL DEFAULT 0,
    used_usd double precision NOT NULL DEFAULT 0,
    period_start timestamp with time zone,
    grant_key text,
    updated_at timestamp with time zone NOT NULL DEFAULT now()
);

-- bought_usd and its one-time backfill (gate G6). The backfill runs only in
-- the transaction that adds the column, so re-running this file is a no-op.
--
--   * No open plan (no billing_subscriptions row in an open status): the
--     whole positive balance was bought.
--   * Open plan: the positive non-plan increases since the last plan grant,
--     capped at the positive balance. Plan grants are the rows written by
--     grant_plan_credits_keyed, sources stripe_subscription (Stripe invoice),
--     apple_subscription and google_subscription (store renewals). Store
--     packs share the apple-/google- key prefixes, so the source, not the
--     key, tells a grant from a pack. Debits since that grant are not
--     subtracted: reads clamp bought to the balance, and this rule never
--     counts a bought dollar as plan money.
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_schema = 'public' AND table_name = 'user_credits' AND column_name = 'bought_usd'
    ) THEN
        ALTER TABLE public.user_credits ADD COLUMN bought_usd double precision NOT NULL DEFAULT 0;

        UPDATE public.user_credits uc
        SET bought_usd = CASE
            WHEN NOT EXISTS (
                SELECT 1 FROM public.billing_subscriptions bs
                WHERE bs.user_id = uc.user_id
                  AND bs.status IN ('active', 'trialing', 'past_due', 'unpaid')
            ) THEN GREATEST(COALESCE(uc.balance_usd, 0), 0)
            ELSE LEAST(
                GREATEST(COALESCE(uc.balance_usd, 0), 0),
                COALESCE((
                    SELECT SUM(ct.amount_usd)
                    FROM public.credit_transactions ct
                    WHERE ct.user_id = uc.user_id
                      AND ct.amount_usd > 0
                      AND COALESCE(ct.source, '') NOT IN ('stripe_subscription', 'apple_subscription', 'google_subscription')
                      AND ct.created_at > COALESCE((
                          SELECT MAX(g.created_at)
                          FROM public.credit_transactions g
                          WHERE g.user_id = uc.user_id
                            AND g.source IN ('stripe_subscription', 'apple_subscription', 'google_subscription')
                      ), '-infinity'::timestamptz)
                ), 0)
            )
        END;
    END IF;
END
$$;
