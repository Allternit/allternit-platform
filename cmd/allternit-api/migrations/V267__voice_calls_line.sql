-- V267: calls on a bot's linked real phone (contract C4, CONTRACT-linked-phone-voice-2026-10-18).
--
-- A linked phone is wired to the owner's computer, so its calls have no Telnyx
-- number: `number_id` becomes nullable. `line` holds the call's line as JSON
-- ({kind:"number",numberId} or {kind:"linked_phone",deviceId,serial?,label,platform});
-- rows from before this migration have none and are number calls.
-- `from_e164` / `to_e164` stay NOT NULL; a withheld caller on a linked phone is ''.
-- SQLite cannot drop a NOT NULL constraint in place, so the table is rebuilt.


CREATE TABLE voice_calls_new (
    call_id     TEXT PRIMARY KEY,
    owner_id    TEXT NOT NULL,
    bot_id      TEXT NOT NULL,
    number_id   TEXT,
    thread_id   TEXT NOT NULL,
    session_id  TEXT NOT NULL,
    from_e164   TEXT NOT NULL,
    to_e164     TEXT NOT NULL,
    direction   TEXT NOT NULL,              -- inbound | outbound
    room        TEXT NOT NULL,
    state       TEXT NOT NULL DEFAULT 'active',  -- active | ended
    started_at  TEXT NOT NULL,
    ended_at    TEXT,
    created_at  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    line        TEXT                        -- JSON CallLine; NULL = number call (pre-V267)
);

INSERT INTO voice_calls_new (call_id, owner_id, bot_id, number_id, thread_id, session_id, from_e164, to_e164, direction, room, state, started_at, ended_at, created_at)
SELECT call_id, owner_id, bot_id, number_id, thread_id, session_id, from_e164, to_e164, direction, room, state, started_at, ended_at, created_at FROM voice_calls;

DROP TABLE voice_calls;
ALTER TABLE voice_calls_new RENAME TO voice_calls;

CREATE INDEX IF NOT EXISTS idx_voice_calls_thread ON voice_calls(thread_id);
CREATE INDEX IF NOT EXISTS idx_voice_calls_owner ON voice_calls(owner_id, state);

