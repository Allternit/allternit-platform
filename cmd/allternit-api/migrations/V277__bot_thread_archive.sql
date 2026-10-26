-- Threads as war rooms (W3): archiving is real, not a client-side flag.
-- An archived thread is closed: it drops out of open-thread context, its task
-- grants are revoked, and reminders/routines tied to it stop (thread_closeout.rs).
-- Transcripts stay readable; POST /threads/:id/unarchive clears it.
ALTER TABLE bot_threads ADD COLUMN archived_at TEXT;
CREATE INDEX IF NOT EXISTS idx_bot_threads_archived ON bot_threads(user_id, archived_at);
