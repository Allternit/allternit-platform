-- AAI contexts the host has handed out, so a context outlives a gateway restart: the binding it was opened with,
-- the vendor conversation it maps to, the last events cursor read, whether it was closed, and (for providers that
-- keep the conversation client-side, e.g. openclaw/hermes) the provider's own snapshot to restore it from.
CREATE TABLE aai_contexts (
  adapter_id TEXT NOT NULL,
  context_id TEXT NOT NULL,
  binding TEXT NOT NULL,
  agent_id TEXT,
  conversation_keys TEXT,
  state TEXT,
  last_cursor TEXT,
  closed INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (adapter_id, context_id)
);
CREATE INDEX aai_contexts_updated ON aai_contexts (closed, updated_at);
