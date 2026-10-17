-- V260: one trace across agent hops, and a receipt per hop (`src/netops/hops.rs`).
--
-- `bot_events` rows carry the W3C trace they belong to. `agent_hops` is one row
-- per bot-to-bot hand-off (message_agent, the Gizzi inbox, the vendor MCP
-- `ask_bot`, A2A): who sent it, who got it, and how far it got
-- (stored → delivered → read → answered, or held / refused / failed with a
-- reason). `UNIQUE(owner, idempotency_key)` makes a replayed send return the
-- earlier hop instead of running twice. `text_hash` lets the loop guard refuse a
-- hand-off that goes back to an ancestor with the same text; `result_json` is
-- what a replay returns.
ALTER TABLE bot_events ADD COLUMN trace_id TEXT;
ALTER TABLE bot_events ADD COLUMN span_id TEXT;
ALTER TABLE bot_events ADD COLUMN parent_span_id TEXT;
CREATE INDEX IF NOT EXISTS idx_bot_events_trace ON bot_events(trace_id);

CREATE TABLE IF NOT EXISTS agent_hops (
    id TEXT PRIMARY KEY,
    owner TEXT NOT NULL,
    trace_id TEXT NOT NULL,
    span_id TEXT NOT NULL,
    parent_span_id TEXT,
    depth INTEGER NOT NULL DEFAULT 1,
    -- message_agent | gizzi_inbox | vendor_mcp | a2a
    via TEXT NOT NULL,
    from_kind TEXT NOT NULL,
    from_id TEXT NOT NULL,
    to_bot_id TEXT NOT NULL,
    thread_id TEXT,
    idempotency_key TEXT,
    -- stored | delivered | read | answered | held | refused | failed
    state TEXT NOT NULL DEFAULT 'stored',
    state_reason TEXT,
    -- push | turn | pull
    tier TEXT NOT NULL DEFAULT 'turn',
    text_hash TEXT,
    result_json TEXT,
    created_at TEXT NOT NULL,
    delivered_at TEXT,
    read_at TEXT,
    answered_at TEXT,
    model TEXT,
    tokens_in INTEGER,
    tokens_out INTEGER,
    cost_cents REAL,
    UNIQUE(owner, idempotency_key)
);
CREATE INDEX IF NOT EXISTS idx_agent_hops_trace ON agent_hops(trace_id);
CREATE INDEX IF NOT EXISTS idx_agent_hops_thread ON agent_hops(thread_id);
CREATE INDEX IF NOT EXISTS idx_agent_hops_pair ON agent_hops(owner, from_id, to_bot_id, created_at);
