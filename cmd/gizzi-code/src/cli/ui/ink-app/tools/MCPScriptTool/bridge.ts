/**
 * Code Mode in the terminal (MCP 2027 step 5b): turns the ink-app's connected
 * MCP tools into the `<server>.<tool>` functions a script can call.
 *
 * Each bridged call takes the same path a model MCP tool call takes in
 * services/tools/toolExecution.ts: `resolveHookPermissionDecision` →
 * `canUseTool` (settings allow/deny rules, the permission dialog, auto mode),
 * then the tool's own `call` (services/mcp/client.ts → callMCPTool). A denied
 * call throws, so it rejects inside the script.
 */
import type { ScriptSandbox } from '@/runtime/services/mcp/script-sandbox'
import { McpScript } from '@/runtime/tools/builtins/mcp-script'
import type { CanUseToolFn } from '../../hooks/useCanUseTool.js'
import { resolveHookPermissionDecision } from '../../services/tools/toolHooks.js'
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
      key: t.name,
      ...labelOf(t),
      description: undefined,
      inputSchema: t.inputJSONSchema,
      call: async () => undefined,
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
        const asked = Date.now()
        const { decision, input: askedInput } =
          await resolveHookPermissionDecision(
            undefined,
            tool,
            args,
            input.context,
            input.canUseTool,
            input.parentMessage,
            innerID,
          )
        input.onPermissionWait?.(Date.now() - asked)
        if (decision.behavior !== 'allow') {
          throw new Error(
            ('message' in decision && decision.message) ||
              `Permission to use ${label.server}.${label.tool} was denied.`,
          )
        }
        const callArgs =
          (decision.updatedInput as Record<string, unknown> | undefined) ??
          askedInput
        const result = await tool.call(
          callArgs,
          input.context,
          input.canUseTool,
          input.parentMessage,
        )
        return scriptValueOf(result.data, result.mcpMeta?.structuredContent)
      },
    }
  })
}
