-- Allternit Agent Gateway WP5: thread runner + dual-authority approvals.
-- Additive only.
--
-- gateway_approvals: Approval { authority: allternit|vendor, actor, action,
-- threadId, remoteRef?, state }. An Allternit approval gates consequential work
-- BEFORE it is sent to a vendor; a vendor approval mirrors one the vendor asked
-- for (remote_ref = the vendor's approval id). They never resolve each other.
CREATE TABLE IF NOT EXISTS gateway_approvals (
    id TEXT PRIMARY KEY,
    owner TEXT NOT NULL,
    thread_id TEXT NOT NULL,
    bot_id TEXT NOT NULL,
    generation INTEGER NOT NULL DEFAULT 1,
    authority TEXT NOT NULL CHECK (authority IN ('allternit', 'vendor')),
    action TEXT NOT NULL,
    detail_json TEXT NOT NULL DEFAULT '{}',
    remote_ref TEXT,
    remote_context_id TEXT,
    correlation_id TEXT,
    -- pending | approved | denied
    state TEXT NOT NULL DEFAULT 'pending',
    consumed INTEGER NOT NULL DEFAULT 0,
    actor_type TEXT,
    actor_id TEXT,
    created_at TEXT NOT NULL,
    resolved_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_gateway_approvals_thread ON gateway_approvals(thread_id, state);
-- one mirrored row per vendor approval (NULL remote_ref rows never collide)
CREATE UNIQUE INDEX IF NOT EXISTS idx_gateway_approvals_remote ON gateway_approvals(thread_id, remote_ref);

-- gateway_sends: idempotency ledger so a retried turn (same correlation id)
-- never double-sends to the vendor.
CREATE TABLE IF NOT EXISTS gateway_sends (
    remote_binding_id TEXT NOT NULL,
    correlation_id TEXT NOT NULL,
    sent_at TEXT NOT NULL,
    PRIMARY KEY (remote_binding_id, correlation_id)
);
