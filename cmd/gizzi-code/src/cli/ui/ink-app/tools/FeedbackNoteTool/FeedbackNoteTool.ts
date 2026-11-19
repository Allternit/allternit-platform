/**
 * feedback_note in the Gizzi TUI: prompt-gap contract stub (G3). Same id, input
 * schema and description as the runtime builtin
 * (runtime/tools/builtins/feedback-note.ts), and enabled only while that module's
 * `available` is true, so flipping it there turns on both. tools.ts already
 * registers this adapter; G4-5 (PR11c) builds it out in this folder and never
 * edits tools.ts.
 */
import { available } from '@/runtime/tools/builtins/feedback-note'
import { FEEDBACK_NOTE_TOOL_ID, FeedbackNoteInput } from '@/runtime/primitives/gap-types'
import DESCRIPTION_TXT from '@/runtime/tools/builtins/feedback-note.txt'
import { buildTool, type ToolDef } from '../../Tool.js'

export const FEEDBACK_NOTE_TOOL_NAME = FEEDBACK_NOTE_TOOL_ID

type InputSchema = typeof FeedbackNoteInput
type Output = { ok: false; message: string }

export const FeedbackNoteTool = buildTool({
  name: FEEDBACK_NOTE_TOOL_ID,
  maxResultSizeChars: 2_000,
  userFacingName: () => 'Feedback note',
  get inputSchema(): InputSchema {
    return FeedbackNoteInput
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
