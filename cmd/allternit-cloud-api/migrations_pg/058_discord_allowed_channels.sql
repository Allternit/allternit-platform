-- 058_discord_allowed_channels.sql
--
-- The channels in a Discord server the bots may answer in. NULL = every channel
-- (the behaviour before this column existed). Messages from other channels are
-- dropped at the cloud and never reach the owner's runtime; DMs and slash
-- commands are not filtered (both are addressed to the app on purpose).
--
-- Idempotent (IF NOT EXISTS).

ALTER TABLE public.discord_installs ADD COLUMN IF NOT EXISTS allowed_channel_ids text[];
