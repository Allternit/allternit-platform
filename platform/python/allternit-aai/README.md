# allternit-aai

Python client for the Allternit Agent Interface (AAI) REST surface. See [docs/gateway/AAI_REST.md](../../../docs/gateway/AAI_REST.md). TypeScript twin: `@allternit/aai-sdk`.

```python
from allternit_aai import AllternitAgents, ApprovalRequiredError

aai = AllternitAgents("https://api.allternit.com", token=token)
acct = aai.create_account("grok", "api_key")["account"]
aai.set_secret(acct["id"], api_key)
agents = aai.discover_agents(acct["id"])["agents"]
aai.bind_execution(bot_id, vendor="grok", account_binding_id=acct["id"], external_agent_id=agents[0]["externalAgentId"])

try:
    aai.send_turn(session_id, "delete the report")
except ApprovalRequiredError as e:   # 428
    print("needs a person:", e.approval_id)

for ev in aai.stream_events(thread_id, after=0):
    print(ev["sequence"], ev["type"])

aai.respond_approval(approval_id, "approve", human_intent=True)  # refused without explicit intent
```

Tests: `pip install -e .[test] && pytest`.
