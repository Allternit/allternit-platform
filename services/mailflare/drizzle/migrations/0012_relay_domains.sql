-- Customer domains served by Allternit's own mail host (mx.allternit.com)
-- instead of Cloudflare Email Routing: `transport = 'relay'`. Their mail
-- arrives through POST /api/v1/relay/inbound and is sent through the relay,
-- which signs it with the domain's DKIM key. `zone_id` is 'relay' for them.
ALTER TABLE `domains` ADD `transport` text DEFAULT 'cloudflare' NOT NULL;
