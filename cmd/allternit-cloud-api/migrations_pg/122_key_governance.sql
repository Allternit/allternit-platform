-- 122_key_governance.sql
--
-- Key governance for the console (Waves 1–6, lane C0).
--
-- * api_keys.expires_at: a key past it is refused with 401 (both key-auth
--   paths: services::api_keys::authenticate_api_key_at and
--   platform_v1::caller::authenticate_from). NULL = never expires.
-- * api_keys.allowed_cidrs: client IPs (from `X-Real-IP` set by the local proxy; see console_governance
--   header) the key may be used from; anything else is a 403. NULL or empty =
--   no restriction. Console sessions are not keys and are never restricted.
-- * api_keys.kind: `standard` (every existing key) or `playground` (the
--   console playground's short-lived key; its usage rows say client
--   `playground`).
-- * audit_log gains organization_id and project_id so the console audit page
--   can scope it (ip_address already exists, see 001_initial.sql). The table
--   comes from the 001 snapshot, so the column adds are skipped where it does
--   not exist (test schemas).
-- * platform_org_policies: one row per Clerk organization with its key and
--   network rules and the spend alert address.
--
-- Additive only, idempotent (safe to re-run).

ALTER TABLE public.api_keys ADD COLUMN IF NOT EXISTS expires_at timestamp with time zone;
ALTER TABLE public.api_keys ADD COLUMN IF NOT EXISTS allowed_cidrs text[];
ALTER TABLE public.api_keys ADD COLUMN IF NOT EXISTS kind text NOT NULL DEFAULT 'standard';

ALTER TABLE IF EXISTS public.audit_log ADD COLUMN IF NOT EXISTS organization_id text;
ALTER TABLE IF EXISTS public.audit_log ADD COLUMN IF NOT EXISTS project_id text;

DO $$
BEGIN
    IF to_regclass('public.audit_log') IS NOT NULL THEN
        CREATE INDEX IF NOT EXISTS idx_audit_log_org_time
            ON public.audit_log (organization_id, "timestamp") WHERE organization_id IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_audit_log_project_time
            ON public.audit_log (project_id, "timestamp") WHERE project_id IS NOT NULL;
    END IF;
END $$;

CREATE TABLE IF NOT EXISTS public.platform_org_policies (
    org_id text PRIMARY KEY,
    require_key_expiry boolean NOT NULL DEFAULT false,
    max_key_ttl_days integer,
    live_keys_admin_only boolean NOT NULL DEFAULT false,
    ip_allowlist text[],
    spend_alert_email text,
    created_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP
);
