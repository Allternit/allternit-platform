// MCP task + elicitation parts on the session event stream (SSE /event, every Bus event).
//
// One task card per task and one form per ask, on every client (web, Desktop, phone, iOS, Android,
// TUI). The shapes match allternit-ai's `TaskCardState` / `ElicitationRequest`
// (src/lib/artifacts/runtime/types.ts) plus the stream fields below.
//
//   mcp.task          { sessionID, part: McpTaskPart }          every status / progress change
//   mcp.elicitation   { sessionID, part: McpElicitationPart }   a form or URL ask for the user
//   mcp.elicitation.resolved { sessionID, requestID, action }   the ask was answered or cancelled
//
// Replies: POST /mcp/elicitation/:requestID/reply  { action, content? }
//          POST /mcp/task/:taskId/cancel           (sends tasks/cancel)
import z from "zod/v4"
import { Bus } from "@/shared/bus"
import { BusEvent } from "@/shared/bus/bus-event"
import { Identifier } from "@/shared/id/id"
import { getStreamContext } from "@/runtime/session/stream-context"
import type { McpInputRequest, McpInputResponse, McpTaskUpdate } from "@/shared/util/mcp-tasks"

export namespace McpTaskStream {
  const Status = z.enum(["working", "input_required", "completed", "cancelled", "failed"])

  export const ElicitationPart = z.discriminatedUnion("mode", [
    z.object({
      type: z.literal("mcp-elicitation"),
      mode: z.literal("form"),
      /** requestID: the id the reply route takes. */
      id: z.string(),
      sessionID: z.string(),
      source: z.string(),
      message: z.string(),
      requestedSchema: z.object({
        type: z.literal("object"),
        properties: z.record(z.string(), z.record(z.string(), z.any())),
        required: z.string().array().optional(),
      }),
      taskId: z.string().optional(),
      toolCallID: z.string().optional(),
    }),
    z.object({
      type: z.literal("mcp-elicitation"),
      mode: z.literal("url"),
      id: z.string(),
      sessionID: z.string(),
      source: z.string(),
      message: z.string(),
      url: z.string(),
      elicitationId: z.string(),
      taskId: z.string().optional(),
      toolCallID: z.string().optional(),
    }),
  ])
  export type ElicitationPart = z.infer<typeof ElicitationPart>

  export const TaskPart = z.object({
    type: z.literal("mcp-task"),
    sessionID: z.string(),
    /** The tool call this task belongs to (render the card under it). */
    toolCallID: z.string().optional(),
    taskId: z.string(),
    status: Status,
    /** Tool name and server display name (TaskCardState.toolName / .source). */
    toolName: z.string(),
    source: z.string(),
    origin: z.literal("server"),
    /** Card heading: "<tool> · <server>". */
    title: z.string(),
    statusMessage: z.string().optional(),
    progress: z.object({ done: z.number(), total: z.number().optional(), label: z.string().optional() }).optional(),
    /** ISO times (TaskCardState.createdAt / .lastUpdatedAt) and epoch ms for elapsed timers. */
    createdAt: z.string(),
    lastUpdatedAt: z.string(),
    startedAt: z.number(),
    updatedAt: z.number(),
    pollIntervalMs: z.number().optional(),
    /** The first open ask while input_required; more arrive as their own mcp-elicitation parts. */
    elicitation: ElicitationPart.optional(),
    result: z.any().optional(),
    error: z.string().optional(),
  })
  export type TaskPart = z.infer<typeof TaskPart>

  export const TaskEvent = BusEvent.define("mcp.task", z.object({ sessionID: z.string(), part: TaskPart }))
  export const ElicitationEvent = BusEvent.define(
    "mcp.elicitation",
    z.object({ sessionID: z.string(), part: ElicitationPart }),
  )
  export const ElicitationResolved = BusEvent.define(
    "mcp.elicitation.resolved",
    z.object({ sessionID: z.string(), requestID: z.string(), action: z.enum(["accept", "decline", "cancel"]) }),
  )

  const pending = new Map<string, { sessionID: string; resolve: (r: McpInputResponse) => void }>()
  const running = new Map<string, AbortController>()

  /** Build the ElicitationPart for one `elicitation/create` ask. */
  export function elicitationPart(input: {
    id: string
    sessionID: string
    source: string
    params: Record<string, any>
    taskId?: string
    toolCallID?: string
  }): ElicitationPart {
    const base = {
      type: "mcp-elicitation" as const,
      id: input.id,
      sessionID: input.sessionID,
      source: input.source,
      message: String(input.params.message ?? ""),
      ...(input.taskId ? { taskId: input.taskId } : {}),
      ...(input.toolCallID ? { toolCallID: input.toolCallID } : {}),
    }
    if (input.params.mode === "url") {
      return {
        ...base,
        mode: "url",
        url: String(input.params.url ?? ""),
        elicitationId: String(input.params.elicitationId ?? input.id),
      }
    }
    const schema = input.params.requestedSchema ?? {}
    return {
      ...base,
      mode: "form",
      requestedSchema: { type: "object", properties: schema.properties ?? {}, ...(schema.required ? { required: schema.required } : {}) },
    }
  }

  /** Publish one ask and wait for the reply route (or an abort, which cancels it). */
  export function ask(input: {
    /** Stable id (the task path uses `<taskId>:<key>`); generated when omitted. */
    id?: string
    source: string
    params: Record<string, any>
    taskId?: string
    toolCallID?: string
    signal?: AbortSignal
  }): Promise<McpInputResponse> {
    const sessionID = getStreamContext()?.sessionID
    // No session on the stream (a call outside a chat turn): nobody to ask.
    if (!sessionID) return Promise.resolve({ action: "cancel" })
    const id = input.id ?? Identifier.ascending("permission").replace(/^per_/, "elic_")
    return new Promise<McpInputResponse>((resolve) => {
      const finish = (r: McpInputResponse) => {
        if (!pending.delete(id)) return
        input.signal?.removeEventListener("abort", onAbort)
        void Bus.publish(ElicitationResolved, { sessionID, requestID: id, action: r.action })
        resolve(r)
      }
      const onAbort = () => finish({ action: "cancel" })
      pending.set(id, { sessionID, resolve: finish })
      if (input.signal?.aborted) return onAbort()
      input.signal?.addEventListener("abort", onAbort, { once: true })
      void Bus.publish(ElicitationEvent, {
        sessionID,
        part: elicitationPart({ id, sessionID, source: input.source, params: input.params, taskId: input.taskId, toolCallID: input.toolCallID }),
      })
    })
  }

  /** The reply route: false when the ask is unknown or already answered. */
  export function reply(requestID: string, response: McpInputResponse): boolean {
    const entry = pending.get(requestID)
    if (!entry) return false
    entry.resolve(response)
    return true
  }

  /** `onElicit` for callToolAsTask: one published ask per input request, answered concurrently. */
  export function elicitFor(source: string, toolCallID?: string) {
    return async (requests: Record<string, McpInputRequest>, ctx: { taskId: string; signal?: AbortSignal }) => {
      const entries = await Promise.all(
        Object.entries(requests).map(async ([key, request]) => {
          if (request.method && request.method !== "elicitation/create") {
            return [key, { action: "decline" }] as const
          }
          const response = await ask({ id: `${ctx.taskId}:${key}`, source, params: request.params ?? {}, taskId: ctx.taskId, toolCallID, signal: ctx.signal })
          return [key, response] as const
        }),
      )
      return Object.fromEntries(entries) as Record<string, McpInputResponse>
    }
  }

  /** Publish the task card state for one update; returns the part. */
  export function publishTask(
    update: McpTaskUpdate,
    info: { source: string; toolName: string; toolCallID?: string; startedAt: number },
  ): TaskPart | undefined {
    const sessionID = getStreamContext()?.sessionID
    if (!sessionID) return undefined
    const now = Date.now()
    const status = Status.safeParse(update.status)
    const first = update.inputRequests ? Object.entries(update.inputRequests)[0] : undefined
    const part: TaskPart = {
      type: "mcp-task",
      sessionID,
      ...(info.toolCallID ? { toolCallID: info.toolCallID } : {}),
      taskId: update.taskId,
      status: status.success ? status.data : "working",
      toolName: info.toolName,
      source: info.source,
      origin: "server",
      title: `${info.toolName} · ${info.source}`,
      ...(update.statusMessage ? { statusMessage: update.statusMessage } : {}),
      ...(update.progress ? { progress: update.progress } : {}),
      createdAt: update.createdAt ?? new Date(info.startedAt).toISOString(),
      lastUpdatedAt: update.lastUpdatedAt ?? new Date(now).toISOString(),
      startedAt: info.startedAt,
      updatedAt: now,
      ...(update.result !== undefined ? { result: update.result } : {}),
      ...(update.error ? { error: update.error } : {}),
      ...(first && (first[1].method ?? "elicitation/create") === "elicitation/create"
        ? {
            elicitation: elicitationPart({
              id: `${update.taskId}:${first[0]}`,
              sessionID,
              source: info.source,
              params: first[1].params ?? {},
              taskId: update.taskId,
              toolCallID: info.toolCallID,
            }),
          }
        : {}),
    }
    void Bus.publish(TaskEvent, { sessionID, part })
    return part
  }

  /** Register a running task so the cancel route can stop it. */
  export function track(taskId: string, controller: AbortController) {
    running.set(taskId, controller)
    return () => running.delete(taskId)
  }

  /** The cancel route: aborts the call, which sends tasks/cancel. False when the task is unknown. */
  export function cancel(taskId: string): boolean {
    const controller = running.get(taskId)
    if (!controller) return false
    controller.abort()
    return true
  }
}
