// POST /v1/tasks, GET /v1/tasks (read-only listing), GET /v1/tasks/:id,
// POST /v1/tasks/:id/cancel.
// Full worker execution is later phases: tasks persist as `queued` and a
// `task.created` ledger event fanned out to the submitting caller.
import { randomUUID } from "node:crypto";
import { Router, type Request, type Response } from "express";
import { z } from "zod";
import {
  capabilityIdSchema,
  initiatedBySchema,
  requesterSchema,
  taskConstraintsSchema,
  taskInputSchema,
  taskRoutingSchema,
  taskStatusSchema,
  type Task,
} from "@allternit/subscription-fabric-contracts";
import { needsResolution, resolveForNewTask } from "../router/dispatch.js";
import {
  getActiveThreadMapping,
  getTask,
  getTaskByIdempotency,
  insertTask,
  listTaskSummaries,
  recordRouteRejections,
  updateTaskStatus,
} from "../store/queries.js";
import { callerOf, requireScope, type GatewayDeps } from "./server.js";

const submitTaskSchema = z.object({
  capability: capabilityIdSchema,
  prompt: z.string().min(1),
  inputs: z.array(taskInputSchema).optional(),
  options: z.record(z.unknown()).optional(),
  routing: taskRoutingSchema.partial().optional(),
  constraints: taskConstraintsSchema.partial().optional(),
  priority: z.enum(["interactive", "normal", "background"]).optional(),
  thread_id: z.string().nullable().optional(),
  project_id: z.string().nullable().optional(),
  parent_task_id: z.string().nullable().optional(),
  idempotency_key: z.string().nullable().optional(),
  requester_kind: requesterSchema.shape.kind.optional(),
  // D16 — checked separately so a missing stamp answers 403, not 400.
  initiated_by: z.unknown().optional(),
});

const LIST_DEFAULT_LIMIT = 20;
const LIST_MAX_LIMIT = 100;

// GET /v1/tasks?status=needs_user,running&limit=20 — newest first. Read-only
// summaries (no inputs/options/results) for Settings → Sessions Computer's
// recent-tasks and needs-user lists. `status` is a comma-separated list.
const listQuerySchema = z.object({
  status: z
    .string()
    .optional()
    .transform((raw) => (raw ? raw.split(",").map((s) => s.trim()).filter(Boolean) : []))
    .pipe(z.array(taskStatusSchema)),
  limit: z.coerce.number().int().min(1).max(LIST_MAX_LIMIT).default(LIST_DEFAULT_LIMIT),
});

export function tasksRouter(deps: GatewayDeps): Router {
  const router = Router();

  router.get("/v1/tasks", requireScope("tasks:read"), (req: Request, res: Response) => {
    const { status, limit } = req.query;
    if (Array.isArray(status) || Array.isArray(limit) || typeof status === "object" || typeof limit === "object") {
      res.status(400).json({ error: "invalid_query", detail: "status and limit may appear once" });
      return;
    }
    const parsed = listQuerySchema.safeParse({ status, limit });
    if (!parsed.success) {
      res.status(400).json({ error: "invalid_query", detail: parsed.error.issues });
      return;
    }
    const statuses = [...new Set(parsed.data.status)];
    res.json({ tasks: listTaskSummaries(deps.db, { statuses, limit: parsed.data.limit }) });
  });

  router.post("/v1/tasks", requireScope("tasks:submit"), (req: Request, res: Response) => {
    const parsed = submitTaskSchema.safeParse(req.body);
    if (!parsed.success) {
      res.status(400).json({ error: "invalid_task", detail: parsed.error.issues });
      return;
    }
    const body = parsed.data;
    const caller = callerOf(req);
    // D16 — no fabric task runs without a human act behind it. Bots, MCP and
    // schedules prepare tasks; the surface where a human confirms stamps this.
    const initiated = initiatedBySchema.safeParse(body.initiated_by);
    if (!initiated.success) {
      res.status(403).json({
        error: "initiated_by_required",
        detail: 'every task needs initiated_by {kind:"human", user_id, action_id} from the surface where a human sent or confirmed it',
      });
      return;
    }

    if (body.idempotency_key) {
      const existing = getTaskByIdempotency(deps.db, caller.caller_id, body.idempotency_key);
      if (existing) {
        res.status(200).json(existing);
        return;
      }
    }

    const now = new Date().toISOString();
    const task: Task = {
      task_id: randomUUID(),
      idempotency_key: body.idempotency_key ?? null,
      capability: body.capability,
      capability_version: 1,
      requester: { kind: body.requester_kind ?? "bot", id: caller.caller_id },
      initiated_by: initiated.data,
      thread_id: body.thread_id ?? null,
      project_id: body.project_id ?? null,
      parent_task_id: body.parent_task_id ?? null,
      prompt: body.prompt,
      inputs: body.inputs ?? [],
      options: body.options ?? {},
      routing: {
        mode: "auto",
        allow_fallback: true,
        allow_metered: false,
        allow_thread_migration: false,
        ...body.routing,
      },
      constraints: {
        sensitivity: "internal",
        deadline_at: null,
        max_metered_usd: null,
        required_export_format: null,
        ...body.constraints,
      },
      approval_id: null,
      priority: body.priority ?? "normal",
      status: "queued",
      status_detail: null,
      route_decision: null,
      attempts: [],
      result: null,
      error: null,
      created_at: now,
      updated_at: now,
      completed_at: null,
    };
    // §S6 — chat.continue on a fabric thread: resolve the provider thread,
    // the divergence fingerprint and the owning account from the active
    // mapping, and pin routing there (a thread lives in one account).
    if (task.capability === "chat.continue" && task.thread_id && task.options.provider_thread_id === undefined) {
      const mapping = getActiveThreadMapping(deps.db, task.thread_id);
      if (!mapping) {
        res.status(409).json({
          error: "thread_not_mapped",
          thread_id: task.thread_id,
          detail: "no active provider thread for this thread_id — start it with chat.create and the same thread_id",
        });
        return;
      }
      task.options = {
        ...task.options,
        provider_thread_id: mapping.provider_thread_id,
        last_turn_fingerprint: mapping.last_turn_fingerprint,
        on_divergence: task.options.on_divergence ?? mapping.on_divergence,
      };
      task.routing = { ...task.routing, provider: mapping.provider, account_id: mapping.account_id };
    }
    // P4 — pick-time routing (§A2): an unrouted auto task is resolved against
    // a fresh snapshot before enqueue; the decision persists on the task and a
    // primary pins (provider, account_id), keying the worker's scheduler lane.
    // With no registry/router wired (unit tests) behavior is unchanged.
    let routedTask = task;
    if (deps.router && deps.adapterRegistry && needsResolution(task)) {
      routedTask = resolveForNewTask(
        { db: deps.db, registry: deps.adapterRegistry, router: deps.router },
        task
      ).task;
    }
    insertTask(deps.db, routedTask);
    if (routedTask.route_decision) {
      recordRouteRejections(deps.db, routedTask.task_id, routedTask.route_decision);
    }
    deps.log.append({
      task_id: routedTask.task_id,
      kind: "task.created",
      payload: {
        task_id: routedTask.task_id,
        status: routedTask.status,
        capability: routedTask.capability,
        thread_id: routedTask.thread_id,
      },
      callers: [caller.caller_id],
    });
    // P3 — enqueue for the worker layer. With zero eligible routes the task
    // stays queued in the unrouted lane; route_decision records why.
    deps.scheduler?.enqueue(routedTask);
    res.status(201).json(routedTask);
  });

  router.get("/v1/tasks/:id", requireScope("tasks:read"), (req: Request, res: Response) => {
    const task = getTask(deps.db, req.params.id);
    if (!task) {
      res.status(404).json({ error: "task_not_found", task_id: req.params.id });
      return;
    }
    res.json(task);
  });

  router.post("/v1/tasks/:id/cancel", requireScope("tasks:submit"), (req: Request, res: Response) => {
    const task = getTask(deps.db, req.params.id);
    if (!task) {
      res.status(404).json({ error: "task_not_found", task_id: req.params.id });
      return;
    }
    if (task.status !== "queued" && task.status !== "needs_user") {
      res.status(409).json({ error: "not_cancellable", status: task.status });
      return;
    }
    deps.scheduler?.remove(task.task_id);
    const completedAt = new Date().toISOString();
    updateTaskStatus(deps.db, task.task_id, "cancelled", { completedAt });
    deps.log.append({
      task_id: task.task_id,
      kind: "task.status",
      payload: { task_id: task.task_id, status: "cancelled", thread_id: task.thread_id },
      callers: [task.requester.id],
    });
    res.json(getTask(deps.db, task.task_id));
  });

  return router;
}
