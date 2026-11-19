/**
 * status_update in the Gizzi TUI: prompt-gap contract stub (G3). Same id, input
 * schema and description as the runtime builtin
 * (runtime/tools/builtins/status-update.ts), and enabled only while that module's
 * `available` is true, so flipping it there turns on both. tools.ts already
 * registers this adapter; G4-5 (PR8b) builds it out in this folder and never
 * edits tools.ts.
 */
import { available } from '@/runtime/tools/builtins/status-update'
import { STATUS_UPDATE_TOOL_ID, StatusUpdateInput } from '@/runtime/primitives/gap-types'
import DESCRIPTION_TXT from '@/runtime/tools/builtins/status-update.txt'
import { buildTool, type ToolDef } from '../../Tool.js'

export const STATUS_UPDATE_TOOL_NAME = STATUS_UPDATE_TOOL_ID

type InputSchema = typeof StatusUpdateInput
type Output = { ok: false; message: string }

export const StatusUpdateTool = buildTool({
  name: STATUS_UPDATE_TOOL_ID,
  maxResultSizeChars: 2_000,
  userFacingName: () => 'Status',
  get inputSchema(): InputSchema {
    return StatusUpdateInput
  },
  isEnabled() {
    return available
  },
  async description() {
    return DESCRIPTION_TXT
  },
  async prompt() {
    return DESCRIPTION_TXT
  },
  mapToolResultToToolResultBlockParam(output, toolUseID) {
    return { tool_use_id: toolUseID, type: 'tool_result', content: output.message }
  },
  renderToolUseMessage() {
    return ''
  },
  async call() {
    return { data: { ok: false, message: 'This tool is not available in this version.' } }
  },
} satisfies ToolDef<InputSchema, Output>)
