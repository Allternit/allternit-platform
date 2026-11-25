/**
 * MCP tasks. Two wire versions, one entry point (`callToolAsTask`):
 *
 * Legacy (protocol 2025-11-25, "Tasks"):
 *   tools/call { …, task: { ttl } }  → { task: { taskId, status, pollInterval } }
 *   tasks/get { taskId }             → status: working | input_required | completed | failed | cancelled
 *   tasks/result { taskId }          → the tool's CallToolResult (blocks until done; carries elicitation)
 *   tasks/cancel { taskId }          → sent when the user stops the call (Esc)
 *
 * Final (SEP-2663 extension `io.modelcontextprotocol/tasks`, 2026-07-28 era):
 *   tools/call                       → a plain result, or a CreateTaskResult when the server chose a task
 *   tasks/get { taskId }             → the task, with `inputRequests` while input_required, `result` when
 *                                      completed, `error` when failed (tasks/result no longer exists)
 *   tasks/update { taskId, inputResponses } → answers `inputRequests` (elicitation/create per key)
 *   tasks/cancel { taskId }
 *
 * The final path is used on a "modern" connection whose server advertises the extension; the legacy
 * path only on a 2025-era connection that advertises `capabilities.tasks.requests.tools.call` and a
 * tool whose `execution.taskSupport` is "optional" or "required". Shared by the TUI (services/mcp)
 * and the engine (runtime/tools/mcp).
 */
import { z } from "zod/v4"

export const MCP_TASKS_EXTENSION_ID = "io.modelcontextprotocol/tasks"

export type McpTaskStatus = "working" | "input_required" | "completed" | "failed" | "cancelled"
export type McpTaskProtocol = "final" | "legacy"

const TaskSchema = z
  .object({
    taskId: z.string(),
    status: z.string(),
    statusMessage: z.string().optional(),
    pollInterval: z.number().optional(),
    ttl: z.number().nullable().optional(),
    createdAt: z.string().optional(),
    lastUpdatedAt: z.string().optional(),
  })
  .passthrough()
type Task = z.infer<typeof TaskSchema>
const AnyResultSchema = z.object({}).passthrough()

/** The parts of an SDK Client this needs (the real Client satisfies it). */
export type McpTaskClient = {
  request(request: { method: string; params?: Record<string, unknown> }, resultSchema: any, options?: any): Promise<any>
  getServerCapabilities(): any
  getProtocolEra?(): string | undefined
}

/** One ask the server put on a task (final spec: `inputRequests[key]`). */
export type McpInputRequest = { method?: string; params?: Record<string, any> }
/** The client's answer to one ask (final spec: `inputResponses[key]`), an ElicitResult. */
export type McpInputResponse = { action: "accept" | "decline" | "cancel"; content?: Record<string, unknown> }
/** Answers every pending ask of a task (UI form, TUI dialog); keys must match `requests`. */
export type McpTaskElicit = (
  requests: Record<string, McpInputRequest>,
  ctx: { taskId: string; signal?: AbortSignal },
) => Promise<Record<string, McpInputResponse>>

export type McpTaskUpdate = {
  taskId: string
  status: string
  statusMessage?: string
  elapsedMs: number
  protocol: McpTaskProtocol
  /** ISO times from the server's task object, when it sent them. */
  createdAt?: string
  lastUpdatedAt?: string
  progress?: { done: number; total?: number; label?: string }
  /** Set while the task is input_required (final spec). */
  inputRequests?: Record<string, McpInputRequest>
  /** Set once the task completes (final spec) or fails. */
  result?: unknown
  error?: string
}

/** Which task wire version applies to this connection and tool, or undefined for a plain call. */
export function taskProtocol(client: McpTaskClient, taskSupport: unknown): McpTaskProtocol | undefined {
  const caps = client.getServerCapabilities()
  if (client.getProtocolEra?.() === "modern") {
    if (taskSupport === "forbidden") return undefined
    return caps?.extensions?.[MCP_TASKS_EXTENSION_ID] ? "final" : undefined
  }
  if (taskSupport !== "optional" && taskSupport !== "required") return undefined
  return caps?.tasks?.requests?.tools?.call ? "legacy" : undefined
}

/** True when this tool call should run as an MCP task. */
export function shouldCallAsTask(client: McpTaskClient, taskSupport: unknown): boolean {
  return taskProtocol(client, taskSupport) !== undefined
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

function pollMs(task: Task): number {
  return Math.min(MAX_POLL_MS, Math.max(MIN_POLL_MS, task.pollInterval ?? 1_000))
}

/** A CreateTaskResult is `{ task }` (2025) or the task itself / `{ resultType: "task", … }` (final). */
function createdTask(value: any): Task | undefined {
  const candidate = value?.task ?? value
  const parsed = TaskSchema.safeParse(candidate)
  if (!parsed.success) return undefined
  // A plain CallToolResult never has a taskId, so this cannot mistake one for a task.
  return parsed.data
}

function progressOf(task: Task): McpTaskUpdate["progress"] {
  const t = task as Record<string, any>
  const done = typeof t.progress === "number" ? t.progress : t.progress?.progress ?? t._meta?.progress?.progress
  if (typeof done !== "number") return undefined
  const total = typeof t.total === "number" ? t.total : t.progress?.total ?? t._meta?.progress?.total
  return { done, ...(typeof total === "number" ? { total } : {}), ...(task.statusMessage ? { label: task.statusMessage } : {}) }
}

function errorText(error: unknown, fallback: string): string {
  if (typeof error === "string") return error
  const message = (error as any)?.message
  return typeof message === "string" && message ? message : fallback
}

/**
 * Run one tool call as an MCP task. Final spec: tools/call, poll tasks/get (respecting
 * pollInterval and ttl), answer `inputRequests` through `onElicit` + tasks/update, return the
 * task's `result`. Legacy: create with a ttl, poll tasks/get, fetch tasks/result. Aborting
 * `signal` sends tasks/cancel and rejects with an AbortError.
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
  /** Overrides detection (tests, callers that already know). */
  protocol?: McpTaskProtocol
  onUpdate?: (update: McpTaskUpdate) => void
  /** Final spec: answers the task's input requests. Without it they are declined. */
  onElicit?: McpTaskElicit
  /** Injectable for tests. */
  wait?: (ms: number, signal?: AbortSignal) => Promise<void>
}): Promise<any> {
  const { client, signal } = opts
  const wait = opts.wait ?? sleep
  const started = Date.now()
  const reqOpts = { signal, timeout: opts.requestTimeoutMs }
  const protocol: McpTaskProtocol =
    opts.protocol ?? (client.getProtocolEra?.() === "modern" ? "final" : "legacy")
  const ttlMs = opts.ttlMs ?? 60 * 60 * 1000
  const params = {
    name: opts.name,
    arguments: opts.args,
    ...(opts.meta ? { _meta: opts.meta } : {}),
    ...(protocol === "legacy" ? { task: { ttl: ttlMs } } : {}),
  }
  const first = await client.request({ method: "tools/call", params }, AnyResultSchema, reqOpts)
  let task = createdTask(first)
  // Final spec: the server may answer a plain result when it did not make a task.
  if (!task) return first
  const taskId = task.taskId
  let latest: Record<string, any> = task
  const report = () =>
    opts.onUpdate?.({
      taskId,
      status: task!.status,
      statusMessage: task!.statusMessage,
      elapsedMs: Date.now() - started,
      protocol,
      createdAt: task!.createdAt,
      lastUpdatedAt: task!.lastUpdatedAt,
      progress: progressOf(task!),
      ...(latest.inputRequests && task!.status === "input_required" ? { inputRequests: latest.inputRequests } : {}),
      ...(latest.result !== undefined && task!.status === "completed" ? { result: latest.result } : {}),
      ...(task!.status === "failed" ? { error: errorText(latest.error, task!.statusMessage ?? "The task failed") } : {}),
    })

  try {
    report()
    if (protocol === "legacy") {
      while (task.status === "working") {
        await wait(pollMs(task), signal)
        const next = await client.request({ method: "tasks/get", params: { taskId } }, TaskSchema, reqOpts)
        task = { ...task, ...next }
        report()
      }
      if (task.status === "cancelled") throw new Error(task.statusMessage || "The MCP server cancelled the task")
      // input_required, completed and failed all continue through tasks/result:
      // the server sends any elicitation it needs there, and a failed task's
      // result carries its error.
      return await client.request({ method: "tasks/result", params: { taskId } }, AnyResultSchema, reqOpts)
    }

    // Final spec.
    const answered = new Set<string>()
    const ttl = typeof task.ttl === "number" ? task.ttl : undefined
    while (!TERMINAL.has(task.status)) {
      if (task.status === "input_required") {
        const asked = (latest.inputRequests ?? {}) as Record<string, McpInputRequest>
        const open = Object.fromEntries(Object.entries(asked).filter(([key]) => !answered.has(key)))
        if (Object.keys(open).length > 0) {
          const responses = opts.onElicit
            ? await opts.onElicit(open, { taskId, signal })
            : Object.fromEntries(Object.keys(open).map((key) => [key, { action: "decline" as const }]))
          for (const key of Object.keys(open)) answered.add(key)
          await client.request(
            { method: "tasks/update", params: { taskId, inputResponses: responses } },
            AnyResultSchema,
            reqOpts,
          )
        }
      }
      if (ttl !== undefined && Date.now() - started > ttl) {
        throw Object.assign(new Error("The MCP task outlived its ttl"), { name: "AbortError", expired: true })
      }
      await wait(pollMs(task), signal)
      latest = (await client.request({ method: "tasks/get", params: { taskId } }, AnyResultSchema, reqOpts)) as Record<string, any>
      task = { ...task, ...(TaskSchema.safeParse(latest).data ?? {}) }
      report()
    }
    if (task.status === "cancelled") throw new Error(task.statusMessage || "The MCP server cancelled the task")
    if (task.status === "failed") throw new Error(errorText(latest.error, task.statusMessage ?? "The MCP task failed"))
    return latest.result ?? { content: [] }
  } catch (error) {
    const expired = (error as any)?.expired === true
    if ((signal?.aborted || expired) && !TERMINAL.has(task.status)) {
      await client
        .request({ method: "tasks/cancel", params: { taskId } }, AnyResultSchema, { timeout: 5_000 })
        .catch(() => {})
      task = { ...task, status: "cancelled" }
      report()
      throw abortError()
    }
    throw error
  }
}

/** "Working · 42s · 3/10 · Indexing 3,000 files" for a tool line. */
export function formatTaskUpdate(update: McpTaskUpdate, server?: string): string {
  const secs = Math.floor(update.elapsedMs / 1000)
  const elapsed = secs < 60 ? `${secs}s` : `${Math.floor(secs / 60)}m ${secs % 60}s`
  const status = update.status === "input_required" ? "Needs input" : update.status === "working" ? "Working" : update.status
  const progress = update.progress ? `${update.progress.done}${update.progress.total ? `/${update.progress.total}` : ""}` : undefined
  return [`MCP task ${status}`, server, elapsed, progress, update.statusMessage].filter(Boolean).join(" · ")
}
