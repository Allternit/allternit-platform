-- Share a chat thread by link (routes/thread_shares.rs). One row per shared
-- session: `snapshot` holds only the allowlisted messages (role, text, images,
-- tool names), rebuilt server-side so nothing else a client sends is stored.
-- `token` is a random 160-bit URL-safe id; deleting the row turns the link off.
-- 121-123 are reserved by the Platform API parity program.
CREATE TABLE IF NOT EXISTS thread_shares (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    token TEXT NOT NULL UNIQUE,
    user_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    title TEXT NOT NULL DEFAULT '',
    snapshot JSONB NOT NULL DEFAULT '[]'::jsonb,
    message_count INTEGER NOT NULL DEFAULT 0,
    bytes INTEGER NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Owner lookup by session, and at most one share per session per user.
CREATE UNIQUE INDEX IF NOT EXISTS thread_shares_user_session ON thread_shares (user_id, session_id);
