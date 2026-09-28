// @ts-nocheck
/**
 * openBotCanonicalChat — the /bots pane Enter action (Phase B5).
 *
 * Opens the bot's canonical chat through the SAME pipeline /resume uses:
 * REPL publishes its full resume callback (transcript reload, hooks,
 * plan/file-history handoff) via `setResumeHandler` in bootstrap/state, and
 * this helper loads the canonical session's log and hands off to it. That
 * keeps the mounted REPL's message list in sync — a bare switchSession only
 * points new turns at the right transcript. Falls back to switchSession when
 * no handler is published (thin/test contexts) or the session has no
 * transcript yet (freshly created canonical chat). Opening also marks the
 * bot read so its unread badge clears.
 *
 * Every collaborator is injectable (OpenBotChatDeps) so tests can drive this
 * module without bun's mock.module — whose registrations leak process-wide
 * and would poison other test files importing the same modules.
 */
import { getBot } from '@/runtime/bots/bot-store.js'
import { openCanonicalChat } from '@/runtime/bots/canonical-chat.js'
import { markBotRead } from '@/runtime/bots/bot-roster.js'
import { getResumeHandler, switchSession } from '../../bootstrap/state.js'
import { asSessionId } from '../../types/ids.js'
import { getLastSessionLog, isLiteLog, loadFullLog } from '../../utils/sessionStorage.js'
import { buildHandoffLog, loadEarlierWindow, resolveHandoffHead } from './handoff-head.js'

export interface OpenBotChatResult {
  projectPath: string
  sessionId: string
  /** True when a fresh canonical session was created and pinned. */
  created: boolean
}

export interface OpenBotChatDeps {
  getBot?: typeof getBot
  openCanonicalChat?: typeof openCanonicalChat
  markBotRead?: typeof markBotRead
  getResumeHandler?: typeof getResumeHandler
  switchSession?: typeof switchSession
  getLastSessionLog?: typeof getLastSessionLog
  isLiteLog?: typeof isLiteLog
  loadFullLog?: typeof loadFullLog
  resolveHandoffHead?: typeof resolveHandoffHead
  loadEarlierWindow?: typeof loadEarlierWindow
}

export async function openBotCanonicalChat(
  name: string,
  deps: OpenBotChatDeps = {},
): Promise<OpenBotChatResult> {
  const d = {
    getBot: deps.getBot ?? getBot,
    openCanonicalChat: deps.openCanonicalChat ?? openCanonicalChat,
    markBotRead: deps.markBotRead ?? markBotRead,
    getResumeHandler: deps.getResumeHandler ?? getResumeHandler,
    switchSession: deps.switchSession ?? switchSession,
    getLastSessionLog: deps.getLastSessionLog ?? getLastSessionLog,
    isLiteLog: deps.isLiteLog ?? isLiteLog,
    loadFullLog: deps.loadFullLog ?? loadFullLog,
    resolveHandoffHead: deps.resolveHandoffHead ?? resolveHandoffHead,
    loadEarlierWindow: deps.loadEarlierWindow ?? loadEarlierWindow,
  }

  const bot = await d.getBot(name)
  if (!bot) throw new Error(`bot '${name}' not found`)
  const result = await d.openCanonicalChat(bot)
  await d.markBotRead(name)

  const handler = d.getResumeHandler()

  // Another client may have handed the chat off to a fresh window (P3.16).
  // Open the newest window; if the REPL has never seen it, seed it with the
  // checkpoint (on screen as the rip, and in the model's context) with the
  // earlier window's last messages above the rip to scroll back to. The pin
  // itself moves server-side, so it is left alone here.
  const head = result.created ? null : await d.resolveHandoffHead(result.sessionId).catch(() => null)
  if (head) {
    const headLog = await d.getLastSessionLog(head.sessionId).catch(() => null)
    if (handler) {
      const log = headLog
        ? d.isLiteLog(headLog) ? await d.loadFullLog(headLog) : headLog
        : buildHandoffLog(head, result.projectPath, await d.loadEarlierWindow(head).catch(() => null))
      await handler(head.sessionId, log, 'bots_pane')
    } else {
      d.switchSession(asSessionId(head.sessionId), result.projectPath)
    }
    return { ...result, sessionId: head.sessionId }
  }

  const log = result.created
    ? null
    : await d.getLastSessionLog(result.sessionId).catch(() => null)
  if (handler && log) {
    const fullLog = d.isLiteLog(log) ? await d.loadFullLog(log) : log
    await handler(result.sessionId, fullLog, 'bots_pane')
  } else {
    // Fallback: point the ink session at the canonical chat (id + project
    // dir atomically) — new turns land in the right transcript, and the
    // SessionPrompt persona hook fires for the new id. Used when REPL's
    // resume pipeline isn't published (thin contexts) or the canonical chat
    // was just created and has no transcript to reload.
    d.switchSession(asSessionId(result.sessionId), result.projectPath)
  }
  return result
}
