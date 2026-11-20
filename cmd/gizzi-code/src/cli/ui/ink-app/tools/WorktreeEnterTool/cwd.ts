/**
 * Moves the TUI's working folder for worktree_enter / worktree_exit. Mirrors the
 * session-level changes the old EnterWorktree / ExitWorktree tools made.
 */
import { setOriginalCwd } from '../../bootstrap/state.js'
import { clearSystemPromptSections } from '../../constants/systemPromptSections.js'
import { clearMemoryFileCaches } from '../../utils/gizzimd.js'
import { getPlansDirectory } from '../../utils/plans.js'
import { setCwd } from '../../utils/Shell.js'

export function moveTuiCwd(dir: string): void {
  process.chdir(dir)
  setCwd(dir)
  setOriginalCwd(dir)
  clearSystemPromptSections()
  clearMemoryFileCaches()
  ;(getPlansDirectory as { cache?: { clear?: () => void } }).cache?.clear?.()
}
