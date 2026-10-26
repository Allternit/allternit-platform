/**
 * Response-parts contract, runtime side (mirrors allternit-ai
 * `src/lib/ai/response-parts.ts`). Every brain's real citations become
 * `ResponseSource`s stored on the part that carries them — a text part's
 * `metadata.sources` (provider citations) or a completed tool part's
 * `state.metadata.sources` (search/fetch results) — and the chat bridges turn
 * those into additive `source` SSE frames. Plans are gizzi's Todo list; the
 * bridges send each `todo.updated` as a `plan` frame.
 */

export type SourceOrigin =
  | "anthropic-citation"
  | "openai-url-citation"
  | "gemini-grounding"
  | "claude-code-websearch"
  | "provider"
  | "tool"

export interface ResponseSource {
  id: string
  url: string
  title: string
  host: string
  snippet?: string
  origin: SourceOrigin
}

function rec(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : {}
}

function str(value: unknown): string {
  return typeof value === "string" ? value.trim() : ""
}

export function hostOf(url: string): string {
  try {
    return new URL(url).hostname.replace(/^www\./, "")
  } catch {
    return ""
  }
}

function idForUrl(url: string): string {
  let h = 0
  for (let i = 0; i < url.length; i++) h = (h * 31 + url.charCodeAt(i)) | 0
  return `src-${(h >>> 0).toString(36)}`
}

/** One source from any `{url|uri|link, title, snippet}` record; undefined without a web url. */
export function toSource(raw: unknown, origin: SourceOrigin, id?: string): ResponseSource | undefined {
  const o = rec(raw)
  const url = [o.url, o.uri, o.link].map(str).find((u) => /^https?:\/\//i.test(u))
  if (!url) return undefined
  const host = hostOf(url)
  const snippet = (str(o.snippet) || str(o.cited_text) || str(o.text)).replace(/\s+/g, " ").slice(0, 220)
  return {
    id: id || str(o.id) || idForUrl(url),
    url,
    title: str(o.title) || host || url,
    host,
    ...(snippet ? { snippet } : {}),
    origin,
  }
}

export function mergeSources(...lists: Array<ReadonlyArray<ResponseSource> | undefined>): ResponseSource[] {
  const seen = new Set<string>()
  const out: ResponseSource[] = []
  for (const list of lists)
    for (const s of list ?? []) {
      if (seen.has(s.url)) continue
      seen.add(s.url)
      out.push(s)
    }
  return out
}

/**
 * Origin of an AI SDK `source` stream part, from the provider that sent it.
 * The SDK maps Anthropic citations / web_search results, OpenAI Responses
 * `url_citation` annotations and Gemini grounding chunks all into `source`.
 */
export function originForProvider(providerID: string | undefined): SourceOrigin {
  const p = (providerID ?? "").toLowerCase()
  if (p.includes("anthropic") || p.includes("claude")) return "anthropic-citation"
  if (p.includes("openai") || p.includes("azure")) return "openai-url-citation"
  if (p.includes("google") || p.includes("gemini") || p.includes("vertex")) return "gemini-grounding"
  return "provider"
}

/** AI SDK v5 `{type:"source", sourceType:"url", id, url, title}` → source. */
export function sourceFromStreamPart(part: unknown, providerID?: string): ResponseSource | undefined {
  const p = rec(part)
  if (p.type !== "source" || (p.sourceType !== undefined && p.sourceType !== "url")) return undefined
  return toSource(p, originForProvider(providerID), str(p.id) || undefined)
}

/**
 * Claude Code's structured WebSearch result (stream-json `tool_use_result`):
 * `{query, results: [{tool_use_id, content: [{title, url}]}, "summary"]}`.
 */
export function sourcesFromClaudeCodeToolResult(toolUseResult: unknown): ResponseSource[] {
  const results = rec(toolUseResult).results
  if (!Array.isArray(results)) return []
  const out: ResponseSource[] = []
  for (const r of results) {
    const content = rec(r).content
    if (!Array.isArray(content)) continue
    for (const item of content) {
      const s = toSource(item, "claude-code-websearch")
      if (s) out.push(s)
    }
  }
  return mergeSources(out)
}

/** gizzi's websearch (Exa) text: blocks of `Title: …` / `URL: …` lines. */
export function sourcesFromExaText(text: string): ResponseSource[] {
  const out: ResponseSource[] = []
  let title = ""
  for (const line of text.split("\n")) {
    const t = /^Title:\s*(.+)$/.exec(line.trim())
    if (t) {
      title = t[1].trim()
      continue
    }
    const u = /^URL:\s*(https?:\/\/\S+)/.exec(line.trim())
    if (u) {
      const s = toSource({ url: u[1], title }, "tool")
      if (s) out.push(s)
      title = ""
    }
  }
  return mergeSources(out)
}

/** The `source` SSE frames a part's stored sources still need (`sent` dedupes by url per turn). */
export function sourceFramesForPart(part: any, messageId: string, sent: Set<string>): Array<Record<string, unknown>> {
  const list =
    part?.type === "text"
      ? part?.metadata?.sources
      : part?.type === "tool" && part?.state?.status === "completed"
        ? part?.state?.metadata?.sources
        : undefined
  if (!Array.isArray(list)) return []
  const frames: Array<Record<string, unknown>> = []
  for (const raw of list) {
    const s = toSource(raw, (str(rec(raw).origin) as SourceOrigin) || "provider", str(rec(raw).id) || undefined)
    if (!s || sent.has(s.url)) continue
    sent.add(s.url)
    frames.push({
      type: "source",
      messageId,
      sourceId: s.id,
      url: s.url,
      title: s.title,
      host: s.host,
      ...(s.snippet ? { snippet: s.snippet } : {}),
      origin: s.origin,
      ...(part?.type === "tool" && typeof part?.callID === "string" ? { toolCallId: part.callID } : {}),
    })
  }
  return frames
}

export type PlanItemStatus = "pending" | "in_progress" | "completed" | "failed"

export function planItemStatus(raw: unknown): PlanItemStatus {
  const v = String(raw ?? "pending").toLowerCase().replace(/[\s-]/g, "_")
  if (v === "complete" || v === "completed" || v === "done") return "completed"
  if (v === "running" || v === "in_progress" || v === "inprogress" || v === "active") return "in_progress"
  if (v === "cancelled" || v === "canceled" || v === "failed" || v === "error") return "failed"
  return "pending"
}

/**
 * Todo rows from a native plan: Codex `turn/plan/updated` `{plan:[{step,status}]}`,
 * ACP `{entries:[{content,status,priority}]}`, TodoWrite `{todos:[{content,status}]}`.
 */
export function todosFromNativePlan(value: unknown): Array<{ content: string; status: string; priority: string }> | undefined {
  const o = rec(value)
  const list = [o.todos, o.plan, o.entries].find(Array.isArray) as unknown[] | undefined
  if (!list) return undefined
  return list
    .map((raw) => {
      const item = rec(raw)
      const content = str(item.content) || str(item.step) || str(item.description)
      const status = planItemStatus(item.status)
      // gizzi's todo rows say "cancelled" for a dropped step.
      return { content, status: status === "failed" ? "cancelled" : status, priority: str(item.priority) || "medium" }
    })
    .filter((t) => t.content)
}

/** The `plan` SSE frame for gizzi's todo list (`steps` for older apps, `items` for the contract). */
export function planFrameForTodos(todos: Array<{ content: string; status: string }>, messageId: string) {
  const items = todos.map((t, i) => ({ id: `todo-${i}`, content: t.content, status: planItemStatus(t.status) }))
  return {
    type: "plan",
    messageId,
    planId: "todos",
    title: "Plan",
    origin: "todowrite",
    items,
    steps: items.map((it) => ({
      id: it.id,
      description: it.content,
      status:
        it.status === "completed"
          ? "complete"
          : it.status === "in_progress"
            ? "in-progress"
            : it.status === "failed"
              ? "error"
              : "pending",
    })),
  }
}
