-- Files drive: decline a waiting key request, and an index for the unlock
-- rate limit (contracts/files-drive/SPEC.md sections 12 and 13). Additive
-- only: old code keeps running on this schema.
--
-- `DELETE /api/v1/files-keys/requests/:device_id` (a device that holds the
-- key declines a waiting request) ends the request as state 'replaced' and
-- records who declined it and when here, so the state CHECK stays as it is.
ALTER TABLE files_key_requests ADD COLUMN IF NOT EXISTS declined_at TIMESTAMPTZ;
ALTER TABLE files_key_requests ADD COLUMN IF NOT EXISTS declined_by TEXT;

-- `POST /api/v1/computer-files/:id/unlock` counts this user's
-- `files.unlock.issued` audit rows in the last 10 minutes. audit_log had no
-- index for that, so every unlock scanned the table. Partial, so it holds
-- only unlock rows.
DO $$
BEGIN
    IF to_regclass('public.audit_log') IS NOT NULL THEN
        CREATE INDEX IF NOT EXISTS idx_audit_log_files_unlock
            ON public.audit_log (user_id, "timestamp") WHERE action = 'files.unlock.issued';
    END IF;
END $$;
