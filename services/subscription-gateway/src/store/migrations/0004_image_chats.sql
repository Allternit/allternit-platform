-- Image-chat history policy (Eoj, 2026-09-28): image tasks run inside a
-- provider project ("Allternit" by default) and reuse one chat per account
-- until it holds image_chat_max images; then the next image opens a new chat
-- in the same project. image.generate cannot use temporary chats (provider
-- rule), so without this every image task left a new chat in history.
CREATE TABLE image_chats (
  chat_id TEXT PRIMARY KEY,
  provider TEXT NOT NULL,
  account_id TEXT NOT NULL,
  provider_thread_id TEXT NOT NULL,
  provider_url TEXT NOT NULL,
  project TEXT,
  image_count INTEGER NOT NULL DEFAULT 0,
  status TEXT NOT NULL CHECK (status IN ('active', 'full')),
  created_at TEXT NOT NULL,
  last_used_at TEXT NOT NULL
);
CREATE INDEX image_chats_active ON image_chats (provider, account_id, status);
