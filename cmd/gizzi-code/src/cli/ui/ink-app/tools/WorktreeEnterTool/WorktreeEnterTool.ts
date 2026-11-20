/**
 * worktree_enter in the Gizzi TUI: G5 adapter over SessionWorktree. Same id, input
 * schema and description as the runtime builtin
 * (runtime/tools/builtins/worktree-enter.ts), and enabled only while that module's
 * `available` is true, so flipping it there turns on both. tools.ts already
 * registers this adapter; G5 (PR1) builds it out in this folder and never
 * edits tools.ts.
 */
import { available } from '@/runtime/tools/builtins/worktree-enter'
import { WORKTREE_ENTER_TOOL_ID, WorktreeEnterInput } from '@/runtime/primitives/gap-types'
import DESCRIPTION_TXT from '@/runtime/tools/builtins/worktree-enter.txt'
import { SessionWorktree } from '@/runtime/primitives/worktree'
import { getSessionId } from '../../bootstrap/state.js'
import { buildTool, type ToolDef } from '../../Tool.js'
import { getCwd } from '../../utils/cwd.js'
import { moveTuiCwd } from './cwd.js'

export const WORKTREE_ENTER_TOOL_NAME = WORKTREE_ENTER_TOOL_ID

type InputSchema = typeof WorktreeEnterInput
type Output = { ok: boolean; message: string; path?: string; branch?: string }

export const WorktreeEnterTool = buildTool({
  name: WORKTREE_ENTER_TOOL_ID,
  aliases: ['EnterWorktree'],
  maxResultSizeChars: 2_000,
  userFacingName: () => 'New worktree',
  get inputSchema(): InputSchema {
    return WorktreeEnterInput
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
  async call(input) {
    const sessionID = getSessionId()
    const current = await SessionWorktree.active(sessionID)
    if (current) {
      return {
        data: {
          ok: false,
          path: current.path,
          branch: current.branch,
          message: `This session already works in ${current.path}. Call worktree_exit before entering another worktree.`,
        },
      }
    }
    const original = getCwd()
    let tree: SessionWorktree.CreateResult
    try {
      tree = await SessionWorktree.create({ cwd: original, sessionID, name: input.name, base: input.base })
    } catch (error) {
      return { data: { ok: false, message: error instanceof Error ? error.message : String(error) } }
    }
    await SessionWorktree.enter(sessionID, tree.path, original, tree.branch)
    moveTuiCwd(tree.path)
    const lines = [
      `${tree.existed ? 'Reusing' : 'Created'} worktree ${tree.path}`,
      `Branch: ${tree.branch} (from ${tree.base.slice(0, 12)})`,
      `The session now works in this folder until worktree_exit. You came from ${original}.`,
    ]
    if (tree.existed && tree.created_by !== sessionID) {
      lines.push('Another session or tool made this tree, so this session will keep it on exit, never remove it.')
    }
    return { data: { ok: true, path: tree.path, branch: tree.branch, message: lines.join('\n') } }
  },
} satisfies ToolDef<InputSchema, Output>)
