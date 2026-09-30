# OpenClaw adapter

AAI provider over the user's own OpenClaw gateway. Lane `local`, guarantee `exact`, isolation `isolated`, mode hosted.
Auto-registered at boot through `aai.ts` (`createAaiRegistration(env)`).

## Configuration (env, non-secret except the optional token)

| Var | Default | Meaning |
|---|---|---|
| `SUBS_GATEWAY_OPENCLAW_URL` | `http://127.0.0.1:18789` | OpenClaw gateway base URL |
| `SUBS_GATEWAY_OPENCLAW_TOKEN` | unset | Optional bearer token for a gateway with local auth. Sent only to that URL; never logged or put in errors |
| `SUBS_GATEWAY_OPENCLAW_AGENT` | `openclaw` | Agent/model id used when `/v1/models` is not exposed |
| `SUBS_GATEWAY_OPENCLAW_MAX_PARALLEL` | `4` | Concurrent conversations |

## Wire usage

- `agent.list`: `GET /v1/models`, each id becomes agent `openclaw:<id>`. A 404 there means "no listing" and falls back to the configured agent.
- `agent.context.message`: `POST /v1/chat/completions` with `stream: true`, parsed as SSE (`data: {choices:[{delta:{content}}]}` ... `[DONE]`). If the server answers plain JSON instead, that is handled too. Each SSE delta becomes an `agent.message.delta` event.
- `agent.context.cancel`: aborts the in-flight HTTP request (the stream really stops); the cancelled turn is not added to history.
- `agent.health`: `GET /v1/models` (`healthy`, `degraded` if reachable but no listing, `down` otherwise).
- Errors: connection refused / 404 on chat / 5xx -> `VENDOR_UNAVAILABLE` (retryable) with a human message telling the user to start OpenClaw and enable its OpenAI-compatible endpoint; 401/403 -> `AUTH_REQUIRED` (no token set) or `AUTH_REVOKED` (token rejected); 429 -> `RATE_LIMITED` with `retryAfterMs` from `Retry-After`.

## Confidence notes (read before trusting this against a live OpenClaw)

- **Conversation keying is NOT confirmed.** No live OpenClaw was available. The OpenAI-compatible endpoint is stateless per request by spec, so this adapter keeps message history **per context, client-side**, and replays it each turn. It does not send `user` or any session header, so it cannot collide with or accidentally join an OpenClaw-side session. Consequences: contexts are isolated by construction; `resume` (adopt by context id) only works while this gateway process holds the context; history is lost on gateway restart. If OpenClaw turns out to key server-side sessions by a header/`user` value, switching to that (and dropping history replay) is a contained change in `provider.ts`.
- Endpoint paths (`/v1/models`, `/v1/chat/completions`), SSE shape and default port `:18789` follow the spec's OpenClaw row and the OpenAI-compatible convention; they are verified only against `fixtures/fake-server.ts`, a real local HTTP server written from the OpenAI spec, not against OpenClaw itself. OpenClaw's chat endpoint may be disabled in its config by default; a 404 is reported with that hint.
- The model ids returned by `/v1/models` are used verbatim as agent ids (so agent routing like `openclaw/<agent>` works if OpenClaw exposes it).
- Not claimed (answers `UNSUPPORTED`): steer/interrupt, tasks, approvals, memory, tools/skills (ClawHub skills and its 50+ channels run inside OpenClaw and are invisible through chat completions), snapshot/sync, computer.
- Look pack: accent colors are placeholders and the icon is a `TODO:` key (no logo asset in the repo).

## Tests

`test/openclaw-adapter.test.ts` runs `runConformance` against a real local fake gateway (`fixtures/fake-server.ts`), plus streaming, history/isolation, JSON fallback, list fallback, token handling, error mapping and cancel. No network, no OpenClaw.
