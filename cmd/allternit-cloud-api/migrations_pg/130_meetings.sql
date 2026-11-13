-- 130_meetings.sql
--
-- Allternit Meetings (docs/meetings/CONTRACT.md section 2, frozen 2026-11-13).
-- Gizzi joins Meet, Zoom and Teams calls as a bot participant; these tables
-- hold the calendar connections, the meetings found in calendars or pasted as
-- links, the bot sessions (one per url_key at a time), what the pod reports
-- (participants, transcript, chat, screen notes), the artifacts and tool
-- approvals made during a call, org and user settings, and the bot-minute
-- meter. Routes: src/routes/meetings.rs, org_meeting_settings.rs,
-- meeting_pod.rs.
--
-- Additive only, idempotent (safe to re-run). Unqualified names, like 124,
-- so the route tests can apply this file in a per-test schema.

-- Providers (contract change 2026-11-13): 'google' (OAuth) and 'ics' (a
-- pasted private calendar feed URL: Outlook "Publish calendar", iCloud, any
-- https .ics). 'microsoft' stays in the CHECK for a later OAuth lane; no
-- Microsoft routes ship in v1. OAuth rows carry account_email and the
-- encrypted refresh token; ics rows carry the encrypted feed URL plus its
-- sha256 (feed_url_hash) so the same feed is not connected twice.
CREATE TABLE IF NOT EXISTS calendar_connections (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id TEXT NOT NULL,
    provider TEXT NOT NULL CHECK (provider IN ('google', 'microsoft', 'ics')),
    account_email TEXT,
    -- Display label (ics: user-given or the feed's X-WR-CALNAME).
    label TEXT,
    encrypted_refresh_token TEXT,
    encrypted_access_token TEXT,
    access_expires_at TIMESTAMPTZ,
    -- ics only: credential_cipher output of the feed URL (it is a secret).
    encrypted_feed_url TEXT,
    -- ics only: sha256 hex of the trimmed feed URL, for dedupe.
    feed_url_hash TEXT,
    scopes TEXT[] NOT NULL DEFAULT '{}',
    sync_token TEXT,
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'needs_reauth', 'revoked')),
    last_synced_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT calendar_connections_provider_fields CHECK (
        CASE provider
            WHEN 'ics' THEN encrypted_feed_url IS NOT NULL AND feed_url_hash IS NOT NULL
            ELSE account_email IS NOT NULL AND encrypted_refresh_token IS NOT NULL
        END
    )
);

-- One connection per (user, provider, account): the account email for OAuth
-- providers, the feed hash for ics.
CREATE UNIQUE INDEX IF NOT EXISTS calendar_connections_user_account
    ON calendar_connections (user_id, provider, COALESCE(account_email, feed_url_hash));
CREATE INDEX IF NOT EXISTS calendar_connections_status_synced
    ON calendar_connections (status, last_synced_at);

-- Zoom (contract change 2026-11-13 "Zoom cloud side", docs/meetings/ZOOM.md):
-- one connected Zoom account per user. The bot joins Zoom with an
-- on-behalf-of token minted from it; tokens go through credential_cipher and
-- refresh under SELECT ... FOR UPDATE. Routes are a later lane.
CREATE TABLE IF NOT EXISTS zoom_connections (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id TEXT NOT NULL UNIQUE,
    zoom_user_id TEXT,
    encrypted_refresh_token TEXT NOT NULL,
    encrypted_access_token TEXT,
    access_expires_at TIMESTAMPTZ,
    scopes TEXT[] NOT NULL DEFAULT '{}',
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'needs_reauth', 'revoked')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS meetings (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id TEXT NOT NULL,
    org_id TEXT,
    platform TEXT NOT NULL CHECK (platform IN ('meet', 'zoom', 'teams')),
    title TEXT NOT NULL DEFAULT '',
    join_url TEXT NOT NULL,
    url_key TEXT NOT NULL,
    starts_at TIMESTAMPTZ,
    ends_at TIMESTAMPTZ,
    -- calendar_ics: rows synced from an ics connection (contract change 11-13).
    source TEXT NOT NULL CHECK (source IN ('calendar_google', 'calendar_microsoft', 'calendar_ics', 'link', 'chat', 'mcp')),
    calendar_connection_id UUID REFERENCES calendar_connections (id) ON DELETE SET NULL,
    external_event_id TEXT,
    attendee_count INTEGER,
    external_attendee_count INTEGER,
    is_one_on_one BOOLEAN NOT NULL DEFAULT false,
    auto_join BOOLEAN,
    reply_mode TEXT NOT NULL DEFAULT 'chat' CHECK (reply_mode IN ('chat', 'voice')),
    attendees JSONB NOT NULL DEFAULT '[]'::jsonb,
    -- No FK to meeting_sessions (it references meetings; avoids a cycle).
    latest_session_id UUID,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    -- NULL starts_at values are distinct; link-only rows dedupe in the route.
    UNIQUE (user_id, url_key, starts_at)
);

CREATE UNIQUE INDEX IF NOT EXISTS meetings_calendar_event
    ON meetings (calendar_connection_id, external_event_id)
    WHERE external_event_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS meetings_user_starts
    ON meetings (user_id, starts_at DESC) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS meetings_url_key ON meetings (url_key);
CREATE INDEX IF NOT EXISTS meetings_starts_scheduler
    ON meetings (starts_at) WHERE deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS meeting_sessions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    meeting_id UUID NOT NULL REFERENCES meetings (id) ON DELETE CASCADE,
    user_id TEXT NOT NULL,
    org_id TEXT,
    url_key TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'scheduled' CHECK (state IN (
        'scheduled', 'launching', 'joining', 'waiting_admit', 'live', 'paused', 'ended'
    )),
    end_reason TEXT CHECK (end_reason IN (
        'left_by_user', 'removed', 'not_admitted', 'empty_room', 'max_duration',
        'other_notetaker', 'join_failed', 'guest_blocked', 'no_capacity', 'error'
    )),
    platform TEXT NOT NULL,
    speak_replies BOOLEAN NOT NULL DEFAULT false,
    listening BOOLEAN NOT NULL DEFAULT true,
    bot_identity TEXT NOT NULL DEFAULT 'Gizzi (Allternit)',
    pod_ref TEXT,
    -- sha256 hex of the pod token; the token itself is never stored.
    pod_token_hash TEXT,
    scheduled_join_at TIMESTAMPTZ,
    joined_at TIMESTAMPTZ,
    live_at TIMESTAMPTZ,
    ended_at TIMESTAMPTZ,
    summary TEXT,
    decisions JSONB NOT NULL DEFAULT '[]'::jsonb,
    action_items JSONB NOT NULL DEFAULT '[]'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- end_reason is set exactly when the session has ended.
    CONSTRAINT meeting_sessions_end_reason_iff_ended
        CHECK ((state = 'ended') = (end_reason IS NOT NULL))
);

CREATE INDEX IF NOT EXISTS meeting_sessions_meeting
    ON meeting_sessions (meeting_id, created_at DESC);
-- One active bot per meeting URL.
CREATE UNIQUE INDEX IF NOT EXISTS meeting_sessions_one_active_per_url
    ON meeting_sessions (url_key) WHERE state NOT IN ('ended');
CREATE INDEX IF NOT EXISTS meeting_sessions_active_state
    ON meeting_sessions (state) WHERE state NOT IN ('ended');
CREATE INDEX IF NOT EXISTS meeting_sessions_pod_token
    ON meeting_sessions (pod_token_hash) WHERE pod_token_hash IS NOT NULL;

CREATE TABLE IF NOT EXISTS meeting_participants (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id UUID NOT NULL REFERENCES meeting_sessions (id) ON DELETE CASCADE,
    platform_participant_id TEXT,
    display_name TEXT NOT NULL,
    email TEXT,
    user_id TEXT,
    class TEXT NOT NULL DEFAULT 'guest' CHECK (class IN ('org_member', 'guest')),
    joined_at TIMESTAMPTZ NOT NULL,
    left_at TIMESTAMPTZ,
    notice_at TIMESTAMPTZ,
    UNIQUE (session_id, platform_participant_id)
);

CREATE INDEX IF NOT EXISTS meeting_participants_session ON meeting_participants (session_id);

CREATE TABLE IF NOT EXISTS meeting_transcript_segments (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id UUID NOT NULL REFERENCES meeting_sessions (id) ON DELETE CASCADE,
    seq INTEGER NOT NULL,
    speaker TEXT,
    participant_id UUID REFERENCES meeting_participants (id) ON DELETE SET NULL,
    confidence REAL NOT NULL DEFAULT 0 CHECK (confidence >= 0 AND confidence <= 1),
    source TEXT NOT NULL CHECK (source IN ('audio', 'captions')),
    start_ms INTEGER NOT NULL,
    end_ms INTEGER NOT NULL,
    text TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (session_id, seq),
    UNIQUE (session_id, idempotency_key)
);

CREATE INDEX IF NOT EXISTS meeting_transcript_segments_session_start
    ON meeting_transcript_segments (session_id, start_ms);

CREATE TABLE IF NOT EXISTS meeting_chat_messages (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id UUID NOT NULL REFERENCES meeting_sessions (id) ON DELETE CASCADE,
    direction TEXT NOT NULL CHECK (direction IN ('in', 'out')),
    sender TEXT NOT NULL,
    text TEXT NOT NULL,
    platform_message_id TEXT,
    -- page_only: a reply shown on the meeting page, not posted in the call.
    visibility TEXT NOT NULL DEFAULT 'chat' CHECK (visibility IN ('chat', 'page_only')),
    idempotency_key TEXT NOT NULL,
    at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (session_id, idempotency_key)
);

CREATE INDEX IF NOT EXISTS meeting_chat_messages_session_at ON meeting_chat_messages (session_id, at);

CREATE TABLE IF NOT EXISTS meeting_artifacts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id UUID NOT NULL REFERENCES meeting_sessions (id) ON DELETE CASCADE,
    -- artifacts_v2 id; no FK (the artifact store is its own service).
    artifact_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('doc', 'slides', 'canvas')),
    title TEXT NOT NULL,
    url TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (session_id, artifact_id)
);

CREATE TABLE IF NOT EXISTS meeting_tool_approvals (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id UUID NOT NULL REFERENCES meeting_sessions (id) ON DELETE CASCADE,
    -- The signed-in user who must approve: the session owner.
    user_id TEXT NOT NULL,
    tool TEXT NOT NULL,
    args JSONB NOT NULL DEFAULT '{}'::jsonb,
    args_summary TEXT NOT NULL,
    -- Speaker label from the call; not authenticated.
    asked_by TEXT,
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'approved', 'denied', 'expired')),
    requested_at TIMESTAMPTZ NOT NULL,
    resolved_at TIMESTAMPTZ,
    expires_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX IF NOT EXISTS meeting_tool_approvals_session_status
    ON meeting_tool_approvals (session_id, status);

-- One row per pod tool call (call_id) that ran a personal tool, so a re-sent
-- call returns the same result instead of running the tool twice.
-- 'running' claims the call while the tool runs.
CREATE TABLE IF NOT EXISTS meeting_tool_calls (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id UUID NOT NULL REFERENCES meeting_sessions (id) ON DELETE CASCADE,
    call_id TEXT NOT NULL,
    tool TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('running', 'ok', 'error')),
    -- The ToolCallResult returned to the pod.
    result JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (session_id, call_id)
);

CREATE TABLE IF NOT EXISTS meeting_screen_notes (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id UUID NOT NULL REFERENCES meeting_sessions (id) ON DELETE CASCADE,
    at_ms INTEGER NOT NULL,
    description TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (session_id, idempotency_key)
);

CREATE TABLE IF NOT EXISTS org_meeting_settings (
    org_id TEXT PRIMARY KEY,
    enabled BOOLEAN NOT NULL DEFAULT true,
    roles TEXT[] NOT NULL DEFAULT '{org:admin,org:member}',
    allow_external_guests BOOLEAN NOT NULL DEFAULT true,
    guest_quiet BOOLEAN NOT NULL DEFAULT true,
    allow_member_connectors BOOLEAN NOT NULL DEFAULT false,
    -- Placeholder until Eoj gate 4 (retention period).
    retention_days INTEGER NOT NULL DEFAULT 90 CHECK (retention_days BETWEEN 1 AND 3650),
    updated_by TEXT,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS user_meeting_settings (
    user_id TEXT PRIMARY KEY,
    auto_join TEXT NOT NULL DEFAULT 'off' CHECK (auto_join IN ('off', 'all', 'all_except_1on1')),
    recap_email BOOLEAN NOT NULL DEFAULT true,
    default_reply_mode TEXT NOT NULL DEFAULT 'chat' CHECK (default_reply_mode IN ('chat', 'voice')),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Bot-minute meter: one row per ended session (UNIQUE session_id makes the
-- write idempotent). Kept apart from voice_usage so 029's engine CHECK stays
-- untouched and the org is carried.
CREATE TABLE IF NOT EXISTS meeting_bot_minutes (
    id BIGSERIAL PRIMARY KEY,
    user_id TEXT NOT NULL,
    org_id TEXT,
    session_id UUID NOT NULL UNIQUE REFERENCES meeting_sessions (id) ON DELETE CASCADE,
    seconds INTEGER NOT NULL CHECK (seconds >= 0),
    platform TEXT NOT NULL,
    occurred_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS meeting_bot_minutes_user ON meeting_bot_minutes (user_id, occurred_at);
CREATE INDEX IF NOT EXISTS meeting_bot_minutes_org ON meeting_bot_minutes (org_id, occurred_at);
