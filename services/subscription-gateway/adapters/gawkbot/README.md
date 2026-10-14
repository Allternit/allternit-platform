# gawkbot native AAI adapter

Primary source: https://github.com/najmuzzaman-mohammad/gawkbot at commit 527fe15225d85c1446cb556ad248ab2439f9f8ea (internal/team/broker_auth.go withAuth, broker_office_members.go, broker_channel_create.go, broker_messages.go, broker_session_messaging.go).

Uses the gawkbot broker's bearer API at `http://127.0.0.1:7890` (override `SUBS_GATEWAY_GAWKBOT_URL`; must be loopback). The credential is the broker token from your Sessions computer, sent as `Authorization: Bearer`, from the sealed account credential or deployment `SUBS_GATEWAY_GAWKBOT_TOKEN`. Routes used: `GET /office-members`, `POST /channels/dm` (direct message with one member), `POST /messages`, `GET /messages?channel=&since_id=`.

Each office bot is one agent. Opening a context creates or reuses its 1:1 direct message. Each send posts into that DM as `human` with the bot tagged. The reply is the bot's message that replies to our message ID, or else its first message after ours; nothing is matched by time alone.

Terminal session members (`provider.kind = local-session`) are listed as `unavailable`. gawkbot delivers to them only for messages posted with the operator key that its own web UI holds in memory. This adapter does not have that key and does not try to obtain it, so those members report `POLICY_DENIED` per member and the rest of the roster still works.

Gawkbot's delivery to its members is handled by gawkbot. No cancellation, approvals, memory or computer control is advertised. Send outcomes and contexts are kept in process; nothing resumes across gateway restarts.

Source-based checks only: static type-check passes. Live verification against a running gawkbot broker remains pending.
