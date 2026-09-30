// Turn-scoped registration of allternit-api's per-user MCP proxy.
//
// A user's MCP connectors (and their OAuth tokens) live in allternit-api, not here. For each chat
// turn allternit-api sends ONE server entry — the proxy URL plus a short-lived token bound to
// {user, session} — as the top-level `mcpProxy` prompt field. The entry is deliberately NOT part of
// `metadata`: metadata is stored on the user message, and a token must never reach disk. It is held in
// memory, keyed by session, for the length of the turn, and dropped afterwards.

import { Client } from "@modelcontextprotocol/sdk/client/index.js"
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js"
import z from "zod/v4"
import { Log } from "@/shared/util/log"
import { Installation } from "@/shared/installation"
import { withTimeout } from "@/shared/util/timeout"
import {
  MCP_APPS_CLIENT_CAPABILITIES,
  MCP_CONNECTOR_META_KEY,
  buildMcpAppFrame,
  mcpAppHtml,
  mcpAppResourceUri,
} from "@/runtime/tools/mcp/apps"

export namespace McpUserProxy {
  const log = Log.create({ service: "mcp.user-proxy" })
  const CONNECT_TIMEOUT = 20_000
  /** Header allternit-api checks against the session the token was minted for. */
  export const SESSION_HEADER = "X-Allternit-Session"

  export const Entry = z.object({
    /** Server key the tools are registered under (`mcp__<server>__<connector>__<tool>`). */
    server: z.string().min(1).max(64),
    url: z.string().refine((u) => {
      try {
        const p = new URL(u).protocol
        return p === "http:" || p === "https:"
      } catch {
        return false
      }
    }, "must be an http(s) URL"),
    /** Session the token is bound to (may differ from the prompt's session after a handoff). */
    sessionId: z.string().min(1),
    token: z.string().min(1),
  })
  export type Entry = z.infer<typeof Entry>

  /** The entry, or undefined when absent/invalid. Never logs the input. */
  export function parse(raw: unknown): Entry | undefined {
    if (raw === undefined || raw === null) return undefined
    const parsed = Entry.safeParse(raw)
    if (!parsed.success) {
      log.warn("ignoring invalid mcpProxy entry")
      return undefined
    }
    return parsed.data
  }

  type Held = { entry: Entry; client?: Promise<Client | undefined> }
  const held = new Map<string, Held>()

  /** Hold `raw` for `sessionID` until the returned release runs. No-op release when there is no entry. */
  export function register(sessionID: string, raw: unknown): () => void {
    const entry = parse(raw)
    if (!entry) return () => {}
    const mine: Held = { entry }
    const previous = held.get(sessionID)
    held.set(sessionID, mine)
    if (previous) void closeHeld(previous)
    return () => {
      if (held.get(sessionID) === mine) held.delete(sessionID)
      void closeHeld(mine)
    }
  }

  export function current(sessionID: string): Entry | undefined {
    return held.get(sessionID)?.entry
  }

  async function closeHeld(h: Held) {
    const client = await h.client?.catch(() => undefined)
    await client?.close().catch(() => {})
  }

  /** Connected client for the session's proxy entry (one per turn), or undefined if it cannot connect. */
  export function client(sessionID: string): Promise<Client | undefined> {
    const h = held.get(sessionID)
    if (!h) return Promise.resolve(undefined)
    h.client ??= connect(h.entry)
    return h.client
  }

  export async function connect(entry: Entry): Promise<Client | undefined> {
    const transport = new StreamableHTTPClientTransport(new URL(entry.url), {
      requestInit: { headers: { Authorization: `Bearer ${entry.token}`, [SESSION_HEADER]: entry.sessionId } },
    })
    const c = new Client({ name: "gizzi", version: Installation.VERSION }, { capabilities: MCP_APPS_CLIENT_CAPABILITIES })
    try {
      await withTimeout(c.connect(transport), CONNECT_TIMEOUT)
      return c
    } catch (error) {
      // The message can echo the URL but never the token (it is only in a header).
      log.warn("connector proxy unreachable", {
        server: entry.server,
        error: error instanceof Error ? error.message : String(error),
      })
      await c.close().catch(() => {})
      return undefined
    }
  }

  const APP_FRAME_TIMEOUT = 20_000

  /** `<connector>__<tool>` → `<tool>`; the connector prefix never contains `__`. */
  export function originalToolName(namespaced: string): string {
    const i = namespaced.indexOf("__")
    return i < 0 ? namespaced : namespaced.slice(i + 2)
  }

  /**
   * The `mcp_app` frame for a completed tool part of the proxy server, or undefined. Same result as
   * allternit-api's emission, for gizzi's own agent-chat route: the proxy names the connector on each
   * tool and serves its `ui://` resource. Best effort — a failure means no app, never a broken turn.
   */
  export async function appFrameForPart(
    sessionID: string,
    part: any,
    messageId: string,
  ): Promise<Record<string, unknown> | undefined> {
    const entry = current(sessionID)
    const mcp = part?.state?.metadata?.mcp
    const callId = typeof part?.callID === "string" ? part.callID : undefined
    if (!entry || !callId || !mcp || mcp.server !== entry.server || typeof mcp.tool !== "string") return undefined
    try {
      return await withTimeout(buildFrame(sessionID, entry, part, mcp, callId, messageId), APP_FRAME_TIMEOUT)
    } catch (error) {
      log.warn("could not build app frame", { error: error instanceof Error ? error.message : String(error) })
      return undefined
    }
  }

  async function buildFrame(
    sessionID: string,
    _entry: Entry,
    part: any,
    mcp: { tool: string; result?: unknown },
    callId: string,
    messageId: string,
  ) {
    const c = await client(sessionID)
    if (!c) return undefined
    let tool: any
    let cursor: string | undefined
    do {
      const page = await c.listTools(cursor ? { cursor } : undefined)
      tool = page.tools.find((t) => t.name === mcp.tool)
      cursor = tool ? undefined : page.nextCursor
    } while (cursor)
    const uri = tool && mcpAppResourceUri(tool)
    if (!uri || !uri.startsWith("ui://")) return undefined
    const owner = (tool._meta as Record<string, any> | undefined)?.[MCP_CONNECTOR_META_KEY]
    if (typeof owner?.id !== "string" || typeof owner?.name !== "string") return undefined
    const doc = mcpAppHtml(await c.readResource({ uri }))
    if (!doc) return undefined
    const output = part?.state?.output
    return buildMcpAppFrame({
      messageId,
      callId,
      connector: { id: owner.id, name: owner.name },
      tool: { ...tool, name: originalToolName(mcp.tool) },
      resourceUri: uri,
      html: doc.html,
      ui: doc.ui,
      toolInput: part?.state?.input ?? {},
      toolResult:
        mcp.result && typeof mcp.result === "object"
          ? mcp.result
          : { content: [{ type: "text", text: typeof output === "string" ? output : "" }] },
    })
  }
}
