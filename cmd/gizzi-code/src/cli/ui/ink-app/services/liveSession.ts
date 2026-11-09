/**
 * Live terminal session: this TUI session as an engine session.
 *
 * The terminal keeps running its own turns. It attaches to the local engine
 * (Desktop's, or `gizzi serve`; GIZZI_ENGINE_URL overrides 127.0.0.1:4096),
 * pushes its transcript, busy/idle and permission asks, and listens on the
 * inbox for prompts, stops and permission answers sent from Desktop, the web
 * app, the PWA or the phone apps. Contract: surfaces/docs/dev/live-terminal-sessions.mdx.
 */
import type { Message } from '../types/message'

export const LIVE_HARNESS = 'gizzi-tui'

export type LivePart =
  | { kind: 'text'; key: string; text: string }
  | { kind: 'reasoning'; key: string; text: string }
  | {
      kind: 'tool'
      key: string
      callID: string
      tool: string
      input: Record<string, unknown>
      status: 'running' | 'completed' | 'error'
      title?: string
      output?: string
      start?: number
      end?: number
    }

export type LiveMessage = {
  key: string
  role: 'user' | 'assistant'
  time: number
  completed?: number
  parent?: { key: string; time: number }
  providerID?: string
  modelID?: string
  error?: string
  parts: LivePart[]
}

export type LiveInbox =
  | { type: 'attached'; sessionID: string }
  | { type: 'heartbeat' }
  | { type: 'input'; id: string; text: string }
  | { type: 'stop' }
  | { type: 'permission'; requestID: string; allow: boolean; message?: string }
  | { type: 'question'; requestID: string; answers?: string[][] }

type LiveState = { engineURL: string; sessionID?: string; connected: boolean }

const state: LiveState = { engineURL: engineURL(), connected: false }
const listeners = new Set<() => void>()

export function engineURL(): string {
  return (process.env.GIZZI_ENGINE_URL || 'http://127.0.0.1:4096').replace(/\/+$/, '')
}

export function liveSessionEnabled(): boolean {
  const flag = process.env.GIZZI_LIVE_SESSIONS
  return flag !== '0' && flag !== 'false'
}

export function getLiveSession(): Readonly<LiveState> {
  return state
}

export function setLiveSession(patch: Partial<LiveState>): void {
  Object.assign(state, patch)
  for (const listener of listeners) listener()
}

export function onLiveSessionChange(listener: () => void): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

/** Engine permission ids must start with `per`; the terminal's are UUIDs. */
export function enginePermissionID(requestId: string): string {
  return `per_${requestId.replace(/-/g, '')}`
}

export function engineQuestionID(requestId: string): string {
  return `que_${requestId.replace(/-/g, '')}`
}

// ── AskUserQuestion ⇄ engine questions ────────────────────────────────────

export const ASK_USER_QUESTION = 'AskUserQuestion'

type TerminalQuestion = {
  question: string
  header?: string
  multiSelect?: boolean
  options?: Array<{ label: string; description?: string; recommended?: boolean; then?: { question: string; options: Array<{ label: string; description?: string }> } }>
}

function terminalQuestions(input: Record<string, unknown>): TerminalQuestion[] {
  return Array.isArray(input.questions) ? (input.questions as TerminalQuestion[]) : []
}

function isTree(q: TerminalQuestion): boolean {
  return !q.multiSelect && (q.options ?? []).some(o => o.then)
}

/** The terminal's AskUserQuestion input as engine `Question.Info[]`. */
export function toEngineQuestions(input: Record<string, unknown>) {
  return terminalQuestions(input).map(q => ({
    question: q.question,
    header: (q.header ?? '').slice(0, 30),
    multiple: q.multiSelect || undefined,
    options: (q.options ?? []).map(o => ({
      label: o.label,
      description: o.description ?? '',
      ...(o.recommended ? { recommended: true } : {}),
      ...(o.then
        ? { then: { question: o.then.question, options: o.then.options.map(f => ({ label: f.label, description: f.description ?? '' })) } }
        : {}),
    })),
  }))
}

/** Engine answers (labels per question, in order) as AskUserQuestion's `answers` record. */
export function toTerminalAnswers(input: Record<string, unknown>, answers: string[][]): Record<string, string> {
  const out: Record<string, string> = {}
  terminalQuestions(input).forEach((q, i) => {
    const picked = answers[i] ?? []
    if (picked.length) out[q.question] = picked.join(isTree(q) ? ' → ' : ', ')
  })
  return out
}

/** The terminal's own answers, for closing the question everywhere. */
export function fromTerminalAnswers(input: Record<string, unknown>, updated: Record<string, unknown> | undefined): string[][] {
  const record = (updated?.answers ?? {}) as Record<string, string>
  return terminalQuestions(input).map(q => {
    const value = record[q.question]
    if (!value) return []
    return isTree(q) ? value.split(' → ') : q.multiSelect ? value.split(', ') : [value]
  })
}

export async function engineFetch(path: string, body?: unknown, signal?: AbortSignal): Promise<Response> {
  const res = await fetch(`${state.engineURL}/v1/terminal${path}`, {
    method: body === undefined ? 'GET' : 'POST',
    headers: body === undefined ? undefined : { 'content-type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: signal ?? AbortSignal.timeout(5_000),
  })
  if (!res.ok) throw new Error(`engine ${path}: ${res.status}`)
  return res
}

// ── transcript → engine messages ──────────────────────────────────────────

function time(m: Message): number {
  const t = m.timestamp
  if (typeof t === 'number') return t
  const parsed = t ? Date.parse(t) : NaN
  return Number.isFinite(parsed) ? parsed : Date.now()
}

type Block = { type: string; [key: string]: any }

function blocks(m: Message): Block[] {
  const content = (m as any).message?.content
  if (typeof content === 'string') return [{ type: 'text', text: content }]
  return Array.isArray(content) ? content : []
}

function resultText(content: unknown): string {
  if (typeof content === 'string') return content
  if (Array.isArray(content))
    return content
      .map((c: any) => (c?.type === 'text' ? c.text : c?.type === 'image' ? '[image]' : ''))
      .filter(Boolean)
      .join('\n')
  return ''
}

const OUTPUT_LIMIT = 16_000

/**
 * One engine user message per prompt, one engine assistant message per turn
 * (its text, reasoning and tool calls, tool results folded into their call).
 */
export function toLiveMessages(messages: readonly Message[], busy: boolean): LiveMessage[] {
  const out: LiveMessage[] = []
  let turn: LiveMessage | undefined
  let prompt: LiveMessage | undefined
  const tools = new Map<string, Extract<LivePart, { kind: 'tool' }>>()
  let last = 0

  for (const m of messages) {
    if (m.type === 'user' || m.type === 'assistant') last = Math.max(last, time(m))
    if (m.type === 'user') {
      const content = blocks(m)
      const results = content.filter(b => b.type === 'tool_result')
      for (const r of results) {
        const tool = tools.get(r.tool_use_id)
        if (!tool) continue
        tool.status = r.is_error ? 'error' : 'completed'
        tool.output = resultText(r.content).slice(0, OUTPUT_LIMIT)
        tool.end = time(m)
      }
      if (results.length || m.isMeta || m.isCompactSummary || m.isVisibleInTranscriptOnly) continue
      const text = content
        .filter(b => b.type === 'text')
        .map(b => b.text as string)
        .join('\n')
        .trim()
      // The terminal's own interrupt marker is not a prompt; the turn's error state already shows it.
      if (!text || text.startsWith('[Request interrupted by user')) continue
      if (turn) turn.completed ??= time(m)
      turn = undefined
      prompt = { key: m.uuid, role: 'user', time: time(m), parts: [{ kind: 'text', key: 'text', text }] }
      out.push(prompt)
      continue
    }
    if (m.type !== 'assistant') continue
    if (!turn) {
      turn = {
        key: `${m.uuid}:turn`,
        role: 'assistant',
        time: time(m),
        parts: [],
        ...(prompt ? { parent: { key: prompt.key, time: prompt.time } } : {}),
      }
      out.push(turn)
    }
    const model = (m as any).message?.model as string | undefined
    if (model) turn.modelID = model
    if (m.isApiErrorMessage) turn.error = resultText((m as any).message?.content) || 'Error'
    content: for (const [i, b] of blocks(m).entries()) {
      const key = `${m.uuid}:${i}`
      switch (b.type) {
        case 'text':
          if (b.text?.trim()) turn.parts.push({ kind: 'text', key, text: b.text })
          break
        case 'thinking':
          if (b.thinking?.trim()) turn.parts.push({ kind: 'reasoning', key, text: b.thinking })
          break
        case 'tool_use': {
          const part: Extract<LivePart, { kind: 'tool' }> = {
            kind: 'tool',
            key: b.id,
            callID: b.id,
            tool: b.name,
            input: (b.input ?? {}) as Record<string, unknown>,
            status: 'running',
            title: b.name,
            start: time(m),
          }
          tools.set(b.id, part)
          turn.parts.push(part)
          break
        }
        default:
          continue content
      }
    }
  }
  if (turn && !busy) turn.completed ??= last
  return out
}

// ── links that open this session elsewhere ────────────────────────────────

export type LiveLinks = {
  /** Phone app / PWA / any browser; goes through this machine's paired runtime. */
  phone?: string
  web?: string
  /** Allternit Desktop on this machine. */
  desktop: string
  runtimeId?: string
  signedIn: boolean
}

export async function liveLinks(sessionID: string): Promise<LiveLinks> {
  const { Pairing } = await import('@/runtime/services/pairing/pairing')
  const device = await Pairing.load().catch(() => undefined)
  const signedIn = Pairing.tokenUsable(device)
  const runtimeId = signedIn ? device?.runtimeId : undefined
  const query = runtimeId
    ? `?${new URLSearchParams({ runtime: runtimeId, session: sessionID })}`
    : undefined
  return {
    phone: query && `https://m.allternit.com/${query}`,
    web: query && `https://ai.allternit.com/${query}`,
    desktop: `allternit://session?${new URLSearchParams({ id: sessionID })}`,
    runtimeId,
    signedIn,
  }
}
