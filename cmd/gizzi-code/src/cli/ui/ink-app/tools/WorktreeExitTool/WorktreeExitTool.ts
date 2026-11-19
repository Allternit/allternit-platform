/**
 * worktree_exit in the Gizzi TUI: prompt-gap contract stub (G3). Same id, input
 * schema and description as the runtime builtin
 * (runtime/tools/builtins/worktree-exit.ts), and enabled only while that module's
 * `available` is true, so flipping it there turns on both. tools.ts already
 * registers this adapter; G5 (PR1) builds it out in this folder and never
 * edits tools.ts.
 */
import { available } from '@/runtime/tools/builtins/worktree-exit'
import { WORKTREE_EXIT_TOOL_ID, WorktreeExitInput } from '@/runtime/primitives/gap-types'
import DESCRIPTION_TXT from '@/runtime/tools/builtins/worktree-exit.txt'
import { buildTool, type ToolDef } from '../../Tool.js'

export const WORKTREE_EXIT_TOOL_NAME = WORKTREE_EXIT_TOOL_ID

type InputSchema = typeof WorktreeExitInput
type Output = { ok: false; message: string }

export const WorktreeExitTool = buildTool({
  name: WORKTREE_EXIT_TOOL_ID,
  maxResultSizeChars: 2_000,
  userFacingName: () => 'Leave worktree',
  get inputSchema(): InputSchema {
    return WorktreeExitInput
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
