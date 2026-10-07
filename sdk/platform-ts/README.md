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

## Regenerate and test

```bash
python3 scripts/platform-sdk/generate.py           # from the repo root, after editing platform-v1.yaml
python3 scripts/platform-sdk/generate.py --check   # fails if the generated code is stale
cd sdk/platform-ts && node --test test/*.test.ts   # Node 22.6+ (runs the .ts sources directly)
```

`src/generated/` is generated; edit `src/client.ts`, `src/core.ts` and `src/errors.ts` by hand.
