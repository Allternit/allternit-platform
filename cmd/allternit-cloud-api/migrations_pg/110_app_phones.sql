-- iPhone via the App Store app (SPEC-iphone-app-screen-2026-10-27 §1).
-- One row per phone the Allternit iOS app paired to one of the account's
-- runtimes. `key_hash` is the SHA-256 (hex) of the 32-byte app phone key the
-- broadcast extension presents (`x-allternit-app-phone-key`); the key itself
-- is never stored. Re-registering a phone id rotates the key; revoking sets
-- `revoked_at` and every later ticket request answers 401.
-- Renumber at merge if main has moved past 109.
CREATE TABLE IF NOT EXISTS public.app_phones (
    id uuid PRIMARY KEY,
    account text NOT NULL,
    runtime_device_id text NOT NULL,
    name text,
    key_hash text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    last_seen_at timestamptz,
    revoked_at timestamptz
);

CREATE INDEX IF NOT EXISTS idx_app_phones_account
    ON public.app_phones (account);
