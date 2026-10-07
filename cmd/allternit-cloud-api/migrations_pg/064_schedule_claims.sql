-- 064_schedule_claims.sql
--
-- Row claims for the schedules poller (audit S12). Both the standalone
-- `allternit-scheduler` daemon (infrastructure/scheduler) and the in-process
-- scheduler in this API claim due rows atomically before firing them:
--
--   UPDATE schedules SET claimed_by = $1, claimed_at = $2
--   WHERE id IN (SELECT id FROM schedules WHERE … next_run_at <= $2
--                  AND (claimed_by IS NULL OR claimed_at < <ttl cutoff>)
--                ORDER BY next_run_at LIMIT $3 FOR UPDATE SKIP LOCKED)
--   RETURNING …
--
-- so two pollers against one database fire each due occurrence once. The
-- claim is cleared when next_run_at is advanced; a claim left by a poller
-- that died mid-fire expires after its TTL.
--
-- Idempotent (IF NOT EXISTS). Additive only: existing rows get NULL claims,
-- which means "unclaimed". No restart ordering needed — but a poller built
-- from this change fails its claim query until the columns exist, so apply
-- this before deploying the new cloud-api binary.

ALTER TABLE public.schedules ADD COLUMN IF NOT EXISTS claimed_by text;
ALTER TABLE public.schedules ADD COLUMN IF NOT EXISTS claimed_at timestamp with time zone;

-- The due-row scan: enabled rows ordered by next_run_at.
CREATE INDEX IF NOT EXISTS idx_schedules_due
    ON public.schedules USING btree (next_run_at)
    WHERE enabled = true AND next_run_at IS NOT NULL;
