import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { RemoteToolHost } from "@/runtime/tools/remote-host"
import { Agent } from "@/runtime/loop/agent"
import { PermissionNext } from "@/runtime/tools/guard/permission/next"
import { SecretRedactor } from "@/runtime/session/environment/redactor"
import { Truncate } from "@/runtime/tools/builtins/truncation"
import DESCRIPTION from "@/runtime/tools/builtins/batch.txt"

const DISALLOWED = new Set(["batch"])
const FILTERED_FROM_SUGGESTIONS = new Set(["invalid", "patch", ...DISALLOWED])

export const BatchTool = Tool.define("batch", async () => {
  return {
    description: DESCRIPTION,
    parameters: z.object({
      tool_calls: z
        .array(
          z.object({
            tool: z.string().describe("The name of the tool to execute"),
            parameters: z.object({}).describe("Parameters for the tool"),
          }),
        )
        .min(1, "Provide at least one tool call")
        .describe("Array of tool calls to execute in parallel"),
    }),
    formatValidationError(error) {
      const formattedErrors = error.issues
        .map((issue) => {
          const path = issue.path.length > 0 ? issue.path.join(".") : "root"
          return `  - ${path}: ${issue.message}`
        })
        .join("\n")

      return `Invalid parameters for tool 'batch':\n${formattedErrors}\n\nExpected payload format:\n  [{"tool": "tool_name", "parameters": {...}}, {...}]`
    },
    async execute(params, ctx) {
      const { Session } = await import("@/runtime/session")
      const { Identifier } = await import("@/shared/id/id")

      const toolCalls = params.tool_calls.slice(0, 25)
      const discardedCalls = params.tool_calls.slice(25)

      const { ToolRegistry } = await import("./registry")
      const availableTools = await ToolRegistry.tools({ modelID: "", providerID: "" })
      const toolMap = new Map(availableTools.map((t) => [t.id, t]))

      // Self-hosted: the session's custom tools (a subagent's are its root
      // session's) run through the remote tool host too. Like the model's
      // tool list (LLM.resolveTools), the agent's permissions can turn one off.
      const agentInfo = await Agent.get(ctx.agent).catch(() => undefined)
      const customMap = new Map(
        (RemoteToolHost.enabled() ? await RemoteToolHost.customToolsFor(ctx.sessionID) : [])
          .filter((def) => !toolMap.has(def.name))
          .map((def) => [def.name, def]),
      )
      if (agentInfo && customMap.size) {
        for (const name of PermissionNext.disabled([...customMap.keys()], agentInfo.permission)) customMap.delete(name)
      }

      // Same output handling as a custom tool the model calls directly (SessionPrompt.addSelfhostCustomTools).
      const runCustom = async (name: string, args: Record<string, unknown>, callID: string) => {
        const result = await RemoteToolHost.execute(name, args, { ...ctx, callID })
        const truncated = await Truncate.output(SecretRedactor.text(result.output), {}, agentInfo)
        return {
          title: result.title,
          output: truncated.content,
          metadata: {
            ...SecretRedactor.deep(result.metadata),
            custom: true,
            truncated: truncated.truncated,
            ...(truncated.truncated && { outputPath: truncated.outputPath }),
          },
        }
      }

      const executeCall = async (call: (typeof toolCalls)[0]) => {
        const callStartTime = Date.now()
        const partID = Identifier.ascending("part")

        try {
          if (DISALLOWED.has(call.tool)) {
            throw new Error(
              `Tool '${call.tool}' is not allowed in batch. Disallowed tools: ${Array.from(DISALLOWED).join(", ")}`,
            )
          }

          const tool = toolMap.get(call.tool)
          const custom = tool ? undefined : customMap.get(call.tool)
          if (!tool && !custom) {
            const availableToolsList = [...toolMap.keys(), ...customMap.keys()].filter(
              (name) => !FILTERED_FROM_SUGGESTIONS.has(name),
            )
            throw new Error(
              `Tool '${call.tool}' not in registry. External tools (MCP, environment) cannot be batched - call them directly. Available tools: ${availableToolsList.join(", ")}`,
            )
          }
          // A custom tool's input is checked by its worker handler against its own schema.
          const validatedParams = tool ? tool.parameters.parse(call.parameters) : call.parameters

          await Session.updatePart({
            id: partID,
            messageID: ctx.messageID,
            sessionID: ctx.sessionID,
            type: "tool",
            tool: call.tool,
            callID: partID,
            state: {
              status: "running",
              input: call.parameters,
              time: {
                start: callStartTime,
              },
            },
          })

          // Self-hosted remote tool host: batch calls tools directly (not via
          // ToolDispatcher), so sandbox and custom tools are routed to the
          // worker here too.
          const result: { title: string; output: string; metadata: Record<string, any>; attachments?: any[] } = custom
            ? await runCustom(call.tool, call.parameters as Record<string, unknown>, partID)
            : RemoteToolHost.handles(tool!.id, ctx.sessionID)
              ? await RemoteToolHost.execute(tool!.id, validatedParams, { ...ctx, callID: partID })
              : await tool!.execute(validatedParams, { ...ctx, callID: partID })
          const attachments = result.attachments?.map((attachment) => ({
            ...attachment,
            id: Identifier.ascending("part"),
            sessionID: ctx.sessionID,
            messageID: ctx.messageID,
          }))

          await Session.updatePart({
            id: partID,
            messageID: ctx.messageID,
            sessionID: ctx.sessionID,
            type: "tool",
            tool: call.tool,
            callID: partID,
            state: {
              status: "completed",
              input: call.parameters,
              output: result.output,
              title: result.title,
              metadata: result.metadata,
              attachments,
              time: {
                start: callStartTime,
                end: Date.now(),
              },
            },
          })

          return { success: true as const, tool: call.tool, result }
        } catch (error) {
          await Session.updatePart({
            id: partID,
            messageID: ctx.messageID,
            sessionID: ctx.sessionID,
            type: "tool",
            tool: call.tool,
            callID: partID,
            state: {
              status: "error",
              input: call.parameters,
              error: error instanceof Error ? error.message : String(error),
              time: {
                start: callStartTime,
                end: Date.now(),
              },
            },
          })

          return { success: false as const, tool: call.tool, error }
        }
      }

      const results = await Promise.all(toolCalls.map((call) => executeCall(call)))

      // Add discarded calls as errors
      const now = Date.now()
      for (const call of discardedCalls) {
        const partID = Identifier.ascending("part")
        await Session.updatePart({
          id: partID,
          messageID: ctx.messageID,
          sessionID: ctx.sessionID,
          type: "tool",
          tool: call.tool,
          callID: partID,
          state: {
            status: "error",
            input: call.parameters,
            error: "Maximum of 25 tools allowed in batch",
            time: { start: now, end: now },
          },
        })
        results.push({
          success: false as const,
          tool: call.tool,
          error: new Error("Maximum of 25 tools allowed in batch"),
        })
      }

      const successfulCalls = results.filter((r) => r.success).length
      const failedCalls = results.length - successfulCalls

      const outputMessage =
        failedCalls > 0
          ? `Executed ${successfulCalls}/${results.length} tools successfully. ${failedCalls} failed.`
          : `All ${successfulCalls} tools executed successfully.\n\nKeep using the batch tool for optimal performance in your next response!`

      return {
        title: `Batch execution (${successfulCalls}/${results.length} successful)`,
        output: outputMessage,
        attachments: results.filter((result) => result.success).flatMap((r) => r.result.attachments ?? []),
        metadata: {
          totalCalls: results.length,
          successful: successfulCalls,
          failed: failedCalls,
          tools: params.tool_calls.map((c) => c.tool),
          details: results.map((r) => ({ tool: r.tool, success: r.success })),
        },
      }
    },
  }
})
