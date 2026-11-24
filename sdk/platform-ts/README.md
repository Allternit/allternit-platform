# @allternit/platform

TypeScript SDK for the [Allternit Platform API](https://docs.allternit.com/api/platform/overview): agents, conversations, numbers, messaging, webhooks and usage.

> **Not yet published to npm.** Publishing needs Eoj's OK. Until then, install it from this repo (see below).

- Zero runtime dependencies; uses the global `fetch` (Node 18+, Deno, Bun, browsers).
- Typed requests and responses, generated from `cmd/allternit-cloud-api/openapi/platform-v1.yaml`.
- Sends an `Idempotency-Key` on every POST (random unless you pass one).
- Errors are typed classes (`NotFoundError`, `RateLimitError`, …).
- Cursor pagination helpers and streaming replies.

## Install (from the repo, until it is published)

```bash
cd sdk/platform-ts
tsc -p tsconfig.json          # builds dist/ (uses the workspace's TypeScript)
npm install /path/to/allternit/sdk/platform-ts   # in your project
```

Inside this pnpm workspace, depend on it as `"@allternit/platform": "workspace:*"`.

## Quickstart

```ts
import { AllternitPlatform } from "@allternit/platform";

const client = new AllternitPlatform(); // reads ALLTERNIT_API_KEY (alt_test_… or alt_live_…)

const account = await client.accounts.create({ name: "Lakeside Dental" });
const agent = await client.agents.create({
  account_id: account.id,
  name: "Front desk",
  instructions: "Answer questions about Lakeside Dental and book cleanings.",
});
const conversation = await client.conversations.create(agent.id);

// Whole reply
const reply = await client.conversations.sendMessage(conversation.id, { content: "Are you open Saturday?" });
console.log(reply.content);

// Streamed reply
const stream = client.conversations.stream(conversation.id, { content: "Can I book a cleaning?" });
for await (const event of stream) {
  if (event.type === "message.delta") process.stdout.write(event.delta);
}
const stored = await stream.finalMessage();
```

## Errors

```ts
import { NotFoundError, RateLimitError, APIError } from "@allternit/platform";

try {
  await client.agents.get("agent_missing");
} catch (err) {
  if (err instanceof NotFoundError) console.log(err.code); // "agent_not_found"
  else if (err instanceof RateLimitError) console.log(`retry in ${err.retryAfter}s`);
  else if (err instanceof APIError) console.log(err.status, err.type, err.code, err.param, err.requestId);
  else throw err;
}
```

`503` with `code: "runtime_starting"` (an `InternalServerError`) means the project's hosted runtime is starting; retry after a few seconds.

## Pagination

```ts
const page = await client.agents.list({ limit: 50 });        // { data, has_more, next_cursor }
for await (const agent of client.agents.listAll()) { … }     // every agent, page by page
```

## Options

`new AllternitPlatform({ apiKey, baseUrl, timeout, defaultHeaders, fetch })`. Every method takes a last `options` argument: `{ idempotencyKey, signal, timeout, headers, query }`.

## Self-hosted Agents Server

`SelfHostClient` talks to an Agents Server you run yourself (`allternit-agents-server`, private beta). It makes no calls to `*.allternit.com`. Its resources are generated from `cmd/allternit-api/openapi/selfhost-v1.yaml`; import them from `@allternit/platform/selfhost` (the package root exports only `SelfHostClient`, `runCustomToolWorker`, `isTurnEnd` and the `selfhost` namespace, because the generated names overlap with the Platform ones).

```ts
import { SelfHostClient, isTurnEnd } from "@allternit/platform";

const client = new SelfHostClient(); // ALLTERNIT_SERVER_URL (default http://127.0.0.1:8400) + ALLTERNIT_SERVER_API_KEY (ask_…)
const agent = await client.agents.create({ name: "Helper", model: process.env.ALLTERNIT_MODEL! });
const env = await client.environments.create({ name: "default", network: "none" });
const session = await client.sessions.create({ agent_id: agent.id, environment_id: env.id });
const sent = await client.sessions.sendMessage(session.id, { content: "Hello" });
for await (const ev of client.sessions.streamEvents(session.id, { afterSeq: sent.seq, until: isTurnEnd })) {
  console.log(ev.type, ev.data);
}
```

`streamEvents` reconnects with `Last-Event-ID` after a dropped connection. Full script: `examples/selfhost-session.ts`.

### Custom-tool worker

`runCustomToolWorker` claims work for one environment with an environment key (`aek_…`), renews the lease every 20 seconds, takes tool calls with the per-session secret, runs your handler and posts the result (cut to 100 KiB). A stale lease stops the item without posting; aborting `signal` hands the item back with `/release`. Built-in sandbox tools (bash, file edits) are run by `gizzi worker`, so give agents with custom tools their own environment.

```ts
import { runCustomToolWorker } from "@allternit/platform";

await runCustomToolWorker({
  environmentId: "env_…",
  environmentKey: process.env.ALLTERNIT_ENVIRONMENT_KEY,
  tools: { lookup_order: async (input) => ({ id: input.order_id, status: "shipped" }) },
});
```

Protocol: `docs/SELFHOST_WORKER_PROTOCOL.md`. Docs: https://docs.allternit.com/self-hosted/sdks

## Regenerate and test

```bash
python3 scripts/platform-sdk/generate.py           # from the repo root, after editing platform-v1.yaml
python3 scripts/platform-sdk/generate.py --check   # fails if the generated code is stale
python3 scripts/platform-sdk/generate.py \
  --spec cmd/allternit-api/openapi/selfhost-v1.yaml \
  --out sdk/platform-ts/src/selfhost \
  --out-py sdk/platform-python/allternit_platform/_selfhost_generated.py \
  --client-name SelfHostClient   # self-hosted client
cd sdk/platform-ts && node --test test/*.test.ts   # Node 22.6+ (runs the .ts sources directly)
```

`src/generated/` and `src/selfhost/{types,operations}.ts` are generated; edit `src/selfhost/{client,worker,index}.ts`, `src/client.ts`, `src/core.ts` and `src/errors.ts` by hand.
