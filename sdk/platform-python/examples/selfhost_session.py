"""Run one session on your self-hosted Agents Server and print its events.

    export ALLTERNIT_SERVER_URL=http://127.0.0.1:8400
    export ALLTERNIT_SERVER_API_KEY=ask_...   # from `allternit-agents-server admin setup`
    export ALLTERNIT_MODEL=...                # a model the server accepts (a model_map key on a gateway)
    python3 examples/selfhost_session.py

A model-only reply needs no worker. If the agent calls a tool, start a worker
for the environment (``gizzi worker``) or the session waits in ``waiting_for_worker``.
"""

import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))  # run from a checkout

from allternit_platform.selfhost import SelfHostClient, is_turn_end  # noqa: E402

model = os.environ.get("ALLTERNIT_MODEL")
if not model:
    sys.exit("Set ALLTERNIT_MODEL to a model your server accepts.")

client = SelfHostClient()  # ALLTERNIT_SERVER_URL + ALLTERNIT_SERVER_API_KEY

info = client.server.get_info()
print(f"server {info['version']}, provider {info['provider']}")

agent = client.agents.create(name="SDK example", model=model, instructions="Answer in one short paragraph.")
environment = client.environments.create(name="sdk-example", network="none")
session = client.sessions.create(agent_id=agent["id"], environment_id=environment["id"], title="SDK example")

sent = client.sessions.send_message(session["id"], content="In two sentences, what is a work queue?")

for event in client.sessions.stream_events(session["id"], after_seq=sent["seq"], until=is_turn_end):
    data = event["data"]
    if event["type"] == "message":
        print(f"{data['role']}: {data['content']}")
    elif event["type"] == "tool_call":
        print(f"tool call {data['name']} ({data['tool_call_id']})")
    elif event["type"] == "tool_result":
        print(f"tool result {data['tool_call_id']}" + (" (error)" if data.get("is_error") else ""))
    elif event["type"] == "status":
        print(f"status {data['status']}" + (f" ({data['reason']})" if data.get("reason") else ""))
    elif event["type"] == "error":
        print(f"error {data['code']}: {data['message']}")

client.sessions.archive(session["id"])
print(f"done: session {session['id']} archived")
