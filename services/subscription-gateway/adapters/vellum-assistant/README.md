# vellum-assistant native AAI adapter

Connects to a Vellum personal-assistant runtime (the `assistant/` runtime of vellum-ai/vellum-assistant, pinned commit `8e0a4fd7fcfcd7c22fef9d579e733dc5c4cabec9`). Only the assistant runtime is used. Vellum's workflow and prompt APIs are not.

Endpoints used, all on the runtime origin:
- `GET /v1/identity` (scope `settings.read`): discovery. One assistant per runtime, exposed as agent `vellum-assistant:default`.
- `POST /v1/messages` (scope `chat.write`): send. Body carries `conversationKey` (one per Allternit context), `clientMessageId` (the send's correlation ID), `sourceChannel` and `interface` set to `vellum`. The 202 answer returns `requestId`, `conversationId` and `messageId`.
- `GET /v1/events/tail?conversationId=&fromSeq=` (scope `chat.read`): polls for `message_request_complete` with the exact `requestId`. An `error` event with that `requestId` fails the send.
- `GET /v1/messages?conversationId=&limit=` (scope `chat.read`): reads the reply. The reply is the latest assistant row created at or after the request's own user row.

Credential: `SUBS_GATEWAY_VELLUM_ASSISTANT_TOKEN` holds the bearer JWT from `POST /v1/pair` on the host (scope profile `actor_client_v1`, which includes `chat.read`, `chat.write` and `settings.read`). Mint it on the machine that runs the assistant. The token is short-lived. When it expires the adapter reports `AUTH_REVOKED` and the user must re-pair. The refresh-token flow (`/v1/guardian/refresh`) is not implemented.

Environment: `SUBS_GATEWAY_VELLUM_ASSISTANT_URL` is the loopback origin of the runtime (for example `http://127.0.0.1:<port>`). Non-loopback origins are rejected, as in the hivekeep adapter. If the URL is unset the adapter is inert and returns `AUTH_REQUIRED`. It does not throw at boot.

Unsupported and set false in CAPABILITIES: streaming, steering, interrupt, cancel, tools, MCP, plugins, memory, tasks, approvals, computer use, artifacts. Guarantee is `best_effort`.

Limitations: an event-tail gap (`complete: false`) fails closed and asks the user to check the conversation. Closing a context only stops this adapter from sending into it; no vendor close call is made. Send state and reply cache are held in memory, so there is no cross-process resume or durable deduplication. Source-verified against the pinned commit. Live verification against a running assistant is pending.
