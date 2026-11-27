/**
 * Per-turn "From past chats" block.
 *
 * Gizzi owns past-chat recall for every surface (CONTRACT.md "One owner for
 * past chats"): before a turn it asks allternit-api's raw-search (`mode:"auto"`)
 * for earlier turns that match the user's message and hands the model a dated
 * `<past-chats>` block as a synthetic part on the last user message (never the
 * system prompt: per-turn system changes void the prompt cache).
 *
 * Best effort and bounded: hard ~900 ms ceiling, never throws, no block on
 * timeout, signed out, an old API or no results.
 */
import { platformRequest } from "@/runtime/bots/platform-api"
import { memoryDriveEnabled } from "@/runtime/memory/drive/paths"
import { MemoryKernelAdapter } from "@/runtime/memory/kernel-adapter"

export interface PastChatRef {
  id: string
  title: string
  action: "recalled"
  source: "past_chat"
  date: string
  session_id: string
}

export interface RawTurn {
  observation_id: string
  session_id: string
  speaker: string
  timestamp: string
  text: string
  score?: number
}

export interface KnowledgeHit {
  path: string
  kind: string
  text: string
  score?: number
}

export interface RawSearchResponse {
  turns?: RawTurn[]
  knowledge?: KnowledgeHit[]
}

export type RawRequest = (method: "POST" | "GET", path: string, body?: unknown, opts?: { timeoutMs?: number }) => Promise<unknown>

export namespace PastChats {
  export const TIMEOUT_MS = 900
  const MAX_REFS = 4
  const MAX_LINE_CHARS = 700
  const MIN_WORDS = 3

  type Flags = { memorySearch?: boolean; incognito?: boolean }
  const turnFlags = new Map<string, Flags>()
  const taken = new Set<string>()
  const results = new Map<string, Promise<Built | undefined>>()

  export interface Built {
    /** The `<past-chats>` block; empty when only knowledge matched. */
    block: string
    /** The separate `<knowledge>` block (approved guides); never mixed into past-chats, never in refs. */
    knowledge?: string
    refs: PastChatRef[]
  }

  /** The turn's switches (request `memory_search`, incognito). Returns a release fn. */
  export function setTurn(sessionID: string, flags: Flags): () => void {
    turnFlags.set(sessionID, flags)
    return () => {
      if (turnFlags.get(sessionID) === flags) turnFlags.delete(sessionID)
    }
  }

  export function flagsFor(sessionID: string): Flags {
    return turnFlags.get(sessionID) ?? {}
  }

  function day(ts: string): string {
    return ts.slice(0, 10)
  }

  function shortDate(ts: string): string {
    const d = new Date(ts)
    if (Number.isNaN(d.getTime())) return day(ts)
    return d.toLocaleDateString("en-US", { month: "short", day: "numeric", timeZone: "UTC" })
  }

  function clean(text: string, max: number): string {
    const t = text.replace(/\s+/g, " ").trim()
    return t.length > max ? `${t.slice(0, max - 1)}…` : t
  }

  export function formatKnowledge(hits: KnowledgeHit[]): string | undefined {
    const usable = hits.filter((h) => h.text?.trim()).slice(0, 3)
    if (usable.length === 0) return undefined
    const lines = usable.map((h) => {
      const name = h.path.split("/").pop()?.replace(/\.md$/, "") ?? h.path
      return `- [${h.kind} · ${name}] ${clean(h.text, MAX_LINE_CHARS)}`
    })
    return [
      `<knowledge note="Approved guides from this person's Memory Drive (business knowledge and data-access guides). Prefer them over guessing; flag with knowledge_flag if one is wrong or stale.">`,
      ...lines,
      `</knowledge>`,
    ].join("\n")
  }

  export function format(turns: RawTurn[], hits: KnowledgeHit[] = []): Built | undefined {
    const usable = turns.filter((t) => t.observation_id && t.text?.trim())
    const knowledge = formatKnowledge(hits)
    if (usable.length === 0) return knowledge ? { block: "", knowledge, refs: [] } : undefined
    const lines = usable.map((t) => `[${day(t.timestamp)} · ${t.speaker === "assistant" ? "assistant" : "user"}] ${clean(t.text, MAX_LINE_CHARS)}`)
    const block = [
      `<past-chats note="Earlier conversations with this person that may be relevant. Dated; may be out of date.">`,
      ...lines,
      `</past-chats>`,
    ].join("\n")
    const refs: PastChatRef[] = []
    for (const t of usable) {
      if (refs.length >= MAX_REFS) break
      if (refs.some((r) => r.id === t.observation_id)) continue
      refs.push({
        id: t.observation_id,
        title: `${shortDate(t.timestamp)} · ${clean(t.text, 60)}`,
        action: "recalled",
        source: "past_chat",
        date: t.timestamp,
        session_id: t.session_id,
      })
    }
    return { block, knowledge, refs }
  }

  export interface BuildInput {
    sessionID: string
    text: string
    /** Request `memory_search`; undefined means on. */
    memorySearch?: boolean
    incognito?: boolean
    request?: RawRequest
    timeoutMs?: number
  }

  /** The block for one user message, or undefined when skipped, empty or late. Never throws. */
  export async function build(input: BuildInput): Promise<Built | undefined> {
    try {
      if (input.incognito || input.memorySearch === false) return undefined
      if (!memoryDriveEnabled() || !MemoryKernelAdapter.enabled()) return undefined
      const query = input.text.trim()
      if (query.split(/\s+/).filter(Boolean).length < MIN_WORDS) return undefined
      const timeoutMs = input.timeoutMs ?? TIMEOUT_MS
      const request: RawRequest = input.request ?? ((m, p, b, o) => platformRequest(m, p, b, o))
      let timer: ReturnType<typeof setTimeout> | undefined
      const late = new Promise<undefined>((resolve) => {
        timer = setTimeout(() => resolve(undefined), timeoutMs)
      })
      const fetched = request(
        "POST",
        "/api/v1/memory/v2/raw-search",
        {
          query,
          mode: "auto",
          calling_session: input.sessionID,
          exclude_sessions: [input.sessionID],
          include_knowledge: true,
        },
        { timeoutMs },
      )
        .then((r) => format((r as RawSearchResponse)?.turns ?? [], (r as RawSearchResponse)?.knowledge ?? []))
        .catch(() => undefined)
      const out = await Promise.race([fetched, late])
      if (timer) clearTimeout(timer)
      return out
    } catch {
      return undefined
    }
  }

  /** One lookup per user message: later steps of the same turn reuse it (stable text keeps the prompt cache). */
  export function forUserMessage(userMessageID: string, input: BuildInput): Promise<Built | undefined> {
    const hit = results.get(userMessageID)
    if (hit) return hit
    const p = build(input)
    results.set(userMessageID, p)
    if (results.size > 64) {
      const oldest = results.keys().next().value
      if (oldest !== undefined) {
        results.delete(oldest)
        taken.delete(oldest)
      }
    }
    return p
  }

  /**
   * Refs for the assistant message that answers this user message (read by the
   * processor). Handed out once: the first assistant message of the turn gets
   * them, later steps get none.
   */
  export async function refsFor(userMessageID: string): Promise<PastChatRef[]> {
    const hit = results.get(userMessageID)
    if (!hit || taken.has(userMessageID)) return []
    taken.add(userMessageID)
    return (await hit)?.refs ?? []
  }

  export function resetForTests() {
    results.clear()
    taken.clear()
    turnFlags.clear()
  }
}
