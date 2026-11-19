/**
 * Local keyword index for `resource` search (PR3, plan decision 3.10).
 *
 * In memory, per session, built lazily on the first search of a doc. Nothing
 * leaves the device and there is no server index. Ranking is BM25 over
 * overlapping line windows, so a hit is a line range the answer can cite.
 *
 * Free of DB, MCP and UI imports so bun tests can rank fixtures directly.
 */
import type { ResourceCitation, ResourceSearchHit } from "@/runtime/primitives/gap-types"

export namespace ResourceIndex {
  /** Lines per window, and the step between window starts (windows overlap by half). */
  export const WINDOW_LINES = 20
  export const WINDOW_STEP = 10
  /** Lines shown per hit snippet, and their width. */
  export const SNIPPET_LINES = 3
  export const SNIPPET_CHARS = 200
  /** Cache bounds: sessions kept, docs per session. */
  const MAX_SESSIONS = 8
  const MAX_DOCS_PER_SESSION = 50
  /** Drive files can change; attachments and saved outputs cannot. */
  const MUTABLE_TTL_MS = 5 * 60 * 1000

  const K1 = 1.2
  const B = 0.75
  const PHRASE_BONUS = 1.5

  const STOP = new Set(
    "a an and are as at be by for from has have in is it its of on or that the this to was were will with".split(" "),
  )

  export interface Doc {
    ref: string
    title: string
    lines: string[]
    mime?: string
    /** True when only the first part of the file was loaded. */
    partial?: boolean
  }

  interface Window {
    /** 0-based, inclusive. */
    start: number
    end: number
    tf: Map<string, number>
    len: number
  }

  interface Indexed {
    doc: Doc
    windows?: Window[]
    at: number
    mutable: boolean
  }

  export interface Hit extends ResourceSearchHit {
    citation: ResourceCitation
    snippet: string[]
  }

  export interface Ranked {
    hits: Hit[]
    /** All hits before paging. */
    total: number
    /** Offset to pass back for the next page, or null at the end. */
    next: number | null
  }

  // ── tokens ─────────────────────────────────────────────────────────────────

  function stem(t: string): string {
    if (t.length > 4 && t.endsWith("ies")) return `${t.slice(0, -3)}y`
    if (t.length > 3 && t.endsWith("s") && !t.endsWith("ss")) return t.slice(0, -1)
    return t
  }

  export function tokenize(text: string): string[] {
    const out: string[] = []
    for (const raw of text.toLowerCase().split(/[^\p{L}\p{N}_]+/u)) {
      if (raw.length < 2 || STOP.has(raw)) continue
      out.push(stem(raw))
    }
    return out
  }

  function windows(lines: string[]): Window[] {
    const out: Window[] = []
    const total = lines.length
    for (let start = 0; start < Math.max(total, 1); start += WINDOW_STEP) {
      const end = Math.min(total, start + WINDOW_LINES) - 1
      const tf = new Map<string, number>()
      let len = 0
      for (let i = start; i <= end; i++) {
        for (const t of tokenize(lines[i].slice(0, 10_000))) {
          tf.set(t, (tf.get(t) ?? 0) + 1)
          len++
        }
      }
      out.push({ start, end: Math.max(start, end), tf, len })
      if (end >= total - 1) break
    }
    return out
  }

  // ── ranking ────────────────────────────────────────────────────────────────

  /**
   * Rank windows of `docs` against `query`. Overlapping windows of one doc
   * collapse to the best; each hit's range is tightened to the lines that
   * hold a query term.
   */
  export function rank(
    docs: Array<Doc | Indexed>,
    query: string,
    opts: { max: number; offset?: number } = { max: 10 },
  ): Ranked {
    const terms = [...new Set(tokenize(query))]
    if (terms.length === 0) return { hits: [], total: 0, next: null }
    const phrase = query.trim().toLowerCase()
    const multiWord = terms.length > 1

    const all: Array<{ doc: Doc; w: Window }> = []
    for (const d of docs) {
      const entry: Indexed = "doc" in d ? d : { doc: d, at: Date.now(), mutable: false }
      entry.windows ??= windows(entry.doc.lines)
      for (const w of entry.windows) all.push({ doc: entry.doc, w })
    }
    if (all.length === 0) return { hits: [], total: 0, next: null }

    const df = new Map<string, number>()
    let lenSum = 0
    for (const { w } of all) {
      lenSum += w.len
      for (const t of terms) if (w.tf.has(t)) df.set(t, (df.get(t) ?? 0) + 1)
    }
    const n = all.length
    const avg = lenSum / n || 1

    const scored: Array<{ doc: Doc; w: Window; score: number }> = []
    for (const item of all) {
      let score = 0
      for (const t of terms) {
        const f = item.w.tf.get(t)
        if (!f) continue
        const d = df.get(t) ?? 0
        const idf = Math.log(1 + (n - d + 0.5) / (d + 0.5))
        score += (idf * f * (K1 + 1)) / (f + K1 * (1 - B + (B * item.w.len) / avg))
      }
      if (score <= 0) continue
      if (multiWord) {
        const text = item.doc.lines.slice(item.w.start, item.w.end + 1).join("\n").toLowerCase()
        if (text.includes(phrase)) score *= PHRASE_BONUS
      }
      scored.push({ ...item, score })
    }
    scored.sort((a, b) => b.score - a.score || a.w.start - b.w.start)

    const kept: Array<{ doc: Doc; w: Window; score: number }> = []
    for (const s of scored) {
      const overlaps = kept.some((k) => k.doc === s.doc && s.w.start <= k.w.end && k.w.start <= s.w.end)
      if (!overlaps) kept.push(s)
    }

    const offset = Math.max(0, opts.offset ?? 0)
    const page = kept.slice(offset, offset + opts.max)
    const hits = page.map((s) => toHit(s.doc, s.w, s.score, terms))
    const next = offset + page.length < kept.length ? offset + page.length : null
    return { hits, total: kept.length, next }
  }

  function toHit(doc: Doc, w: Window, score: number, terms: string[]): Hit {
    const matched: number[] = []
    const counts = new Map<string, number>()
    for (let i = w.start; i <= w.end; i++) {
      const toks = tokenize((doc.lines[i] ?? "").slice(0, 10_000))
      let any = false
      for (const t of toks) {
        if (!terms.includes(t)) continue
        counts.set(t, (counts.get(t) ?? 0) + 1)
        any = true
      }
      if (any) matched.push(i)
    }
    const first = matched[0] ?? w.start
    const last = matched[matched.length - 1] ?? w.end
    const snippet = matched.slice(0, SNIPPET_LINES).map((i) => {
      const line = doc.lines[i].trim()
      return `${i + 1}| ${line.length > SNIPPET_CHARS ? `${line.slice(0, SNIPPET_CHARS)}…` : line}`
    })
    const why = [...counts.entries()]
      .sort((a, b) => b[1] - a[1])
      .map(([t, c]) => `"${t}" x${c}`)
      .join(", ")
    const missing = terms.filter((t) => !counts.has(t))
    return {
      ref: doc.ref,
      title: doc.title,
      why: missing.length ? `${why}; no "${missing.join('", "')}"` : why,
      score: Math.round(score * 100) / 100,
      citation: { ref: doc.ref, start_line: first + 1, end_line: last + 1 },
      snippet,
    }
  }

  // ── per-session cache ──────────────────────────────────────────────────────

  const sessions = new Map<string, Map<string, Indexed>>()

  /** A cached doc, or undefined when absent or stale. */
  export function get(sessionID: string, ref: string): Doc | undefined {
    const docs = sessions.get(sessionID)
    const entry = docs?.get(ref)
    if (!entry) return undefined
    if (entry.mutable && Date.now() - entry.at > MUTABLE_TTL_MS) {
      docs!.delete(ref)
      return undefined
    }
    // Refresh recency of the session.
    sessions.delete(sessionID)
    sessions.set(sessionID, docs!)
    return entry.doc
  }

  export function put(sessionID: string, doc: Doc, opts: { mutable?: boolean } = {}): void {
    let docs = sessions.get(sessionID)
    if (docs) sessions.delete(sessionID)
    else docs = new Map()
    sessions.set(sessionID, docs)
    docs.set(doc.ref, { doc, at: Date.now(), mutable: opts.mutable ?? false })
    while (docs.size > MAX_DOCS_PER_SESSION) docs.delete(docs.keys().next().value!)
    while (sessions.size > MAX_SESSIONS) sessions.delete(sessions.keys().next().value!)
  }

  /** Cached entries (with any windows built so far) for `refs`, in order. */
  export function entries(sessionID: string, refs: string[]): Indexed[] {
    const docs = sessions.get(sessionID)
    if (!docs) return []
    return refs.map((r) => docs.get(r)).filter((e): e is Indexed => e !== undefined)
  }

  export function clear(sessionID?: string): void {
    if (sessionID) sessions.delete(sessionID)
    else sessions.clear()
  }
}
