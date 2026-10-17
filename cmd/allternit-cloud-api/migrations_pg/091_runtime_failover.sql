-- Agent network WP7: always-on failover. The runtime reports the cloud computer
-- its always-on bots fail over to (in each heartbeat); while the home runtime is
-- offline the cloud delivers queued channel messages there instead, and records
-- each switch so the runtime can write it to the bots' threads when it is back.
ALTER TABLE public.runtime_devices
    ADD COLUMN IF NOT EXISTS failover_runtime_id text;

CREATE TABLE IF NOT EXISTS public.runtime_failovers (
    id text PRIMARY KEY,
    user_id text NOT NULL,
    from_runtime text NOT NULL,
    to_runtime text NOT NULL,
    reason text NOT NULL DEFAULT 'home runtime offline',
    started_at timestamptz NOT NULL DEFAULT now(),
    ended_at timestamptz,
    reported_at timestamptz
);
-- At most one open failover per home runtime.
CREATE UNIQUE INDEX IF NOT EXISTS idx_runtime_failovers_open
    ON public.runtime_failovers (from_runtime) WHERE ended_at IS NULL;
