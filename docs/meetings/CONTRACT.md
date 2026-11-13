# Allternit Meetings: frozen contract (step 1.1, 2026-11-13)

Every lane builds against this file. Companion code: `services/meeting-bot/src/adapter.ts` (adapter + pod config) and `allternit-ai/src/lib/meetings/types.ts` (web types). The MCP shapes are bound by `allternit-ai/src/lib/app-screens/packs/meetings.json`. Change this file, the two code files and the pack together or not at all.

Conventions: ids are UUID (`gen_random_uuid()`, serialized as strings); `user_id` and `org_id` are Clerk ids as `TEXT` (like `provider_tokens`, `thread_shares`); timestamps are `timestamptz NOT NULL DEFAULT now()`, serialized ISO-8601 UTC; offsets are integer ms; migrations are additive and idempotent (`CREATE TABLE IF NOT EXISTS`). Next free migration on origin/main is 130 (128 `voice_keys`, 129 `platform_agents_cloud_model_resync` exist; SPEC-meetings section 9 said 128 and is stale).

## 1. Decisions fixed here

- Identity (Eoj 11-13, from spike result 3): Meet = a signed-in bot Google account (free-Gmail calls refuse guests and automation-mode browsers); Zoom = Meeting SDK with an OBF token from the user's connected Zoom account; Teams = anonymous web join. Display name `Gizzi (Allternit)`.
- Replies go to meeting chat by default; voice (`speak_replies`) is opt-in per meeting.
- Default auto-join rule after connecting a calendar: `all_except_1on1`. Google and Outlook auto-join are both v1.
- One bot per `url_key`; later requesters attach to the running session.
- Calendar tokens live in cloud-api, encrypted with `state.credential_cipher` (same as `provider_tokens`); plaintext only if `ALLTERNIT_CREDENTIALS_KEY` is unset (same warning path).
- Google scope: `https://www.googleapis.com/auth/calendar.events.readonly` plus `openid email`. Microsoft: `Calendars.Read offline_access openid email` (delegated, no admin consent).
- Free plan: every Meetings route except `GET /me/meeting-settings` and the calendar OAuth routes returns `402 meetings_requires_paid_plan`. Plan lookup: `voice_usage::plan_for_user` (`free|plus|super|ultra`).

## 2. Migration `130_meetings.sql`

Table-by-table. `PK` = primary key; all `id` columns `uuid PRIMARY KEY DEFAULT gen_random_uuid()` unless stated.

### calendar_connections
| column | type | null | notes |
|---|---|---|---|
| id | uuid | no | PK |
| user_id | text | no | |
| provider | text | no | CHECK `google|microsoft` |
| account_email | text | no | |
| encrypted_refresh_token | text | no | cipher output |
| encrypted_access_token | text | yes | cached |
| access_expires_at | timestamptz | yes | |
| scopes | text[] | no | default `'{}'` |
| sync_token | text | yes | Google syncToken / Graph deltaLink |
| status | text | no | CHECK `active|needs_reauth|revoked`, default `active` |
| last_synced_at | timestamptz | yes | |
| created_at, updated_at | timestamptz | no | |

UNIQUE `(user_id, provider, account_email)`. Index `(status, last_synced_at)`. Delete = hard delete of the row (tokens gone); meetings rows keep `calendar_connection_id` set NULL.

### meetings
| column | type | null | notes |
|---|---|---|---|
| id | uuid | no | PK |
| user_id | text | no | owner |
| org_id | text | yes | |
| platform | text | no | CHECK `meet|zoom|teams` |
| title | text | no | default `''` |
| join_url | text | no | |
| url_key | text | no | normalised, see section 7 |
| starts_at, ends_at | timestamptz | yes | |
| source | text | no | CHECK `calendar_google|calendar_microsoft|link|chat|mcp` |
| calendar_connection_id | uuid | yes | FK `calendar_connections(id) ON DELETE SET NULL` |
| external_event_id | text | yes | |
| attendee_count | integer | yes | incl. the user |
| external_attendee_count | integer | yes | |
| is_one_on_one | boolean | no | default false, see section 7 |
| auto_join | boolean | yes | per-meeting override; NULL = follow the rule |
| reply_mode | text | no | CHECK `chat|voice`, default `chat` |
| attendees | jsonb | no | default `'[]'` (`[{email,name,response}]`) |
| latest_session_id | uuid | yes | no FK (avoids cycle) |
| created_at, updated_at | timestamptz | no | |
| deleted_at | timestamptz | yes | soft delete |

UNIQUE `(user_id, url_key, starts_at)` (NULLs distinct; link-only rows dedupe in the route). UNIQUE partial `(calendar_connection_id, external_event_id) WHERE external_event_id IS NOT NULL`. Index `(user_id, starts_at DESC) WHERE deleted_at IS NULL`; index `(url_key)`; index `(starts_at) WHERE deleted_at IS NULL` for the scheduler.

### meeting_sessions
| column | type | null | notes |
|---|---|---|---|
| id | uuid | no | PK |
| meeting_id | uuid | no | FK `meetings(id) ON DELETE CASCADE` |
| user_id | text | no | requester who created it |
| org_id | text | yes | |
| url_key | text | no | |
| state | text | no | CHECK the 7 states, default `scheduled` |
| end_reason | text | yes | CHECK the 10 reasons |
| platform | text | no | |
| speak_replies | boolean | no | default false |
| listening | boolean | no | default true; false while `paused` |
| bot_identity | text | no | default `Gizzi (Allternit)` |
| pod_ref | text | yes | launcher handle |
| pod_token_hash | text | yes | sha256 of the pod token; the token itself is never stored |
| scheduled_join_at | timestamptz | yes | start minus 2 min |
| joined_at, live_at, ended_at | timestamptz | yes | |
| summary | text | yes | |
| decisions | jsonb | no | default `'[]'` (string[]) |
| action_items | jsonb | no | default `'[]'` (`[{text,owner,due}]`) |
| created_at, updated_at | timestamptz | no | |

Index `(meeting_id, created_at DESC)`; partial unique index `(url_key) WHERE state NOT IN ('ended')` (one active bot per URL); index `(state)` where not ended.

### meeting_participants
`id` PK; `session_id uuid NOT NULL REFERENCES meeting_sessions(id) ON DELETE CASCADE`; `platform_participant_id text`; `display_name text NOT NULL`; `email text`; `user_id text`; `class text NOT NULL CHECK (org_member|guest) DEFAULT 'guest'`; `joined_at timestamptz NOT NULL`; `left_at timestamptz`; `notice_at timestamptz` (when the consent notice was posted while this person was present). UNIQUE `(session_id, platform_participant_id)`; index `(session_id)`.

### meeting_transcript_segments
`id` PK; `session_id uuid NOT NULL REFERENCES meeting_sessions ON DELETE CASCADE`; `seq integer NOT NULL` (assigned by the pod, monotonic per session); `speaker text`; `participant_id uuid REFERENCES meeting_participants ON DELETE SET NULL`; `confidence real NOT NULL DEFAULT 0 CHECK (0..1)`; `source text NOT NULL CHECK (audio|captions)`; `start_ms integer NOT NULL`; `end_ms integer NOT NULL`; `text text NOT NULL`; `idempotency_key text NOT NULL`; `created_at`. UNIQUE `(session_id, seq)`; UNIQUE `(session_id, idempotency_key)`; index `(session_id, start_ms)`.

### meeting_chat_messages
`id` PK; `session_id` FK cascade; `direction text NOT NULL CHECK (in|out)`; `sender text NOT NULL`; `text text NOT NULL`; `platform_message_id text`; `idempotency_key text NOT NULL`; `at timestamptz NOT NULL`; `created_at`. UNIQUE `(session_id, idempotency_key)`; index `(session_id, at)`.

### meeting_artifacts
`id` PK; `session_id` FK cascade; `artifact_id text NOT NULL` (artifacts_v2 id; no FK, different service); `kind text NOT NULL CHECK (doc|slides|canvas)`; `title text NOT NULL`; `url text NOT NULL`; `version integer NOT NULL DEFAULT 1`; `created_at`, `updated_at`. UNIQUE `(session_id, artifact_id)`.

### meeting_tool_approvals
`id` PK; `session_id` FK cascade; `user_id text NOT NULL` (the signed-in user who must approve: the session owner); `tool text NOT NULL`; `args jsonb NOT NULL DEFAULT '{}'`; `args_summary text NOT NULL`; `asked_by text` (speaker label, unauthenticated); `status text NOT NULL CHECK (pending|approved|denied|expired) DEFAULT 'pending'`; `requested_at timestamptz NOT NULL`; `resolved_at timestamptz`; `expires_at timestamptz NOT NULL` (requested + 10 min). Index `(session_id, status)`. Approval is per user, per tool, per meeting: an `approved` row for `(session_id, tool)` covers later calls of that tool in the session.

### meeting_screen_notes
`id` PK; `session_id` FK cascade; `at_ms integer NOT NULL`; `description text NOT NULL`; `idempotency_key text NOT NULL`; `created_at`. UNIQUE `(session_id, idempotency_key)`. Rate cap enforced at ingest: at most 1 note per 30 s per session (excess dropped, counted in the response).

### org_meeting_settings
`org_id text PRIMARY KEY`; `enabled boolean NOT NULL DEFAULT true`; `roles text[] NOT NULL DEFAULT '{org:admin,org:member}'`; `allow_external_guests boolean NOT NULL DEFAULT true`; `guest_quiet boolean NOT NULL DEFAULT true`; `allow_member_connectors boolean NOT NULL DEFAULT false`; `retention_days integer NOT NULL DEFAULT 90 CHECK (retention_days BETWEEN 1 AND 3650)` (placeholder until Eoj gate 4); `updated_by text`; `updated_at`.

### user_meeting_settings
`user_id text PRIMARY KEY`; `auto_join text NOT NULL DEFAULT 'off' CHECK (off|all|all_except_1on1)` (the connect flow PUTs `all_except_1on1` when the user picks "yes"); `recap_email boolean NOT NULL DEFAULT true`; `default_reply_mode text NOT NULL DEFAULT 'chat' CHECK (chat|voice)`; `updated_at`.

### meeting_bot_minutes (meter)
`id bigserial PRIMARY KEY`; `user_id text NOT NULL`; `org_id text`; `session_id uuid NOT NULL UNIQUE REFERENCES meeting_sessions ON DELETE CASCADE` (UNIQUE makes the report idempotent); `seconds integer NOT NULL CHECK (seconds >= 0)`; `platform text NOT NULL`; `occurred_at timestamptz NOT NULL`; `created_at`. Index `(user_id, occurred_at)`; index `(org_id, occurred_at)`. Chosen over widening `voice_usage.engine` (keeps the 029 CHECK untouched and carries `org_id`).

## 3. State machine

`scheduled -> launching -> joining -> waiting_admit -> live <-> paused -> ended`. Skips allowed: `joining -> live` (open call, no lobby), any non-ended state `-> ended`. `live -> paused` on `stop_listening`; `paused -> live` on `resume`. `end_reason` is set exactly when `state = ended`.

`end_reason`: `left_by_user | removed | not_admitted | empty_room | max_duration | other_notetaker | join_failed | guest_blocked | no_capacity | error`.

Triggers: `left_by_user` (stop / leave control), `removed` (kicked or meeting closed by host), `not_admitted` (declined, or not admitted within 10 min), `empty_room` (alone 5 min), `max_duration` (8 h), `other_notetaker` (another vendor's notetaker joined), `join_failed` (selectors/timeouts), `guest_blocked` (platform refuses the bot's identity), `no_capacity` (global or per-org cap, no pod launched), `error` (anything else).

Per-meeting `gizzi` (UI field, must equal a key of `statuses` in meetings.json): `scheduled|off|launching|joining|waiting_admit|live|paused|ended|not_admitted`.
Derivation, first match wins: latest session exists and `state = ended` and `end_reason IN (not_admitted, guest_blocked)` -> `not_admitted`; latest session `state = ended` -> `ended`; latest session active -> its `state`; no active session and the meeting will be auto-joined (`auto_join = true`, or `auto_join IS NULL` and the user rule allows it, and `starts_at` in the future) -> `scheduled`; otherwise `off`. The UI label "Recap ready" for `ended` is shown only when `summary` is not null; a session ended for any other reason (`join_failed`, `no_capacity`, `error`) shows `ended` with the reason text.

## 4. REST (`/api/v1`)

Auth: **User** = Clerk JWT (existing `AuthUser`; org role from `org_role`, admin = `admin` or `org:admin`). **Pod** = `Authorization: Bearer <pod token>` issued by the launcher, scoped to one session id, valid until the session ends plus 5 min. User routes require the session/meeting to belong to the caller (or be attached to a url_key the caller requested). Errors are `{ "error": "<code>", "message": "<text>" }` (types.ts `MeetingApiError`).

Codes: `400 invalid_request`, `400 unsupported_link`, `401` no/invalid token, `402 meetings_requires_paid_plan`, `403 meetings_disabled_by_org` / `forbidden`, `404 meeting_not_found` / `session_not_found`, `409 conflict` (e.g. session already active) / `session_not_active`, `429 no_capacity`, `412 calendar_not_connected`.

| Route | Auth | Request | Response |
|---|---|---|---|
| `GET /meetings?range=now\|today\|week\|upcoming\|past&cursor=&limit=` | User | | `200 { meetings: Meeting[], next_cursor }` (agenda order: live first, then by `starts_at`) |
| `POST /meetings` | User | `{ url, title?, starts_at?, reply_mode? }` | `201 Meeting`; if the same `url_key` and start already exists for the user, `200` with that row |
| `GET /meetings/:id` | User | | `200 Meeting` with `gizzi` and `latest_session: {id, state, end_reason, live_at, ended_at, summary} \| null` (404 when not the caller's) |
| `PATCH /meetings/:id` | User | `{ title?, auto_join?, reply_mode? }` | `200 Meeting` |
| `DELETE /meetings/:id` | User | | `204` (soft delete; refused with 409 while a session is active) |
| `POST /meetings/:id/sessions` | User | `{ when: "now"\|"start", speak_replies? }` | `201 MeetingSession` (`when=start` creates `scheduled` with `scheduled_join_at`; `now` goes to `launching`). If a session for the `url_key` is active, `200` with that session (attach). On `429 no_capacity` nothing is created (the scheduler path instead records an ended session with `end_reason = no_capacity`). |
| `GET /meeting-sessions/:sid` | User | | `200 { session: MeetingSession, meeting: Meeting, participants: MeetingParticipant[], pending_approvals: ToolApproval[] }` |
| `GET /meeting-sessions/:sid/transcript?after_seq=&limit=` | User | | `200 { segments: TranscriptSegment[], next_after_seq }` (limit default 500, max 2000) |
| `GET /meeting-sessions/:sid/artifacts` | User | | `200 { artifacts: MeetingArtifact[] }` |
| `GET /meeting-sessions/:sid/events` | User | `Last-Event-ID` optional | SSE stream, section 5 |
| `POST /meeting-sessions/:sid/control` | User | `{ action: "stop_listening"\|"resume"\|"leave"\|"speak", text? }` | `200 MeetingSession` (`speak` needs `text`, only when `speak_replies`; else 409) |
| `PATCH /meeting-sessions/:sid` | User | `{ speak_replies?: boolean }` (the live page's Speak replies switch; added because the plan's control list has no toggle) | `200 MeetingSession`; pushes `set_speak_replies` on the control stream |
| `POST /meeting-sessions/:sid/approvals/:aid` | User (session owner only) | `{ decision: "approve"\|"deny" }` | `200 ToolApproval` |
| `GET /me/meeting-settings` | User (free ok) | | `200 UserMeetingSettings` |
| `PUT /me/meeting-settings` | User | `UserMeetingSettings` (partial allowed) | `200 UserMeetingSettings` |
| `GET /orgs/:org/meeting-settings` | User, member of org | | `200 OrgMeetingSettings` (defaults if no row) |
| `PUT /orgs/:org/meeting-settings` | User, org admin | `OrgMeetingSettings` (partial) | `200 OrgMeetingSettings` |
| `GET /me/meeting-usage` | User | | `200 MeetingUsage` |
| `GET /calendar/connections` | User | | `200 { connections: CalendarConnection[] }` (no tokens ever) |
| `DELETE /calendar/connections/:id` | User | | `204` (revokes the token with the provider best-effort, then deletes the row) |
| `GET /calendar/oauth/{google\|microsoft}/start?return_to=` | User (JWT in header, or short-lived `state` signed server-side for browser redirect) | | `200 { url }` the provider consent URL (the clients open it); `state` = signed `{user_id, return_to, nonce, exp 10 min}` |
| `GET /calendar/oauth/{google\|microsoft}/callback?code=&state=` | none (state-verified) | | `302` to `return_to` (default `https://ai.allternit.com/settings/meetings`) with `?calendar=connected\|error` and, on first connection, `&pick_rule=1` so the UI asks the auto-join rule inline. Exchanges the code, stores encrypted tokens, upserts the connection, starts a first sync. |
| `POST /meeting-sessions/:sid/ingest` | Pod | batch, section 6 | `200 { accepted, duplicates, dropped }` |
| `POST /meeting-sessions/:sid/tool-calls` | Pod | tool call, section 6 | `200 ToolCallResult` |
| `GET /meeting-sessions/:sid/control-stream` | Pod | SSE | control messages, section 6 |

Registered redirect URIs: `https://api.allternit.com/api/v1/calendar/oauth/google/callback` (Google). The Microsoft one is the same shape with `microsoft`. The start route uses `<API_BASE>/api/v1/calendar/oauth/<provider>/callback` where `API_BASE` is `https://api.allternit.com` in prod.

Plan/limit checks on `POST /meetings/:id/sessions` and the scheduler's `meeting_join` run, in order: plan (402) -> org enabled and role (403) -> guests allowed when the meeting has outside attendees (403 `forbidden` message names the setting) -> concurrency/capacity (429) -> dedupe by `url_key` (attach).

## 5. SSE events (`GET /meeting-sessions/:sid/events`)

Each frame: `id: <monotonic event id>`, `event: <type>`, `data: <JSON payload>`. A `: ping` comment every 20 s. On connect the server first sends `meeting.session.state` with the current state. `Last-Event-ID` replays up to the last 500 events. Payloads match the `MeetingEvent` union in types.ts:

- `meeting.session.state` `{session_id, state, gizzi, end_reason|null, at}`
- `meeting.participant.changed` `{session_id, participant, change: joined|left|updated}`
- `meeting.transcript.delta` `{session_id, segments: TranscriptSegment[]}`
- `meeting.chat.message` `{session_id, message}`
- `meeting.reply` `{session_id, channel: chat|voice, text, at}`
- `meeting.approval.requested` / `meeting.approval.resolved` `{session_id, approval}`
- `meeting.artifact.created` / `meeting.artifact.updated` `{session_id, artifact}`
- `meeting.ended` `{session_id, end_reason, ended_at}`
- `meeting.summary` `{session_id, summary, decisions, action_items}`

## 6. Pod and cloud

**Launch.** cloud-api `meeting_launcher` creates the pod with env mapped to `PodConfig` (adapter.ts), including a fresh pod token (random 256-bit; only its sha256 is stored in `meeting_sessions.pod_token_hash`).

**Ingest batch** `POST /meeting-sessions/:sid/ingest` (Pod):
```json
{
  "batch_id": "uuid, retry-safe",
  "sent_at": "ISO",
  "state": { "state": "live", "end_reason": null, "detail": "optional text" },
  "participants": [{ "platform_participant_id": "...", "display_name": "...", "email": null, "class": "guest", "joined_at": "ISO", "left_at": null }],
  "transcript": [{ "seq": 41, "idempotency_key": "seg-41", "speaker": "Jordan", "confidence": 0.82, "source": "audio", "start_ms": 12000, "end_ms": 14500, "text": "..." }],
  "chat": [{ "idempotency_key": "meet:<msgid>", "direction": "in", "sender": "Jordan", "text": "...", "at": "ISO" }],
  "screen_notes": [{ "idempotency_key": "scr-3", "at_ms": 90000, "description": "..." }],
  "notice_at": "ISO or null"
}
```
Every array is optional. Rows are inserted `ON CONFLICT (session_id, idempotency_key) DO NOTHING`; the response counts `accepted`, `duplicates`, `dropped` (screen notes over the rate cap). A `state` field applies the state machine (invalid transitions return 409 and are ignored). Batch every 2 s while live, max 500 rows. When `state` moves to `ended`, cloud-api writes `meeting_bot_minutes` (live_at to ended_at, rounded up to whole seconds) exactly once.

**Tool call** `POST /meeting-sessions/:sid/tool-calls` (Pod; called by the in-pod agent session):
```json
{ "call_id": "uuid (idempotency)", "tool": "create_doc|create_slides|create_canvas|update_artifact|<connector tool>", "args": {}, "asked_by": "Jordan", "speaker_confidence": 0.7 }
```
Response (`ToolCallResult`): `{ "status": "ok", "result": {...} }` | `{ "status": "approval_required", "approval_id": "uuid" }` | `{ "status": "denied" }` | `{ "status": "clarify", "question": "..." }` (speaker unclear and tool is personal). An unapproved tool always returns `approval_required` and emits `meeting.approval.requested`; the pod polls the same `call_id` until approved, denied or expired. Artifact tools run in cloud (artifacts_v2, linked in `meeting_artifacts`, `meeting.artifact.created` emitted). Display names are not authenticated; the approval is the owner's click on the signed-in page.

**Control stream** `GET /meeting-sessions/:sid/control-stream` (Pod SSE). Messages: `{type:"stop_listening"|"resume"|"leave"}`, `{type:"speak", text}`, `{type:"set_speak_replies", on}`, `{type:"set_guest_quiet", on}`, `{type:"approval", approval_id, status}`. The pod acks nothing; effects come back as `state` in the next ingest batch.

## 7. Link parsing and 1:1 rule

Parse in this order on the lowercased host; the stored `join_url` keeps the original, `url_key` is the normalised key.

- **Meet:** `^https://meet\.google\.com/([a-z]{3}-[a-z]{4}-[a-z]{3})(?:[/?#].*)?$`. `url_key = meet:<code>`. Also accept `https://meet.google.com/lookup/<id>` (`url_key = meet:lookup:<id>`).
- **Zoom:** `^https://([a-z0-9-]+\.)?zoom\.(us|com)/(?:j|my|wc/join)/(\d{9,11})(?:[/?#].*)?$` (also `/wc/<id>/join`). `url_key = zoom:<digits>`; keep the `pwd` query param in `join_url` only. Personal-room `/my/<name>` has no number: `url_key = zoom:my:<name>`.
- **Teams:** `^https://teams\.microsoft\.com/(?:l/meetup-join/[^\s]+|meet/\d{9,15}(?:\?p=[\w-]+)?)$` and `^https://teams\.live\.com/meet/\d{9,15}`. `url_key = teams:<sha256 of path without query, first 16 hex>` for `meetup-join`, `teams:<digits>` for `/meet/`.
- Anything else: `400 unsupported_link`. URLs found in free text (chat paste, calendar event body, location, `conferenceData`, `onlineMeeting.joinUrl`) are scanned with the same patterns, first match wins; a calendar event's structured conference link beats text.
- **1:1 detection:** `is_one_on_one = (attendee_count == 2)` where `attendee_count` counts every attendee including the user and excluding resource rooms and declined-by-everyone-else cases are NOT special-cased (a declined invite still counts). Events with no attendee list are not 1:1. `all_except_1on1` skips rows with `is_one_on_one = true`. A per-meeting `auto_join` boolean always overrides the rule.

## 8. MCP connector `meetings`

Served through the connectors MCP proxy; owner = the calling user; all tools return 402-equivalent `{error:"meetings_requires_paid_plan"}` as an MCP error on the free plan. Statuses/labels per meetings.json. `page_url = https://ai.allternit.com/meetings/<meeting_id>`. `platform_label`: `Google Meet|Zoom|Microsoft Teams` (recap: `"<platform> · <N> min"` when ended). `people_label`: `"N people"`, `"N people, M outside"`, or `"N people, from your calendar"` on `get_meeting`.

| Tool | Input schema | Output |
|---|---|---|
| `list_meetings` | `{ "type":"object", "properties": { "range": { "enum": ["now","today","week","past"], "default": "today" } }, "additionalProperties": false }` | `{ meetings: [{id,title,platform_label,people_label,gizzi,starts_at,page_url}] }` |
| `get_meeting` | `{ "type":"object", "properties": { "url": {"type":"string"}, "meeting_id": {"type":"string"} }, "minProperties": 1, "additionalProperties": false }` (a url creates nothing; unknown url returns a transient record with `id` of an existing row, else error `unsupported_link`/`meeting_not_found`) | `{ id,title,platform_label,people_label,gizzi,starts_at,join_url,notice }`. `notice` = the in-call consent text, per meetings.json sample. |
| `add_meeting` | `{ "type":"object", "properties": { "url":{"type":"string"}, "title":{"type":"string"}, "starts_at":{"type":"string","format":"date-time"} }, "required":["url"], "additionalProperties": false }` | same as `get_meeting` |
| `join_meeting` | `{ "type":"object", "properties": { "meeting_id":{"type":"string"}, "when":{"enum":["now","start"]} }, "required":["meeting_id","when"], "additionalProperties": false }` | `{ id, gizzi, session_id, page_url }` |
| `set_auto_join` | `{ "type":"object", "properties": { "meeting_id":{"type":"string"}, "on":{"type":"boolean"} }, "required":["meeting_id","on"], "additionalProperties": false }` | `{ id, gizzi, auto_join }` |
| `get_recap` | `{ "type":"object", "properties": { "meeting_id":{"type":"string"} }, "required":["meeting_id"], "additionalProperties": false }` | `{ id,title,platform_label,gizzi,starts_at,decisions,action_items,artifacts,page_url,summary }` where `decisions`, `action_items`, `artifacts` are **counts** (numbers), per `kind: number` columns in meetings.json |
| `get_transcript` | `{ "type":"object", "properties": { "meeting_id":{"type":"string"}, "after_seq":{"type":"integer","minimum":0}, "limit":{"type":"integer","minimum":1,"maximum":2000,"default":500} }, "required":["meeting_id"], "additionalProperties": false }` | `{ id, segments:[{seq,speaker,start_ms,text}], next_after_seq }` |
| `leave_meeting` | `{ "type":"object", "properties": { "meeting_id":{"type":"string"} }, "required":["meeting_id"], "additionalProperties": false }` | `{ id, gizzi }` |

## 9. Surface Matrix (copied from the plan)

| Surface | Status | What |
|---|---|---|
| Desktop | BUILD | Meetings view under Home, live page, settings, org admin panel |
| ai.allternit.com + phone layout | BUILD | Same; drawer entry and phone-sized page |
| m.allternit.com PWA | BUILD | Same code, coarse-pointer sizing |
| iOS | BUILD | Meetings section inside `Features/Calls`, a Settings row, a calendar prompt in Connectors; no new rail tab |
| Android | HAS (via PWA) | Arrives with the PWA deploy; no native hook needed; checked in the Play app |
| Gizzi Code | BUILD | `gizzi meetings list/add/status/transcript/leave` |
| Backend | BUILD | Contract above, pod, worker, scheduler |
| Docs | BUILD | `guides/meetings.mdx`, API page, nav, cross-links |

## 10. UI contract notes (Eoj 11-13)

Meetings list = agenda list: live call on top, per-meeting Gizzi switch (drives `PATCH auto_join`), past calls with recap. Live page = the live voice view look: mascot stage plus side panel tabs Transcript / Artifacts / Approvals; controls Stop listening (`stop_listening`/`resume`), Speak replies (`PATCH /meeting-sessions/:sid`), Leave (`leave`). After connecting a calendar the callback returns `pick_rule=1` and the UI offers the three rules with `all_except_1on1` preselected.


## Change 2026-11-13: ICS calendar links, no Microsoft OAuth in v1
- `calendar_connections.provider` allows `google` | `ics` (`microsoft` kept in the CHECK for later; no Microsoft OAuth routes ship in v1).
- `ics`: the user pastes a private calendar feed URL (Outlook "Publish calendar" ICS link, iCloud public calendar, any https .ics). Stored encrypted (`encrypted_feed_url`). Sync fetches it server-side on the same schedule as Google; meeting links are pulled from LOCATION / DESCRIPTION / URL with the section 7 patterns; attendees from ATTENDEE lines (1:1 rule unchanged; no attendees = not 1:1).
- Route: `POST /api/v1/calendar/connections {provider:'ics', feed_url, label?}` validates by fetching once (must parse as VCALENDAR) and returns the connection; `GET` and `DELETE` as for Google.
- UI connect step: "Connect Google Calendar" and "Paste a calendar link" (help text names Outlook and iCloud).

## Change 2026-11-13: Zoom cloud side (from docs/meetings/ZOOM.md, accepted)
- Table `zoom_connections` (in `130_meetings.sql`): one row per user. Columns:
  - id (uuid PK)
  - user_id (text, UNIQUE)
  - zoom_user_id (text null)
  - encrypted_refresh_token (text)
  - encrypted_access_token (text null)
  - access_expires_at (timestamptz null)
  - scopes (text[])
  - status (CHECK `active|needs_reauth|revoked`, default `active`)
  - created_at, updated_at
  
  Tokens are stored through `credential_cipher`. Refresh happens under `SELECT … FOR UPDATE`.
- Routes, specified in ZOOM.md sections 3–6:
  - `GET /api/v1/zoom/oauth/start`
  - `GET /api/v1/zoom/oauth/callback`
  - `GET|DELETE /api/v1/zoom/connection`
  - `POST /api/v1/zoom/webhook` (deauthorization + url_validation, HMAC-verified)
  - pod-token `POST /api/v1/meeting-sessions/:sid/zoom-tokens` (OBF token + optional local-recording join token, minted just before join)
- Error code `zoom_not_connected` (409) on `POST /meetings/:id/sessions` for a Zoom meeting when the user has no active zoom_connections row. The web/iOS connect step gets a "Connect Zoom" row.
- `BotIdentity.zoom_obf.sdkSecretRef`:
  - `jwt:<SDK JWT signed by cloud-api>` in production; the client secret never reaches the pod.
  - `env:<NAME>` is for local dev only.
- Zoom owner-present retry (decision: yes). While Zoom reports that the authorizing user is not in the meeting, the Zoom adapter retries the OBF join every 20 s, up to `admitTimeoutMs`, and reports phase `waiting_admit`. The page label is "Waiting for you to join Zoom". `PlatformAdapter.join` stays one call from the runner's point of view; the retry is inside the adapter.

## Change 2026-11-13: pod ↔ worker names (from lane 2.1, accepted)
- Pod env: `ALLTERNIT_POD_CONFIG` (JSON or base64) or `ALLTERNIT_POD_CONFIG_FILE`, plus `ALLTERNIT_POD_TOKEN` and `ALLTERNIT_LIVEKIT_TOKEN`.
- LiveKit tracks and topics:
  - The pod publishes track `call-audio` and plays track `tts`.
  - STT segments arrive on data topic `allternit.meeting.stt` as `{start_ms, end_ms, text, confidence?, start_at?}`, timed from when `call-audio` was published.
  - The pod labels each segment's speaker and `seq`, then ingests it.
  - Wake-word turns arrive on data topic `allternit.meeting.turn` as `{turn_id, text, asker_label, confidence, start_ms}`. `text` has the wake word removed; `asker_label` is null when the worker doesn't know the speaker (the pod then labels it from its speaker timeline); `start_ms` is on the `call-audio` clock.
  - The pod sends text to speak on data topic `allternit.meeting.speak` as `{turn_id, text}`. The worker synthesizes it and plays it on its `tts` track.
- Control `speak {text}` means: post this text in meeting chat (unless guest-quiet), and, when `speak_replies` is on, send it to `allternit.meeting.speak` as well.
- Turns (follow-up, lane 2.1):
  - The runner starts a turn on a wake-word message or on a chat message that mentions "Gizzi" (a stop command is never a turn). One turn runs at a time per meeting, up to two wait, and extra turns are dropped.
  - The runner sends the last ~3 min of labelled segments, `participants` with their ingest class, `speak_replies` and `guest_quiet` from the control state, and `artifacts: []`.
  - The runner posts the final `chat` with chat idempotency key `turn:<turn_id>`, the same key allternit-api records the reply under. When the reply is page-only (`route.chat = "notice"`, or `page_only: true`), the runner ingests the full reply as `turn:<turn_id>` with `visibility: page_only`, does not post it, and posts only the notice (key `turn:<turn_id>:notice`). A non-null `speak` goes to `allternit.meeting.speak` when `speak_replies` is on.
- Turn route env for the runner:
  - `ALLTERNIT_MEETING_TURN_URL` (default `http://127.0.0.1:8013`) and `ALLTERNIT_INTERNAL_SERVICE_TOKEN`.
  - The launcher writes these, together with `ALLTERNIT_MEETING_CLOUD_API_URL` / `ALLTERNIT_MEETING_POD_TOKEN`, into `/etc/allternit/meeting-pod.env`. The image loads that file into both the runner and the Allternit app (`allternit-headless.service` drop-in, `ALLTERNIT_API_PORT=8013`).
- Optional pod config JSON fields beside `PodConfig` (adapter.ts unchanged): `meetingTitle` and `ownerName`, both used in the turn and summary prompts.
- End summary: the runner calls `/meeting-turns/summary` with every labelled segment after the session ends, then sends the result as `summary` in the batch that carries `state: ended`.

## Change 2026-11-13: web lane additions (step 1.2)
- `GET /meetings/:id` adds `latest_session`: `{ id, state, end_reason, live_at, ended_at, summary }`, or `null` before Gizzi was sent.
- Every `Meeting` JSON adds `live_at`: the active (not ended) latest session's `live_at`, else `null`.
- Every `Meeting` JSON (list and get) adds `recap`: `{ ready: boolean, decisions: number, action_items: number }` when the latest session ended (`ready` = a summary exists; the counts are the lengths of `decisions` and `action_items`), else `null`. Saves the list one request per row.
- `calendar_connections.label` (text, null) is in `130_meetings.sql`; the `CalendarConnection` JSON returns it as `label: string | null` (ICS calendars show it). The calendar routes ship in lane 2.7.
- Feature flag: `GET /api/v1/me/flags` returns `flags.meetings` (boolean, read-only, default off). On with `ALLTERNIT_MEETINGS_ENABLED=1`, or for user ids in `ALLTERNIT_MEETINGS_USER_IDS`, or for admins (`ALLTERNIT_ADMIN_USER_IDS`). A PUT naming it is a 400.
- `meetings.source` also allows `calendar_ics` (rows synced from an ICS connection).

## Change 2026-11-13: meeting turns (from lane 2.6, accepted)
- **Turn request.** The in-pod allternit-api accepts `POST /api/v1/meeting-turns/turn` and answers with an SSE stream. Auth is `x-allternit-internal-token`, or localhost in dev. The body:
  ```
  {turn_id, session_id,
   trigger: {kind: wake_word|chat_mention, text},
   asker: {label, participant_class: org_member|guest|unknown, confidence},
   meeting: {title, platform, participants: [{name, class}], owner_name},
   transcript: [segments], artifacts: [MeetingArtifact],
   speak_replies, guest_quiet, model?}
  ```
  The final event carries `chat` (text the runner posts in meeting chat) and `speak` (text for the `tts` track, or null).
- **End summary.** `POST /api/v1/meeting-turns/summary` returns `{summary, decisions, action_items: [{text, owner, due}]}`. The runner calls it when the meeting ends.
- **Pod env** for the in-pod allternit-api: `ALLTERNIT_MEETING_CLOUD_API_URL` and `ALLTERNIT_MEETING_POD_TOKEN`. The launcher sets them.
- **Summary carrier (decision).** The pod ingest batch gets an optional `summary: {summary, decisions, action_items}`. Cloud writes it to `meeting_sessions` (summary / decisions / action_items) and emits `meeting.summary`. The recap lane (2.9) reads from there.
- **Page-only replies (decision).** Ingest chat rows get `visibility: chat|page_only`, default `chat`. Cloud emits `meeting.reply` for every `direction: out` row. Rows marked `page_only` are shown only on the meeting page.
- **`/tool-calls` behaviour:**
  - Artifact tools return `{status: "ok", result: {artifact: MeetingArtifact}}`.
  - Approval is re-checked every time the same `call_id` is re-sent; results are idempotent per `call_id`.
  - Connector tools run in cloud, after approval, through the user's connectors.

## Change 2026-11-13: meeting turns, as built in cloud-api (step 1.2)
- `meeting_chat_messages.visibility` (`chat|page_only`, default `chat`) stores the ingest field; `meeting.chat.message` and `meeting.reply` payloads carry `visibility`.
- Ingest `summary` is validated (decisions are at most 100 strings; action items are at most 100 `{text, owner|null, due|null}`), written to `meeting_sessions`, and emitted as `meeting.summary`. It is accepted while the pod token is valid (up to 5 min after the end).
- New table `meeting_tool_calls (session_id, call_id UNIQUE per session, tool, status running|ok|error, result jsonb)` makes personal tool results idempotent per `call_id`. A re-sent call returns the stored result. A `call_id` reused for another tool gets 400.
- Connector tools run in cloud after approval. cloud-api sends one MCP `tools/call` (name = the pod's `tool`, arguments = `args`) over the signed relay to the owner's runtime at `/webhooks/mcp-edge/connectors`, with `x-allternit-mcp-client: allternit-meetings`. This is the same connector union `mcp.allternit.com/mcp/connectors` serves; connected runtimes are tried first.
- More `ToolCallResult` values: `{status:"error", error:"connectors_unavailable"}` (no runtime answered; nothing stored; retry later), `{status:"error", error:"in_progress"}` (the same call is running), `{status:"error", error:"tool_failed", message?|result?}` (stored).
- Limit: the runtime's connectors server refuses tools that need its own confirmation prompt (it never auto-confirms outside callers). The meeting approval does not lift that.

## Change 2026-11-13: Teams display name
Teams guest names allow only letters, numbers, spaces and `- ' . _ @`. On Teams the bot joins as "Gizzi - Allternit" (`teamsDisplayName`). The consent notice and the docs name it that way for Teams.
