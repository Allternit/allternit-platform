-- Agent network WP10: per-bot failover. The heartbeat now carries one
-- {botId, failoverRuntimeId} per always-on bot; a channel route belongs to a bot;
-- a failover is recorded per bot. Additive: the WP7 column stays for old runtimes.
ALTER TABLE public.runtime_devices
    ADD COLUMN IF NOT EXISTS bot_failovers jsonb NOT NULL DEFAULT '[]'::jsonb;
ALTER TABLE public.channel_inbound_routes
    ADD COLUMN IF NOT EXISTS bot_id text;
ALTER TABLE public.runtime_failovers
    ADD COLUMN IF NOT EXISTS bot_id text;
-- At most one open failover per home runtime and bot.
DROP INDEX IF EXISTS public.idx_runtime_failovers_open;
CREATE UNIQUE INDEX IF NOT EXISTS idx_runtime_failovers_open_bot
    ON public.runtime_failovers (from_runtime, COALESCE(bot_id, '')) WHERE ended_at IS NULL;
