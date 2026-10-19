import { randomBytes } from "node:crypto"
import { ContextBreakdown } from "@/runtime/session/context-breakdown"
import { Log } from "@/shared/util/log"

/**
 * A local pass-through for coding CLIs whose session records do not hold
 * what they send the model (OpenCode, Gemini CLI…). The CLI's provider base
 * URL points here; every request goes on to the real provider unchanged
 * (same headers, streamed reply) and the last model request of each session
 * is kept, so the context panel measures the brain's exact system prompt,
 * tool schemas and messages.
 *
 * Bound to 127.0.0.1 with a per-process secret in the path.
 */
export namespace BrainRelay {
  const log = Log.create({ service: "brain-relay" })
  const secret = randomBytes(12).toString("hex")
  let server: ReturnType<typeof Bun.serve> | undefined

  export interface Captured {
    body: Record<string, unknown>
    at: number
  }
  const MAX_SESSIONS = 128
  const latest = new Map<string, Captured>()

  /** A model request (chat, messages, responses, Gemini generate), not a model list or token count. */
  function isModelRequest(body: unknown): body is Record<string, unknown> {
    if (!body || typeof body !== "object") return false
    const b = body as Record<string, any>
    const inner = b.request && typeof b.request === "object" ? b.request : b
    return Array.isArray(inner.messages) || Array.isArray(inner.contents) || Array.isArray(inner.input) || typeof inner.input === "string"
  }

  function keep(sessionID: string, body: Record<string, unknown>) {
    latest.delete(sessionID)
    latest.set(sessionID, { body, at: Date.now() })
    while (latest.size > MAX_SESSIONS) latest.delete(latest.keys().next().value!)
  }

  function start() {
    if (server) return server
    server = Bun.serve({
      hostname: "127.0.0.1",
      port: 0,
      idleTimeout: 255,
      async fetch(req) {
        const url = new URL(req.url)
        // /<secret>/<sessionID>/<upstream origin, encoded>/<path…>
        const [, key, sessionID, upstream, ...rest] = url.pathname.split("/")
        if (key !== secret || !sessionID || !upstream) return new Response("not found", { status: 404 })
        const origin = decodeURIComponent(upstream)
        if (!/^https:\/\/[a-z0-9.-]+(:\d+)?$/i.test(origin)) return new Response("bad upstream", { status: 400 })
        const target = `${origin}/${rest.join("/")}${url.search}`
        const body = req.method === "GET" || req.method === "HEAD" ? undefined : await req.arrayBuffer()
        if (body && body.byteLength > 0 && (req.headers.get("content-type") ?? "").includes("json")) {
          try {
            const parsed = JSON.parse(new TextDecoder().decode(body))
            if (isModelRequest(parsed)) keep(decodeURIComponent(sessionID), parsed)
          } catch {
            // Not JSON after all: forward as is.
          }
        }
        const headers = new Headers(req.headers)
        headers.delete("host")
        headers.delete("content-length")
        try {
          const res = await fetch(target, { method: req.method, headers, body, redirect: "manual" })
          const out = new Headers(res.headers)
          // fetch already decoded the body.
          out.delete("content-encoding")
          out.delete("content-length")
          return new Response(res.body, { status: res.status, statusText: res.statusText, headers: out })
        } catch (error) {
          log.warn("relay upstream failed", { origin, error: error instanceof Error ? error.message : String(error) })
          return new Response(JSON.stringify({ error: { message: "Allternit relay could not reach the provider" } }), {
            status: 502,
            headers: { "content-type": "application/json" },
          })
        }
      },
    })
    return server
  }

  /** The relay URL standing in for `upstream` (e.g. "https://openrouter.ai/api/v1") for one session. */
  export function url(sessionID: string, upstream: string): string {
    const parsed = new URL(upstream)
    const s = start()
    return `http://127.0.0.1:${s.port}/${secret}/${encodeURIComponent(sessionID)}/${encodeURIComponent(parsed.origin)}${parsed.pathname.replace(/\/$/, "")}`
  }

  export function captured(sessionID: string): Captured | undefined {
    return latest.get(sessionID)
  }

  type Block = { type?: string; text?: unknown }
  const text = (value: unknown): string =>
    typeof value === "string"
      ? value
      : Array.isArray(value)
        ? value.map((part: Block | string) => (typeof part === "string" ? part : typeof part?.text === "string" ? part.text : "")).join("\n")
        : ""

  /** Gemini CLI marks memory files `--- Context from: <path> ---`; gizzi/OpenCode `Instructions from: <path>`. */
  function normalizeMemory(system: string) {
    return system.replace(/^--- Context from: (.+?) ---$/gm, "Instructions from: $1")
  }

  /**
   * The request as ContextBreakdown.build input: system text, tools by name
   * (MCP tools as `mcp__server__tool` when `mcpServers` names their prefix),
   * messages. Handles OpenAI chat / responses, Anthropic messages, Gemini
   * (and Code Assist's `{ request: … }` wrapper).
   */
  export function toBuild(body: Record<string, any>, mcpServers: string[] = []) {
    const inner = body.request && typeof body.request === "object" ? body.request : body
    const system: string[] = []
    let messages: unknown[] = []
    const tools: Record<string, unknown> = {}
    const addTool = (name: unknown, schema: unknown) => {
      if (typeof name !== "string" || !name) return
      const server = mcpServers.find((s) => name.startsWith(`${s}_`) || name.startsWith(`${s}__`))
      const key = name.startsWith("mcp__") || !server ? name : `mcp__${server}__${name.slice(server.length).replace(/^_+/, "")}`
      tools[key] = schema
    }
    // Anthropic
    if (inner.system !== undefined) system.push(text(inner.system))
    // OpenAI responses
    if (typeof inner.instructions === "string") system.push(inner.instructions)
    // Gemini
    if (inner.systemInstruction) system.push(text(inner.systemInstruction.parts ?? inner.systemInstruction))
    if (Array.isArray(inner.messages)) {
      for (const m of inner.messages) {
        if (m?.role === "system" || m?.role === "developer") system.push(text(m.content))
        else messages.push(m)
      }
    }
    if (Array.isArray(inner.input)) {
      for (const m of inner.input) {
        if (m?.role === "system" || m?.role === "developer") system.push(text(m.content))
        else messages.push(m)
      }
    } else if (typeof inner.input === "string") messages.push(inner.input)
    if (Array.isArray(inner.contents)) messages = messages.concat(inner.contents)
    for (const t of Array.isArray(inner.tools) ? inner.tools : []) {
      if (Array.isArray(t?.functionDeclarations)) for (const f of t.functionDeclarations) addTool(f?.name, f)
      else if (t?.function) addTool(t.function.name, t.function)
      else addTool(t?.name, t)
    }
    const joined = normalizeMemory(system.filter(Boolean).join("\n"))
    return {
      system: joined ? [joined] : [],
      instructions: joined.split(/\n(?=(?:Instructions from|Auto-memory from): |# MCP Server Instructions)/),
      tools,
      messages: messages as any[],
      model: typeof inner.model === "string" ? inner.model : typeof body.model === "string" ? body.model : undefined,
    }
  }

  /** The session's breakdown from the last request the brain sent through the relay. */
  export function breakdown(input: {
    sessionID: string
    providerID: string
    modelID: string
    size: number
    used?: number
    mcpServers?: string[]
  }): ContextBreakdown.Snapshot | undefined {
    const hit = latest.get(input.sessionID)
    if (!hit || !(input.size > 0)) return undefined
    const request = toBuild(hit.body as Record<string, any>, input.mcpServers)
    const built = ContextBreakdown.build({
      sessionID: input.sessionID,
      providerID: input.providerID,
      modelID: request.model ?? input.modelID,
      contextWindow: input.size,
      system: request.system,
      instructions: request.instructions,
      tools: request.tools,
      messages: request.messages,
      autoCompactBuffer: 0,
    })
    const reconciled = input.used ? ContextBreakdown.reconcile(built, { providerContextTokens: input.used }) : built
    return { ...reconciled, source: "brain", brain: input.providerID, split: "brain", measuredAt: hit.at }
  }

  /** Test hook. */
  export function clear() {
    latest.clear()
  }
}
