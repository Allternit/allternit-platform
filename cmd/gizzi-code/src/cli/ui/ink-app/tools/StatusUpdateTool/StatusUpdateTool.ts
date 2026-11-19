/**
 * status_update in the Gizzi TUI: prompt-gap contract stub (G3). Same id, input
 * schema and description as the runtime builtin
 * (runtime/tools/builtins/status-update.ts), and enabled only while that module's
 * `available` is true, so flipping it there turns on both. tools.ts already
 * registers this adapter; G4-5 (PR8b) builds it out in this folder and never
 * edits tools.ts.
 */
import { available, StatusUpdateTool as RuntimeStatusUpdateTool } from '@/runtime/tools/builtins/status-update'
import { STATUS_UPDATE_TOOL_ID, StatusUpdateInput } from '@/runtime/primitives/gap-types'
import DESCRIPTION_TXT from '@/runtime/tools/builtins/status-update.txt'
import { buildTool, type ToolDef } from '../../Tool.js'

export const STATUS_UPDATE_TOOL_NAME = STATUS_UPDATE_TOOL_ID

type InputSchema = typeof StatusUpdateInput
type Output = { ok: boolean; message: string; phase?: string; note?: string }

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
  renderToolUseMessage(input: { phase?: string; note?: string }) {
    // Dim one-liner in the transcript: "▸ <phase>: <note>".
    return input.phase ? `▸ ${input.phase}${input.note ? `: ${input.note}` : ''}` : ''
  },
  async call(input, context) {
    // The runtime tool publishes session.status with the phase and note.
    const sessionID = String(context.agentId ?? 'tui')
    const tool = await RuntimeStatusUpdateTool.init()
    const out = await tool.execute(input, {
      sessionID,
      messageID: sessionID,
      agent: 'gizzi',
      abort: context.abortController.signal,
      extra: {},
      messages: [],
      metadata() {},
      async ask() {},
    })
    return { data: { ok: true, message: out.output, phase: out.metadata.phase, note: out.metadata.note } }
  },
} satisfies ToolDef<InputSchema, Output>)
