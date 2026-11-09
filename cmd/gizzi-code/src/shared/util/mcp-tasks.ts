/**
 * MCP tasks (protocol revision 2025-11-25, "Tasks"): a long-running tool call
 * comes back as a task the client polls instead of one blocking request.
 *
 *   tools/call { …, task: { ttl } }  → { task: { taskId, status, pollInterval } }
 *   tasks/get { taskId }             → status: working | input_required | completed | failed | cancelled
 *   tasks/result { taskId }          → the tool's CallToolResult (blocks until done)
 *   tasks/cancel { taskId }          → sent when the user stops the call (Esc)
 *
 * Used only when the server advertises `capabilities.tasks.requests.tools.call`
 * and the tool says `execution.taskSupport` is "optional" or "required", on a
 * 2025-era connection. The 2026-07-28 revision moved tasks into an extension
 * (SEP-2663) that @modelcontextprotocol/client 2.3.1 does not drive, so a
 * 2026-era peer gets a plain tools/call. Shared by the TUI (services/mcp) and
 * the engine (runtime/tools/mcp).
 */
import { z } from "zod/v4"

export type McpTaskStatus = "working" | "input_required" | "completed" | "failed" | "cancelled"

const TaskSchema = z
  .object({
    taskId: z.string(),
    status: z.string(),
    statusMessage: z.string().optional(),
    pollInterval: z.number().optional(),
  })
  .passthrough()
const CreateTaskResultSchema = z.object({ task: TaskSchema }).passthrough()
const AnyResultSchema = z.object({}).passthrough()

/** The parts of an SDK Client this needs (the real Client satisfies it). */
export type McpTaskClient = {
  request(request: { method: string; params?: Record<string, unknown> }, resultSchema: any, options?: any): Promise<any>
  getServerCapabilities(): any
  getProtocolEra?(): string | undefined
}

export type McpTaskUpdate = { taskId: string; status: string; statusMessage?: string; elapsedMs: number }

/** True when this tool call should run as an MCP task. */
export function shouldCallAsTask(client: McpTaskClient, taskSupport: unknown): boolean {
  if (taskSupport !== "optional" && taskSupport !== "required") return false
  if (client.getProtocolEra?.() === "modern") return false
  return !!client.getServerCapabilities()?.tasks?.requests?.tools?.call
}

const TERMINAL = new Set(["completed", "failed", "cancelled"])
const MIN_POLL_MS = 250
const MAX_POLL_MS = 5_000

function sleep(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) return reject(abortError())
    const timer = setTimeout(() => {
      signal?.removeEventListener("abort", onAbort)
      resolve()
    }, ms)
    const onAbort = () => {
      clearTimeout(timer)
      reject(abortError())
    }
    signal?.addEventListener("abort", onAbort, { once: true })
  })
}

function abortError(): Error {
  const err = new Error("The MCP task was cancelled")
  err.name = "AbortError"
  return err
}

/**
 * Run one tool call as an MCP task: create it, poll tasks/get (reporting each
 * status), then fetch tasks/result. Aborting `signal` sends tasks/cancel and
 * rejects with an AbortError.
 */
export async function callToolAsTask(opts: {
  client: McpTaskClient
  name: string
  args: Record<string, unknown>
  meta?: Record<string, unknown>
  signal?: AbortSignal
  /** Per-request timeout for each JSON-RPC call (not the whole task). */
  requestTimeoutMs?: number
  /** How long the server should keep the task's result. */
  ttlMs?: number
  onUpdate?: (update: McpTaskUpdate) => void
  /** Injectable for tests. */
  wait?: (ms: number, signal?: AbortSignal) => Promise<void>
}): Promise<any> {
  const { client, signal } = opts
  const wait = opts.wait ?? sleep
  const started = Date.now()
  const reqOpts = { signal, timeout: opts.requestTimeoutMs }
  const created = await client.request(
    {
      method: "tools/call",
      params: {
        name: opts.name,
        arguments: opts.args,
        ...(opts.meta ? { _meta: opts.meta } : {}),
        task: { ttl: opts.ttlMs ?? 60 * 60 * 1000 },
      },
    },
    CreateTaskResultSchema,
    reqOpts,
  )
  let task = created.task as z.infer<typeof TaskSchema>
  const taskId = task.taskId
  const report = () =>
    opts.onUpdate?.({ taskId, status: task.status, statusMessage: task.statusMessage, elapsedMs: Date.now() - started })

  try {
    report()
    while (task.status === "working") {
      const interval = Math.min(MAX_POLL_MS, Math.max(MIN_POLL_MS, task.pollInterval ?? 1_000))
      await wait(interval, signal)
      const next = await client.request({ method: "tasks/get", params: { taskId } }, TaskSchema, reqOpts)
      task = { ...task, ...next }
      report()
    }
    if (task.status === "cancelled") throw new Error(task.statusMessage || "The MCP server cancelled the task")
    // input_required, completed and failed all continue through tasks/result:
    // the server sends any elicitation it needs there, and a failed task's
    // result carries its error.
    return await client.request({ method: "tasks/result", params: { taskId } }, AnyResultSchema, reqOpts)
  } catch (error) {
    if (signal?.aborted && !TERMINAL.has(task.status)) {
      await client
        .request({ method: "tasks/cancel", params: { taskId } }, AnyResultSchema, { timeout: 5_000 })
        .catch(() => {})
      throw abortError()
    }
    throw error
  }
}

/** "Working · 42s · Indexing 3,000 files" for a tool line. */
export function formatTaskUpdate(update: McpTaskUpdate): string {
  const secs = Math.floor(update.elapsedMs / 1000)
  const elapsed = secs < 60 ? `${secs}s` : `${Math.floor(secs / 60)}m ${secs % 60}s`
  const status = update.status === "input_required" ? "Needs input" : update.status === "working" ? "Working" : update.status
  return [`MCP task ${status}`, elapsed, update.statusMessage].filter(Boolean).join(" · ")
}
