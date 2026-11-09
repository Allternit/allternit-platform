-- Stable per-machine identity for runtime pairing. The client generates a random
-- machine id once and sends it with every pairing; it replaces hostname|platform
-- as the key for the active-machine cap (hostname stays the fallback for old rows
-- and clients that do not send one). Re-pairing the same account on the same
-- machine and runtime type replaces the old row instead of adding a second one.
ALTER TABLE public.runtime_pairings ADD COLUMN IF NOT EXISTS machine_id text;
ALTER TABLE public.runtime_devices ADD COLUMN IF NOT EXISTS machine_id text;
CREATE UNIQUE INDEX IF NOT EXISTS uq_runtime_devices_user_machine
    ON public.runtime_devices (user_id, machine_id, runtime_type)
    WHERE revoked_at IS NULL AND machine_id IS NOT NULL;
