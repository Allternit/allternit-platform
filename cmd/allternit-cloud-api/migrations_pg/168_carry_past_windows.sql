-- 168_carry_past_windows.sql
--
-- Usage credits toggle (Claude polish C1b, Eoj 2026-11-26 option 2): when
-- true and the credits balance is above 0, a request over an enforced Cloud
-- usage window (5h / 7d) is allowed and billed from the balance as today;
-- when false the window is hard until it resets (HTTP 429
-- usage_window_exhausted).
--
-- Additive only, idempotent (safe to re-run). Old code ignores the column.

ALTER TABLE public.user_credits
    ADD COLUMN IF NOT EXISTS carry_past_windows boolean NOT NULL DEFAULT false;
