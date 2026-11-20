import { Hono } from "hono"
import type { MiddlewareHandler } from "hono"
import { describeRoute, validator, resolver } from "@/runtime/server/openapi"
import z from "zod/v4"
import { RuntimeService as DefaultRuntimeService, RuntimeNotFoundError } from "@/runtime/runtime-service"
import { RuntimeDriverFactory as DefaultRuntimeDriverFactory } from "@/runtime/runtime-driver-factory"
import { ExecutionLogService as DefaultExecutionLogService } from "@/runtime/execution-log"
import type { AgentEvent, AgentTask } from "@/runtime/runtime-driver"
import { errors } from "@/runtime/server/error"
import { lazy } from "@/shared/util/lazy"
import { streamSSE } from "hono/streaming"
import { randomUUID } from "node:crypto"
import PROMPT_SAFETY from "@/runtime/session/prompt/safety.txt"

function runtimeAuthMiddleware(
  RuntimeService: Pick<typeof DefaultRuntimeService, "get">,
): MiddlewareHandler {
  return async (c, next) => {
    const runtimeID = c.req.param("runtimeID")
    if (!runtimeID) return next()

    const runtime = await RuntimeService.get(runtimeID)
    if (!runtime) throw new RuntimeNotFoundError({ runtimeId: runtimeID })

    const configuredToken = runtime.metadata?.token
    if (!configuredToken) return next()

    const headerToken = c.req.header("x-runtime-token")
    const queryToken = c.req.query("token")
    const providedToken = headerToken ?? queryToken

    if (!providedToken || providedToken !== configuredToken) {
      return c.json(
        { error: "unauthorized", message: "Invalid or missing runtime token" },
        401,
      )
    }

    return next()
  }
}

const TaskRequestSchema = z.object({
  cliName: z.string(),
  prompt: z.string(),
  cwd: z.string().optional(),
  env: z.record(z.string(), z.string()).optional(),
  systemPrompt: z.string().optional(),
  attachments: z
    .array(
      z.object({
        filename: z.string(),
        mimeType: z.string(),
        content: z.union([z.string(), z.instanceof(Uint8Array)]),
      }),
    )
    .optional(),
})

export interface RuntimeRouteDeps {
  RuntimeService?: Pick<
    typeof DefaultRuntimeService,
    "list" | "get" | "remove" | "heartbeat" | "markBusy"
  >
  RuntimeDriverFactory?: Pick<typeof DefaultRuntimeDriverFactory, "forRuntime">
  ExecutionLogService?: Pick<
    typeof DefaultExecutionLogService,
    "create" | "appendEvent" | "get" | "listByRuntime"
  >
}

export function createRuntimeRoutes(deps: Partial<RuntimeRouteDeps> = {}): Hono {
  const RuntimeService = deps.RuntimeService ?? DefaultRuntimeService
  const RuntimeDriverFactory = deps.RuntimeDriverFactory ?? DefaultRuntimeDriverFactory
  const ExecutionLogService = deps.ExecutionLogService ?? DefaultExecutionLogService

  return new Hono()
    .use("/:runtimeID/*", runtimeAuthMiddleware(RuntimeService))
    .get(
      "/",
      describeRoute({
        summary: "List runtimes",
        description: "List all registered agent runtimes and their discovered CLIs.",
        operationId: "runtime.list",
        responses: {
          200: {
            description: "List of runtimes",
            content: {
              "application/json": {
                schema: resolver(z.object({ runtimes: z.array(z.any()) })),
              },
            },
          },
        },
      }),
      async (c) => {
        const runtimes = await RuntimeService.list()
        return c.json({ runtimes })
      },
    )
    .get(
      "/:runtimeID",
      describeRoute({
        summary: "Get runtime",
        description: "Get a single runtime and its discovered CLIs.",
        operationId: "runtime.get",
        responses: {
          200: {
            description: "Runtime details",
            content: { "application/json": { schema: resolver(z.any()) } },
          },
          ...errors(404),
        },
      }),
      validator("param", z.object({ runtimeID: z.string() })),
      async (c) => {
        const { runtimeID } = c.req.valid("param")
        const runtime = await RuntimeService.get(runtimeID)
        if (!runtime) throw new RuntimeNotFoundError({ runtimeId: runtimeID })
        return c.json({ runtime })
      },
    )
    .delete(
      "/:runtimeID",
      describeRoute({
        summary: "Delete runtime",
        description: "Remove a runtime registration.",
        operationId: "runtime.delete",
        responses: {
          200: {
            description: "Deletion result",
            content: { "application/json": { schema: resolver(z.object({ ok: z.boolean() })) } },
          },
          ...errors(404),
        },
      }),
      validator("param", z.object({ runtimeID: z.string() })),
      async (c) => {
        const { runtimeID } = c.req.valid("param")
        const ok = await RuntimeService.remove(runtimeID)
        if (!ok) throw new RuntimeNotFoundError({ runtimeId: runtimeID })
        return c.json({ ok: true })
      },
    )
    .post(
      "/:runtimeID/heartbeat",
      describeRoute({
        summary: "Runtime heartbeat",
        description: "Mark a runtime as online.",
        operationId: "runtime.heartbeat",
        responses: {
          200: {
            description: "Heartbeat accepted",
            content: { "application/json": { schema: resolver(z.object({ ok: z.boolean() })) } },
          },
          ...errors(404),
        },
      }),
      validator("param", z.object({ runtimeID: z.string() })),
      async (c) => {
        const { runtimeID } = c.req.valid("param")
        const runtime = await RuntimeService.get(runtimeID)
        if (!runtime) throw new RuntimeNotFoundError({ runtimeId: runtimeID })
        await RuntimeService.heartbeat(runtimeID)
        return c.json({ ok: true })
      },
    )
    .get(
      "/:runtimeID/logs",
      describeRoute({
        summary: "List runtime execution logs",
        description: "Recent execution logs for a runtime.",
        operationId: "runtime.logs",
        responses: {
          200: {
            description: "Execution logs",
            content: { "application/json": { schema: resolver(z.object({ logs: z.array(z.any()) })) } },
          },
          ...errors(404),
        },
      }),
      validator("param", z.object({ runtimeID: z.string() })),
      validator("query", z.object({ limit: z.coerce.number().optional() })),
      async (c) => {
        const { runtimeID } = c.req.valid("param")
        const { limit } = c.req.valid("query")
        const runtime = await RuntimeService.get(runtimeID)
        if (!runtime) throw new RuntimeNotFoundError({ runtimeId: runtimeID })
        const logs = await ExecutionLogService.listByRuntime(runtimeID, limit ?? 100)
        return c.json({ logs })
      },
    )
    .post(
      "/:runtimeID/tasks",
      describeRoute({
        summary: "Assign a task",
        description: "Assign a task to a runtime and receive a task handle.",
        operationId: "runtime.task.assign",
        responses: {
          200: {
            description: "Task handle",
            content: { "application/json": { schema: resolver(z.any()) } },
          },
          ...errors(400, 404),
        },
      }),
      validator("param", z.object({ runtimeID: z.string() })),
      validator("json", TaskRequestSchema),
      async (c) => {
        const { runtimeID } = c.req.valid("param")
        const input = c.req.valid("json")
        const runtime = await RuntimeService.get(runtimeID)
        if (!runtime) throw new RuntimeNotFoundError({ runtimeId: runtimeID })

        const driver = await RuntimeDriverFactory.forRuntime(runtimeID, input.cliName)
        const taskId = randomUUID()
        const task: AgentTask = {
          taskId,
          prompt: input.prompt,
          cwd: input.cwd,
          env: input.env,
          // The client string is the whole system text for this task, so the hard limits ride with it.
          systemPrompt: input.systemPrompt?.includes(PROMPT_SAFETY.trim())
            ? input.systemPrompt
            : input.systemPrompt?.trim()
              ? `${input.systemPrompt}\n\n${PROMPT_SAFETY.trim()}`
              : PROMPT_SAFETY.trim(),
          attachments: input.attachments as AgentTask["attachments"],
        }

        const handle = await driver.assign(task)
        await ExecutionLogService.create(handle)
        return c.json({ handle })
      },
    )
    .post(
      "/:runtimeID/tasks/:taskID/abort",
      describeRoute({
        summary: "Abort a task",
        description: "Abort a running task on a runtime.",
        operationId: "runtime.task.abort",
        responses: {
          200: {
            description: "Abort result",
            content: { "application/json": { schema: resolver(z.object({ ok: z.boolean() })) } },
          },
          ...errors(404),
        },
      }),
      validator("param", z.object({ runtimeID: z.string(), taskID: z.string() })),
      async (c) => {
        const { runtimeID, taskID } = c.req.valid("param")
        const logEntry = await ExecutionLogService.get(taskID)
        if (!logEntry || logEntry.runtimeId !== runtimeID) {
          throw new RuntimeNotFoundError({ runtimeId: `${runtimeID}/${taskID}` })
        }
        const driver = await RuntimeDriverFactory.forRuntime(runtimeID, logEntry.cliName)
        await driver.abort({ taskId: taskID, runtimeId: runtimeID, cliName: logEntry.cliName })
        return c.json({ ok: true })
      },
    )
    .get(
      "/:runtimeID/tasks/:taskID",
      describeRoute({
        summary: "Inspect a task",
        description: "Get the execution log for a task.",
        operationId: "runtime.task.inspect",
        responses: {
          200: {
            description: "Execution log",
            content: { "application/json": { schema: resolver(z.any()) } },
          },
          ...errors(404),
        },
      }),
      validator("param", z.object({ runtimeID: z.string(), taskID: z.string() })),
      async (c) => {
        const { runtimeID, taskID } = c.req.valid("param")
        const logEntry = await ExecutionLogService.get(taskID)
        if (!logEntry || logEntry.runtimeId !== runtimeID) {
          throw new RuntimeNotFoundError({ runtimeId: `${runtimeID}/${taskID}` })
        }
        return c.json({ log: logEntry })
      },
    )
    .get(
      "/:runtimeID/tasks/:taskID/stream",
      describeRoute({
        summary: "Stream task events",
        description: "Stream task events as server-sent events.",
        operationId: "runtime.task.stream",
        responses: {
          200: {
            description: "SSE stream of AgentEvent objects",
            content: { "text/event-stream": { schema: resolver(z.any()) } },
          },
          ...errors(404),
        },
      }),
      validator("param", z.object({ runtimeID: z.string(), taskID: z.string() })),
      async (c) => {
        const { runtimeID, taskID } = c.req.valid("param")
        const logEntry = await ExecutionLogService.get(taskID)
        if (!logEntry || logEntry.runtimeId !== runtimeID) {
          throw new RuntimeNotFoundError({ runtimeId: `${runtimeID}/${taskID}` })
        }

        const driver = await RuntimeDriverFactory.forRuntime(runtimeID, logEntry.cliName)
        const handle = { taskId: taskID, runtimeId: runtimeID, cliName: logEntry.cliName }

        return streamSSE(c, async (s) => {
          await RuntimeService.markBusy(runtimeID, true)
          try {
            for await (const event of driver.stream(handle)) {
              await ExecutionLogService.appendEvent(taskID, event)
              await s.write(`data: ${JSON.stringify(event)}\n\n`)
            }
            await s.write(`data: ${JSON.stringify({ type: "stream-end" })}\n\n`)
          } catch (err) {
            const errorEvent: AgentEvent = { type: "error", error: err instanceof Error ? err.message : String(err) }
            await ExecutionLogService.appendEvent(taskID, errorEvent)
            await s.write(`data: ${JSON.stringify(errorEvent)}\n\n`)
          } finally {
            await RuntimeService.markBusy(runtimeID, false)
          }
        })
      },
    )
}

export const RuntimeRoutes = lazy(() => createRuntimeRoutes())
