# allternit-computer-driver

Drive an Allternit hosted computer (`/v1/computers`) from Claude, OpenAI computer use, or Gemini computer use. Standard library only; `pip install allternit-computer-driver[anthropic]` adds the Anthropic SDK. Needs a project key with the `computers` scope and the hosted driver turned on for the project.

```python
from allternit_computer_driver import AllternitComputers
client = AllternitComputers()  # reads ALLTERNIT_API_KEY
computer = client.create(name="agent-1")
```

## Claude

```python
from allternit_computer_driver.anthropic import AllternitComputerToolset
toolset = AllternitComputerToolset(client, computer["id"], on_approval=lambda approval: ask_a_person(approval))
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

The OpenAI and Gemini adapters raise `ApprovalRequiredError`; call `client.approve()` and resend. All other failures raise `AllternitApiError` with `status`, `code` and `type`.
