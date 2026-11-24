-- 131_meeting_recap_email.sql
-- Allternit Meetings step 2.9: the recap email. It goes through the
-- customer email path (services/customer_emails.rs), which claims
-- (user_id, kind, ref) in customer_emails first, so a recap is sent at most
-- once per person per session (ref = the meeting session id). The kind list
-- only needs the new value; idempotent.

ALTER TABLE public.customer_emails DROP CONSTRAINT IF EXISTS customer_emails_kind_check;
ALTER TABLE public.customer_emails ADD CONSTRAINT customer_emails_kind_check
    CHECK (kind IN ('welcome', 'plan_started', 'computer_ready', 'auto_topup_failed', 'meeting_recap'));
