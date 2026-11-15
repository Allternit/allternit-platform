import type { LocalJSXCommandCall } from '../../types/command.js'

/**
 * /board — mount the live Factory board (the same rows as `gizzi board`).
 * REPL owns the screen (AppState.screen), like /factory; this just flips it.
 */
export const call: LocalJSXCommandCall = async (onDone, context, _args) => {
  if (context.options?.isNonInteractiveSession) {
    onDone('The board is only available in the interactive TUI. Use `gizzi board --once` or `gizzi swarm wait`.', { display: 'system' })
    return null
  }
  context.setAppState(prev => ({ ...prev, screen: 'board' }))
  onDone(undefined, { display: 'skip' })
  return null
}
