# allternit-platform

Python SDK for the [Allternit Platform API](https://docs.allternit.com/api/platform/overview): agents, conversations, numbers, messaging, webhooks and usage.

> **Not yet published to PyPI.** Publishing needs Eoj's OK. Until then, install it from this repo (see below).

- Python 3.9+, standard library only.
- Typed responses (`TypedDict`s) and one method per API operation, generated from `cmd/allternit-cloud-api/openapi/platform-v1.yaml`.
- Sends an `Idempotency-Key` on every POST (random unless you pass one).
- Errors are typed exceptions (`NotFoundError`, `RateLimitError`, …).
- Cursor pagination helpers and streaming replies.

## Install (from the repo, until it is published)

```bash
pip install /path/to/allternit/sdk/platform-python
```

## Quickstart

```python
from allternit_platform import AllternitPlatform, MessageDelta

client = AllternitPlatform()  # reads ALLTERNIT_API_KEY (alt_test_... or alt_live_...)

account = client.accounts.create(name="Lakeside Dental")
agent = client.agents.create(
    account_id=account["id"],
    name="Front desk",
    instructions="Answer questions about Lakeside Dental and book cleanings.",
)
conversation = client.conversations.create(agent["id"])

# Whole reply
reply = client.conversations.send_message(conversation["id"], content="Are you open Saturday?")
print(reply["content"])

# Streamed reply
stream = client.conversations.stream(conversation["id"], content="Can I book a cleaning?")
for event in stream:
    if isinstance(event, MessageDelta):
        print(event.delta, end="", flush=True)
print(stream.final_message["id"])
```

## Errors

```python
from allternit_platform import APIError, NotFoundError, RateLimitError

try:
    client.agents.get("agent_missing")
except NotFoundError as e:
    print(e.code)  # "agent_not_found"
except RateLimitError as e:
    print(f"retry in {e.retry_after}s")
except APIError as e:
    print(e.status, e.type, e.code, e.param, e.request_id)
```

`503` with `code == "runtime_starting"` (an `InternalServerError`) means the project's hosted runtime is starting; retry after a few seconds.

## Pagination

```python
page = client.agents.list(limit=50)       # {"data": [...], "has_more": ..., "next_cursor": ...}
for agent in client.agents.list_all():   # every agent, page by page
    ...
```

## Options

`AllternitPlatform(api_key=None, *, base_url=None, timeout=60.0, default_headers=None)`. Every method takes `timeout=` and `extra_headers=`; POST methods also take `idempotency_key=`. Parameters whose names are Python keywords get a trailing underscore (`from_=`).

## Self-hosted Agents Server

`allternit_platform.selfhost.SelfHostClient` talks to an Agents Server you run yourself (`allternit-agents-server`, private beta). It makes no calls to `*.allternit.com`. Its resources are generated from `cmd/allternit-api/openapi/selfhost-v1.yaml`.

```python
import os
from allternit_platform.selfhost import SelfHostClient, is_turn_end

client = SelfHostClient()  # ALLTERNIT_SERVER_URL (default http://127.0.0.1:8400) + ALLTERNIT_SERVER_API_KEY (ask_...)
agent = client.agents.create(name="Helper", model=os.environ["ALLTERNIT_MODEL"])
env = client.environments.create(name="default", network="none")
session = client.sessions.create(agent_id=agent["id"], environment_id=env["id"])
sent = client.sessions.send_message(session["id"], content="Hello")
for ev in client.sessions.stream_events(session["id"], after_seq=sent["seq"], until=is_turn_end):
    print(ev["type"], ev["data"])
```

`stream_events` reconnects with `Last-Event-ID` after a dropped connection. Full script: `examples/selfhost_session.py`.

### Custom-tool worker

`run_custom_tool_worker` claims work for one environment with an environment key (`aek_...`), renews the lease every 20 seconds in a background thread, takes tool calls with the per-session secret, runs your handler and posts the result (cut to 100 KiB). A stale lease stops the item without posting; setting `stop` (or Ctrl-C) hands the item back with `/release`. Built-in sandbox tools (bash, file edits) are run by `gizzi worker`, so give agents with custom tools their own environment.

```python
from allternit_platform.selfhost import run_custom_tool_worker

run_custom_tool_worker(
    "env_...",
    {"lookup_order": lambda inp, ctx: {"id": inp["order_id"], "status": "shipped"}},
    environment_key=os.environ["ALLTERNIT_ENVIRONMENT_KEY"],
)
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
cd sdk/platform-python && python3 -m unittest discover -s tests
```

`allternit_platform/_generated.py` and `_selfhost_generated.py` are generated; edit the other modules by hand.
