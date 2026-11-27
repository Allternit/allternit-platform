/**
 * `mcp_script` in the Gizzi terminal (MCP 2027 step 5b): Code Mode for the
 * ink-app, which has its own MCP client and tool loop (the runtime session
 * loop's tool is runtime/tools/builtins/mcp-script.ts). Same sandbox
 * (runtime/services/mcp/script-sandbox.ts), same description, same typed
 * declarations, same limits.
 *
 * Offered by tools.ts `assembleToolPool` when 2+ MCP tools are connected and
 * GIZZI_DISABLE_MCP_CODE_MODE is not set. The tool itself needs no permission;
 * every bridged call asks through the normal MCP permission path (bridge.ts).
 * The model reads only the result (<= 8 KB); the script, the call log and the
 * result ride `mcpMeta.mcpScript` to the row UI.tsx draws.
 */
import { z } from 'zod/v4'
import { ScriptSandbox } from '@/runtime/services/mcp/script-sandbox'
import { McpScriptShared as McpScript } from '@/runtime/tools/builtins/mcp-script-shared'
import DESCRIPTION from '@/runtime/tools/builtins/mcp-script.txt'
import * as React from 'react'
import { Text } from '../../ink.js'
import { buildTool, type ToolDef } from '../../Tool.js'
import { lazySchema } from '../../utils/lazySchema.js'
import { bridgeMcpTools, declarationsFor, mcpToolsOf } from './bridge.js'

export const MCP_SCRIPT_TOOL_NAME = McpScript.ID

const inputSchema = lazySchema(() =>
  z.strictObject({
    script: z
      .string()
      .min(1)
      .describe(
        'TypeScript: the body of an async function. Call tools as `await <server>.<tool>(args)` and `return` the result.',
      ),
  }),
)
type InputSchema = ReturnType<typeof inputSchema>

const outputSchema = lazySchema(() =>
  z.string().describe('What the script returned (JSON text, at most 8 KB)'),
)
type OutputSchema = ReturnType<typeof outputSchema>
export type Output = z.infer<OutputSchema>

function scriptToolUseId(
  parent: { message: { content: unknown } },
  script: string,
): string {
  const blocks = Array.isArray(parent.message.content)
    ? (parent.message.content as Array<Record<string, any>>)
    : []
  const mine = blocks.filter(
    b => b?.type === 'tool_use' && b.name === MCP_SCRIPT_TOOL_NAME,
  )
  const match = mine.find(b => b.input?.script === script) ?? mine[0]
  return typeof match?.id === 'string' ? match.id : `mcp_script-${Date.now()}`
}

export const MCPScriptTool = buildTool({
  name: MCP_SCRIPT_TOOL_NAME,
  searchHint: 'run one script that calls several MCP tools',
  maxResultSizeChars: ScriptSandbox.LIMITS.resultBytes + 200,
  userFacingName: () => 'Script',
  get inputSchema(): InputSchema {
    return inputSchema()
  },
  get outputSchema(): OutputSchema {
    return outputSchema()
  },
  isEnabled() {
    return McpScript.enabled()
  },
  // Each bridged call is checked on its own; the script itself may write.
  isReadOnly() {
    return false
  },
  isConcurrencySafe() {
    return false
  },
  toAutoClassifierInput(input) {
    return input.script
  },
  async description() {
    return DESCRIPTION
  },
  async prompt({ tools }) {
    const decl = declarationsFor(tools)
    return decl ? `${DESCRIPTION}\n\n<declarations>\n${decl}\n</declarations>` : DESCRIPTION
  },
  async checkPermissions(input) {
    return { behavior: 'allow', updatedInput: input }
  },
  mapToolResultToToolResultBlockParam(output, toolUseID) {
    return { tool_use_id: toolUseID, type: 'tool_result', content: output }
  },
  renderToolUseMessage(input) {
    const lines = (input.script ?? '').split('\n').length
    return React.createElement(
      Text,
      null,
      `${lines} line${lines === 1 ? '' : 's'}`,
    )
  },
  // The result row is drawn by UserToolSuccessMessage from mcpMeta.mcpScript (UI.tsx).
  renderToolResultMessage() {
    return null
  },
  async call({ script }, context, canUseTool, parentMessage) {
    const tools = context.options.refreshTools?.() ?? context.options.tools
    if (mcpToolsOf(tools).length < 2) {
      throw new Error('Code Mode needs 2 or more connected MCP tools.')
    }

    let js: string
    try {
      js = McpScript.toJavaScript(script)
    } catch (error) {
      throw new Error(
        `The script has a syntax error: ${error instanceof Error ? error.message : String(error)}`,
      )
    }

    // Time spent in the permission dialog does not count toward the 60 s limit.
    let waitedMs = 0
    const run = await ScriptSandbox.run({
      js,
      abort: context.abortController.signal,
      approvalWaitMs: () => waitedMs,
      bridges: bridgeMcpTools({
        tools,
        context,
        canUseTool,
        parentMessage,
        toolUseID: scriptToolUseId(parentMessage, script),
        onPermissionWait: ms => {
          waitedMs += ms
        },
      }),
    })

    const servers = [...new Set(run.calls.map(c => c.server))]
    const output = run.ok
      ? run.result || '(the script returned nothing)'
      : `The script failed: ${run.error}`
    return {
      data: output,
      mcpMeta: {
        // Display only (PLAN decision 3): the model reads `data` and nothing else.
        mcpScript: {
          lang: 'ts',
          script,
          servers,
          calls: run.calls,
          durationMs: run.durationMs,
          result: run.ok ? run.result : '',
        },
      },
    }
  },
} satisfies ToolDef<InputSchema, Output>)
