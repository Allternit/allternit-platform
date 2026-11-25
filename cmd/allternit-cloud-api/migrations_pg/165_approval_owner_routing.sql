-- 165_approval_owner_routing.sql
--
-- Island bar + Team map, lane 14 (plan handoffs/island-bar-team-map-2026-11-25,
-- section 2.2). Owner routing on `platform_approvals` (migration 122): who must
-- approve the row, which approval rule routed it there, and who asked.
-- All three are NULL on every existing row, which keeps today's behavior: the
-- Platform list (`GET /v1/approvals`) and its resolver ignore them.
-- `GET /api/v1/approvals?approver=me` reads pending rows by
-- `(approver_user_id, status)`.
--
-- Additive and idempotent. Applied automatically before the binary swap by
-- deploy-contabo.sh via the ops_applied_migrations ledger; never edit after it ships.

ALTER TABLE public.platform_approvals
    ADD COLUMN IF NOT EXISTS approver_user_id text,
    ADD COLUMN IF NOT EXISTS rule_id text,
    ADD COLUMN IF NOT EXISTS actor_ref text;

CREATE INDEX IF NOT EXISTS idx_platform_approvals_approver
    ON public.platform_approvals (approver_user_id, status)
    WHERE approver_user_id IS NOT NULL;
