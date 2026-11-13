-- 129: compute gate (CONTRACT-compute-gate-2026-11-13 sections 3 and 6).
--
-- * account_compute_acknowledgements: one row per account that confirmed
--   "Just chat for now" (POST /api/v1/account/compute-status/acknowledge).
--   It never lifts the gate for agent entry; clients use it to show a small
--   banner instead of the full-screen gate. Repeat calls keep the first time.
-- * customer_emails gains the credit computer kinds and a 'queued' status:
--   warnings are recorded as 'queued' while ALLTERNIT_CREDIT_COMPUTER_EMAILS
--   is off (the copy waits for Eoj's approval) and sent when it is on.
--
-- Idempotent: safe to apply twice. No DO blocks (the test harness splits on ';').

CREATE TABLE IF NOT EXISTS public.account_compute_acknowledgements (
    user_id         TEXT PRIMARY KEY,
    choice          TEXT NOT NULL CHECK (choice IN ('chat_only')),
    acknowledged_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

ALTER TABLE public.customer_emails DROP CONSTRAINT IF EXISTS customer_emails_kind_check;
ALTER TABLE public.customer_emails ADD CONSTRAINT customer_emails_kind_check
    CHECK (kind IN ('welcome', 'plan_started', 'computer_ready', 'auto_topup_failed',
                    'credit_low_24h', 'credit_low_2h', 'credit_stopped', 'credit_delete_warning'));

ALTER TABLE public.customer_emails DROP CONSTRAINT IF EXISTS customer_emails_status_check;
ALTER TABLE public.customer_emails ADD CONSTRAINT customer_emails_status_check
    CHECK (status IN ('sent', 'failed', 'skipped', 'queued'));
