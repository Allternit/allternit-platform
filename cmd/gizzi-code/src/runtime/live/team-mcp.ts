// Portions adapted from Reason Network (reason-machines/reason-network), Apache-2.0. Copyright 2026 Reason Machines, Inc.
/**
 * Gizzi's team MCP server (stdio): `list_team`, `send_message`, `check_inbox`
 * for any MCP client (Claude Code, Codex, Gemini, Cursor, Copilot, ...).
 *
 * Tools talk to allternit-api: `GET /api/v1/live-sessions` for the roster and
 * `POST /api/factory/send` for sends (the engine picks the delivery tier and
 * records the hop). `check_inbox` long-polls the local live inbox
 * (`runtime/live/inbox.ts`) and is hidden from sessions that get push.
 *
 * Free of CLI imports so bun tests can drive `handle` with a stubbed fetch.
 */
import { platformApiBase, platformToken } from "@/runtime/bots/platform-api"
import { clearWaiting, markRead, readPending, sessionKey, touchWaiting, unreadNotice } from "@/runtime/live/inbox"

export const MAX_WAIT_SECS = 600

interface Rpc {
  jsonrpc: "2.0"
  id?: number | string | null
  method: string
  params?: any
}

export interface LiveSessionRow {
  address: string
  harness: string
  status: string
  doing?: string
  machineName?: string
  unread?: number
}

const TOOLS = [
  {
    name: "list_team",
    description:
      "List your team's live agent sessions (address, harness, status, what each is doing). Send to one with send_message.",
    inputSchema: {
      type: "object",
      properties: {
        filter: { type: "string", description: "Only addresses, harnesses or activity containing this text" },
        all: { type: "boolean", description: "Include offline sessions" },
      },
    },
  },
  {
    name: "send_message",
    description:
      'Message a teammate\'s agent session by address ("person/session"). The result says how it landed: push, turn or pull.',
    inputSchema: {
      type: "object",
      properties: {
        to: { type: "string", description: 'Address, e.g. "eoj/allternit-claude"' },
        message: { type: "string" },
        reply_to: { type: "string", description: "The id of the message you are answering" },
      },
      required: ["to", "message"],
    },
  },
  {
    name: "check_inbox",
    description:
      "Wait for messages from teammates' agents (long-poll). Returns as soon as one arrives, or says the inbox is empty.",
    inputSchema: {
      type: "object",
      properties: { wait_secs: { type: "number", description: `Seconds to wait, at most ${MAX_WAIT_SECS}` } },
    },
  },
]

/** Sessions that get messages pushed into their running turn never need `check_inbox`. */
export function isPushSession(env: NodeJS.ProcessEnv = process.env): boolean {
  return !!env.CLAUDE_CODE_MESSAGING_SOCKET || env.ALLTERNIT_LIVE_TIER === "push"
}

export function toolList(env: NodeJS.ProcessEnv = process.env) {
  return isPushSession(env) ? TOOLS.filter((t) => t.name !== "check_inbox") : TOOLS
}

export async function api(path: string, init?: RequestInit): Promise<any> {
  const token = await platformToken().catch(() => undefined)
  if (!token) throw new Error("This runtime isn't connected to your Allternit account. Run `gizzi login`.")
  const res = await fetch(`${platformApiBase()}${path}`, {
    ...init,
    headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}`, ...(init?.headers ?? {}) },
    signal: AbortSignal.timeout(35_000),
  })
  const text = await res.text()
  let body: any = text
  try {
    body = JSON.parse(text)
  } catch {
    // plain text error
  }
  if (!res.ok) {
    throw new Error(typeof body === "string" ? body || `HTTP ${res.status}` : (body.fact ?? body.error ?? `HTTP ${res.status}`))
  }
  return body
}

/** Send to a live session address through the engine (C1 result). */
export async function sendToAddress(
  to: string,
  text: string,
  opts: { replyTo?: string; fromAddress?: string; idempotencyKey?: string } = {},
): Promise<{ state?: string; tier?: string | null; note?: string | null; hopId?: string; detail?: string | null }> {
  return api("/api/factory/send", {
    method: "POST",
    headers: opts.idempotencyKey ? { "Idempotency-Key": opts.idempotencyKey } : undefined,
    body: JSON.stringify({ to, text, replyTo: opts.replyTo, fromAddress: opts.fromAddress }),
  })
}

export function rosterText(rows: LiveSessionRow[], filter?: string, all?: boolean): string {
  const f = filter?.trim().toLowerCase()
  const shown = rows
    .filter((r) => all || r.status !== "offline")
    .filter((r) => !f || `${r.address} ${r.harness} ${r.doing ?? ""}`.toLowerCase().includes(f))
  if (!shown.length) return "No live sessions on your team right now."
  return shown
    .map((r) => `- ${r.address} (${r.harness}, ${r.status}${r.machineName ? `, ${r.machineName}` : ""})${r.doing ? `: ${r.doing}` : ""}`)
    .join("\n")
}

async function withNotice(text: string, key: string, env: NodeJS.ProcessEnv): Promise<string> {
  if (isPushSession(env)) return text
  const notice = unreadNotice(await readPending(key).catch(() => []))
  return notice ? `${text}\n\n${notice}` : text
}

export async function callTool(name: string, args: any, env: NodeJS.ProcessEnv = process.env): Promise<string> {
  const key = sessionKey(env.ALLTERNIT_LIVE_HARNESS ?? "agent")
  if (name === "list_team") {
    const body = await api("/api/v1/live-sessions").catch((e: Error) => {
      throw new Error(`Couldn't read the team roster: ${e.message}`)
    })
    const rows: LiveSessionRow[] = Array.isArray(body) ? body : (body.sessions ?? [])
    return withNotice(rosterText(rows, args?.filter, args?.all), key, env)
  }
  if (name === "send_message") {
    const to = String(args?.to ?? "").trim()
    const message = String(args?.message ?? "").trim()
    if (!to || !message) throw new Error("send_message needs `to` and `message`.")
    const d = await sendToAddress(to, message, { replyTo: args?.reply_to, fromAddress: env.ALLTERNIT_LIVE_ADDRESS })
    const state = d.state === "verified" ? "delivered" : d.state
    const line = `${state} (${d.tier ?? "?"}): ${d.note ?? d.detail ?? ""}`.trim()
    return withNotice(`${line}${d.hopId ? `\nhop: ${d.hopId}` : ""}`, key, env)
  }
  if (name === "check_inbox") {
    const wait = Math.min(Math.max(Number(args?.wait_secs ?? 0), 0), MAX_WAIT_SECS)
    const until = Date.now() + wait * 1000
    try {
      for (;;) {
        const pending = await readPending(key)
        if (pending.length) {
          await markRead(key, pending.map((m) => m.id))
          return pending.map((m) => m.envelope).join("\n\n")
        }
        if (Date.now() >= until) return "Your inbox is empty."
        await touchWaiting(key)
        await new Promise((r) => setTimeout(r, 1000))
      }
    } finally {
      await clearWaiting(key)
    }
  }
  throw new Error(`Unknown tool ${name}`)
}

/** One JSON-RPC message in, the reply (or undefined for a notification) out. */
export async function handle(msg: Rpc, env: NodeJS.ProcessEnv = process.env): Promise<object | undefined> {
  const reply = (result: unknown) => ({ jsonrpc: "2.0", id: msg.id ?? null, result })
  const fail = (code: number, message: string) => ({ jsonrpc: "2.0", id: msg.id ?? null, error: { code, message } })
  if (msg.id === undefined) return undefined
  switch (msg.method) {
    case "initialize":
      return reply({
        protocolVersion: msg.params?.protocolVersion ?? "2025-06-18",
        capabilities: { tools: {} },
        serverInfo: { name: "allternit-team", version: "1.0.0" },
      })
    case "ping":
      return reply({})
    case "tools/list":
      return reply({ tools: toolList(env) })
    case "tools/call":
      try {
        const text = await callTool(String(msg.params?.name), msg.params?.arguments ?? {}, env)
        return reply({ content: [{ type: "text", text }] })
      } catch (e) {
        return reply({ content: [{ type: "text", text: (e as Error).message }], isError: true })
      }
    default:
      return fail(-32601, `Method not found: ${msg.method}`)
  }
}

/** Serve newline-delimited JSON-RPC on stdio until stdin closes. */
export async function serveStdio(): Promise<void> {
  let buf = ""
  process.stdin.setEncoding("utf8")
  const pending: Promise<void>[] = []
  process.stdin.on("data", (chunk: string) => {
    buf += chunk
    let nl: number
    while ((nl = buf.indexOf("\n")) >= 0) {
      const line = buf.slice(0, nl).trim()
      buf = buf.slice(nl + 1)
      if (!line) continue
      pending.push(
        (async () => {
          try {
            const out = await handle(JSON.parse(line) as Rpc)
            if (out) process.stdout.write(JSON.stringify(out) + "\n")
          } catch {
            process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: null, error: { code: -32700, message: "Parse error" } }) + "\n")
          }
        })(),
      )
    }
  })
  await new Promise<void>((resolve) => process.stdin.on("end", () => resolve()))
  await Promise.all(pending)
}
