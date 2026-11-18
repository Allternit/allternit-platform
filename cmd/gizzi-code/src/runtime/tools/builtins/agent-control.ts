/**
 * agent_control: list, interrupt, resume, follow up on and close your own
 * child agents (Primitives Wave B). Every client.
 *
 * Permission: one class, `agent_control`, with the action as the pattern. The
 * defaults (loop/agent.ts) allow everything but `close`, which asks; "always"
 * covers only the action it was given for.
 *
 * Core: primitives/agent-control.ts (AgentControl.run, lane L1).
 */
import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { AgentControl } from "@/runtime/primitives/agent-control"
import {
  AGENT_CONTROL_ACTIONS,
  AGENT_CONTROL_TARGETED,
  AGENT_CONTROL_TOOL_ID,
  type AgentControlResult,
} from "@/runtime/primitives/wave-b-types"
import DESCRIPTION from "@/runtime/tools/builtins/agent-control.txt"

export const parameters = z.object({
  action: z.enum(AGENT_CONTROL_ACTIONS).describe("list, interrupt, resume, followup or close"),
  target: z
    .string()
    .min(1)
    .max(200)
    .optional()
    .describe("The agent's id or name path (such as /root/build/tests). Needed for every action but list."),
  message: z.string().min(1).max(20_000).optional().describe("followup: what to tell the agent"),
  path_prefix: z.string().max(200).optional().describe("list: only agents whose name path starts with this"),
  reason: z.string().max(500).optional().describe("interrupt, close: why, shown on the agent's row"),
})

export type AgentControlParams = z.infer<typeof parameters>

/** A model-readable error for input the schema can't express, or undefined when the input is fine. */
export function agentControlInputError(params: AgentControlParams): string | undefined {
  if (AGENT_CONTROL_TARGETED.includes(params.action) && !params.target)
    return `agent_control ${params.action} needs a target: an agent id or name path. Use action "list" to see them.`
  if (params.action === "followup" && !params.message) return "agent_control followup needs a message."
  return undefined
}

export function agentControlTitle(result: AgentControlResult): string {
  switch (result.action) {
    case "list":
      return `${result.agents.length} agent${result.agents.length === 1 ? "" : "s"}`
    case "close":
      return `Closed ${result.closed.length} agent${result.closed.length === 1 ? "" : "s"}`
    default: {
      const label = result.target.path ?? result.target.name ?? result.target.id
      return `${result.action} ${label}: ${result.outcome.replace("_", " ")}`
    }
  }
}

export const AgentControlTool = Tool.define<typeof parameters, AgentControlResult>(AGENT_CONTROL_TOOL_ID, {
  description: DESCRIPTION,
  parameters,
  async execute(params, ctx) {
    // Checked before the ask so the user is never asked to approve a call that would be refused.
    const error = agentControlInputError(params)
    if (error) throw new Error(error)
    await ctx.ask({
      permission: AGENT_CONTROL_TOOL_ID,
      patterns: [params.action],
      always: [params.action],
      metadata: { action: params.action, target: params.target, reason: params.reason },
    })
    const result = await AgentControl.run({ sessionID: ctx.sessionID, abort: ctx.abort, ...params })
    return { title: agentControlTitle(result), output: JSON.stringify(result), metadata: result }
  },
})
