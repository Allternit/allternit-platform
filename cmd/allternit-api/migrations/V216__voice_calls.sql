-- V215: phone calls relayed to this runtime (voice session ⇄ cloud-api relay).
--
-- One row per call the cloud relays to `POST /api/v1/voice/calls`. It pins the
-- call to the thread and session `channel_phone::resolve_thread_async` chose,
-- so events and turns for that callId land in the caller's one conversation.
CREATE TABLE IF NOT EXISTS voice_calls (
    call_id     TEXT PRIMARY KEY,
    owner_id    TEXT NOT NULL,
    bot_id      TEXT NOT NULL,
    number_id   TEXT NOT NULL,
    thread_id   TEXT NOT NULL,
    session_id  TEXT NOT NULL,
    from_e164   TEXT NOT NULL,
    to_e164     TEXT NOT NULL,
    direction   TEXT NOT NULL,              -- inbound | outbound
    room        TEXT NOT NULL,
    state       TEXT NOT NULL DEFAULT 'active',  -- active | ended
    started_at  TEXT NOT NULL,
    ended_at    TEXT,
    created_at  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_voice_calls_thread ON voice_calls(thread_id);
CREATE INDEX IF NOT EXISTS idx_voice_calls_owner ON voice_calls(owner_id, state);
