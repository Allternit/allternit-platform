/**
 * Remote tool host (self-hosted Agents Server, plan step 8).
 *
 * When `GIZZI_TOOL_HOST=remote`, this gizzi is the agent loop child of
 * allternit-agents-server. Model calls and MCP stay here; every tool in the
 * sandbox set, and every custom tool of the session's agent, is handed to the
 * server's loopback queue instead of running locally:
 *
 *   POST ${GIZZI_TOOL_HOST_URL}/internal/v1/sessions/{gizzi session id}/tool_calls
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

  /**
   * Check a session's custom tools against the names already taken and make
   * them the session's set (replacing the previous turn's; an empty list
   * clears it). A custom tool may not shadow a built-in, MCP or sandbox tool:
   * throws a RemoteToolError (`custom_tool_conflict`) and registers nothing.
   * Names are compared without case so `Bash` cannot pass for `bash`.
   */
  export function registerCustomTools(sessionID: string, tools: CustomTool[], reserved: Iterable<string>): void {
    if (tools.length) {
      const taken = new Set<string>()
      for (const name of [...reserved, ...SANDBOX_TOOLS, ...SERVER_LOCAL_REFUSED, ...Object.keys(ALIASES)]) {
        taken.add(name.toLowerCase())
      }
      for (const tool of tools) {
        if (taken.has(tool.name.toLowerCase())) {
          throw new RemoteToolError(
            `The custom tool "${tool.name}" has the same name as a built-in tool of the agent loop. ` +
              "Custom tools may not replace built-in tools: rename it on the agent and try again.",
            "custom_tool_conflict",
          )
        }
      }
    }
    customBySession.delete(sessionID)
    if (!tools.length) return
    customBySession.set(sessionID, new Map(tools.map((tool) => [tool.name, tool])))
    while (customBySession.size > MAX_CUSTOM_TOOL_SESSIONS) {
      const oldest = customBySession.keys().next().value
      if (oldest === undefined) break
      customBySession.delete(oldest)
    }
  }

  /** True when `name` is one of the session's registered custom tools. */
  export function isCustomTool(sessionID: string, name: string): boolean {
    return customBySession.get(sessionID)?.has(name) ?? false
  }

  export class RemoteToolError extends Error {
    constructor(
      message: string,
      readonly code: string,
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
    const tool_call_id = toolCallId(toolID, args, ctx)
    const timeout_ms = timeoutFor(toolID, args)
    const title = titleFor(toolID, args)

    ctx.metadata({ title, metadata: metadataFor(toolID, args, "", { status: "queued" }) })

    const signals = [AbortSignal.timeout(timeout_ms + HTTP_GRACE_MS)]
    if (ctx.abort) signals.push(ctx.abort)
    const signal = AbortSignal.any(signals)

    let response: Response
    try {
      response = await fetchImpl(`${url}/internal/v1/sessions/${encodeURIComponent(ctx.sessionID)}/tool_calls`, {
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
