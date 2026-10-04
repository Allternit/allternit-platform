-- 040_vendor_bot_keys.sql
--
-- Scoped keys for the `allternit-bot` CLI (channels swarm, g-vendor-reach). A vendor agent that runs in its
-- own sandbox with a shell (no OAuth browser) calls the MCP edge on mcp.allternit.com with
-- `Authorization: Bearer abk_...`. The edge looks the key up here, learns the owner and the one vendor bot
-- it opens, and relays the call to the owner's runtime like an OAuth call. Applied by hand to prod, like 039.
-- Inert until MCP_PUBLIC_URL is set on cloud-api.
--
-- Only the SHA-256 of the key is stored; the plaintext is shown once, when it is issued.

CREATE TABLE IF NOT EXISTS public.vendor_bot_keys (
    id text PRIMARY KEY,
    user_id text NOT NULL,
    vendor_bot_id text NOT NULL,
    label text NOT NULL DEFAULT '',
    key_hash text NOT NULL UNIQUE,
    key_prefix text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    last_used_at timestamptz,
    revoked_at timestamptz
);
CREATE INDEX IF NOT EXISTS idx_vendor_bot_keys_owner ON public.vendor_bot_keys (user_id, vendor_bot_id) WHERE revoked_at IS NULL;
