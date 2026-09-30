// Turn-scoped registration of allternit-api's per-user MCP proxy.
//
// A user's MCP connectors (and their OAuth tokens) live in allternit-api, not here. For each chat
// turn allternit-api sends ONE server entry — the proxy URL plus a short-lived token bound to
// {user, session} — as the top-level `mcpProxy` prompt field. The entry is deliberately NOT part of
// `metadata`: metadata is stored on the user message, and a token must never reach disk. It is held in
// memory, keyed by session, for the length of the turn, and dropped afterwards.

import { createGuardedFetch } from "@/shared/utils/hooks/ssrfGuard"
import { Client } from "@modelcontextprotocol/sdk/client/index.js"
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js"
import z from "zod/v4"
import { Log } from "@/shared/util/log"
import { Installation } from "@/shared/installation"
import { withTimeout } from "@/shared/util/timeout"
import {
  MCP_APPS_CLIENT_CAPABILITIES,
  MCP_CONNECTOR_META_KEY,
  MCP_REQUIRES_CONFIRMATION_META_KEY,
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
      // allternit-api is usually on localhost; metadata/link-local/private stay refused.
      fetch: createGuardedFetch({ allowLoopback: true }) as unknown as typeof fetch,
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

  // ── Permission gate for model-initiated calls ─────────────────────────────
  // allternit-api marks each tool that needs the user's confirmation under its install's permission
  // mode. gizzi asks through its permission system (class `mcp_app`, which every mode still asks) and
  // only then lets the call carry the approval flag the proxy demands.

  /** Permission class of the ask; listed in PermissionNext.ALWAYS_ASK. */
  export const APPROVAL_PERMISSION = "mcp_app"
  export const DECLINED_MESSAGE = "The user declined this tool call"
  const ARGS_LIMIT = 1000

  /** Does the proxy want the user's approval before running this tool? Unmarked → no. */
  export function requiresConfirmation(tool: { _meta?: unknown } | undefined): boolean {
    const meta = tool?._meta as Record<string, unknown> | undefined
    return meta?.[MCP_REQUIRES_CONFIRMATION_META_KEY] === true
  }

  /** Arguments as shown to the user; a cut is announced so padding cannot hide what runs. */
  export function describeArguments(args: unknown): string {
    let full: string
    try {
      full = JSON.stringify(args ?? {}, null, 2) ?? "{}"
    } catch {
      return "(arguments could not be shown)"
    }
    return full.length > ARGS_LIMIT
      ? `${full.slice(0, ARGS_LIMIT)}\n… (${full.length - ARGS_LIMIT} more characters not shown)`
      : full
  }

  const approved = new Set<string>()
  /** True once, for a call the user approved; the caller then sends the approval flag. */
  export function consumeApproval(callID: string | undefined): boolean {
    return callID !== undefined && approved.delete(callID)
  }
  export function clearApproval(callID: string | undefined) {
    if (callID !== undefined) approved.delete(callID)
  }

  export interface GateInput {
    callID: string
    /** Tool name as the connector knows it (`<tool>`, without the connector prefix). */
    tool: string
    title?: string
    /** Display name of the app/connector the tool belongs to. */
    app?: string
    args: unknown
    ask: (req: { permission: string; patterns: string[]; always: string[]; metadata: Record<string, unknown> }) => Promise<unknown>
  }

  /** Ask the user to approve this call. Resolves when approved; throws "The user declined this tool call" otherwise. */
  export async function gate(input: GateInput): Promise<void> {
    const app = input.app ?? "An app"
    const label = input.title ?? input.tool
    const shown = describeArguments(input.args)
    try {
      await input.ask({
        permission: APPROVAL_PERMISSION,
        patterns: [`${app} wants to run “${label}”\nArguments:\n${shown}`],
        // Never remembered: each call is approved on its own.
        always: [],
        metadata: { app, tool: input.tool, title: label, arguments: shown },
      })
    } catch {
      throw new Error(DECLINED_MESSAGE)
    }
    approved.add(input.callID)
  }

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
