/**
 * Code Mode in the terminal (MCP 2027 step 5b): turns the ink-app's connected
 * MCP tools into the `<server>.<tool>` functions a script can call.
 *
 * Each bridged call takes the same path a model MCP tool call takes in
 * services/tools/toolExecution.ts: the user's PreToolUse hooks
 * (`runPreToolUseHooks`), `resolveHookPermissionDecision` → `canUseTool`
 * (settings allow/deny rules, the permission dialog, auto mode), the tool's own
 * `call` (services/mcp/client.ts → callMCPTool), then the PostToolUse or
 * PostToolUseFailure hooks. A hook or permission denial throws, so the call
 * rejects inside the script.
 */
import type { ScriptSandbox } from '@/runtime/services/mcp/script-sandbox'
import { McpScriptShared as McpScript } from '@/runtime/tools/builtins/mcp-script-shared'
import type { CanUseToolFn } from '../../hooks/useCanUseTool.js'
import {
  resolveHookPermissionDecision,
  runPostToolUseFailureHooks,
  runPostToolUseHooks,
  runPreToolUseHooks,
} from '../../services/tools/toolHooks.js'
import type { PermissionResult } from '../../utils/permissions/PermissionResult.js'
import type { Tool, Tools, ToolUseContext } from '../../Tool.js'
import type { AssistantMessage } from '../../types/message.js'

/** The connected MCP tools a script can call (the same set the model sees). */
export function mcpToolsOf(tools: Tools): Tool[] {
  return tools.filter(t => t.isMcp === true && t.mcpInfo !== undefined)
}

/** True when `mcp_script` should be offered: flag on and 2+ MCP tools connected. */
export function shouldOfferMcpScript(mcpTools: Tools): boolean {
  return McpScript.enabled() && mcpToolsOf(mcpTools).length >= 2
}

/** Script label for a tool: the server's own name, or the connector name for proxied app tools. */
export function labelOf(tool: Tool): { server: string; tool: string } {
  const info = tool.mcpInfo!
  return McpScript.bridgeName(
    {
      serverName: info.serverName,
      originalName: info.toolName,
      appName: info.connector?.name,
      connectorId: info.connector?.id,
    },
    tool.name,
  )
}

/** Typed declarations for the prompt, built from each tool's input schema. */
export function declarationsFor(tools: Tools): string {
  return McpScript.declarations(
    mcpToolsOf(tools).map(t => ({
      ...labelOf(t),
      inputSchema: t.inputJSONSchema,
    })),
  )
}

/** What the tool's call returned, as the value the script receives. */
function scriptValueOf(data: unknown, structuredContent: unknown): unknown {
  const content =
    typeof data === 'string' ? [{ type: 'text', text: data }] : data
  return McpScript.scriptValue({ content, structuredContent })
}

export interface BridgeInput {
  tools: Tools
  context: ToolUseContext
  canUseTool: CanUseToolFn
  parentMessage: AssistantMessage
  /** The mcp_script tool_use id; inner calls get `<id>:<n>`. */
  toolUseID: string
  /** Called with the ms each permission decision took (excluded from the run budget). */
  onPermissionWait?: (ms: number) => void
}

export function bridgeMcpTools(input: BridgeInput): ScriptSandbox.Bridge[] {
  let n = 0
  return mcpToolsOf(input.tools).map(tool => {
    const label = labelOf(tool)
    return {
      ...label,
      async call(args: Record<string, unknown>): Promise<unknown> {
        const innerID = `${input.toolUseID}:${++n}`
        const { context, canUseTool, parentMessage } = input
        // Loaded on call: toolExecution imports tools.ts, which imports this tool.
        const { getMcpServerType, getMcpServerBaseUrlFromToolName } =
          await import('../../services/tools/toolExecution.js')
        const mcpClients = context.options.mcpClients ?? []
        const serverType = getMcpServerType(tool.name, mcpClients)
        const serverBaseUrl = getMcpServerBaseUrlFromToolName(
          tool.name,
          mcpClients,
        )
        const messageId = parentMessage.message.id
        const requestId = parentMessage.requestId

        // The user's PreToolUse hooks, exactly as toolExecution runs them.
        let callArgs: Record<string, unknown> = args
        let hookPermissionResult: PermissionResult | undefined
        let stopReason: string | undefined
        for await (const r of runPreToolUseHooks(
          context,
          tool,
          callArgs,
          innerID,
          messageId,
          requestId,
          serverType,
          serverBaseUrl,
        )) {
          if (r.type === 'hookPermissionResult')
            hookPermissionResult = r.hookPermissionResult
          else if (r.type === 'hookUpdatedInput') callArgs = r.updatedInput
          else if (r.type === 'stopReason') stopReason = r.stopReason
          else if (r.type === 'stop')
            throw new Error(
              stopReason ||
                `A PreToolUse hook stopped ${label.server}.${label.tool}.`,
            )
        }

        const asked = Date.now()
        const { decision, input: askedInput } =
          await resolveHookPermissionDecision(
            hookPermissionResult,
            tool,
            callArgs,
            context,
            canUseTool,
            parentMessage,
            innerID,
          )
        input.onPermissionWait?.(Date.now() - asked)
        if (decision.behavior !== 'allow') {
          throw new Error(
            ('message' in decision && decision.message) ||
              `Permission to use ${label.server}.${label.tool} was denied.`,
          )
        }
        callArgs =
          (decision.updatedInput as Record<string, unknown> | undefined) ??
          askedInput

        let result: Awaited<ReturnType<Tool['call']>>
        try {
          result = await tool.call(callArgs, context, canUseTool, parentMessage)
        } catch (error) {
          const message = error instanceof Error ? error.message : String(error)
          for await (const _ of runPostToolUseFailureHooks(
            context,
            tool,
            innerID,
            messageId,
            callArgs,
            message,
            context.abortController.signal.aborted,
            requestId,
            serverType,
            serverBaseUrl,
          )) {
            // Hook output is not shown inside a script.
          }
          throw error
        }

        // PostToolUse hooks may rewrite an MCP tool's output, as in toolExecution.
        let data: unknown = result.data
        for await (const r of runPostToolUseHooks(
          context,
          tool,
          innerID,
          messageId,
          callArgs,
          data,
          requestId,
          serverType,
          serverBaseUrl,
        )) {
          if ('updatedMCPToolOutput' in r) data = r.updatedMCPToolOutput
        }
        return scriptValueOf(data, result.mcpMeta?.structuredContent)
      },
    }
  })
}
