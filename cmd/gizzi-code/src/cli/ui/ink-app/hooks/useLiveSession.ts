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
    let retrySync: ReturnType<typeof setTimeout> | undefined
    let lastBusy: boolean | undefined
    const sent = new Map<string, string>()
    const handlers = new Map<string, (response: BridgePermissionResponse) => void>()

    // AskUserQuestion requests go out as engine questions (the surfaces' question card), not approvals.
    const questions = new Map<string, Record<string, unknown>>()
    // Open asks by engine id, re-sent when the inbox reconnects (an engine restart forgets them).
    const open = new Map<string, { path: string; body: unknown }>()
    const ask = (id: string, path: string, body: unknown) => {
      open.set(id, { path, body })
      void engineFetch(path, body).catch(() => {})
    }
    const settle = (id: string, path: string, body: unknown) => {
      open.delete(id)
      void engineFetch(path, body).catch(() => {})
    }

    const permissionCallbacks: BridgePermissionCallbacks = {
      sendRequest(requestId, toolName, input, toolUseId, description) {
        if (!sessionID) return
        if (toolName === ASK_USER_QUESTION) {
          questions.set(engineQuestionID(requestId), input)
          ask(engineQuestionID(requestId), `/sessions/${sessionID}/question`, {
            requestID: engineQuestionID(requestId),
            questions: toEngineQuestions(input),
            callID: toolUseId,
          })
          return
        }
        ask(enginePermissionID(requestId), `/sessions/${sessionID}/permission`, {
          requestID: enginePermissionID(requestId),
          tool: toolName,
          description,
          input,
          callID: toolUseId,
        })
      },
      sendResponse(requestId, response) {
        const question = questions.get(engineQuestionID(requestId))
        if (question) {
          questions.delete(engineQuestionID(requestId))
          settle(engineQuestionID(requestId), `/question/${engineQuestionID(requestId)}/settle`, {
            answers: response.behavior === 'allow' ? fromTerminalAnswers(question, response.updatedInput) : undefined,
          })
          return
        }
        settle(enginePermissionID(requestId), `/permission/${enginePermissionID(requestId)}/settle`, {
          allow: response.behavior === 'allow',
        })
      },
      cancelRequest(requestId) {
        if (questions.delete(engineQuestionID(requestId))) {
          settle(engineQuestionID(requestId), `/question/${engineQuestionID(requestId)}/settle`, {})
          return
        }
        settle(enginePermissionID(requestId), `/permission/${enginePermissionID(requestId)}/settle`, { allow: false })
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
          // Like Esc: close any open approval or question (deny), then end the turn.
          for (const [id, handler] of [...handlers]) {
            handlers.delete(id)
            questions.delete(id)
            handler({ behavior: 'deny', message: 'Stopped from another device.' })
            // Answered here, so close its card on the other surfaces too.
            const kind = id.startsWith('que_') ? 'question' : 'permission'
            if (open.has(id)) settle(id, `/${kind}/${id}/settle`, kind === 'question' ? {} : { allow: false })
          }
          abortControllerRef.current?.abort()
          // Report the real state again even if it did not change since the last sync.
          lastBusy = undefined
          sync()
          return
        case 'attached':
          // (Re)attached, e.g. after an engine restart: it may have missed turns and lost the
          // open asks. Push the whole transcript and the real status now, and the asks again.
          sent.clear()
          lastBusy = undefined
          sync()
          for (const { path, body } of open.values()) void engineFetch(path, body).catch(() => {})
          return
        case 'permission': {
          open.delete(event.requestID)
          const handler = handlers.get(event.requestID)
          handlers.delete(event.requestID)
          handler?.(event.allow ? { behavior: 'allow' } : { behavior: 'deny', message: event.message })
          return
        }
        case 'question': {
          open.delete(event.requestID)
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
        // An engine that lost its data has no row for this session: adopt it again (same terminal id).
        if (!disposed && sessionID === id && (await readopt(id))) return
      }
    }

    /** Adopt again after the engine went away; true when it handed back a different session id (a new listener took over). */
    async function readopt(id: string): Promise<boolean> {
      try {
        const res = await engineFetch('/sessions', adoptBody())
        const body = (await res.json()) as { sessionID: string }
        if (body.sessionID === id) return false
        sessionID = body.sessionID
        sent.clear()
        lastBusy = undefined
        setLiveSession({ sessionID })
        void listen(body.sessionID)
        sync()
        return true
      } catch {
        return false
      }
    }

    function adoptBody() {
      return {
        terminalID,
        cwd: getCwd(),
        title: firstPrompt(messagesRef.current),
        transcriptPath: getTranscriptPath(),
        model: modelRef(modelRefCurrent.current),
      }
    }

    async function attach() {
      if (disposed) return
      terminalID = String(getSessionId())
      try {
        const res = await engineFetch('/sessions', adoptBody())
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
        lastBusy = undefined
        logForDebugging(`[live] sync failed: ${error}`)
        // Retry on its own: the engine may be restarting, and nothing else would trigger another push.
        if (!disposed && !retrySync) retrySync = setTimeout(() => { retrySync = undefined; sync() }, RECONNECT_MS)
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
      if (retrySync) clearTimeout(retrySync)
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
