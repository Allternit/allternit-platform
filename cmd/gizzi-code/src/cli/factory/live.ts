/**
 * Live link to the running Factory engine (`allternit-factory serve`).
 *
 * The board, `gizzi swarm wait` and the TUI footer badge read the engine's
 * HTTP surface instead of spawning `allternit-factory … --json` per frame:
 *
 *   GET /api/factory/agents              one snapshot of every bot
 *   GET /api/factory/events  (SSE)       ledger events + `agent.state` changes
 *   GET /api/factory/agents/:id/stream   (SSE) `screen` frames when a pane changes
 *
 * Nothing here polls the engine on a timer. A snapshot is re-read when the
 * event stream says something changed; a pane's last line comes from its
 * screen stream. A dropped stream reconnects with backoff.
 */
import { factoryEnginePorts } from "@/cli/factory/engine"
import type { FactoryAgent } from "@/cli/factory/types"

export interface EngineSnapshot {
  agents: FactoryAgent[]
}

/** First engine port that answers, or null. ALLTERNIT_FACTORY_URL wins. */
export async function findEngineUrl(env: NodeJS.ProcessEnv = process.env): Promise<string | null> {
  if (env.ALLTERNIT_FACTORY_URL) return env.ALLTERNIT_FACTORY_URL.replace(/\/$/, "")
  for (const port of factoryEnginePorts(env)) {
    const base = `http://127.0.0.1:${port}`
    try {
      const res = await fetch(`${base}/api/factory/health`, { signal: AbortSignal.timeout(1500) })
      if (res.ok) return base
    } catch {
      // next port
    }
  }
  return null
}

export const ENGINE_NOT_SERVING =
  "The Factory engine isn't serving on this computer (checked 127.0.0.1:3018 and :3011). Open Allternit Desktop, or run `allternit-factory serve`, or set ALLTERNIT_FACTORY_PORT."

export async function fetchSnapshot(base: string): Promise<EngineSnapshot> {
  const res = await fetch(`${base}/api/factory/agents`, { signal: AbortSignal.timeout(8000) })
  if (!res.ok) throw new Error(`The engine answered ${res.status} for its agents list`)
  const doc = (await res.json()) as { agents?: FactoryAgent[] }
  return { agents: Array.isArray(doc.agents) ? doc.agents : [] }
}

export interface SseEvent {
  event: string
  data: string
  id?: string
}

/**
 * Read one SSE response until it ends or `signal` aborts. Resolves when the
 * stream closes; rejects on a non-200 answer.
 */
export async function readSse(url: string, onEvent: (e: SseEvent) => void, signal: AbortSignal): Promise<void> {
  const res = await fetch(url, { headers: { accept: "text/event-stream" }, signal })
  if (!res.ok || !res.body) throw new Error(`${url} answered ${res.status}`)
  const reader = res.body.getReader()
  const decoder = new TextDecoder()
  let buf = ""
  let cur: SseEvent = { event: "message", data: "" }
  for (;;) {
    const { value, done } = await reader.read()
    if (done) return
    buf += decoder.decode(value, { stream: true })
    let nl: number
    while ((nl = buf.indexOf("\n")) >= 0) {
      const line = buf.slice(0, nl).replace(/\r$/, "")
      buf = buf.slice(nl + 1)
      if (line === "") {
        if (cur.data || cur.event !== "message") onEvent(cur)
        cur = { event: "message", data: "" }
      } else if (line.startsWith(":")) {
        // keep-alive comment
      } else {
        const at = line.indexOf(":")
        const field = at < 0 ? line : line.slice(0, at)
        const val = at < 0 ? "" : line.slice(at + 1).replace(/^ /, "")
        if (field === "event") cur.event = val
        else if (field === "data") cur.data = cur.data ? `${cur.data}\n${val}` : val
        else if (field === "id") cur.id = val
      }
    }
  }
}

/**
 * Keep an SSE subscription open: reconnect with backoff (1s → 15s) while
 * not stopped. `onOpen` fires on every (re)connect so callers can re-read
 * a snapshot they may have missed while disconnected.
 */
export function subscribeSse(
  url: string,
  handlers: { onEvent: (e: SseEvent) => void; onOpen?: () => void; onDown?: (err: unknown) => void },
): () => void {
  const ctl = new AbortController()
  let delay = 1000
  void (async () => {
    while (!ctl.signal.aborted) {
      try {
        handlers.onOpen?.()
        await readSse(
          url,
          (e) => {
            delay = 1000
            handlers.onEvent(e)
          },
          ctl.signal,
        )
      } catch (err) {
        if (ctl.signal.aborted) return
        handlers.onDown?.(err)
      }
      if (ctl.signal.aborted) return
      await new Promise((r) => setTimeout(r, delay))
      delay = Math.min(15000, delay * 2)
    }
  })()
  return () => ctl.abort()
}

const ANSI = /\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[@-Z\\-_]/g

/** Harness chrome that is never "what the bot is doing". */
const CHROME = [
  /^[─━═\-_│┃|╭╮╰╯┌┐└┘\s]+$/, // rules and box edges
  /^[│┃|]?\s*[❯›>]\s*/, // the prompt line
  /shift\+tab|\? for shortcuts|bypass permissions|esc to interrupt|ctrl\+[a-z] to/i,
  /Transcript saving is off|auto-accept edits|plan mode on/i,
  /^\s*⏵⏵/,
  /^\s*(⏎|enter) (send|to send)/i,
  /context left|tokens? used|% context/i,
  /\/effort\b|\/model\b.*to change/i, // model/effort footer
  /enter to (select|confirm)|esc to cancel|to navigate/i, // picker hints
]

/**
 * The last line of a pane's screen that says what the bot is doing: ANSI
 * stripped, harness chrome (prompt box, footer hints, rules) skipped.
 */
export function lastMeaningfulLine(screen: string): string {
  const lines = screen.replace(ANSI, "").split(/\r?\n/)
  for (let i = lines.length - 1; i >= 0; i--) {
    const raw = lines[i]!.replace(/[│┃]\s*$/, "").trim()
    if (!raw || !/[\p{L}\p{N}]/u.test(raw)) continue
    if (CHROME.some((re) => re.test(raw))) continue
    return raw.replace(/\s{2,}/g, " ")
  }
  return ""
}

/**
 * One live view of the engine for a whole process: the agents snapshot,
 * re-read on every engine event, and the last line of each live pane from
 * its screen stream. Listeners get the current view on every change.
 */
export interface LiveView {
  connected: boolean
  error: string | null
  agents: FactoryAgent[]
  lastLine: Record<string, string>
  /** When each agent entered its current state, as seen by this process. */
  since: Record<string, number>
}

type Listener = (v: LiveView) => void

export class FactoryLive {
  private view: LiveView = { connected: false, error: null, agents: [], lastLine: {}, since: {} }
  private listeners = new Set<Listener>()
  private stopEvents: (() => void) | null = null
  private streams = new Map<string, () => void>()
  private base: string | null = null
  private reading = false
  private again = false
  private stopped = false
  private retry: ReturnType<typeof setTimeout> | null = null
  /** Open a screen stream per live pane (the board); the badge doesn't need lines. */
  constructor(private readonly opts: { lines: boolean } = { lines: true }) {}

  get current(): LiveView {
    return this.view
  }

  get url(): string | null {
    return this.base
  }

  subscribe(fn: Listener): () => void {
    this.listeners.add(fn)
    fn(this.view)
    return () => this.listeners.delete(fn)
  }

  async start(): Promise<void> {
    this.stopped = false
    this.base = await findEngineUrl()
    if (this.stopped) return
    if (!this.base) {
      this.set({ connected: false, error: ENGINE_NOT_SERVING })
      // Desktop may start later: look again in a while (a health check, not a data poll).
      this.retry = setTimeout(() => void this.start(), 30000)
      return
    }
    this.stopEvents = subscribeSse(`${this.base}/api/factory/events`, {
      onOpen: () => void this.reread(),
      onEvent: () => void this.reread(),
      onDown: (err) => this.set({ connected: false, error: `Lost the engine's event stream: ${(err as Error)?.message ?? err}` }),
    })
  }

  stop(): void {
    this.stopped = true
    if (this.retry) clearTimeout(this.retry)
    this.stopEvents?.()
    this.stopEvents = null
    for (const s of this.streams.values()) s()
    this.streams.clear()
  }

  /** Re-read now (after a local action such as stop or send). */
  refresh(): void {
    void this.reread()
  }

  private set(patch: Partial<LiveView>) {
    this.view = { ...this.view, ...patch }
    for (const l of this.listeners) l(this.view)
  }

  private async reread(): Promise<void> {
    if (!this.base) return
    if (this.reading) {
      this.again = true
      return
    }
    this.reading = true
    try {
      do {
        this.again = false
        try {
          const snap = await fetchSnapshot(this.base)
          const since = { ...this.view.since }
          const prev = new Map(this.view.agents.map((a) => [a.id, a.state]))
          for (const a of snap.agents) if (prev.get(a.id) !== a.state || !since[a.id]) since[a.id] = Date.now()
          this.set({ connected: true, error: null, agents: snap.agents, since })
          if (this.opts.lines) this.syncStreams(snap.agents)
        } catch (err) {
          this.set({ connected: false, error: (err as Error)?.message ?? String(err) })
        }
      } while (this.again)
    } finally {
      this.reading = false
    }
  }

  private syncStreams(agents: FactoryAgent[]) {
    const live = new Set(agents.filter((a) => a.pane && a.state !== "offline").map((a) => a.id))
    for (const [id, stop] of this.streams) {
      if (!live.has(id)) {
        stop()
        this.streams.delete(id)
      }
    }
    for (const id of live) {
      if (this.streams.has(id)) continue
      let pending: string | null = null
      let timer: ReturnType<typeof setTimeout> | null = null
      const stop = subscribeSse(`${this.base}/api/factory/agents/${encodeURIComponent(id)}/stream`, {
        onEvent: (e) => {
          if (e.event !== "screen") return
          try {
            pending = String((JSON.parse(e.data) as { ansi?: string }).ansi ?? "")
          } catch {
            return
          }
          // Spinners repaint every frame; take the newest screen at most 4x a second.
          if (timer) return
          timer = setTimeout(() => {
            timer = null
            const line = lastMeaningfulLine(pending ?? "")
            if (line && line !== this.view.lastLine[id]) this.set({ lastLine: { ...this.view.lastLine, [id]: line } })
          }, 250)
        },
      })
      this.streams.set(id, () => {
        if (timer) clearTimeout(timer)
        stop()
      })
    }
  }
}

let shared: { live: FactoryLive; refs: number } | null = null

/** One badge-grade (no screen streams) live view shared across the TUI. */
export function acquireSharedLive(): { live: FactoryLive; release: () => void } {
  if (!shared) {
    shared = { live: new FactoryLive({ lines: false }), refs: 0 }
    void shared.live.start()
  }
  shared.refs++
  const mine = shared
  return {
    live: mine.live,
    release: () => {
      mine.refs--
      if (mine.refs <= 0 && shared === mine) {
        mine.live.stop()
        shared = null
      }
    },
  }
}
