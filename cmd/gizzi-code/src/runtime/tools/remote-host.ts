/**
 * Remote tool host (self-hosted Agents Server, plan step 8).
 *
 * When `GIZZI_TOOL_HOST=remote`, this gizzi is the agent loop child of
 * allternit-agents-server. Model calls and MCP stay here; every tool in the
 * sandbox set, and every custom tool of the session's agent, is handed to the
 * server's loopback queue instead of running locally:
 *
 *   POST ${GIZZI_TOOL_HOST_URL}/internal/v1/sessions/{root gizzi session id}/tool_calls
 *   Authorization: Bearer ${GIZZI_TOOL_HOST_TOKEN}
 *   { tool_call_id, name, input, timeout_ms }
 *
 * The request blocks until a `gizzi worker` posts the result
 * (`{ output, is_error }`), which is mapped back to the `{ title, metadata,
 * output }` shape the local tool returns, so the session loop and UI are
 * unchanged. Remote mode never falls back to local execution: a missing URL
 * or token, no worker (504), a timeout (408) or any transport failure is a
 * tool error.
 *
 * Subagents (task, agent_swarm) run in child sessions with their own gizzi
 * ids, but the server maps only the session it prompts (the root). A child's
 * calls therefore go to the root's queue, and a child is offered the root's
 * custom tools (rootSessionOf).
 *
 * Spec: cmd/allternit-api/openapi/selfhost-v1.yaml (Internal),
 * docs/SELFHOST_WORKER_PROTOCOL.md.
 */
import { createHash } from "node:crypto"
import path from "node:path"
import type { Tool } from "@/runtime/tools/builtins/tool"
import { Log } from "@/shared/util/log"

export namespace RemoteToolHost {
  const log = Log.create({ service: "tool.remote-host" })

  /**
   * Tools that run on a worker, by their real gizzi ids
   * (src/runtime/tools/builtins/*). Everything else (MCP, task, todo, skill,
   * memory, artifacts...) stays in the agent loop.
   */
  export const SANDBOX_TOOLS: ReadonlySet<string> = new Set([
    "bash",
    "read",
    "write",
    "edit",
    "multiedit",
    "apply_patch",
    "notebook_edit",
    "grep",
    "glob",
    "list",
    "symbol_edit",
    "task_list",
    "task_output",
    "task_stop",
    "scratchpad_list",
    "scratchpad_read",
    "scratchpad_write",
    "scratchpad_remove",
  ])

  /**
   * Tools that would run code or touch files on the server itself and have
   * no worker form. Refused in remote mode: the server executes nothing.
   */
  export const SERVER_LOCAL_REFUSED: ReadonlySet<string> = new Set(["run_code"])

  /** Short names used in the plan and protocol doc, mapped to the real ids. */
  export const ALIASES: Readonly<Record<string, string>> = {
    ls: "list",
    notebook: "notebook_edit",
    "symbol-edit": "symbol_edit",
  }

  export function canonicalName(name: string): string {
    return ALIASES[name] ?? name
  }

  /** Default wait for one tool call, including time spent waiting for a worker. */
  const DEFAULT_TIMEOUT_MS = 10 * 60 * 1000
  /** Bash's own default timeout (bash.ts DEFAULT_TIMEOUT). */
  const BASH_DEFAULT_TIMEOUT_MS = 2 * 60 * 1000
  /** Slack on top of a bash timeout for queueing, sandbox start and the result post. */
  const BASH_GRACE_MS = 60 * 1000
  /** Extra time the HTTP request waits past timeout_ms before giving up locally. */
  const HTTP_GRACE_MS = 15 * 1000
  const METADATA_OUTPUT_LIMIT = 30_000

  export function enabled(): boolean {
    return (process.env.GIZZI_TOOL_HOST ?? "").trim().toLowerCase() === "remote"
  }

  /**
   * True when this call must not run locally (forwarded or refused). With a
   * session id, the session's custom tools count too.
   */
  export function handles(toolID: string, sessionID?: string): boolean {
    if (!enabled()) return false
    if (SANDBOX_TOOLS.has(toolID) || SERVER_LOCAL_REFUSED.has(toolID)) return true
    return sessionID !== undefined && isCustomTool(sessionID, toolID)
  }

  // ─── Root session ─────────────────────────────────────────────────────────
  //
  // The server knows one gizzi session per self-hosted session: the one it
  // prompts (loop.rs adopt -> map_gizzi_session). A subagent's child session
  // (Session.parentID, any depth) is unknown to it, so its tool calls use the
  // root's id in the queue URL and the root's custom tool set.

  /** Each session's parent, or null for a root. A session's parent never changes. */
  const parentBySession = new Map<string, string | null>()
  const MAX_LINKED_SESSIONS = 5000
  /** Longest parent chain followed; deeper is treated as a loop. */
  const MAX_DEPTH = 32

  function remember(sessionID: string, parentID: string | null) {
    parentBySession.delete(sessionID)
    parentBySession.set(sessionID, parentID)
    while (parentBySession.size > MAX_LINKED_SESSIONS) {
      const oldest = parentBySession.keys().next().value
      if (oldest === undefined) break
      parentBySession.delete(oldest)
    }
  }

  /** Record a session's parent (undefined: it is a root). */
  export function linkSession(sessionID: string, parentID: string | undefined): void {
    remember(sessionID, parentID && parentID !== sessionID ? parentID : null)
  }

  /** The root of `sessionID` from what is already known, without reading the store. */
  export function rootSession(sessionID: string): string {
    let current = sessionID
    const seen = new Set<string>()
    for (let depth = 0; depth < MAX_DEPTH; depth++) {
      seen.add(current)
      const parent = parentBySession.get(current)
      if (!parent || seen.has(parent)) return current
      current = parent
    }
    return current
  }

  /**
   * The root of `sessionID`, reading each unknown parent from the session
   * store. A session the store does not have counts as a root.
   */
  export async function rootSessionOf(sessionID: string): Promise<string> {
    let current = sessionID
    const seen = new Set<string>()
    for (let depth = 0; depth < MAX_DEPTH; depth++) {
      seen.add(current)
      let parent = parentBySession.get(current)
      if (parent === undefined) {
        // Imported here: the session module pulls in the whole loop.
        const { Session } = await import("@/runtime/session")
        const info = await Session.get(current).catch(() => undefined)
        if (!info) return current
        linkSession(current, info.parentID)
        parent = parentBySession.get(current) ?? null
      }
      if (!parent || seen.has(parent)) return current
      current = parent
    }
    return current
  }

  // ─── Custom tools ─────────────────────────────────────────────────────────
  //
  // An agent's custom tools (`type: custom`) reach this child in each turn's
  // prompt metadata, `metadata.selfhost.custom_tools` (loop.rs message_body):
  // `[{ type: "custom", name, description?, input_schema }]`. They are offered
  // to the model for that session (SessionPrompt.resolveTools), and a call to
  // one goes to the same tool queue as a sandbox tool, where an SDK worker
  // with a handler answers it. They never run here.

  /** One custom tool, checked and ready to offer to the model. */
  export interface CustomTool {
    name: string
    description: string
    /** JSON Schema of the input; always `type: "object"`. */
    inputSchema: Record<string, unknown>
  }

  /** The tool name shape every model provider accepts. */
  const CUSTOM_TOOL_NAME = /^[a-zA-Z0-9_-]{1,64}$/
  /** Sessions whose custom tools are remembered; the oldest is dropped past this. */
  const MAX_CUSTOM_TOOL_SESSIONS = 1000
  const customBySession = new Map<string, Map<string, CustomTool>>()

  function isPlainObject(value: unknown): value is Record<string, unknown> {
    return value !== null && typeof value === "object" && !Array.isArray(value)
  }

  /**
   * The custom tools a turn's prompt metadata carries. Empty when remote mode
   * is off or there are none. Throws a RemoteToolError (`invalid_custom_tool`)
   * on an entry the model could not be given.
   */
  export function customToolsFrom(metadata: Record<string, unknown> | undefined): CustomTool[] {
    if (!enabled()) return []
    const selfhost = metadata?.["selfhost"]
    const raw = isPlainObject(selfhost) ? selfhost["custom_tools"] : undefined
    if (raw === undefined || raw === null) return []
    if (!Array.isArray(raw)) {
      throw new RemoteToolError("metadata.selfhost.custom_tools must be a list of custom tools.", "invalid_custom_tool")
    }
    const seen = new Set<string>()
    return raw.map((entry, index) => {
      const name = isPlainObject(entry) ? entry["name"] : undefined
      if (typeof name !== "string" || !CUSTOM_TOOL_NAME.test(name)) {
        throw new RemoteToolError(
          `Custom tool #${index + 1} has the name ${JSON.stringify(name)}. A custom tool name must be 1 to 64 letters, ` +
            "digits, underscores or hyphens.",
          "invalid_custom_tool",
        )
      }
      if (seen.has(name)) {
        throw new RemoteToolError(`The agent has two custom tools named "${name}".`, "invalid_custom_tool")
      }
      seen.add(name)
      const schema = (entry as Record<string, unknown>)["input_schema"]
      if (!isPlainObject(schema)) {
        throw new RemoteToolError(`The custom tool "${name}" needs an input_schema object.`, "invalid_custom_tool")
      }
      if (schema["type"] !== undefined && schema["type"] !== "object") {
        throw new RemoteToolError(
          `The custom tool "${name}" has an input_schema of type ${JSON.stringify(schema["type"])}. ` +
            'Tool input is always a JSON object, so the schema must be type "object".',
          "invalid_custom_tool",
        )
      }
      const description = (entry as Record<string, unknown>)["description"]
      return {
        name,
        description: typeof description === "string" ? description : "",
        inputSchema: { ...schema, type: "object" },
      }
    })
  }

  /** What holds a name a custom tool may not take. */
  export type Clash =
    | { kind: "builtin"; name: string }
    /** `name` is the tool's name as the model gets it (`mcp__<server>__<tool>`). */
    | { kind: "mcp"; name: string; server: string; tool: string }

  /** A taken name: a plain string is a built-in tool of the agent loop. */
  export type ReservedTool = string | Clash

  /** The contract's `clashes_with` value: `builtin:<tool>` or `mcp:<server>:<tool>`. */
  export function clashRef(clash: Clash): string {
    return clash.kind === "mcp" ? `mcp:${clash.server}:${clash.tool}` : `builtin:${clash.name}`
  }

  function clashText(clash: Clash): string {
    return clash.kind === "mcp"
      ? `the tool "${clash.tool}" of the MCP server "${clash.server}"`
      : `the built-in tool "${clash.name}"`
  }

  /**
   * Refuse custom tools that take a name already held by a built-in, MCP or
   * sandbox tool: throws a RemoteToolError (`custom_tool_conflict`, with
   * `conflict` = { tool, clashes_with } naming the tool and what holds the name). Names are compared
   * without case so `Bash` cannot pass for `bash`. Changes nothing.
   */
  export function checkCustomTools(tools: CustomTool[], reserved: Iterable<ReservedTool>): void {
    if (!tools.length) return
    const taken = new Map<string, Clash>()
    const take = (clash: Clash) => {
      const key = clash.name.toLowerCase()
      if (!taken.has(key)) taken.set(key, clash)
    }
    for (const entry of reserved) take(typeof entry === "string" ? { kind: "builtin", name: entry } : entry)
    for (const name of [...SANDBOX_TOOLS, ...SERVER_LOCAL_REFUSED]) take({ kind: "builtin", name })
    // An alias names the real tool it stands for.
    for (const [alias, real] of Object.entries(ALIASES)) {
      if (!taken.has(alias)) taken.set(alias, { kind: "builtin", name: real })
    }
    for (const tool of tools) {
      const clash = taken.get(tool.name.toLowerCase())
      if (!clash) continue
      throw new RemoteToolError(
        `The custom tool "${tool.name}" has the same name as ${clashText(clash)}. ` +
          "Custom tools may not replace built-in or MCP tools: rename it on the agent and try again.",
        "custom_tool_conflict",
        { tool: tool.name, clashes_with: clashRef(clash) },
      )
    }
  }

  /**
   * Check a session's custom tools against the names already taken
   * (checkCustomTools) and make them the session's set (replacing the
   * previous turn's; an empty list clears it). On a clash it throws and
   * registers nothing.
   */
  export function registerCustomTools(sessionID: string, tools: CustomTool[], reserved: Iterable<ReservedTool>): void {
    checkCustomTools(tools, reserved)
    customBySession.delete(sessionID)
    if (!tools.length) return
    customBySession.set(sessionID, new Map(tools.map((tool) => [tool.name, tool])))
    while (customBySession.size > MAX_CUSTOM_TOOL_SESSIONS) {
      const oldest = customBySession.keys().next().value
      if (oldest === undefined) break
      customBySession.delete(oldest)
    }
  }

  /**
   * True when `name` is one of the session's custom tools: its own, or its
   * root's for a subagent session already linked (linkSession/rootSessionOf).
   */
  export function isCustomTool(sessionID: string, name: string): boolean {
    return (customBySession.get(sessionID) ?? customBySession.get(rootSession(sessionID)))?.has(name) ?? false
  }

  /**
   * The custom tools a session may call: its own registered set, else its
   * root's (a subagent gets the tools of the agent that started it).
   */
  export async function customToolsFor(sessionID: string): Promise<CustomTool[]> {
    const own = customBySession.get(sessionID)
    if (own) return [...own.values()]
    const root = await rootSessionOf(sessionID)
    return [...(customBySession.get(root)?.values() ?? [])]
  }

  export class RemoteToolError extends Error {
    constructor(
      message: string,
      readonly code: string,
      /**
       * Set on `custom_tool_conflict`, in the contract's Error fields
       * (selfhost-v1.yaml): the refused custom tool and what holds its name.
       */
      readonly conflict?: { tool: string; clashes_with: string },
    ) {
      super(message)
      this.name = "RemoteToolError"
    }
  }

  function config(): { url: string; token: string } {
    const url = (process.env.GIZZI_TOOL_HOST_URL ?? "").trim().replace(/\/+$/, "")
    const token = (process.env.GIZZI_TOOL_HOST_TOKEN ?? "").trim()
    if (!url || !token) {
      throw new RemoteToolError(
        "Remote tool host is on (GIZZI_TOOL_HOST=remote) but GIZZI_TOOL_HOST_URL or GIZZI_TOOL_HOST_TOKEN is not set. " +
          "The tool did not run. Tools never fall back to running on the server.",
        "not_configured",
      )
    }
    return { url, token }
  }

  function stableJson(value: unknown): string {
    if (value === null || typeof value !== "object") return JSON.stringify(value) ?? "null"
    if (Array.isArray(value)) return `[${value.map(stableJson).join(",")}]`
    const entries = Object.keys(value as Record<string, unknown>)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${stableJson((value as Record<string, unknown>)[key])}`)
    return `{${entries.join(",")}}`
  }

  /**
   * The call's id: the model's tool call id when there is one, else a hash of
   * session, message, tool and input, so a retry of the same call reuses the
   * same id (the server is idempotent on it).
   */
  export function toolCallId(toolID: string, args: unknown, ctx: Pick<Tool.Context, "sessionID" | "messageID" | "callID">): string {
    if (ctx.callID) return ctx.callID
    const digest = createHash("sha256")
      .update(`${ctx.sessionID}\n${ctx.messageID}\n${toolID}\n${stableJson(args)}`)
      .digest("hex")
      .slice(0, 32)
    return `gz_${digest}`
  }

  export function timeoutFor(toolID: string, args: any): number {
    const configured = Number(process.env.GIZZI_TOOL_HOST_TIMEOUT_MS) || DEFAULT_TIMEOUT_MS
    if (toolID !== "bash" || args?.run_in_background) return configured
    const bashTimeout = typeof args?.timeout === "number" && args.timeout > 0 ? args.timeout : BASH_DEFAULT_TIMEOUT_MS
    return Math.max(configured, bashTimeout + BASH_GRACE_MS)
  }

  function displayPath(filePath: unknown): string | undefined {
    if (typeof filePath !== "string" || !filePath) return undefined
    return path.isAbsolute(filePath) ? filePath : filePath.replace(/^\.\//, "")
  }

  /** The title the local tool would show, from its input alone. */
  export function titleFor(toolID: string, args: any): string {
    switch (toolID) {
      case "bash":
        return String(args?.description || args?.command || "bash")
      case "read":
      case "write":
      case "edit":
      case "multiedit":
      case "notebook_edit":
      case "symbol_edit":
        return displayPath(args?.filePath) ?? toolID
      case "grep":
      case "glob":
        return String(args?.pattern ?? toolID)
      case "list":
        return displayPath(args?.path) ?? "."
      case "apply_patch":
        return "Apply patch"
      case "scratchpad_read":
      case "scratchpad_write":
      case "scratchpad_remove":
        return String(args?.path ?? toolID)
      case "scratchpad_list":
        return "Scratchpad files"
      default:
        return toolID
    }
  }

  function metadataFor(toolID: string, args: any, output: string, extra: Record<string, unknown> = {}): Record<string, any> {
    const base: Record<string, any> = { remote: true, ...extra }
    if (toolID === "bash") {
      base.description = args?.description
      base.output = output.length > METADATA_OUTPUT_LIMIT ? output.slice(0, METADATA_OUTPUT_LIMIT) + "\n\n..." : output
      if (args?.run_in_background) base.background = true
    }
    if (typeof args?.filePath === "string") base.filepath = args.filePath
    return base
  }

  async function readError(response: Response): Promise<{ code?: string; message?: string }> {
    const text = await response.text().catch(() => "")
    try {
      const body = JSON.parse(text) as { error?: { code?: string; message?: string } }
      return { code: body.error?.code, message: body.error?.message }
    } catch {
      return { message: text.slice(0, 300) || undefined }
    }
  }

  /**
   * Run one tool through the server's worker queue. Resolves with the local
   * tool result shape, or throws (a tool error) — never runs the tool here.
   */
  export async function execute(
    toolID: string,
    args: any,
    ctx: Tool.Context,
    fetchImpl: typeof fetch = fetch,
  ): Promise<{ title: string; metadata: Record<string, any>; output: string }> {
    if (SERVER_LOCAL_REFUSED.has(toolID)) {
      throw new RemoteToolError(
        `${toolID} is not available on a self-hosted Agents Server: it would run code on the server, and tools ` +
          "only run on workers. Use bash instead; it runs in the environment's sandbox.",
        "tool_not_available",
      )
    }
    const { url, token } = config()
    // A subagent's call goes to the root's queue: the server knows no other id.
    const queueSession = await rootSessionOf(ctx.sessionID)
    const tool_call_id = toolCallId(toolID, args, ctx)
    const timeout_ms = timeoutFor(toolID, args)
    const title = titleFor(toolID, args)

    ctx.metadata({ title, metadata: metadataFor(toolID, args, "", { status: "queued" }) })

    const signals = [AbortSignal.timeout(timeout_ms + HTTP_GRACE_MS)]
    if (ctx.abort) signals.push(ctx.abort)
    const signal = AbortSignal.any(signals)

    let response: Response
    try {
      response = await fetchImpl(`${url}/internal/v1/sessions/${encodeURIComponent(queueSession)}/tool_calls`, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          Authorization: `Bearer ${token}`,
        },
        body: JSON.stringify({ tool_call_id, name: toolID, input: args ?? {}, timeout_ms }),
        signal,
      })
    } catch (error) {
      if (ctx.abort?.aborted) throw new RemoteToolError(`The ${toolID} call was cancelled.`, "cancelled")
      const reason = error instanceof Error ? error.message : String(error)
      log.warn("remote tool call failed to reach the server", { toolID, tool_call_id, reason })
      throw new RemoteToolError(
        `The ${toolID} call could not reach the Agents Server's tool queue (${reason}). The tool did not run here.`,
        "transport",
      )
    }

    if (response.status === 504) {
      const err = await readError(response)
      throw new RemoteToolError(
        `No worker is running for this session's environment, so the ${toolID} call did not run. ` +
          "Start a worker (`gizzi worker --server ... --environment-key ...`) and try again." +
          (err.message ? ` (${err.message})` : ""),
        err.code ?? "no_worker",
      )
    }
    if (response.status === 408) {
      const err = await readError(response)
      throw new RemoteToolError(
        `A worker took the ${toolID} call but sent no result within ${Math.round(timeout_ms / 1000)}s.` +
          (err.message ? ` (${err.message})` : ""),
        err.code ?? "timeout",
      )
    }
    if (!response.ok) {
      const err = await readError(response)
      throw new RemoteToolError(
        `The Agents Server refused the ${toolID} call (HTTP ${response.status}${err.code ? ` ${err.code}` : ""})` +
          (err.message ? `: ${err.message}` : "."),
        err.code ?? `http_${response.status}`,
      )
    }

    const body = (await response.json().catch(() => undefined)) as { output?: unknown; is_error?: unknown } | undefined
    if (!body || typeof body.output !== "string") {
      throw new RemoteToolError(`The Agents Server sent an unreadable result for the ${toolID} call.`, "bad_result")
    }
    if (body.is_error === true) {
      // Same as a local tool throwing: the loop records a tool error with this text.
      throw new RemoteToolError(body.output || `The ${toolID} call failed on the worker.`, "tool_error")
    }
    return { title, metadata: metadataFor(toolID, args, body.output), output: body.output }
  }
}
