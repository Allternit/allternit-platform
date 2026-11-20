/**
 * second_opinion in the Gizzi TUI: prompt-gap contract stub (G3). Same id, input
 * schema and description as the runtime builtin
 * (runtime/tools/builtins/second-opinion.ts), and enabled only while that module's
 * `available` is true, so flipping it there turns on both. tools.ts already
 * registers this adapter; G5 (PR6) builds it out in this folder and never
 * edits tools.ts.
 */
import { available } from '@/runtime/tools/builtins/second-opinion'
import { SECOND_OPINION_TOOL_ID, SecondOpinionInput } from '@/runtime/primitives/gap-types'
import DESCRIPTION_TXT from '@/runtime/tools/builtins/second-opinion.txt'
import { buildTool, type ToolDef } from '../../Tool.js'

export const SECOND_OPINION_TOOL_NAME = SECOND_OPINION_TOOL_ID

type InputSchema = typeof SecondOpinionInput
type Output = { ok: false; message: string }

export const SecondOpinionTool = buildTool({
  name: SECOND_OPINION_TOOL_ID,
  maxResultSizeChars: 2_000,
  userFacingName: () => 'Second opinion',
  get inputSchema(): InputSchema {
    return SecondOpinionInput
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
