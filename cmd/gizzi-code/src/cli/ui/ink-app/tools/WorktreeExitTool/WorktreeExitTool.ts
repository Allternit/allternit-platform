/**
 * worktree_exit in the Gizzi TUI: G5 adapter over SessionWorktree. Same id, input
 * schema and description as the runtime builtin
 * (runtime/tools/builtins/worktree-exit.ts), and enabled only while that module's
 * `available` is true, so flipping it there turns on both. tools.ts already
 * registers this adapter; G5 (PR1) builds it out in this folder and never
 * edits tools.ts.
 */
import { available } from '@/runtime/tools/builtins/worktree-exit'
import { WORKTREE_EXIT_TOOL_ID, WorktreeExitInput } from '@/runtime/primitives/gap-types'
import DESCRIPTION_TXT from '@/runtime/tools/builtins/worktree-exit.txt'
import { SessionWorktree } from '@/runtime/primitives/worktree'
import { getSessionId } from '../../bootstrap/state.js'
import { buildTool, type ToolDef } from '../../Tool.js'
import { moveTuiCwd } from '../WorktreeEnterTool/cwd.js'

export const WORKTREE_EXIT_TOOL_NAME = WORKTREE_EXIT_TOOL_ID

type InputSchema = typeof WorktreeExitInput
type Output = { ok: boolean; message: string; result?: string; path?: string; branch?: string }

function plural(n: number, word: string) {
  return `${n} ${word}${n === 1 ? '' : 's'}`
}

export const WorktreeExitTool = buildTool({
  name: WORKTREE_EXIT_TOOL_ID,
  aliases: ['ExitWorktree'],
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
  async call(input) {
    const sessionID = getSessionId()
    const current = await SessionWorktree.leave(sessionID)
    if (!current) {
      return {
        data: {
          ok: false,
          result: 'refused',
          message: 'This session is not working in a worktree, so there is nothing to leave.',
        },
      }
    }
    // Back to the original folder first, whatever happens to the tree.
    moveTuiCwd(current.original)
    const back = `The session is back in ${current.original}.`
    const branch = current.branch ?? (await SessionWorktree.owner(current.path))?.branch

    if (input.action === 'keep') {
      return {
        data: {
          ok: true,
          result: 'kept',
          path: current.path,
          branch,
          message: `Kept the worktree at ${current.path}${branch ? ` on branch ${branch}` : ''}. ${back}`,
        },
      }
    }

    const removed = await SessionWorktree.remove({ path: current.path, sessionID })
    if (removed.removed) {
      return {
        data: {
          ok: true,
          result: 'removed',
          path: removed.path,
          branch: removed.branch ?? branch,
          message: `Removed the worktree at ${removed.path}${removed.branch ? ` and its branch ${removed.branch}` : ''}. ${back}`,
        },
      }
    }
    const s = removed.status
    const why =
      removed.reason === 'not_created_here'
        ? 'this session did not create it'
        : removed.reason === 'unpushed_work'
          ? `it has ${plural(s?.unpushed ?? 0, 'unpushed commit')}`
          : removed.reason === 'uncommitted_changes'
            ? s?.error
              ? `its state could not be read (${s.error})`
              : `it has ${plural(s?.uncommitted ?? 0, 'uncommitted change')}`
            : `git could not remove it (${removed.detail ?? removed.reason ?? 'unknown error'})`
    if (removed.reason === 'not_a_worktree') {
      return { data: { ok: true, result: 'removed', path: removed.path, message: `The worktree folder ${removed.path} no longer exists. ${back}` } }
    }
    return {
      data: {
        ok: true,
        result: removed.reason === 'not_created_here' ? 'refused' : 'kept',
        path: removed.path,
        branch,
        message: `Did not remove ${removed.path} because ${why}; it is kept${branch ? ` on branch ${branch}` : ''}. ${back}`,
      },
    }
  },
} satisfies ToolDef<InputSchema, Output>)
