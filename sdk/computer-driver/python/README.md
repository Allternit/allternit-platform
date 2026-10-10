# allternit-computer-driver

Drive an Allternit hosted computer (`/v1/computers`) from Claude, OpenAI computer use, or Gemini computer use. Standard library only; `pip install allternit-computer-driver[anthropic]` adds the Anthropic SDK. Needs a project key with the `computers` scope and the hosted driver turned on for the project.

```python
from allternit_computer_driver import AllternitComputers
client = AllternitComputers()  # reads ALLTERNIT_API_KEY
computer = client.create(name="agent-1")
```

## Structured UI driving (allternit.computer.v2)

Contract v2 adds ten driver-backed structured members — `read_ui`, `act`, `run_batch`, `verify`, `request_human`, `use_credential`, `run_subtask`, `run_parallel`, `run_skill`, `skills` — typed end to end. `ComputerV2Driver` calls them directly; every adapter also exposes them to the model as the `computer_v2` function tool. The structured members need the Allternit Driver on the computer (this-device today).

```python
from allternit_computer_driver import ComputerV2Driver

v2 = ComputerV2Driver(client, computer["id"], on_approval=lambda a: ask_a_person(a))

# Read the UI as an element tree — no screenshots.
ui = v2.read_ui({"app": "Safari"})
field = next((e for e in ui.get("elements", []) if e.get("role") == "textfield"), None)
if field:
    v2.act({"id": field["id"], "op": "set_value", "value": "hello@example.com", "version": ui["version"]})

# Hand a bounded step sequence to the fast decision loop instead of clicking through it yourself.
sub = v2.run_subtask({
    "goal": "Fill the signup form and submit it",
    "inputs": [{"name": "email", "value": "hello@example.com"}],
    "success": [{"role": "button", "name": "Submit"}],
})
if sub["status"] != "done":
    print(sub["status"], sub.get("next"))
```

### Subtask safety statuses

`run_subtask` / `run_skill` / `run_parallel` results end with a `status`:

- `done` — the goal is met.
- `escalated` — handed back with a `reason` and the current `screen`; continue from there yourself.
- `needs_confirmation` — the next step (`held_step`) needs the person's confirmation; run it yourself with `act`/`run_batch` and the server asks them.
- `paused` — the safety monitor paused the subtask; stop and call `request_human`.
- `denied` — the step isn't allowed on this computer (app/domain lists or a credential binding); find another way.
- `use_api` — make the named API/MCP call yourself (`api["tool"]`); the screen is untouched.
- `failed` — ran out of steps, budget or a hard error.

### The computer_v2 function tool in a model loop

Each adapter exposes the structured members with the same steering guidance gizzi sends:

```python
from allternit_computer_driver.anthropic import ComputerV2Tool  # to_dict()/tool_result(tool_use)
from allternit_computer_driver import (
    openai_computer_v2_tool, run_openai_v2_call,
    gemini_computer_v2_declaration, run_gemini_v2_call,
    computer_v2_tool,
)

# Anthropic: put ComputerV2Tool(client, computer_id).to_dict() next to the toolset in tools.
# OpenAI: put openai_computer_v2_tool() in tools; answer function_call items with
# run_openai_v2_call(client, computer_id, call.call_id, json.loads(call.arguments)).
# Gemini: put gemini_computer_v2_declaration() in function_declarations; answer
# functionCall parts with run_gemini_v2_call(client, computer_id, part.function_call.name, dict(part.function_call.args)).
```

`computer_v2_tool()` returns the provider-neutral `{"name", "description", "schema"}` dict if you wire tools yourself.

## Claude

```python
from allternit_computer_driver.anthropic import AllternitComputerToolset, ComputerV2Tool
toolset = AllternitComputerToolset(client, computer["id"], on_approval=lambda approval: ask_a_person(approval))
v2_tool = ComputerV2Tool(client, computer["id"], on_approval=lambda approval: ask_a_person(approval))
# tools=[toolset.to_dict(), v2_tool.to_dict()]  (the toolset subclasses the SDK's abstract toolsets when present)
```

Each member runs on the server through `POST /v1/computers/{id}/toolset`. A 409 `approval_required` calls `on_approval`: `True` approves it and resends with the grant, `False` returns the held result to the model as an error. When the installed `anthropic` package has `anthropic.tools.computer` / `anthropic.tools.browser`, these classes subclass its abstract toolsets. Older releases get a small stand-in with `to_dict()` and `tool_result(tool_use)` for a hand-written loop.

## OpenAI computer use

```python
from allternit_computer_driver import run_openai_action
step = run_openai_action(client, computer_id, call.call_id, call.action.model_dump(), display={"width": 1280, "height": 800})
next_input.append(step["output"])
```

## Gemini computer use

```python
from allternit_computer_driver import run_gemini_call
step = run_gemini_call(client, computer_id, fc.name, dict(fc.args))
# step["function_response"] and step["inline_data"] (0-999 coords sent as normalized_1000)
```

The OpenAI and Gemini pixel adapters raise `ApprovalRequiredError`; call `client.approve()` and resend.

## Errors

- `ApprovalRequiredError` — 409 `approval_required`; carries `approval` (id, member, `approve_url`) and the held `result`. `client.toolset_with_approval(computer_id, call, on_approval)` approves and resends with the `approval_grant` for you.
- `ComputerV2Error` — a structured member answered `is_error: True`; carries the member name and raw `result`.
- `ComputerBusyError` — 423 `computer_busy` / `computer_controlled_elsewhere`.
- `SandboxRequiredError` — 409 `sandbox_required` (the call needs a sandbox computer).
- `ComputerConflictError` — 409 `computer_conflict` (another subtask or lease conflicts).
- `AllternitApiError` — everything else, with `status`, `code` and `type`.
