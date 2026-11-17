/**
 * resource in the Gizzi TUI: a thin adapter over the shared core
 * (runtime/primitives/resource.ts), the same one the runtime builtin uses.
 * The core bounds its own text.
 */
import { z } from 'zod/v4'
import { Resource } from '@/runtime/primitives/resource'
import { getSessionId } from '../../bootstrap/state.js'
import { buildTool, type ToolDef } from '../../Tool.js'
import { lazySchema } from '../../utils/lazySchema.js'
import { DESCRIPTION, RESOURCE_TOOL_NAME } from './prompt.js'
import { renderToolResultMessage, renderToolUseMessage } from './UI.js'

const inputSchema = lazySchema(() => Resource.Input)
const outputSchema = lazySchema(() => z.object({ summary: z.string(), text: z.string() }))
type OutputSchema = ReturnType<typeof outputSchema>
export type Output = z.infer<OutputSchema>

export const ResourceTool = buildTool({
  name: RESOURCE_TOOL_NAME,
  searchHint: 'read or search saved large tool output and MCP resources by ref',
  maxResultSizeChars: 100_000,
  userFacingName: () => 'Resource',
  get inputSchema() {
    return inputSchema()
  },
  get outputSchema(): OutputSchema {
    return outputSchema()
  },
  isReadOnly: () => true,
  isConcurrencySafe: () => true,
  toAutoClassifierInput: input => input.ref ?? input.action,
  async description() {
    return DESCRIPTION
  },
  async prompt() {
    return DESCRIPTION
  },
  mapToolResultToToolResultBlockParam(output, toolUseID) {
    return { tool_use_id: toolUseID, type: 'tool_result', content: output.text }
  },
  renderToolUseMessage,
  renderToolResultMessage,
  async call(input, context) {
    const summary = input.ref ? `${input.action} ${input.ref}` : 'list refs'
    try {
      const r = await Resource.run(getSessionId(), input, context.abortController?.signal)
      return { data: { summary, text: r.text ?? '' } }
    } catch (error) {
      if (context.abortController?.signal.aborted) throw error
      return { data: { summary: 'Resource not read', text: error instanceof Error ? error.message : String(error) } }
    }
  },
} satisfies ToolDef<any, Output>)
