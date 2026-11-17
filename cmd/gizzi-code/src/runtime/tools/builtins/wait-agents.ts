/**
 * wait_agents: block until a child agent settles, the user sends a message,
 * or the timeout passes (Primitives Wave B). Every client; permission allow.
 * Core: primitives/agent-control.ts (AgentControl.waitAgents, lane L1).
 */
import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { AgentControl } from "@/runtime/primitives/agent-control"
import {
  WAIT_AGENTS_TIMEOUT_MS,
  WAIT_AGENTS_TOOL_ID,
  type WaitAgentsResult,
} from "@/runtime/primitives/wave-b-types"
import DESCRIPTION from "@/runtime/tools/builtins/wait-agents.txt"

export const parameters = z.object({
  targets: z
    .array(z.string().min(1).max(200))
    .max(50)
    .optional()
    .describe("Child agent ids or name paths to wait on. Leave out to wait on every running child."),
  timeout_ms: z
    .number()
    .int()
    .min(WAIT_AGENTS_TIMEOUT_MS.min)
    .max(WAIT_AGENTS_TIMEOUT_MS.max)
    .optional()
    .describe(`How long to wait, in milliseconds (default ${WAIT_AGENTS_TIMEOUT_MS.default})`),
})

export function waitAgentsTitle(result: WaitAgentsResult): string {
  if (result.woke === "settled") return `${result.settled.length} agent${result.settled.length === 1 ? "" : "s"} finished`
  if (result.woke === "steer") return "New message from the user"
  return "Still running"
}

export const WaitAgentsTool = Tool.define<typeof parameters, WaitAgentsResult>(WAIT_AGENTS_TOOL_ID, {
  description: DESCRIPTION,
  parameters,
  async execute(params, ctx) {
    await ctx.ask({
      permission: WAIT_AGENTS_TOOL_ID,
      patterns: ["*"],
      always: ["*"],
      metadata: { targets: params.targets },
    })
    const result = await AgentControl.waitAgents({
      sessionID: ctx.sessionID,
      abort: ctx.abort,
      targets: params.targets,
      timeout_ms: params.timeout_ms ?? WAIT_AGENTS_TIMEOUT_MS.default,
    })
    return { title: waitAgentsTitle(result), output: JSON.stringify(result), metadata: result }
  },
})
