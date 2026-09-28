/**
 * The terminal `/bots` chat as a view of the bot's shared session (decision
 * 2026-09-28, HANDOFF-gateway-key-and-bots-storage Part 2, option 2): turns go
 * to `gizzi serve` through the platform API, and what the terminal shows is
 * rebuilt from the session's rows plus the live `/agent-sessions/sync` feed.
 * Desktop and the pet HUD read the same rows, so all three show one
 * conversation per bot.
 *
 * Pure: history messages and sync events in, chat items out. The ink hook
 * (`useBotChatSession`) turns items into REPL messages; keeping the mapping
 * here lets bun tests drive it without a terminal.
 */
import { messageHandoff, type ThreadMessage } from "@/runtime/bots/platform-threads"

export type BotChatItem =
  | { kind: "user"; key: string; text: string }
  | { kind: "assistant"; key: string; text: string }
  | { kind: "tool"; key: string; tool: string; title: string; error?: string }
  | { kind: "rip"; key: string; generation?: number; reason?: string; from?: string }

/** A sync-feed event, as the platform gateway (and gizzi agent-compat) emit it. */
export interface SyncEvent {
  type: string
  session_id?: string | null
  message_id?: string | null
  part_id?: string | null
  field?: string | null
  delta?: string | null
  part?: RawPart | null
  to?: string | null
  reason?: string | null
  generation?: number | null
  request_id?: string | null
  id?: string
  role?: string
  content?: string
  metadata?: ThreadMessage["metadata"] & { parts?: RawPart[] }
  [key: string]: unknown
}

/** The fields of a gizzi MessageV2 part this view reads. */
export interface RawPart {
  id?: string
  sessionID?: string
  messageID?: string
  type?: string
  text?: string
  synthetic?: boolean
  ignored?: boolean
  time?: { start?: number; end?: number }
  tool?: string
  callID?: string
  state?: { status?: string; input?: Record<string, unknown>; title?: string; error?: string }
  metadata?: { handoff?: unknown } | null
}

function visibleText(part: RawPart): string | null {
  if (part.type !== "text" || part.synthetic || part.ignored) return null
  const text = part.text?.trim()
  return text ? part.text! : null
}

function summarizeInput(input: Record<string, unknown> | undefined): string {
  for (const value of Object.values(input ?? {})) {
    if (typeof value === "string" && value.trim()) {
      const line = value.trim().split("\n")[0]!
      return line.length > 80 ? `${line.slice(0, 79)}…` : line
    }
  }
  return ""
}

/** A finished tool call as one status line, or null while it is still running. */
function toolItem(part: RawPart): BotChatItem | null {
  if (part.type !== "tool" || !part.tool) return null
  const status = part.state?.status
  if (status !== "completed" && status !== "error") return null
  const title = part.state?.title || summarizeInput(part.state?.input)
  return {
    kind: "tool",
    key: part.id ?? `${part.messageID}:${part.callID}`,
    tool: part.tool,
    title,
    ...(status === "error" ? { error: part.state?.error ?? "failed" } : {}),
  }
}

function ripItem(message: ThreadMessage): BotChatItem | null {
  const handoff = messageHandoff(message)
  if (!handoff) return null
  return {
    kind: "rip",
    key: `msg:${message.id}`,
    generation: handoff.generation,
    reason: handoff.reason,
    from: handoff.from,
  }
}

function userText(message: ThreadMessage & { metadata?: { parts?: RawPart[] } }): string {
  const parts = message.metadata?.parts
  if (!parts?.length) return message.content
  return parts
    .map(visibleText)
    .filter((t): t is string => t !== null)
    .join("\n")
}

/** Items for one stored message (history load, or a `message_added` event). */
export function itemsFromMessage(message: ThreadMessage & { metadata?: { parts?: RawPart[] } }): BotChatItem[] {
  const rip = ripItem(message)
  if (rip) return [rip]
  if (message.role === "user") {
    const text = userText(message)
    return text.trim() ? [{ kind: "user", key: `msg:${message.id}`, text }] : []
  }
  if (message.role !== "assistant") return []
  const parts = message.metadata?.parts
  if (!parts?.length) {
    return message.content.trim() ? [{ kind: "assistant", key: `msg:${message.id}`, text: message.content }] : []
  }
  const items: BotChatItem[] = []
  for (const part of parts) {
    const text = visibleText(part)
    if (text !== null) items.push({ kind: "assistant", key: part.id ?? `msg:${message.id}`, text })
    const tool = toolItem(part)
    if (tool) items.push(tool)
  }
  return items
}

export function itemsFromMessages(messages: ThreadMessage[]): BotChatItem[] {
  return messages.flatMap(itemsFromMessage)
}

/** What one sync event means for the open chat. */
export interface BotChatUpdate {
  /** Items to append, in order (already de-duplicated). */
  commit: BotChatItem[]
  /** New streaming preview: a string, `null` to clear it, `undefined` to leave it. */
  streamingText?: string | null
  /** The conversation moved to a fresh window; follow it. */
  handedOffTo?: { sessionId: string; generation?: number; reason?: string }
  permission?: SyncEvent
  question?: SyncEvent
}

/**
 * Folds the sync feed for one session into chat updates. Keys already shown
 * (from history, an earlier event, or this terminal's own send) are skipped,
 * so a message arriving several times — `message.updated` fires on every
 * change — is committed once.
 */
export class BotChatTracker {
  private readonly seen = new Set<string>()
  private readonly textParts = new Set<string>()
  private readonly streaming = new Map<string, string>()
  /** Text this terminal just sent; its echo from the feed is not shown twice. */
  private readonly pendingEchoes: string[] = []

  constructor(public sessionId: string) {}

  /** Record items the view already shows (history load, a turn reply). */
  markShown(items: BotChatItem[]): void {
    for (const item of items) this.seen.add(item.key)
  }

  expectEcho(text: string): void {
    this.pendingEchoes.push(text.trim())
  }

  /** Follow a handoff: new window, fresh streaming state. */
  follow(sessionId: string): void {
    this.sessionId = sessionId
    this.streaming.clear()
    this.textParts.clear()
  }

  private fresh(items: BotChatItem[]): BotChatItem[] {
    const out: BotChatItem[] = []
    for (const item of items) {
      if (this.seen.has(item.key)) continue
      this.seen.add(item.key)
      if (item.kind === "user") {
        const echo = this.pendingEchoes.indexOf(item.text.trim())
        if (echo !== -1) {
          this.pendingEchoes.splice(echo, 1)
          continue
        }
      }
      out.push(item)
    }
    return out
  }

  private preview(): string | null {
    const texts = [...this.streaming.values()].filter((t) => t.length > 0)
    return texts.length ? texts.join("\n\n") : null
  }

  handle(event: SyncEvent): BotChatUpdate {
    const sessionId = event.session_id ?? event.part?.sessionID ?? null
    if (sessionId !== this.sessionId) return { commit: [] }

    switch (event.type) {
      case "part_updated": {
        const part = event.part
        if (!part?.id) return { commit: [] }
        if (part.type === "text" && !part.synthetic && !part.ignored) {
          this.textParts.add(part.id)
          if (part.time?.end === undefined) {
            this.streaming.set(part.id, part.text ?? this.streaming.get(part.id) ?? "")
            return { commit: [], streamingText: this.preview() }
          }
          this.streaming.delete(part.id)
          const text = visibleText(part)
          const commit = text === null ? [] : this.fresh([{ kind: "assistant", key: part.id, text }])
          return { commit, streamingText: this.preview() }
        }
        const tool = toolItem(part)
        return { commit: tool ? this.fresh([tool]) : [] }
      }
      case "part_delta": {
        const id = event.part_id
        if (!id || event.field !== "text" || !this.textParts.has(id) || this.seen.has(id)) return { commit: [] }
        this.streaming.set(id, (this.streaming.get(id) ?? "") + (event.delta ?? ""))
        return { commit: [], streamingText: this.preview() }
      }
      case "part_removed": {
        if (event.part_id && this.streaming.delete(event.part_id)) return { commit: [], streamingText: this.preview() }
        return { commit: [] }
      }
      case "message_added": {
        if (!event.id || !event.role) return { commit: [] }
        const message = { id: event.id, role: event.role, content: event.content ?? "", metadata: event.metadata }
        // Assistant text commits from its finished part; only users (typed on
        // another surface) and handoff seeds arrive whole here.
        if (event.role === "assistant") return { commit: [] }
        return { commit: this.fresh(itemsFromMessage(message)) }
      }
      case "handed_off": {
        if (!event.to) return { commit: [] }
        return {
          commit: [],
          streamingText: null,
          handedOffTo: {
            sessionId: event.to,
            generation: event.generation ?? undefined,
            reason: event.reason ?? undefined,
          },
        }
      }
      case "permission_asked":
        return { commit: [], permission: event }
      case "question_asked":
        return { commit: [], question: event }
      default:
        return { commit: [] }
    }
  }
}

/**
 * Parse Server-Sent Events from a byte stream. Yields each event's `id` and
 * parsed JSON `data`; comments (heartbeats) and non-JSON payloads are skipped.
 */
export async function* parseSSE(
  body: AsyncIterable<Uint8Array> | ReadableStream<Uint8Array>,
): AsyncGenerator<{ id?: string; data: SyncEvent }> {
  const decoder = new TextDecoder()
  let buffer = ""
  for await (const chunk of body as AsyncIterable<Uint8Array>) {
    buffer += decoder.decode(chunk, { stream: true }).replace(/\r\n/g, "\n")
    let boundary = buffer.indexOf("\n\n")
    while (boundary !== -1) {
      const block = buffer.slice(0, boundary)
      buffer = buffer.slice(boundary + 2)
      boundary = buffer.indexOf("\n\n")
      let id: string | undefined
      const data: string[] = []
      for (const line of block.split("\n")) {
        if (line.startsWith("id:")) id = line.slice(3).trim()
        else if (line.startsWith("data:")) data.push(line.slice(5).replace(/^ /, ""))
      }
      if (data.length === 0) continue
      try {
        yield { id, data: JSON.parse(data.join("\n")) as SyncEvent }
      } catch {
        // not JSON; skip
      }
    }
  }
}
