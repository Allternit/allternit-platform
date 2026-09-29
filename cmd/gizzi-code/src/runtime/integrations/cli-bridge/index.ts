import { randomBytes, timingSafeEqual } from "node:crypto"
import z from "zod/v4"
import type { Tool } from "@/runtime/tools/builtins/tool"
import { PaneArtifactTool } from "@/runtime/tools/builtins/pane-artifact"
import { PaneBrowserTool } from "@/runtime/tools/builtins/pane-browser"
import { MediaGenerateTool } from "@/runtime/tools/builtins/media-generate"
import { Log } from "@/shared/util/log"

/**
 * CLI tool bridge: an installed CLI agent (Claude Code, Codex, Kimi…) runs a
 * gizzi session turn with its own tools only. This serves gizzi's
 * session-bound tools — the document open in the pane, the pane's browser,
 * native image/video rendering — to that CLI as an MCP server over HTTP
 * (stateless, JSON responses), scoped to one session and guarded by a
 * per-process token. The CLI driver hands the CLI this server's config.
 */
export namespace CliBridge {
  const log = Log.create({ service: "cli-bridge" })

  export const SERVER_NAME = "allternit"
  const PROTOCOL_VERSION = "2025-06-18"

  /** The tools a CLI gets: the ones only gizzi can do for this session. */
  export const TOOLS: Tool.Info[] = [PaneArtifactTool, PaneBrowserTool, MediaGenerateTool]

  const token = randomBytes(32).toString("hex")

  /**
   * Own header, not Authorization: the server's auth middleware treats any
   * Bearer token as a Clerk/API credential.
   */
  export const TOKEN_HEADER = "X-Allternit-Bridge-Token"

  export function authorized(header: string | undefined): boolean {
    const given = Buffer.from(header ?? "")
    const want = Buffer.from(token)
    return given.length === want.length && timingSafeEqual(given, want)
  }

  export interface ServerConfig {
    name: string
    url: string
    headers: Record<string, string>
  }

  /** MCP server config for one session, on this gizzi server. */
  export function serverConfig(input: { baseURL: URL | string; sessionID: string; directory?: string }): ServerConfig {
    const url = new URL(`/cli-bridge/${encodeURIComponent(input.sessionID)}/mcp`, input.baseURL)
    if (input.directory) url.searchParams.set("directory", input.directory)
    const headers: Record<string, string> = { [TOKEN_HEADER]: token }
    // A password-protected server (serve --password) needs basic auth too.
    const password = process.env.GIZZI_SERVER_PASSWORD
    if (password) {
      const username = process.env.GIZZI_SERVER_USERNAME ?? "gizzi"
      headers.Authorization = `Basic ${Buffer.from(`${username}:${password}`).toString("base64")}`
    }
    return { name: SERVER_NAME, url: url.toString(), headers }
  }

  type JsonRpcRequest = { jsonrpc?: string; id?: string | number | null; method?: string; params?: any }
  type JsonRpcResponse = { jsonrpc: "2.0"; id: string | number | null; result?: unknown; error?: { code: number; message: string } }

  const ok = (id: JsonRpcRequest["id"], result: unknown): JsonRpcResponse => ({ jsonrpc: "2.0", id: id ?? null, result })
  const fail = (id: JsonRpcRequest["id"], code: number, message: string): JsonRpcResponse => ({
    jsonrpc: "2.0",
    id: id ?? null,
    error: { code, message },
  })

  async function listTools() {
    return Promise.all(
      TOOLS.map(async (t) => {
        const info = await t.init()
        const schema = z.toJSONSchema(info.parameters) as Record<string, unknown>
        delete schema.$schema
        return { name: t.id, description: info.description, inputSchema: schema }
      }),
    )
  }

  function content(result: Awaited<ReturnType<Awaited<ReturnType<Tool.Info["init"]>>["execute"]>>) {
    const parts: Array<Record<string, unknown>> = [{ type: "text", text: result.output }]
    for (const a of result.attachments ?? []) {
      const m = /^data:([^;,]+);base64,(.*)$/s.exec(a.url ?? "")
      if (m && m[1].startsWith("image/")) parts.push({ type: "image", mimeType: m[1], data: m[2] })
    }
    return parts
  }

  async function callTool(sessionID: string, params: any, signal: AbortSignal) {
    const tool = TOOLS.find((t) => t.id === params?.name)
    if (!tool) throw new Error(`Unknown tool: ${params?.name}`)
    // Loaded on use: the session store pulls in most of the runtime.
    const [{ Session }, { PermissionNext }] = await Promise.all([
      import("@/runtime/session"),
      import("@/runtime/tools/guard/permission/next"),
    ])
    const session = await Session.get(sessionID)
    const info = await tool.init()
    const callID = `cli_${randomBytes(6).toString("hex")}`
    const ctx: Tool.Context = {
      sessionID,
      messageID: "cli-bridge",
      agent: "cli",
      abort: signal,
      callID,
      messages: [],
      metadata: () => {},
      async ask(req) {
        return await PermissionNext.ask({ ...req, sessionID, ruleset: session.permission ?? [] })
      },
    }
    log.info("call", { sessionID, tool: tool.id })
    try {
      const result = await info.execute(params?.arguments ?? {}, ctx)
      return { content: content(result), isError: false }
    } catch (error) {
      return { content: [{ type: "text", text: error instanceof Error ? error.message : String(error) }], isError: true }
    }
  }

  /** Answer one JSON-RPC message (or a batch). Notifications get no reply. */
  export async function handle(
    sessionID: string,
    body: JsonRpcRequest | JsonRpcRequest[],
    signal: AbortSignal,
  ): Promise<JsonRpcResponse | JsonRpcResponse[] | null> {
    if (Array.isArray(body)) {
      const replies = (await Promise.all(body.map((b) => handleOne(sessionID, b, signal)))).filter(
        (r): r is JsonRpcResponse => r !== null,
      )
      return replies.length ? replies : null
    }
    return handleOne(sessionID, body, signal)
  }

  async function handleOne(sessionID: string, req: JsonRpcRequest, signal: AbortSignal): Promise<JsonRpcResponse | null> {
    const isNotification = req.id === undefined || req.id === null
    switch (req.method) {
      case "initialize":
        return ok(req.id, {
          protocolVersion: req.params?.protocolVersion ?? PROTOCOL_VERSION,
          capabilities: { tools: { listChanged: false } },
          serverInfo: { name: SERVER_NAME, version: "1.0.0" },
          instructions:
            "Allternit session tools: the document open beside the chat (pane_artifact), the pane's browser (pane_browser), and image/video generation rendered in the user's app (media_generate).",
        })
      case "ping":
        return ok(req.id, {})
      case "tools/list":
        return ok(req.id, { tools: await listTools() })
      case "tools/call":
        return ok(req.id, await callTool(sessionID, req.params, signal))
      default:
        if (isNotification) return null
        return fail(req.id, -32601, `Method not found: ${req.method}`)
    }
  }
}
