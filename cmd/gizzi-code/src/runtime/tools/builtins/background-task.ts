import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { BackgroundTask } from "@/runtime/session/background-task"
import { SessionPrompt } from "@/runtime/session/prompt"
import { AgentControl } from "@/runtime/primitives/agent-control"

const terminal = new Set(["completed", "failed", "cancelled", "interrupted"])

export const BackgroundTaskListTool = Tool.define("task_list", {
  description: "List durable background tasks owned by the current session.",
  parameters: z.object({ active_only: z.boolean().default(false) }),
  async execute(params, ctx) {
    const tasks = await BackgroundTask.list(ctx.sessionID, params.active_only)
    return {
      title: "Background tasks",
      metadata: { count: tasks.length },
      output: JSON.stringify(tasks, null, 2),
    }
  },
})

export const BackgroundTaskOutputTool = Tool.define("task_output", {
  description: "Read a background task's durable status and output, optionally waiting for it to settle.",
  parameters: z.object({
    task_id: z.string(),
    block: z.boolean().default(false),
    timeout_ms: z.number().int().min(0).max(600_000).default(30_000),
  }),
  async execute(params, ctx) {
    let task = await BackgroundTask.get(params.task_id)
    if (!task || task.parentSessionID !== ctx.sessionID) throw new Error(`Background task ${params.task_id} not found`)
    if (params.block && !terminal.has(task.status)) {
      task = await BackgroundTask.wait(task.id, params.timeout_ms, ctx.abort)
      if (!task) throw new Error(`Background task ${params.task_id} disappeared while waiting`)
    }
    return {
      title: task.description,
      metadata: { status: task.status, taskID: task.id },
      output: JSON.stringify(task, null, 2),
    }
  },
})

export const BackgroundTaskStopTool = Tool.define("task_stop", {
  description:
    "Cancel a running background task owned by the current session. A subagent's own agents and background tasks stop with it, deepest first.",
  parameters: z.object({ task_id: z.string(), reason: z.string().optional() }),
  async execute(params, ctx) {
    const task = await BackgroundTask.get(params.task_id)
    if (!task || task.parentSessionID !== ctx.sessionID) throw new Error(`Background task ${params.task_id} not found`)
    if (terminal.has(task.status)) {
      return {
        title: task.description,
        metadata: { status: task.status, taskID: task.id },
        output: `Task ${task.id} is already ${task.status}.`,
      }
    }
    await ctx.ask({
      permission: "task_stop",
      patterns: [task.id],
      always: [],
      metadata: { taskID: task.id, description: task.description },
    })
    // Cascade: the subagent's own agents and shells stop first, deepest first; then the subagent itself.
    if (task.childSessionID) {
      await AgentControl.stopSubtree(task.childSessionID, {
        reason: params.reason ?? "Its parent task was stopped",
        includeRoot: false,
        archive: false,
      })
      SessionPrompt.cancel(task.childSessionID)
    }
    const cancelled = await BackgroundTask.cancel(task.id, params.reason)
    return {
      title: task.description,
      metadata: { status: cancelled.status, taskID: cancelled.id },
      output: `Cancelled background task ${cancelled.id}.`,
    }
  },
})
