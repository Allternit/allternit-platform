-- Plan chat store (CONTRACT-connections-2026-11 section 9.10, brain runner
-- step 1.4). Chats with plan (`plan/<vendor>/<model>`) and Copilot
-- (`copilot/<model>`) models, held by cloud-api so they sync across a
-- person's devices without a computer (the agent-session relay needs one).
-- One row per thread; `messages` is the ordered list of
-- {role, content, model?, at}. Erased with the account (account_erase.rs).
CREATE TABLE IF NOT EXISTS plan_threads (
    id          TEXT PRIMARY KEY,
    user_id     TEXT NOT NULL,
    title       TEXT,
    model       TEXT NOT NULL,
    messages    JSONB NOT NULL DEFAULT '[]'::jsonb,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS plan_threads_user_updated ON plan_threads (user_id, updated_at DESC);
