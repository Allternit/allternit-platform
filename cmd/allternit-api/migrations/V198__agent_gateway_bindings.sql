-- Allternit Agent Gateway WP2: binding tables (spec: Research/specs/agent-gateway.md
-- "Data model", channel-packs.md "Mapping and sync contract").
--
-- Vendor identity lives here, never on bot_threads. Secrets are only ever
-- referenced (secret_ref / session_ref), never stored raw. State machines are
-- enforced by allternit-api (agent_gateway_routes.rs), not by CHECK constraints,
-- so this stays portable between SQLite and Postgres.

-- A connected vendor or channel account.
CREATE TABLE IF NOT EXISTS provider_account_bindings (
    id                  TEXT PRIMARY KEY,
    owner               TEXT NOT NULL,
    vendor              TEXT NOT NULL,
    auth_type           TEXT NOT NULL,
    external_account_id TEXT,
    display_name        TEXT,
    workspace           TEXT,
    secret_ref          TEXT,
    session_ref         TEXT,
    scopes_json         TEXT NOT NULL DEFAULT '[]',
    -- optional restriction of the account to a single bot
    restricted_bot_id   TEXT,
    state               TEXT NOT NULL DEFAULT 'DISCONNECTED',
    verified_at         TEXT,
    expires_at          TEXT,
    created_at          TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at          TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_provider_account_bindings_owner ON provider_account_bindings(owner);

-- How a Bot executes. One row per bot; rebinding never changes bot_id.
CREATE TABLE IF NOT EXISTS bot_execution_bindings (
    id                 TEXT PRIMARY KEY,
    owner              TEXT NOT NULL,
    bot_id             TEXT NOT NULL,
    type               TEXT NOT NULL DEFAULT 'vendor',
    mode               TEXT NOT NULL DEFAULT 'hosted',
    vendor             TEXT,
    adapter_id         TEXT,
    account_binding_id TEXT,
    preferred_lane     TEXT,
    external_agent_id  TEXT,
    capabilities_json  TEXT NOT NULL DEFAULT '{}',
    health_json        TEXT NOT NULL DEFAULT '{}',
    state              TEXT NOT NULL DEFAULT 'UNBOUND',
    created_at         TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at         TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (bot_id)
);
CREATE INDEX IF NOT EXISTS idx_bot_execution_bindings_owner ON bot_execution_bindings(owner);
CREATE INDEX IF NOT EXISTS idx_bot_execution_bindings_account ON bot_execution_bindings(account_binding_id);

-- One Thread generation <-> one remote context. lane and capability_snapshot
-- are frozen at create so the Details tab stays truthful after a rebind.
CREATE TABLE IF NOT EXISTS remote_thread_bindings (
    id                    TEXT PRIMARY KEY,
    owner                 TEXT NOT NULL,
    thread_id             TEXT NOT NULL,
    generation            INTEGER NOT NULL,
    bot_id                TEXT NOT NULL,
    execution_binding_id  TEXT,
    external_context_id   TEXT,
    external_task_id      TEXT,
    continuation_token    TEXT,
    sync_cursor           TEXT,
    last_remote_event_id  TEXT,
    capability_snapshot   TEXT NOT NULL DEFAULT '{}',
    lane                  TEXT,
    state                 TEXT NOT NULL DEFAULT 'UNBOUND',
    created_at            TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at            TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    closed_at             TEXT,
    UNIQUE (thread_id, generation)
);
CREATE INDEX IF NOT EXISTS idx_remote_thread_bindings_owner ON remote_thread_bindings(owner);
CREATE INDEX IF NOT EXISTS idx_remote_thread_bindings_thread ON remote_thread_bindings(thread_id);
CREATE INDEX IF NOT EXISTS idx_remote_thread_bindings_bot ON remote_thread_bindings(bot_id);

-- One Thread <-> one external channel conversation (Channel Packs).
CREATE TABLE IF NOT EXISTS channel_conversation_bindings (
    id                        TEXT PRIMARY KEY,
    owner                     TEXT NOT NULL,
    thread_id                 TEXT NOT NULL,
    provider                  TEXT NOT NULL,
    account_binding_id        TEXT,
    external_workspace_id     TEXT,
    external_channel_id       TEXT,
    external_conversation_id  TEXT NOT NULL,
    external_thread_id        TEXT,
    canonical_url             TEXT,
    bidirectional             INTEGER NOT NULL DEFAULT 1,
    read_only                 INTEGER NOT NULL DEFAULT 0,
    posting_identity_id       TEXT,
    last_inbound_cursor       TEXT,
    last_outbound_cursor      TEXT,
    sync_state                TEXT NOT NULL DEFAULT 'LIVE',
    created_at                TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at                TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_channel_conversation_bindings_owner ON channel_conversation_bindings(owner);
CREATE INDEX IF NOT EXISTS idx_channel_conversation_bindings_thread ON channel_conversation_bindings(thread_id);
CREATE INDEX IF NOT EXISTS idx_channel_conversation_bindings_account ON channel_conversation_bindings(account_binding_id);

-- Installed Vendor Pack manifests.
CREATE TABLE IF NOT EXISTS vendor_pack_registry (
    vendor_pack_id TEXT NOT NULL,
    version        TEXT NOT NULL,
    manifest_json  TEXT NOT NULL DEFAULT '{}',
    enabled        INTEGER NOT NULL DEFAULT 1,
    installed_at   TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (vendor_pack_id, version)
);

-- PackGaps: fallback rendering used where a pack has no renderer.
CREATE TABLE IF NOT EXISTS vendor_pack_gaps (
    id             TEXT PRIMARY KEY,
    owner          TEXT NOT NULL,
    vendor         TEXT NOT NULL,
    capability     TEXT NOT NULL,
    surface        TEXT NOT NULL,
    fallback_used  INTEGER NOT NULL DEFAULT 1,
    severity       TEXT NOT NULL DEFAULT 'visual_parity',
    status         TEXT NOT NULL DEFAULT 'open',
    first_seen_at  TEXT NOT NULL,
    last_seen_at   TEXT NOT NULL,
    occurrences    INTEGER NOT NULL DEFAULT 1,
    sample_ref     TEXT,
    UNIQUE (owner, vendor, capability, surface)
);
CREATE INDEX IF NOT EXISTS idx_vendor_pack_gaps_owner ON vendor_pack_gaps(owner);
CREATE INDEX IF NOT EXISTS idx_vendor_pack_gaps_vendor ON vendor_pack_gaps(vendor);

-- Every auth, verification and revocation event for an account.
CREATE TABLE IF NOT EXISTS connection_audit (
    id                 TEXT PRIMARY KEY,
    owner              TEXT NOT NULL,
    account_binding_id TEXT NOT NULL,
    event              TEXT NOT NULL,
    from_state         TEXT,
    to_state           TEXT,
    actor              TEXT,
    detail_json        TEXT NOT NULL DEFAULT '{}',
    created_at         TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_connection_audit_owner ON connection_audit(owner);
CREATE INDEX IF NOT EXISTS idx_connection_audit_account ON connection_audit(account_binding_id);
