-- 134: Plus-plan computer hours and overage (Eoj, 2026-11-13).
--
-- A plan's included computer (Plus) gets a number of running hours per UTC
-- calendar month (ALLTERNIT_PLUS_INCLUDED_HOURS, default 200). Hours past
-- that bill from the credit wallet at the credit computer running rate,
-- per minute, when ALLTERNIT_PLAN_OVERAGE=1. Metering always runs.
--
-- * plan_computer_hours: one meter row per plan computer per month. Running
--   seconds come from provisioned_instance_usage_sessions, clipped to the
--   month. charged_minutes + waived_minutes is the overage cursor: minutes
--   past the included hours that are already settled (charged, or waived
--   while the flag was off). The tick moves it and writes the ledger row
--   (credit_transactions, source 'plan_computer_overage') in one transaction
--   under a row lock, so a repeat or a restart never charges a minute twice.
-- * provisioned_instances.plan_overage_stopped_at: set when a plan computer
--   past its hours stopped at $0 (status 'suspended', nothing deleted). It
--   is released to stopped/sleeping when the next month starts, credits are
--   added, or the flag is turned off.
-- * customer_emails gains 'plan_hours_stopped'.
--
-- Idempotent: safe to apply twice. No DO blocks (the test harness splits on ';').

CREATE TABLE IF NOT EXISTS public.plan_computer_hours (
    instance_id      TEXT NOT NULL,
    period_start     TIMESTAMPTZ NOT NULL,
    user_id          TEXT NOT NULL,
    plan_id          TEXT,
    included_seconds BIGINT NOT NULL DEFAULT 0,
    running_seconds  BIGINT NOT NULL DEFAULT 0,
    metered_through  TIMESTAMPTZ NOT NULL,
    charged_minutes  BIGINT NOT NULL DEFAULT 0,
    waived_minutes   BIGINT NOT NULL DEFAULT 0,
    charged_usd      DOUBLE PRECISION NOT NULL DEFAULT 0,
    warned_24h_at    TIMESTAMPTZ,
    warned_2h_at     TIMESTAMPTZ,
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (instance_id, period_start)
);

CREATE INDEX IF NOT EXISTS idx_plan_computer_hours_user
    ON public.plan_computer_hours(user_id, period_start);

ALTER TABLE public.provisioned_instances ADD COLUMN IF NOT EXISTS plan_overage_stopped_at TIMESTAMPTZ;

ALTER TABLE public.customer_emails DROP CONSTRAINT IF EXISTS customer_emails_kind_check;
ALTER TABLE public.customer_emails ADD CONSTRAINT customer_emails_kind_check
    CHECK (kind IN ('welcome', 'plan_started', 'computer_ready', 'auto_topup_failed', 'meeting_recap',
                    'credit_low_24h', 'credit_low_2h', 'credit_stopped', 'credit_delete_warning',
                    'plan_hours_stopped'));
