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

## Regenerate and test

```bash
python3 scripts/platform-sdk/generate.py           # from the repo root, after editing platform-v1.yaml
python3 scripts/platform-sdk/generate.py --check   # fails if the generated code is stale
cd sdk/platform-python && python3 -m unittest discover -s tests
```

`allternit_platform/_generated.py` is generated; edit the other modules by hand.
