# @allternit/computer-driver

Drive an Allternit hosted computer (`/v1/computers`) from Claude, OpenAI computer use, or Gemini computer use. Needs a project key with the `computers` scope and the project's hosted driver turned on.

```ts
import { AllternitComputers } from "@allternit/computer-driver"
const client = new AllternitComputers({ apiKey: process.env.ALLTERNIT_API_KEY })
const computer = await client.create({ name: "agent-1" })
```

## Structured UI driving (allternit.computer.v2)

Contract v2 adds ten driver-backed structured members — `read_ui`, `act`, `run_batch`, `verify`, `request_human`, `use_credential`, `run_subtask`, `run_parallel`, `run_skill`, `skills` — typed end to end. `ComputerV2Driver` calls them directly; every adapter also exposes them to the model as the `computer_v2` function tool. The structured members need the Allternit Driver on the computer (this-device today).

```ts
import { ComputerV2Driver } from "@allternit/computer-driver"
const v2 = new ComputerV2Driver({ client, computerId: computer.id, onApproval: async (a) => askAPerson(a) })

// Read the UI as an element tree — no screenshots.
const ui = await v2.read_ui({ app: "Safari" })
const field = ui.elements?.find((e) => e.role === "textfield")
if (field) await v2.act({ id: field.id, op: "set_value", value: "hello@example.com", version: ui.version })

// Hand a bounded step sequence to the fast decision loop instead of clicking through it yourself.
const sub = await v2.run_subtask({
  goal: "Fill the signup form and submit it",
  inputs: [{ name: "email", value: "hello@example.com" }],
  success: [{ role: "button", name: "Submit" }],
})
if (sub.status !== "done") console.log(sub.status, sub.next)
```

### Subtask safety statuses

`run_subtask` / `run_skill` / `run_parallel` results end with a `status`:

- `done` — the goal is met.
- `escalated` — handed back with a `reason` and the current `screen`; continue from there yourself.
- `needs_confirmation` — the next step (`held_step`) needs the person's confirmation; run it yourself with `act`/`run_batch` and the server asks them.
- `paused` — the safety monitor paused the subtask; stop and call `request_human`.
- `denied` — the step isn't allowed on this computer (app/domain lists or a credential binding); find another way.
- `use_api` — make the named API/MCP call yourself (`api.tool`); the screen is untouched.
- `failed` — ran out of steps, budget or a hard error.

### The computer_v2 function tool in a model loop

Each adapter exposes the structured members with the same steering guidance gizzi sends:

```ts
import { computerV2AnthropicTool, runAnthropicComputerV2 } from "@allternit/computer-driver/anthropic"
import { openaiComputerV2Tool, runOpenAIV2Call } from "@allternit/computer-driver"
import { geminiComputerV2Declaration, runGeminiV2Call } from "@allternit/computer-driver"

// Anthropic: put computerV2AnthropicTool() next to the toolset in tools, and answer
// tool_use blocks whose name is "computer_v2" with runAnthropicComputerV2({ client, computerId, onApproval }, name, input).
// OpenAI: put openaiComputerV2Tool() in tools and answer function_call items with
// runOpenAIV2Call({ client, computerId }, call.call_id, JSON.parse(call.arguments)).
// Gemini: put geminiComputerV2Declaration() in functionDeclarations and answer
// functionCall parts with runGeminiV2Call({ client, computerId }, part.functionCall.name, part.functionCall.args).
```

`computerV2Tool()` returns the provider-neutral `{ name, description, schema }` if you wire tools yourself.

## Claude (`computer_toolset_20260801` / `browser_toolset_20260801`)

Requires `@anthropic-ai/sdk` >= 0.132.0 (optional peer).

```ts
import { AllternitComputerToolset } from "@allternit/computer-driver/anthropic"
const toolset = new AllternitComputerToolset({
  client, computerId: computer.id,
  onApproval: async (approval) => askAPerson(approval), // server-held risky calls
})
// pass `toolset` to client.beta.messages.toolRunner({ tools: [toolset, computerV2AnthropicTool()], ... })
```

Every member runs on the server through `POST /v1/computers/{id}/toolset`. When the server holds a call (409 `approval_required`), `onApproval` decides: `true` approves it and resends with the grant, `false` returns the held result to the model as an error. The SDK's own `confirm` option still runs first. Approving with an API key needs the project's `approval_mode` set to `api_key`.

## OpenAI computer use

```ts
import { runOpenAIAction } from "@allternit/computer-driver"
const { output } = await runOpenAIAction({ client, computerId, display: { width: 1280, height: 800 } }, call.call_id, call.action)
// push `output` into the next responses.create input
```

## Gemini computer use

```ts
import { runGeminiCall } from "@allternit/computer-driver"
const { functionResponse, inlineData } = await runGeminiCall({ client, computerId }, part.functionCall)
```

Gemini's 0–999 coordinates are sent as `coordinate_space: "normalized_1000"`; the server scales them.

The OpenAI and Gemini pixel adapters let `ApprovalRequiredError` propagate; catch it, call `client.approve()`, and resend the action.

## Errors

- `ApprovalRequiredError` — 409 `approval_required`; carries `approval` (id, member, `approve_url`) and the held `result`. `client.toolsetWithApproval(id, call, onApproval)` approves and resends with the `approval_grant` for you.
- `ComputerV2Error` — a structured member answered `is_error: true`; carries the member name and raw `result`.
- `ComputerBusyError` — 423 `computer_busy` / `computer_controlled_elsewhere`.
- `SandboxRequiredError` — 409 `sandbox_required` (the call needs a sandbox computer).
- `ComputerConflictError` — 409 `computer_conflict` (another subtask or lease conflicts).
- `AllternitApiError` — everything else, with `status`, `code`, `type`.
