-- Inbound webhook triggers that work end to end (2026-11-03).
--
-- verify:    how a delivery proves where it came from. 'allternit' (HMAC of the
--            body with the trigger secret in X-Allternit-Signature), 'github',
--            'stripe', 'linear', 'shopify' (each vendor's own signature, checked
--            with the secret the vendor gave or was given), or 'address' (no
--            signature: the private public address is the key; only accepted
--            when cloud-api relayed it, signed with this runtime's device token).
-- route_id:  the cloud-api channel_inbound_routes row whose public address
--            (https://api.allternit.com/channels/in/<key>) relays here, to
--            /webhooks/inbound/route/<route_id>.
-- public_url: that address, kept so the trigger page can show it again.
-- The plaintext `secret` column (V87/V88 lineage, nullable) now holds the
-- signing secret on this runtime so HMACs are checked without the sender
-- ever sending the secret itself.
ALTER TABLE webhook_triggers ADD COLUMN verify TEXT NOT NULL DEFAULT 'allternit';
ALTER TABLE webhook_triggers ADD COLUMN route_id TEXT;
ALTER TABLE webhook_triggers ADD COLUMN public_url TEXT;
CREATE INDEX IF NOT EXISTS idx_webhook_triggers_route ON webhook_triggers(route_id);

-- The bot thread a delivery started.
ALTER TABLE webhook_trigger_deliveries ADD COLUMN thread_id TEXT;
