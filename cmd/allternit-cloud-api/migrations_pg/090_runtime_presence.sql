-- Agent network WP4: runtime presence. The runtime heartbeats every 60 s with
-- the routines due in the next 24 h; the cloud (awake when the Mac is not)
-- raises attention items when scheduled work is missed or messages wait.
ALTER TABLE public.runtime_devices
    ADD COLUMN IF NOT EXISTS last_heartbeat_at timestamptz,
    ADD COLUMN IF NOT EXISTS relay_disconnected_at timestamptz,
    ADD COLUMN IF NOT EXISTS next_due jsonb NOT NULL DEFAULT '[]'::jsonb;

CREATE TABLE IF NOT EXISTS public.runtime_presence_alerts (
    id text PRIMARY KEY,
    runtime_id text NOT NULL,
    user_id text NOT NULL,
    kind text NOT NULL,
    summary text NOT NULL DEFAULT '',
    opened_at timestamptz NOT NULL DEFAULT now(),
    resolved_at timestamptz
);
-- At most one open alert per runtime and kind.
CREATE UNIQUE INDEX IF NOT EXISTS idx_runtime_presence_alerts_open
    ON public.runtime_presence_alerts (runtime_id, kind) WHERE resolved_at IS NULL;
