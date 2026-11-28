import type { LocalJSXCommandCall } from '../../types/command.js'

/**
 * /sessions — mount the live sessions view: every gizzi session and every
 * Factory pane running another coding tool, with status, the "doing" line
 * and the last output lines. REPL owns the screen (AppState.screen); this
 * just flips it. /dash, /dashboard, /board, /live and /factory land here too.
 */
export const call: LocalJSXCommandCall = async (onDone, context, _args) => {
  if (context.options?.isNonInteractiveSession) {
    onDone('The sessions view is only available in the interactive TUI. Use `gizzi agents sessions`.', { display: 'system' })
    return null
  }
  context.setAppState(prev => ({ ...prev, screen: 'sessions' }))
  onDone(undefined, { display: 'skip' })
  return null
}
