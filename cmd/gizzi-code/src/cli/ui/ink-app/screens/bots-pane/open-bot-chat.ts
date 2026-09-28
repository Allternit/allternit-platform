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
import { ensurePlatformBot } from '@/runtime/bots/platform-bot.js'
import { PlatformApiError, PlatformSignedOutError } from '@/runtime/bots/platform-api.js'
import { ensureStandingThread } from '@/runtime/bots/platform-threads.js'
import { getResumeHandler, setActiveBotChat, switchSession } from '../../bootstrap/state.js'
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

export interface OpenBotChatOutcome extends Partial<OpenBotChatResult> {
  /** live: a client of the bot's shared session; local: the terminal-only chat. */
  mode: 'live' | 'local'
  /** Why the live chat couldn't open (shown to the user), when mode is local. */
  notice?: string
}

export interface OpenBotLiveChatDeps extends OpenBotChatDeps {
  ensurePlatformBot?: typeof ensurePlatformBot
  ensureStandingThread?: typeof ensureStandingThread
  setActiveBotChat?: typeof setActiveBotChat
  openBotCanonicalChat?: typeof openBotCanonicalChat
}

function fallbackNotice(botName: string, err: unknown): string {
  const local = `Opened ${botName}'s terminal-only chat; Desktop won't see it.`
  if (err instanceof PlatformSignedOutError) return `${local} Run \`gizzi login\` to share it.`
  if (err instanceof PlatformApiError) return `${local} Allternit said: ${err.message}`
  return `${local} Allternit isn't reachable (${(err as Error)?.message ?? err}).`
}

/**
 * The /bots pane Enter action: open the bot's chat as a live client of its
 * shared session, the platform bot's standing thread, which Desktop and the
 * pet HUD show too. The local bot is registered on the platform first.
 * Signed out, or the platform unreachable: fall back to the terminal-only
 * canonical chat, with a notice saying why.
 */
export async function openBotChat(name: string, deps: OpenBotLiveChatDeps = {}): Promise<OpenBotChatOutcome> {
  const d = {
    ensurePlatformBot: deps.ensurePlatformBot ?? ensurePlatformBot,
    ensureStandingThread: deps.ensureStandingThread ?? ensureStandingThread,
    setActiveBotChat: deps.setActiveBotChat ?? setActiveBotChat,
    openBotCanonicalChat: deps.openBotCanonicalChat ?? openBotCanonicalChat,
    markBotRead: deps.markBotRead ?? markBotRead,
    switchSession: deps.switchSession ?? switchSession,
    getBot: deps.getBot ?? getBot,
  }
  const local = await d.getBot(name)
  if (!local) throw new Error(`bot '${name}' not found`)
  const botName = local.title.trim() || local.name

  let live: { botId: string; threadId: string; sessionId: string; model: string | null } | null = null
  let failure: unknown = null
  try {
    const { id, bot } = await d.ensurePlatformBot(local.name)
    const thread = await d.ensureStandingThread(id, botName)
    if (!thread.currentSessionId) throw new Error(`${botName}'s thread has no live session`)
    live = { botId: id, threadId: thread.id, sessionId: thread.currentSessionId, model: bot.model }
  } catch (err) {
    failure = err
  }

  if (!live) {
    d.setActiveBotChat(null)
    const result = await d.openBotCanonicalChat(local.name, deps)
    return { ...result, mode: 'local', notice: fallbackNotice(botName, failure) }
  }

  await d.markBotRead(local.name)
  // Switch first: switchSession ends any other bot chat.
  d.switchSession(asSessionId(live.sessionId))
  d.setActiveBotChat({ botId: live.botId, botName, threadId: live.threadId, sessionId: live.sessionId, model: live.model })
  return { mode: 'live', sessionId: live.sessionId, projectPath: undefined, created: false }
}
