/**
 * useBotChatSession — the terminal `/bots` chat as a live client of the bot's
 * shared `gizzi serve` session (HANDOFF-gateway-key-and-bots-storage Part 2,
 * option 2). Same result shape as useDirectConnect so REPL can pick it as the
 * active remote: turns go to the platform API instead of the local query
 * loop, and the screen is rebuilt from the session's rows plus the live
 * `/agent-sessions/sync` feed, the same data Desktop and the pet HUD show.
 *
 * Active while bootstrap state has an `activeBotChat` (set by the /bots pane
 * on Enter). The mapping from rows/events to chat items lives in
 * runtime/bots/bot-chat-view.ts; this hook only turns items into REPL
 * messages and wires prompts into toolUseConfirmQueue.
 */
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { abortSession, openSyncStream, rejectQuestion, replyPermission, replyQuestion } from '@/runtime/bots/bot-chat-client'
import { BotChatTracker, itemsFromMessage, itemsFromMessages, type BotChatItem, type SyncEvent } from '@/runtime/bots/bot-chat-view'
import { PlatformSignedOutError } from '@/runtime/bots/platform-api'
import {
  followThread,
  listSessionMessages,
  parseModelRef,
  sendThreadTurn,
  threadApi,
  turnContextTokens,
} from '@/runtime/bots/platform-threads'
import {
  type ActiveBotChat,
  getActiveBotChat,
  onActiveBotChatChange,
  setActiveBotChat,
  switchSession,
} from '../bootstrap/state.js'
import type { ToolUseConfirm } from '../components/permissions/PermissionRequest.js'
import { createToolStub } from '../remote/remotePermissionBridge.js'
import type { Tool } from '../Tool.js'
import { AskUserQuestionTool } from '../tools/AskUserQuestionTool/AskUserQuestionTool.js'
import { asSessionId } from '../types/ids.js'
import type { AssistantMessage, Message as MessageType } from '../types/message.js'
import type { PermissionAskDecision } from '../types/permissions.js'
import { logForDebugging } from '../utils/debug.js'
import {
  createAssistantMessage,
  createCompactBoundaryMessage,
  createSystemMessage,
  createUserMessage,
} from '../utils/messages.js'
import type { RemoteMessageContent } from '../utils/teleport/api.js'

type UseBotChatSessionResult = {
  isRemoteMode: boolean
  sendMessage: (content: RemoteMessageContent) => Promise<boolean>
  cancelRequest: () => void
  disconnect: () => void
}

type UseBotChatSessionProps = {
  setMessages: React.Dispatch<React.SetStateAction<MessageType[]>>
  setIsLoading: (loading: boolean) => void
  setToolUseConfirmQueue: React.Dispatch<React.SetStateAction<ToolUseConfirm[]>>
  onStreamingText: (f: (current: string | null) => string | null) => void
  tools: Tool[]
}

/**
 * One chat item as a REPL message. Tools and rips are not tool_use blocks:
 * the bot's tools aren't loaded here, and unknown tool names render nothing.
 */
export function botChatItemToMessage(item: BotChatItem): MessageType {
  switch (item.kind) {
    case 'user':
      return createUserMessage({ content: item.text })
    case 'assistant':
      return createAssistantMessage({ content: item.text })
    case 'tool':
      return item.error
        ? createSystemMessage(`${item.tool} ${item.title} — ${item.error}`.trim(), 'error')
        : createSystemMessage(`${item.tool} ${item.title}`.trim(), 'info')
    case 'rip': {
      const rip = createCompactBoundaryMessage('auto', 0)
      rip.compactMetadata.handoff = { generation: item.generation, reason: item.reason }
      return rip
    }
  }
}

/** Plain text of what the user typed; images can't be sent to a bot turn yet. */
function contentText(content: RemoteMessageContent): { text: string; dropped: boolean } {
  if (typeof content === 'string') return { text: content, dropped: false }
  const texts = content.filter(b => b.type === 'text').map(b => String(b.text ?? ''))
  return { text: texts.join('\n\n'), dropped: texts.length !== content.length }
}

/** The tool_use a prompt is about; PermissionRequest reads it from an assistant message. */
function promptMessage(name: string, input: Record<string, unknown>, id: string): AssistantMessage {
  return createAssistantMessage({ content: [{ type: 'tool_use', id, name, input }] as never })
}

const permToolUseId = (requestId: string) => `bot-perm-${requestId}`
const questionToolUseId = (requestId: string) => `bot-question-${requestId}`

/**
 * gizzi's question request as AskUserQuestion input, or null when it doesn't
 * fit that prompt (it takes 2–4 options per question).
 */
function askUserQuestionInput(event: SyncEvent): { questions: unknown[] } | null {
  const questions = Array.isArray(event.questions) ? event.questions : []
  const input = {
    questions: questions.map((q: any) => ({
      question: q.question,
      header: String(q.header ?? '').slice(0, 12),
      options: (q.options ?? []).map((o: any) => ({ label: o.label, description: o.description ?? '' })),
      multiSelect: !!q.multiple,
    })),
  }
  return AskUserQuestionTool.inputSchema.safeParse(input).success ? input : null
}

/** AskUserQuestion's answers (question → "a, b") back to gizzi's label lists. */
function questionAnswers(input: { questions: any[] }, answers: Record<string, string> = {}): string[][] {
  return input.questions.map(q => {
    const answer = answers[q.question]
    if (!answer) return []
    if (!q.multiSelect || q.options.some(o => o.label === answer)) return [answer]
    return answer.split(', ').filter(Boolean)
  })
}

export function useBotChatSession({
  setMessages,
  setIsLoading,
  setToolUseConfirmQueue,
  onStreamingText,
  tools,
}: UseBotChatSessionProps): UseBotChatSessionResult {
  const [chat, setChat] = useState<ActiveBotChat | null>(getActiveBotChat)
  useEffect(() => onActiveBotChatChange(setChat), [])

  const trackerRef = useRef<BotChatTracker | null>(null)
  const turnRef = useRef<AbortController | null>(null)
  const chatRef = useRef(chat)
  chatRef.current = chat
  const toolsRef = useRef(tools)
  useEffect(() => {
    toolsRef.current = tools
  }, [tools])

  const append = useCallback(
    (items: BotChatItem[]) => {
      if (items.length) setMessages(prev => [...prev, ...items.map(botChatItemToMessage)])
    },
    [setMessages],
  )
  const notice = useCallback(
    (text: string, level: 'info' | 'warning' | 'error' = 'info') =>
      setMessages(prev => [...prev, createSystemMessage(text, level)]),
    [setMessages],
  )

  /** The thread moved to a fresh window: point the chat and the session at it. */
  const followWindow = useCallback((sessionId: string) => {
    const current = chatRef.current
    if (!current || current.sessionId === sessionId) return
    setActiveBotChat({ ...current, sessionId })
    switchSession(asSessionId(sessionId))
  }, [])

  const dropPrompt = useCallback(
    (requestId: string) =>
      setToolUseConfirmQueue(queue =>
        queue.filter(item => item.toolUseID !== permToolUseId(requestId) && item.toolUseID !== questionToolUseId(requestId)),
      ),
    [setToolUseConfirmQueue],
  )

  const askPermission = useCallback(
    (event: SyncEvent) => {
      const requestId = event.request_id
      if (!requestId) return
      const toolName = String(event.permission ?? 'tool')
      const patterns = Array.isArray(event.patterns) ? (event.patterns as string[]) : []
      const description = patterns.length ? `${toolName}: ${patterns.join(', ')}` : `${toolName} requires permission`
      const input = (event.metadata && typeof event.metadata === 'object' ? event.metadata : { patterns }) as Record<string, unknown>
      const toolUseID = permToolUseId(requestId)
      const answer = (reply: 'once' | 'always' | 'reject', message?: string) => {
        dropPrompt(requestId)
        replyPermission(requestId, reply, message).catch(error =>
          notice(`Couldn't answer the bot's permission request: ${error?.message ?? error}`, 'error'),
        )
      }
      const confirm: ToolUseConfirm = {
        assistantMessage: promptMessage(toolName, input, toolUseID),
        // Stub, not the local tool: the bot's tool runs in its own session.
        tool: createToolStub(toolName),
        description,
        input,
        toolUseContext: {} as ToolUseConfirm['toolUseContext'],
        toolUseID,
        permissionResult: { behavior: 'ask', message: description } as PermissionAskDecision,
        permissionPromptStartTimeMs: Date.now(),
        onUserInteraction() {},
        onAbort: () => answer('reject'),
        onAllow: (_input, permissionUpdates) => answer(permissionUpdates?.length ? 'always' : 'once'),
        onReject: (feedback?: string) => answer('reject', feedback),
        async recheckPermission() {},
      }
      setToolUseConfirmQueue(queue => [...queue.filter(item => item.toolUseID !== confirm.toolUseID), confirm])
    },
    [dropPrompt, notice, setToolUseConfirmQueue],
  )

  const askQuestion = useCallback(
    (event: SyncEvent) => {
      const requestId = event.request_id
      if (!requestId) return
      const input = askUserQuestionInput(event)
      if (!input) {
        notice(`${chatRef.current?.botName ?? 'The bot'} asked a question this terminal can't show. Answer it in Desktop.`, 'warning')
        return
      }
      const toolUseID = questionToolUseId(requestId)
      const settle = (run: () => Promise<unknown>) => {
        dropPrompt(requestId)
        run().catch(error => notice(`Couldn't answer the bot's question: ${error?.message ?? error}`, 'error'))
      }
      const confirm: ToolUseConfirm = {
        assistantMessage: promptMessage(AskUserQuestionTool.name, input, toolUseID),
        tool: AskUserQuestionTool,
        description: 'The bot has a question',
        input,
        toolUseContext: {} as ToolUseConfirm['toolUseContext'],
        toolUseID,
        permissionResult: { behavior: 'ask', message: 'The bot has a question' } as PermissionAskDecision,
        permissionPromptStartTimeMs: Date.now(),
        onUserInteraction() {},
        onAbort: () => settle(() => rejectQuestion(requestId)),
        onAllow: updatedInput =>
          settle(() => replyQuestion(requestId, questionAnswers(input, updatedInput?.answers as Record<string, string> | undefined))),
        onReject: () => settle(() => rejectQuestion(requestId)),
        async recheckPermission() {},
      }
      setToolUseConfirmQueue(queue => [...queue.filter(item => item.toolUseID !== confirm.toolUseID), confirm])
    },
    [dropPrompt, notice, setToolUseConfirmQueue],
  )

  // Open: load the window's history, then follow the live feed. Keyed on the
  // thread: following a handoff changes sessionId but keeps this connection.
  const threadId = chat?.threadId ?? null
  useEffect(() => {
    const opened = chatRef.current
    if (!threadId || !opened) return
    const tracker = new BotChatTracker(opened.sessionId)
    trackerRef.current = tracker
    const stream = new AbortController()
    let retrying = false

    const onEvent = (event: SyncEvent) => {
      const update = tracker.handle(event)
      append(update.commit)
      if (update.streamingText !== undefined) onStreamingText(() => update.streamingText ?? null)
      if (update.handedOffTo) followWindow(update.handedOffTo.sessionId)
      if (update.permission) askPermission(update.permission)
      if (update.question) askQuestion(update.question)
      if (update.resolved) dropPrompt(update.resolved)
    }

    void (async () => {
      try {
        const history = itemsFromMessages(await listSessionMessages(opened.sessionId))
        if (stream.signal.aborted) return
        tracker.markShown(history)
        setMessages(history.map(botChatItemToMessage))
      } catch (error) {
        if (stream.signal.aborted) return
        notice(`Couldn't load ${opened.botName}'s conversation: ${error?.message ?? error}`, 'error')
      }
      try {
        await openSyncStream({
          signal: stream.signal,
          onEvent,
          onStatus: (status, error) => {
            if (status === 'open') {
              if (retrying) notice(`Reconnected to ${opened.botName}.`)
              retrying = false
            } else if (!retrying) {
              retrying = true
              logForDebugging(`[useBotChatSession] sync stream dropped: ${error}`)
              notice(`Lost the live connection to ${opened.botName}. Reconnecting…`, 'warning')
            }
          },
        })
      } catch (error) {
        if (stream.signal.aborted) return
        notice(
          error instanceof PlatformSignedOutError
            ? error.message
            : `The live connection to ${opened.botName} closed: ${error?.message ?? error}`,
          'error',
        )
      }
    })()

    return () => {
      stream.abort()
      turnRef.current?.abort()
      turnRef.current = null
      trackerRef.current = null
      onStreamingText(() => null)
    }
  }, [threadId, append, askPermission, askQuestion, dropPrompt, followWindow, notice, onStreamingText, setMessages])

  const sendMessage = useCallback(
    async (content: RemoteMessageContent): Promise<boolean> => {
      const current = chatRef.current
      const tracker = trackerRef.current
      if (!current || !tracker) return false
      const { text, dropped } = contentText(content)
      if (dropped) notice("Images aren't sent to bot chats yet; sent the text only.", 'warning')
      if (!text.trim()) return false
      // REPL already drew the user's message; don't draw the feed's copy.
      tracker.expectEcho(text)
      const turn = new AbortController()
      turnRef.current = turn
      setIsLoading(true)
      try {
        const thread = await threadApi.get(current.threadId)
        const model = parseModelRef(current.model)
        const reply = await sendThreadTurn(thread, text, { model, signal: turn.signal })
        // The feed normally commits these as they finish; this covers a gap
        // in the connection during the turn.
        append(tracker.commitItems(itemsFromMessage(reply)))
        const next = await followThread(thread, {
          tokensUsed: turnContextTokens(reply),
          model: current.model ?? undefined,
        })
        if (next.currentSessionId && next.currentSessionId !== tracker.sessionId) {
          const rip = tracker.handle({
            type: 'handed_off',
            session_id: tracker.sessionId,
            to: next.currentSessionId,
            generation: next.generation,
          })
          append(rip.commit)
          followWindow(next.currentSessionId)
        }
        return true
      } catch (error) {
        if (turn.signal.aborted) return false
        notice(`${current.botName} couldn't take that turn: ${error?.message ?? error}`, 'error')
        return false
      } finally {
        if (turnRef.current === turn) turnRef.current = null
        setIsLoading(false)
      }
    },
    [append, followWindow, notice, setIsLoading],
  )

  const cancelRequest = useCallback(() => {
    const tracker = trackerRef.current
    if (tracker) {
      abortSession(tracker.sessionId).catch(error =>
        logForDebugging(`[useBotChatSession] abort failed: ${error?.message ?? error}`),
      )
    }
    turnRef.current?.abort()
    onStreamingText(() => null)
    setIsLoading(false)
  }, [onStreamingText, setIsLoading])

  const disconnect = useCallback(() => {
    setActiveBotChat(null)
  }, [])

  const isRemoteMode = !!chat
  return useMemo(
    () => ({ isRemoteMode, sendMessage, cancelRequest, disconnect }),
    [isRemoteMode, sendMessage, cancelRequest, disconnect],
  )
}
