# sidemates native AAI adapter

Connects to a Sidemates app running on the subscription gateway computer through its local HTTP API (`OPENBOT_PORT`, default 4311). Upstream: `PrisacariuRobert/sidemates` at commit `535857970f775887c6eb4c8043d9d13ee24f7173`, `src/server/index.ts`.

Endpoints used (all with `Authorization: Bearer <access key>`):
- `GET /api/state`: the teammate roster (`bots[]` with `id`, `name`, `threadId`, `retiredAt`).
- `POST /api/messages`: `{threadId, body, requestId, targetBotIds:[botId]}`; returns `runIds`.
- `GET /api/runs/:id`: run `status` (`completed`, `awaiting_approval`, `failed`, `cancelled`).
- `GET /api/threads/:id/messages?limit=200`: the reply is the last `senderType: "bot"` message whose `runId` matches the send.

Credential: the Sidemates access key entered in the app (the value in `access.token` in its data folder, or a paired device key). Set `SUBS_GATEWAY_SIDEMATES_URL` (loopback origin only, for example `http://127.0.0.1:4311`) and `SUBS_GATEWAY_SIDEMATES_TOKEN`. With the URL unset the adapter is inert and returns `AUTH_REQUIRED`.

Context: a context is a handle on the teammate's own thread, so `isolation` is `shared`; the user's own chat with that teammate is in the same thread. Closing a context does not delete or end the conversation.

Unsupported (all `false`): streaming, steer, interrupt, cancel, teammate memory, tools, tasks, approvals, computer, artifacts. Guarantee is `best_effort`: a send is retried only with the same correlation ID, and the reply is attributed by run ID. A run that waits for approval in the app is reported as `POLICY_DENIED`, not completed.

Tests: not run in this change (typecheck only). No live Sidemates account has been used against this adapter yet.
