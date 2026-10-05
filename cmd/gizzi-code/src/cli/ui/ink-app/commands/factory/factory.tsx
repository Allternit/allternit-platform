import type { LocalJSXCommandCall } from '../../types/command.js'

/**
 * /factory — mount the full-screen Allternit Factory floor.
 *
 * The screen is REPL-owned (AppState.screen), same as /bots and /dashboard;
 * the command just flips it on. Also reachable with ctrl+x f, and f on the
 * dashboard and bots screens.
 */
export const call: LocalJSXCommandCall = async (onDone, context, _args) => {
  if (context.options?.isNonInteractiveSession) {
    onDone('The factory floor is only available in the interactive TUI. Use `gizzi agents ps` and `gizzi workspace board`.', {
      display: 'system',
    })
    return null
  }
  context.setAppState(prev => ({
    ...prev,
    screen: 'factory',
  }))
  onDone(undefined, { display: 'skip' })
  return null
}
