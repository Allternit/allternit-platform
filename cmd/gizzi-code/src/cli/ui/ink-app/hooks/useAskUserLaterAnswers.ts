/**
 * ask_user_later answers arrive as the next turn of this REPL.
 *
 * Two ways an answer reaches the terminal:
 * - Given on another device while the terminal is a live engine session: the
 *   engine's Question.reply starts the turn with SessionPrompt.prompt, which
 *   hands the text to the attached terminal (TerminalLive.deliver), and
 *   useLiveSession enqueues it like any prompt from a surface. Nothing to do here.
 * - Given in this process (/answer): Question.reply runs here and calls
 *   Question.asyncTurn.start. In the TUI that must not start a runtime turn in
 *   this process; for a session this REPL owns it queues the same marked text
 *   ("[answer to your question <id>] …") as the next prompt instead, so it
 *   runs after the current turn ends, like the runtime's queued reply turn.
 */
import { useEffect } from 'react'
import { ownedSessionIDs } from '../tools/AskUserLaterTool/session'
import { logForDebugging } from '../utils/debug'
import { enqueuePendingNotification } from '../utils/messageQueueManager'

export function useAskUserLaterAnswers(): void {
  useEffect(() => {
    let disposed = false
    let restore: (() => void) | undefined
    void import('@/runtime/integrations/question')
      .then(({ Question }) => {
        if (disposed) return
        const turn = Question.asyncTurn
        const original = turn.start
        const start: typeof original = async input => {
          // Not this REPL's thread: keep the runtime's own behavior.
          if (!ownedSessionIDs().has(input.sessionID)) return original(input)
          logForDebugging(`[ask_user_later] answer to ${input.requestID} queued as the next turn`)
          enqueuePendingNotification({ value: input.text, mode: 'prompt', priority: 'later', skipSlashCommands: true })
        }
        turn.start = start
        restore = () => {
          if (turn.start === start) turn.start = original
        }
      })
      .catch(error => logForDebugging(`[ask_user_later] answer hook not installed: ${error}`))
    return () => {
      disposed = true
      restore?.()
    }
  }, [])
}
