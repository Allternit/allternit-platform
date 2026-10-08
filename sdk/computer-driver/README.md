# @allternit/computer-driver

Drive an Allternit hosted computer (`/v1/computers`) from Claude, OpenAI computer use, or Gemini computer use. Needs a project key with the `computers` scope and the project's hosted driver turned on.

```ts
import { AllternitComputers } from "@allternit/computer-driver"
const client = new AllternitComputers({ apiKey: process.env.ALLTERNIT_API_KEY })
const computer = await client.create({ name: "agent-1" })
```

## Claude (`computer_toolset_20260801` / `browser_toolset_20260801`)

Requires `@anthropic-ai/sdk` >= 0.132.0 (optional peer).

```ts
import { AllternitComputerToolset } from "@allternit/computer-driver/anthropic"
const toolset = new AllternitComputerToolset({
  client, computerId: computer.id,
  onApproval: async (approval) => askAPerson(approval), // server-held risky calls
})
// pass `toolset` to client.beta.messages.toolRunner({ tools: [toolset], ... })
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

The OpenAI and Gemini adapters let `ApprovalRequiredError` propagate; catch it, call `client.approve()`, and resend the action.

Errors: `ApprovalRequiredError` (409, carries `approval` and `result`) and `AllternitApiError` (everything else, with `status`, `code`, `type`).
