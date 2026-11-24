-- 137: meeting sessions can end `no_runtime` (Eoj, 2026-11-14).
--
-- The Meetings bot runs on the user's own runtime (their cloud computer or
-- Allternit Desktop), never on Allternit's servers
-- (services/meeting_launcher.rs, docs/meetings/CONTRACT.md "Change
-- 2026-11-14: runtime launcher"). A session ends `no_runtime` when none of
-- the owner's runtimes was online or wakeable, or the runtime running the
-- bot went offline.
--
-- Widens the end_reason CHECK from 130_meetings.sql (an unnamed column
-- constraint, so Postgres named it meeting_sessions_end_reason_check).
-- Idempotent: drop, then add the wider list. No rows change.

ALTER TABLE meeting_sessions DROP CONSTRAINT IF EXISTS meeting_sessions_end_reason_check;
ALTER TABLE meeting_sessions ADD CONSTRAINT meeting_sessions_end_reason_check CHECK (end_reason IN (
    'left_by_user', 'removed', 'not_admitted', 'empty_room', 'max_duration',
    'other_notetaker', 'join_failed', 'guest_blocked', 'no_capacity', 'no_runtime', 'error'
));
