# nanomuse native AAI adapter

Connects to the authenticated agent API of `nanomuse serve` on the subscription gateway computer (the nanoMuse Desktop runtime; default `http://127.0.0.1:8787`). Upstream: `nano-muse/nanoMuse` at commit `2ab584c0916dc0d03ed7d2b3f65e78077fa29c25`, `nanomuse/server/api.py` and `nanomuse/service.py`, documented in `docs/app.md` ("Everything the app does goes through this API").

Endpoints used (all with `Authorization: Bearer <server token>`):
- `GET /api/state`: the agent profile (`profile.name`). One `nanomuse serve` process is one agent, listed as `nanomuse:agent`.
- `POST /api/threads` `{title}`: a new conversation for each context (isolated).
- `POST /api/threads/:id/send` `{text}`: queues the message; returns the user `event.id`.
- `GET /api/threads/:id/events?limit=1000`: the reply is the first `assistant` event with `final: true` after that user event, in the same thread. The thread's `busy` flag tells when the turn has ended.

Credential: the server token. `nanomuse serve` writes it to `<data_dir>/server_token` (or the `server.token` / `NANOMUSE_SERVER_TOKEN` setting). Set `SUBS_GATEWAY_NANOMUSE_URL` (loopback origin only) and `SUBS_GATEWAY_NANOMUSE_TOKEN`. With the URL unset the adapter is inert and returns `AUTH_REQUIRED`. Phone runtimes do not expose this API (the Android and iOS agents run inside the app and have no port), so only a computer running `nanomuse serve` can be connected.

Context: each context is a new thread, so `isolation` is `isolated`. Closing a context ends only the Allternit handle; the thread stays on the computer.

Unsupported (all `false`): streaming, steer, interrupt, cancel (the upstream `POST /api/threads/:id/stop` exists but is not wired), agent memory, tools, tasks, approvals, computer, artifacts. Guarantee is `best_effort`. Replies are attributed by the user event ID. A turn that ends without a final reply is reported as `VENDOR_UNAVAILABLE`.

Tests: not run in this change (typecheck only). No live nanoMuse server has been used against this adapter yet.
