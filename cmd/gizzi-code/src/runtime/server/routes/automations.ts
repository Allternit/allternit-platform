import { Hono } from "hono"
import { lazy } from "@/shared/util/lazy"
import { Database } from "@/runtime/session/storage/db"
import { RoutineTable, LoopTable, GoalTable } from "@/runtime/session/session.sql"
import { eq } from "drizzle-orm"
import { RoutineEngine } from "@/runtime/automation/routine-engine"
import { LoopEngine } from "@/runtime/automation/loop-engine"
import { GoalEngine } from "@/runtime/automation/goal-engine"
import { describeRoute, validator, resolver } from "@/runtime/server/openapi"
import { errors } from "@/runtime/server/error"
import z from "zod/v4"
import crypto from "crypto"

const RoutineCreateSchema = z.object({
  id: z.string().optional(),
  agent_id: z.string().optional(),
  name: z.string(),
  steps: z.array(z.object({ command: z.string(), status: z.string() })).default([]),
  trigger: z.string().optional(),
  schedule: z.string().optional(),
})

const RoutineUpdateSchema = z.object({
  agent_id: z.string().optional(),
  name: z.string().optional(),
  steps: z.array(z.object({ command: z.string(), status: z.string() })).optional(),
  trigger: z.string().optional(),
  schedule: z.string().optional(),
  state: z.string().optional(),
})

const LoopMetricSchema = z.object({
  command: z.string().min(1),
  direction: z.enum(["lower", "higher"]),
  keepBar: z.number().min(0).default(0.05),
  noiseRuns: z.number().int().min(2).max(20).default(5),
  minPairs: z.number().int().min(2).max(50).default(5),
  stallLimit: z.number().int().min(1).max(20).optional(),
})

const LoopCreateSchema = z.object({
  metric: LoopMetricSchema.optional(),
  cwd: z.string().optional(),
  id: z.string().optional(),
  agent_id: z.string().optional(),
  command: z.string(),
  exit_condition: z.string().optional(),
  max_iterations: z.number().default(10),
})

const LoopUpdateSchema = z.object({
  metric: LoopMetricSchema.nullable().optional(),
  cwd: z.string().nullable().optional(),
  agent_id: z.string().optional(),
  command: z.string().optional(),
  exit_condition: z.string().optional(),
  max_iterations: z.number().optional(),
  state: z.string().optional(),
})

const GoalCreateSchema = z.object({
  id: z.string().optional(),
  agent_id: z.string().optional(),
  objective: z.string().min(1),
  completion_criterion: z.string().min(1).optional(),
  budget: z.object({
    turnBudget: z.number().int().positive().nullable().optional(),
    tokenBudget: z.number().int().positive().nullable().optional(),
    wallClockBudgetMs: z.number().int().positive().nullable().optional(),
  }).optional(),
  enqueue: z.boolean().optional(),
  replace: z.boolean().optional(),
})

const GoalUpdateSchema = z.object({
  objective: z.string().min(1).optional(),
  completion_criterion: z.string().min(1).nullable().optional(),
})

const GoalQueueOrderSchema = z.object({
  agent_id: z.string().nullable().optional(),
  goalIDs: z.array(z.string()),
})

const GoalBudgetSchema = z.object({
  turnBudget: z.number().int().positive().nullable().optional(),
  tokenBudget: z.number().int().positive().nullable().optional(),
  wallClockBudgetMs: z.number().int().positive().nullable().optional(),
})

const GoalContinuationSchema = z.object({
  tokensUsed: z.number().int().nonnegative().optional(),
  blocker: z.string().min(1).optional(),
})

const GoalMilestoneSchema = z.object({
  name: z.string().min(1),
  status: z.enum(["pending", "in_progress", "completed", "failed"]),
  completedAt: z.string().optional(),
})

const GoalValidationSchema = z.object({
  testName: z.string().min(1),
  status: z.enum(["passed", "failed"]),
  output: z.string().optional(),
})

const IdParamSchema = z.object({
  id: z.string(),
})

export const AutomationsRoutes = lazy(() => {
  GoalEngine.initialize()
  return new Hono()
    // Routines CRUD
    .get(
      "/routines",
      describeRoute({
        summary: "List routines",
        description: "Get all automation routines",
        operationId: "automation.routines.list",
        responses: {
          200: {
            description: "List of routines",
            content: { "application/json": { schema: resolver(z.any()) } },
          },
        },
      }),
      async (c) => {
        const rows = Database.use((db) => db.select().from(RoutineTable).all())
        return c.json(rows)
      },
    )
    .post(
      "/routines",
      describeRoute({
        summary: "Create routine",
        description: "Create a new automation routine",
        operationId: "automation.routines.create",
        responses: {
          201: {
            description: "Routine created",
            content: { "application/json": { schema: resolver(z.any()) } },
          },
          ...errors(400),
        },
      }),
      validator("json", RoutineCreateSchema),
      async (c) => {
        const body = c.req.valid("json")
        const routine = {
          id: body.id || crypto.randomUUID(),
          agent_id: body.agent_id || null,
          name: body.name,
          steps: body.steps || [],
          trigger: body.trigger || null,
          schedule: body.schedule || null,
          state: "defined",
          time_created: Date.now(),
          time_updated: Date.now(),
        }
        Database.use((db) => db.insert(RoutineTable).values(routine as any).run())
        return c.json(routine, 201)
      },
    )
    .put(
      "/routines/:id",
      describeRoute({
        summary: "Update routine",
        description: "Update an existing automation routine",
        operationId: "automation.routines.update",
        responses: {
          200: { description: "Routine updated", content: { "application/json": { schema: resolver(z.any()) } } },
          ...errors(404),
        },
      }),
      validator("param", IdParamSchema),
      validator("json", RoutineUpdateSchema),
      async (c) => {
        const { id } = c.req.valid("param")
        const body = c.req.valid("json")
        Database.use((db) =>
          db
            .update(RoutineTable)
            .set({
              ...body,
              time_updated: Date.now(),
            } as any)
            .where(eq(RoutineTable.id, id))
            .run()
        )
        return c.json({ success: true })
      },
    )
    .delete(
      "/routines/:id",
      describeRoute({
        summary: "Delete routine",
        description: "Delete an automation routine",
        operationId: "automation.routines.delete",
        responses: {
          200: { description: "Routine deleted", content: { "application/json": { schema: resolver(z.any()) } } },
          ...errors(404),
        },
      }),
      validator("param", IdParamSchema),
      async (c) => {
        const { id } = c.req.valid("param")
        Database.use((db) => db.delete(RoutineTable).where(eq(RoutineTable.id, id)).run())
        return c.json({ success: true })
      },
    )
    .post(
      "/routines/:id/run",
      describeRoute({
        summary: "Run routine",
        description: "Start executing an automation routine",
        operationId: "automation.routines.run",
        responses: {
          200: { description: "Routine started", content: { "application/json": { schema: resolver(z.any()) } } },
          ...errors(404),
        },
      }),
      validator("param", IdParamSchema),
      async (c) => {
        const { id } = c.req.valid("param")
        Database.use((db) =>
          db
            .update(RoutineTable)
            .set({ state: "running", time_updated: Date.now() })
            .where(eq(RoutineTable.id, id))
            .run()
        )
        RoutineEngine.startRoutine(id).catch(console.error)
        return c.json({ success: true, state: "running" })
      },
    )

    // Loops CRUD
    .get(
      "/loops",
      describeRoute({
        summary: "List loops",
        description: "Get all automation loops",
        operationId: "automation.loops.list",
        responses: {
          200: { description: "List of loops", content: { "application/json": { schema: resolver(z.any()) } } },
        },
      }),
      async (c) => {
        const rows = Database.use((db) => db.select().from(LoopTable).all())
        return c.json(rows)
      },
    )
    .post(
      "/loops",
      describeRoute({
        summary: "Create loop",
        description: "Create a new automation loop",
        operationId: "automation.loops.create",
        responses: {
          201: { description: "Loop created", content: { "application/json": { schema: resolver(z.any()) } } },
          ...errors(400),
        },
      }),
      validator("json", LoopCreateSchema),
      async (c) => {
        const body = c.req.valid("json")
        const loop = {
          id: body.id || crypto.randomUUID(),
          agent_id: body.agent_id || null,
          command: body.command,
          exit_condition: body.exit_condition || null,
          max_iterations: body.max_iterations,
          iteration_log: [],
          metric: body.metric ?? null,
          ledger: [],
          cwd: body.cwd ?? null,
          last_outcome: null,
          state: "running",
          time_created: Date.now(),
          time_updated: Date.now(),
        }
        Database.use((db) => db.insert(LoopTable).values(loop).run())
        LoopEngine.startLoop(loop.id).catch(console.error)
        return c.json(loop, 201)
      },
    )
    .put(
      "/loops/:id",
      describeRoute({
        summary: "Update loop",
        description: "Update an existing automation loop",
        operationId: "automation.loops.update",
        responses: {
          200: { description: "Loop updated", content: { "application/json": { schema: resolver(z.any()) } } },
          ...errors(404),
        },
      }),
      validator("param", IdParamSchema),
      validator("json", LoopUpdateSchema),
      async (c) => {
        const { id } = c.req.valid("param")
        const body = c.req.valid("json")
        Database.use((db) =>
          db
            .update(LoopTable)
            .set({
              ...body,
              time_updated: Date.now(),
            })
            .where(eq(LoopTable.id, id))
            .run()
        )
        return c.json({ success: true })
      },
    )
    .delete(
      "/loops/:id",
      describeRoute({
        summary: "Delete loop",
        description: "Delete an automation loop",
        operationId: "automation.loops.delete",
        responses: {
          200: { description: "Loop deleted", content: { "application/json": { schema: resolver(z.any()) } } },
          ...errors(404),
        },
      }),
      validator("param", IdParamSchema),
      async (c) => {
        const { id } = c.req.valid("param")
        Database.use((db) => db.delete(LoopTable).where(eq(LoopTable.id, id)).run())
        return c.json({ success: true })
      },
    )
    .post(
      "/loops/:id/run",
      describeRoute({
        summary: "Run loop",
        description: "Restart an automation loop",
        operationId: "automation.loops.run",
        responses: {
          200: { description: "Loop started", content: { "application/json": { schema: resolver(z.any()) } } },
          ...errors(404),
        },
      }),
      validator("param", IdParamSchema),
      async (c) => {
        const { id } = c.req.valid("param")
        if (c.req.query("dry_run") === "1" || c.req.query("dry_run") === "true") {
          // Measures once and reports; the loop record and the tree stay untouched.
          return c.json({ dry_run: true, ...(await LoopEngine.dryRun(id)) })
        }
        Database.use((db) =>
          db
            .update(LoopTable)
            .set({ state: "running", time_updated: Date.now() })
            .where(eq(LoopTable.id, id))
            .run()
        )
        LoopEngine.startLoop(id).catch(console.error)
        return c.json({ success: true, state: "running" })
      },
    )

    // Goals CRUD
    .get(
      "/goals",
      describeRoute({
        summary: "List goals",
        description: "Get all automation goals",
        operationId: "automation.goals.list",
        responses: {
          200: { description: "List of goals", content: { "application/json": { schema: resolver(z.any()) } } },
        },
      }),
      async (c) => {
        const rows = Database.use((db) => db.select().from(GoalTable).all())
        return c.json(rows)
      },
    )
    .post(
      "/goals",
      describeRoute({
        summary: "Create goal",
        description: "Create a new automation goal",
        operationId: "automation.goals.create",
        responses: {
          201: { description: "Goal created", content: { "application/json": { schema: resolver(z.any()) } } },
          ...errors(400),
        },
      }),
      validator("json", GoalCreateSchema),
      async (c) => {
        const body = c.req.valid("json")
        const id = body.id || crypto.randomUUID()
        const result = GoalEngine.createGoal({
          id,
          agentId: body.agent_id,
          objective: body.objective,
          completionCriterion: body.completion_criterion,
          budget: body.budget,
          enqueue: body.enqueue,
          replace: body.replace,
        })
        if (!result.ok) return c.json({ success: false, error: result.reason }, 409)
        return c.json({ id, objective: body.objective, success: true, ...result }, 201)
      },
    )
    .put(
      "/goals/:id",
      describeRoute({
        summary: "Update goal",
        description: "Update an existing automation goal",
        operationId: "automation.goals.update",
        responses: {
          200: { description: "Goal updated", content: { "application/json": { schema: resolver(z.any()) } } },
          ...errors(404),
        },
      }),
      validator("param", IdParamSchema),
      validator("json", GoalUpdateSchema),
      async (c) => {
        const { id } = c.req.valid("param")
        const body = c.req.valid("json")
        const result = GoalEngine.updateDetails(id, {
          objective: body.objective,
          completionCriterion: body.completion_criterion,
        })
        if (!result.ok) return c.json({ success: false, error: result.reason }, result.reason === "Goal not found" ? 404 : 409)
        return c.json({ success: true, ...result })
      },
    )
    .delete(
      "/goals/:id",
      describeRoute({
        summary: "Delete goal",
        description: "Delete an automation goal",
        operationId: "automation.goals.delete",
        responses: {
          200: { description: "Goal deleted", content: { "application/json": { schema: resolver(z.any()) } } },
          ...errors(404),
        },
      }),
      validator("param", IdParamSchema),
      async (c) => {
        const { id } = c.req.valid("param")
        const result = GoalEngine.deleteGoal(id)
        if (!result.ok) return c.json({ success: false, error: result.reason }, 404)
        return c.json({ success: true, ...result })
      },
    )
    .post(
      "/goals/queue/reorder",
      describeRoute({
        summary: "Reorder queued goals",
        description: "Replace the complete queue order for one goal owner",
        operationId: "automation.goals.queue.reorder",
        responses: { 200: { description: "Goal queue reordered", content: { "application/json": { schema: resolver(z.any()) } } }, ...errors(409) },
      }),
      validator("json", GoalQueueOrderSchema),
      async (c) => {
        const body = c.req.valid("json")
        const result = GoalEngine.reorderQueue(body.agent_id ?? null, body.goalIDs)
        if (!result.ok) return c.json({ success: false, error: result.reason }, 409)
        return c.json({ success: true, ...result })
      },
    )
    .post(
      "/goals/:id/run",
      describeRoute({
        summary: "Run goal",
        description: "Restart an automation goal",
        operationId: "automation.goals.run",
        responses: {
          200: { description: "Goal started", content: { "application/json": { schema: resolver(z.any()) } } },
          ...errors(404),
        },
      }),
      validator("param", IdParamSchema),
      async (c) => {
        const { id } = c.req.valid("param")
        const result = GoalEngine.startGoal(id)
        if (!result.ok) return c.json({ success: false, error: result.reason }, result.reason === "Goal not found" ? 404 : 409)
        return c.json({ success: true, ...result })
      },
    )
    .post(
      "/goals/:id/pause",
      describeRoute({
        summary: "Pause goal",
        description: "Pause a running automation goal without discarding progress",
        operationId: "automation.goals.pause",
        responses: { 200: { description: "Goal paused", content: { "application/json": { schema: resolver(z.any()) } } }, ...errors(404) },
      }),
      validator("param", IdParamSchema),
      async (c) => {
        const { id } = c.req.valid("param")
        const result = GoalEngine.pauseGoal(id)
        if (!result.ok) return c.json({ success: false, error: result.reason }, result.reason === "Goal not found" ? 404 : 409)
        return c.json({ success: true, ...result })
      },
    )
    .post(
      "/goals/:id/budget",
      describeRoute({
        summary: "Set goal budget",
        description: "Set or clear hard turn, token, and wall-clock budgets",
        operationId: "automation.goals.budget.set",
        responses: { 200: { description: "Goal budget updated", content: { "application/json": { schema: resolver(z.any()) } } }, ...errors(404, 409) },
      }),
      validator("param", IdParamSchema),
      validator("json", GoalBudgetSchema),
      async (c) => {
        const { id } = c.req.valid("param")
        const result = GoalEngine.setBudget(id, c.req.valid("json"))
        if (!result.ok) return c.json({ success: false, error: result.reason }, result.reason === "Goal not found" ? 404 : 409)
        return c.json({ success: true, ...result })
      },
    )
    .post(
      "/goals/:id/continuations",
      describeRoute({
        summary: "Record goal continuation",
        description: "Account an autonomous turn, enforce budgets, and audit repeated blockers",
        operationId: "automation.goals.continuations.record",
        responses: { 200: { description: "Goal continuation recorded", content: { "application/json": { schema: resolver(z.any()) } } }, ...errors(404, 409) },
      }),
      validator("param", IdParamSchema),
      validator("json", GoalContinuationSchema),
      async (c) => {
        const { id } = c.req.valid("param")
        const result = GoalEngine.recordContinuation(id, c.req.valid("json"))
        if (!result.ok) return c.json({ success: false, error: result.reason }, result.reason === "Goal not found" ? 404 : 409)
        return c.json({ success: true, ...result })
      },
    )
    .post(
      "/goals/:id/milestones",
      describeRoute({
        summary: "Publish goal milestone",
        description: "Create or update a milestone using real orchestrator progress",
        operationId: "automation.goals.milestones.publish",
        responses: { 200: { description: "Milestone recorded", content: { "application/json": { schema: resolver(z.any()) } } }, ...errors(404, 409) },
      }),
      validator("param", IdParamSchema),
      validator("json", GoalMilestoneSchema),
      async (c) => {
        const { id } = c.req.valid("param")
        const result = await GoalEngine.recordMilestone(id, c.req.valid("json"))
        if (!result.ok) return c.json({ success: false, error: result.reason }, result.reason === "Goal not found" ? 404 : 409)
        return c.json({ success: true, ...result })
      },
    )
    .post(
      "/goals/:id/validations",
      describeRoute({
        summary: "Publish goal validation",
        description: "Create or update validation evidence from an actual check",
        operationId: "automation.goals.validations.publish",
        responses: { 200: { description: "Validation recorded", content: { "application/json": { schema: resolver(z.any()) } } }, ...errors(404, 409) },
      }),
      validator("param", IdParamSchema),
      validator("json", GoalValidationSchema),
      async (c) => {
        const { id } = c.req.valid("param")
        const result = await GoalEngine.recordValidation(id, c.req.valid("json"))
        if (!result.ok) return c.json({ success: false, error: result.reason }, result.reason === "Goal not found" ? 404 : 409)
        return c.json({ success: true, ...result })
      },
    )
    .post(
      "/goals/:id/block",
      describeRoute({
        summary: "Block goal",
        description: "Mark a goal blocked pending external input or state change",
        operationId: "automation.goals.block",
        responses: { 200: { description: "Goal blocked", content: { "application/json": { schema: resolver(z.any()) } } }, ...errors(404) },
      }),
      validator("param", IdParamSchema),
      async (c) => {
        const { id } = c.req.valid("param")
        const result = GoalEngine.blockGoal(id)
        if (!result.ok) return c.json({ success: false, error: result.reason }, result.reason === "Goal not found" ? 404 : 409)
        return c.json({ success: true, ...result })
      },
    )
    .post(
      "/goals/:id/complete",
      describeRoute({
        summary: "Complete goal",
        description: "Complete a goal after its evidence has been audited",
        operationId: "automation.goals.complete",
        responses: { 200: { description: "Goal completed", content: { "application/json": { schema: resolver(z.any()) } } }, ...errors(404) },
      }),
      validator("param", IdParamSchema),
      async (c) => {
        const { id } = c.req.valid("param")
        const result = await GoalEngine.completeGoal(id)
        if (!result.ok) return c.json({ success: false, error: result.reason }, result.reason === "Goal not found" ? 404 : 409)
        return c.json({ success: true, ...result })
      },
    )
})
