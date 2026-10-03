-- Microsoft Teams shared app (ao-teams, 2026-10-02): the runtime-side record
-- of a user's connection through Allternit's shared Teams app (single-tenant
-- Azure Bot + multi-tenant Entra app; per cloud-api migrations_pg 027).
-- One row per owner; account_id points at the provider_account_bindings row
-- (vendor 'teams', auth_type 'channel_oauth', external_account_id
-- 'teams-app-shared') that the Messaging switchboard switches bots on
-- against. The shared app's real secrets never reach the runtime — they live
-- only in cloud-api env vars — so the binding row carries no usable secret
-- and inbound arrives over the cloud relay at /webhooks/channels/teams-app.
CREATE TABLE IF NOT EXISTS teams_app_connections (
    owner TEXT PRIMARY KEY,
    account_id TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'CONNECTED',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
