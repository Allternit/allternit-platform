/**
 * Every TUI session is a live engine session (see services/liveSession.ts).
 * Pushes the transcript and busy/idle to the engine and applies what other
 * surfaces send: prompts (queued like typed ones, so a prompt during a turn
 * steers it), Stop, and permission answers (raced against the local dialog
 * through the same callbacks the old Claude bridge used).
 */
import { hostname } from 'node:os'
import { useEffect, useRef } from 'react'
import { getSessionId } from '../bootstrap/state'
import type { BridgePermissionCallbacks, BridgePermissionResponse } from '../bridge/bridgePermissionCallbacks'
import { useAppStateStore } from '../state/AppState'
import type { Message } from '../types/message'
import { getCwd } from '../utils/cwd'
import { logForDebugging } from '../utils/debug'
import { enqueue } from '../utils/messageQueueManager'
import { getTranscriptPath } from '../utils/sessionStorage'
import {
  ASK_USER_QUESTION,
  engineFetch,
  enginePermissionID,
  engineQuestionID,
  fromTerminalAnswers,
  toEngineQuestions,
  toTerminalAnswers,
  getLiveSession,
  liveSessionEnabled,
  setLiveSession,
  toLiveMessages,
  type LiveInbox,
  type LiveMessage,
} from '../services/liveSession'

const SYNC_DEBOUNCE_MS = 120
const RETRY_MS = 15_000
const RECONNECT_MS = 2_000

function modelRef(model: string | undefined) {
  if (!model) return undefined
  const slash = model.indexOf('/')
  return slash > 0
    ? { providerID: model.slice(0, slash), modelID: model.slice(slash + 1) }
    : { providerID: 'anthropic', modelID: model }
}

function firstPrompt(messages: readonly Message[]): string | undefined {
  const first = toLiveMessages(messages, true).find(m => m.role === 'user')
  const text = first?.parts[0]?.kind === 'text' ? first.parts[0].text : undefined
  return text ? text.replace(/\s+/g, ' ').slice(0, 80) : undefined
}

export function useLiveSession(
  messages: readonly Message[],
  isLoading: boolean,
  abortControllerRef: React.RefObject<AbortController | null>,
  mainLoopModel: string,
): void {
  const store = useAppStateStore()
  const messagesRef = useRef(messages)
  messagesRef.current = messages
  const loadingRef = useRef(isLoading)
  loadingRef.current = isLoading
  const syncRef = useRef<() => void>(() => {})
  const modelRefCurrent = useRef(mainLoopModel)
  modelRefCurrent.current = mainLoopModel

  useEffect(() => {
    if (!liveSessionEnabled()) return
    let disposed = false
    let terminalID = ''
    let sessionID: string | undefined
    let inbox: AbortController | undefined
    let retry: ReturnType<typeof setTimeout> | undefined
    let syncTimer: ReturnType<typeof setTimeout> | undefined
    let lastBusy: boolean | undefined
    const sent = new Map<string, string>()
    const handlers = new Map<string, (response: BridgePermissionResponse) => void>()

    // AskUserQuestion requests go out as engine questions (the surfaces' question card), not approvals.
    const questions = new Map<string, Record<string, unknown>>()

    const permissionCallbacks: BridgePermissionCallbacks = {
      sendRequest(requestId, toolName, input, toolUseId, description) {
        if (!sessionID) return
        if (toolName === ASK_USER_QUESTION) {
          questions.set(engineQuestionID(requestId), input)
          void engineFetch(`/sessions/${sessionID}/question`, {
            requestID: engineQuestionID(requestId),
            questions: toEngineQuestions(input),
            callID: toolUseId,
          }).catch(() => {})
          return
        }
        void engineFetch(`/sessions/${sessionID}/permission`, {
          requestID: enginePermissionID(requestId),
          tool: toolName,
          description,
          input,
          callID: toolUseId,
        }).catch(() => {})
      },
      sendResponse(requestId, response) {
        const question = questions.get(engineQuestionID(requestId))
        if (question) {
          questions.delete(engineQuestionID(requestId))
          void engineFetch(`/question/${engineQuestionID(requestId)}/settle`, {
            answers: response.behavior === 'allow' ? fromTerminalAnswers(question, response.updatedInput) : undefined,
          }).catch(() => {})
          return
        }
        void engineFetch(`/permission/${enginePermissionID(requestId)}/settle`, {
          allow: response.behavior === 'allow',
        }).catch(() => {})
      },
      cancelRequest(requestId) {
        if (questions.delete(engineQuestionID(requestId))) {
          void engineFetch(`/question/${engineQuestionID(requestId)}/settle`, {}).catch(() => {})
          return
        }
        void engineFetch(`/permission/${enginePermissionID(requestId)}/settle`, { allow: false }).catch(() => {})
      },
      onResponse(requestId, handler) {
        const ids = [enginePermissionID(requestId), engineQuestionID(requestId)]
        for (const id of ids) handlers.set(id, handler)
        return () => ids.forEach(id => handlers.delete(id))
      },
    }

    function handle(event: LiveInbox) {
      switch (event.type) {
        case 'input':
          logForDebugging(`[live] prompt from a surface: ${event.text.slice(0, 80)}`)
          enqueue({ value: event.text, mode: 'prompt', uuid: event.id as `${string}-${string}-${string}-${string}-${string}`, skipSlashCommands: true, bridgeOrigin: true })
          return
        case 'stop':
          abortControllerRef.current?.abort()
          return
        case 'permission': {
          const handler = handlers.get(event.requestID)
          handlers.delete(event.requestID)
          handler?.(event.allow ? { behavior: 'allow' } : { behavior: 'deny', message: event.message })
          return
        }
        case 'question': {
          const handler = handlers.get(event.requestID)
          const input = questions.get(event.requestID)
          handlers.delete(event.requestID)
          questions.delete(event.requestID)
          if (!handler || !input) return
          handler(
            event.answers
              ? { behavior: 'allow', updatedInput: { ...input, answers: toTerminalAnswers(input, event.answers) } }
              : { behavior: 'deny', message: 'The user declined to answer.' },
          )
          return
        }
      }
    }

    async function listen(id: string) {
      while (!disposed && sessionID === id) {
        inbox = new AbortController()
        try {
          const params = new URLSearchParams({ host: hostname(), pid: String(process.pid) })
          const res = await engineFetch(`/sessions/${id}/inbox?${params}`, undefined, inbox.signal)
          setLiveSession({ connected: true })
          const reader = res.body!.pipeThrough(new TextDecoderStream()).getReader()
          let buffer = ''
          for (;;) {
            const { value, done } = await reader.read()
            if (done) break
            buffer += value
            let cut: number
            while ((cut = buffer.indexOf('\n\n')) >= 0) {
              const frame = buffer.slice(0, cut)
              buffer = buffer.slice(cut + 2)
              const data = frame
                .split('\n')
                .filter(line => line.startsWith('data:'))
                .map(line => line.slice(5).trimStart())
                .join('\n')
              if (data) handle(JSON.parse(data) as LiveInbox)
            }
          }
        } catch (error) {
          if (disposed) return
          logForDebugging(`[live] inbox dropped: ${error}`)
        }
        setLiveSession({ connected: false })
        if (!disposed) await new Promise(resolve => setTimeout(resolve, RECONNECT_MS))
      }
    }

    async function attach() {
      if (disposed) return
      terminalID = String(getSessionId())
      try {
        const res = await engineFetch('/sessions', {
          terminalID,
          cwd: getCwd(),
          title: firstPrompt(messagesRef.current),
          transcriptPath: getTranscriptPath(),
          model: modelRef(modelRefCurrent.current),
        })
        const body = (await res.json()) as { sessionID: string }
        sessionID = body.sessionID
        sent.clear()
        lastBusy = undefined
        setLiveSession({ sessionID })
        store.setState(prev => ({ ...prev, replBridgePermissionCallbacks: permissionCallbacks }))
        void listen(body.sessionID)
        sync()
      } catch (error) {
        logForDebugging(`[live] engine not reachable, retrying: ${error}`)
        retry = setTimeout(() => void attach(), RETRY_MS)
      }
    }

    async function push() {
      // /clear and resume switch the terminal session: follow it.
      if (sessionID && String(getSessionId()) !== terminalID) {
        inbox?.abort()
        sessionID = undefined
        setLiveSession({ sessionID: undefined, connected: false })
        return attach()
      }
      const id = sessionID
      if (!id) return
      const busy = loadingRef.current
      const changed: LiveMessage[] = []
      for (const m of toLiveMessages(messagesRef.current, busy)) {
        const json = JSON.stringify(m)
        if (sent.get(m.key) !== json) {
          changed.push(m)
          sent.set(m.key, json)
        }
      }
      try {
        if (changed.length) await engineFetch(`/sessions/${id}/messages`, { messages: changed })
        if (busy !== lastBusy) {
          await engineFetch(`/sessions/${id}/status`, { busy })
          lastBusy = busy
        }
      } catch (error) {
        for (const m of changed) sent.delete(m.key)
        logForDebugging(`[live] sync failed: ${error}`)
      }
    }

    function sync() {
      if (syncTimer) return
      syncTimer = setTimeout(() => {
        syncTimer = undefined
        void push()
      }, SYNC_DEBOUNCE_MS)
    }
    syncRef.current = sync

    void attach()
    return () => {
      disposed = true
      inbox?.abort()
      if (retry) clearTimeout(retry)
      if (syncTimer) clearTimeout(syncTimer)
      syncRef.current = () => {}
      store.setState(prev =>
        prev.replBridgePermissionCallbacks === permissionCallbacks
          ? { ...prev, replBridgePermissionCallbacks: undefined }
          : prev,
      )
      setLiveSession({ sessionID: undefined, connected: false })
    }
  }, [store, abortControllerRef])

  useEffect(() => {
    syncRef.current()
  }, [messages, isLoading])
}
