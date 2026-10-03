-- 025_discord_installs.sql
--
-- Discord shared app. One Allternit Discord application is added to a user's
-- server with "Add to Discord"; the bot token lives only in cloud-api env.
--
-- discord_installs : guild -> (user, runtime). The OAuth callback writes it;
--                    the gateway and interactions look the owner up by guild.
--                    route_id is the channel_inbound_routes row (provider
--                    'discord_app') events are queued under for the runtime.
-- discord_webhooks : the app-owned webhook cloud-api created in a channel, so
--                    each Allternit bot can speak with its own name and avatar.
--                    Only the webhook id is kept: the token is re-read from
--                    Discord with the bot token when needed.

CREATE TABLE IF NOT EXISTS public.discord_installs (
    guild_id text PRIMARY KEY,
    user_id text NOT NULL,
    runtime_id text NOT NULL,
    route_id text REFERENCES public.channel_inbound_routes(id) ON DELETE SET NULL,
    guild_name text,
    installed_by_discord_id text,
    permissions text,
    installed_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    revoked_at timestamptz
);
CREATE INDEX IF NOT EXISTS idx_discord_installs_user ON public.discord_installs (user_id);
CREATE INDEX IF NOT EXISTS idx_discord_installs_discord_user
    ON public.discord_installs (installed_by_discord_id) WHERE revoked_at IS NULL;

CREATE TABLE IF NOT EXISTS public.discord_webhooks (
    channel_id text PRIMARY KEY,
    guild_id text NOT NULL,
    webhook_id text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_discord_webhooks_guild ON public.discord_webhooks (guild_id);
