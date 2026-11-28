// Eval loop routes: cases, suites and runs (CONTRACT.md section 6).
import { Hono } from "hono"
import { describeRoute } from "@/runtime/server/openapi"
import z from "zod/v4"
import { lazy } from "@/shared/util/lazy"
import { Log } from "@/shared/util/log"
import { EvalStore } from "@/runtime/eval/store"
import { Label, QuickCheck, Surface } from "@/runtime/eval/schema"
import { EvalRunner } from "@/runtime/eval/runner"
import { estimate as makeEstimate, loadSuite } from "@/runtime/eval/estimate"
import * as Jobs from "@/runtime/eval/jobs"
import { mulberry32 } from "@/runtime/eval/stats"

const log = Log.create({ service: "eval-routes" })

const SaveBody = z.object({
  session_id: z.string().min(1),
  label: Label,
  note: z.string().optional(),
  through_message_id: z.string().optional(),
  quick_check: z.array(QuickCheck).optional(),
  suite: z.string().optional(),
})

const ListQuery = z.object({ session_id: z.string().optional(), suite: z.string().optional() })

const EstimateBody = z.object({
  suite: z.string().min(1),
  case_ids: z.array(z.string()).optional(),
  k: z.number().int().min(1).max(20).optional(),
  model: z.string().optional(),
})

const RunBody = EstimateBody.extend({
  mode: z.enum(["replay", "live"]),
  estimate_id: z.string().optional(),
  fraction: z.number().gt(0).max(1).optional(),
  seed: z.number().int().optional(),
})

const RunsQuery = z.object({ suite: z.string().optional() })

const STATUS = {
  session_not_found: 404,
  case_not_found: 404,
  case_limit: 409,
  empty_session: 422,
} as const

function fail(c: any, error: string, message: string, status: number) {
  return c.json({ error, message }, status)
}

function invalid(c: any, err: z.ZodError) {
  const first = err.issues[0]
  const where = first?.path.join(".") || "request"
  return fail(c, "invalid_request", `Check "${where}" and try again.`, 400)
}

function handle(c: any, err: unknown) {
  if (err instanceof Jobs.JobError) return fail(c, err.code, err.message, err.code === "run_not_found" ? 404 : 409)
  if (err instanceof EvalStore.EvalError) {
    const status = (STATUS as Record<string, number>)[err.code]
    if (status) return fail(c, err.code, err.message, status)
  }
  log.error("eval route failed", { error: String(err) })
  return fail(c, "internal", "Couldn't save the test case.", 500)
}

function surfaceOf(c: any) {
  const parsed = Surface.safeParse(c.req.header("x-allternit-device-platform")?.toLowerCase())
  return parsed.success ? parsed.data : "web"
}

export const EvalRoutes = lazy(() =>
  new Hono()
    .post(
      "/cases",
      describeRoute({
        summary: "Save a chat as a test case",
        operationId: "eval.case.create",
        responses: { 201: { description: "Saved case" } },
      }),
      async (c) => {
        const body = SaveBody.safeParse(await c.req.json().catch(() => undefined))
        if (!body.success) return invalid(c, body.error)
        try {
          const saved = await EvalStore.saveCase({
            sessionID: body.data.session_id,
            label: body.data.label,
            note: body.data.note,
            throughMessageID: body.data.through_message_id,
            quickCheck: body.data.quick_check,
            suite: body.data.suite,
            surface: surfaceOf(c),
          })
          return c.json(saved, 201)
        } catch (err) {
          return handle(c, err)
        }
      },
    )
    .get(
      "/cases",
      describeRoute({
        summary: "List test cases",
        operationId: "eval.case.list",
        responses: { 200: { description: "Cases" } },
      }),
      async (c) => {
        const q = ListQuery.safeParse(c.req.query())
        if (!q.success) return invalid(c, q.error)
        try {
          return c.json({ cases: await EvalStore.listCases({ sessionID: q.data.session_id, suite: q.data.suite }) })
        } catch (err) {
          return handle(c, err)
        }
      },
    )
    .get(
      "/cases/:id",
      describeRoute({
        summary: "Get one test case",
        operationId: "eval.case.get",
        responses: { 200: { description: "Case" } },
      }),
      async (c) => {
        try {
          return c.json({ case: await EvalStore.getCase(c.req.param("id")) })
        } catch (err) {
          return handle(c, err)
        }
      },
    )
    .delete(
      "/cases/:id",
      describeRoute({
        summary: "Delete a test case",
        operationId: "eval.case.delete",
        responses: { 204: { description: "Deleted" } },
      }),
      async (c) => {
        try {
          await EvalStore.deleteCase(c.req.param("id"))
          return c.body(null, 204)
        } catch (err) {
          return handle(c, err)
        }
      },
    )
    .get(
      "/suites",
      describeRoute({
        summary: "List test suites",
        operationId: "eval.suite.list",
        responses: { 200: { description: "Suites" } },
      }),
      async (c) => {
        try {
          const suites = await EvalStore.listSuites()
          return c.json({
            suites: await Promise.all(
              suites.map(async (s) => {
                const last = await Jobs.lastRun(s.id)
                return last ? { ...s, last_run: last } : s
              }),
            ),
          })
        } catch (err) {
          return handle(c, err)
        }
      },
    )
    .post(
      "/runs/estimate",
      describeRoute({
        summary: "Price a run before it starts",
        operationId: "eval.run.estimate",
        responses: { 200: { description: "Estimate" } },
      }),
      async (c) => {
        const body = EstimateBody.safeParse(await c.req.json().catch(() => undefined))
        if (!body.success) return invalid(c, body.error)
        try {
          const e = await makeEstimate({ suite: body.data.suite, caseIds: body.data.case_ids, k: body.data.k, model: body.data.model })
          const { suite: _s, case_ids: _c, model: _m, ...out } = e
          return c.json(out)
        } catch (err) {
          return handle(c, err)
        }
      },
    )
    .post(
      "/runs",
      describeRoute({
        summary: "Start a run",
        operationId: "eval.run.start",
        responses: { 200: { description: "Replay report" }, 202: { description: "Live run started" } },
      }),
      async (c) => {
        const body = RunBody.safeParse(await c.req.json().catch(() => undefined))
        if (!body.success) return invalid(c, body.error)
        const b = body.data
        try {
          if (b.mode === "live") {
            // fraction is not part of the price, so live runs use the cases the estimate named.
            const { run_id } = await Jobs.start({ suite: b.suite, caseIds: b.case_ids, k: b.k, model: b.model, estimateId: b.estimate_id, seed: b.seed })
            return c.json({ run_id }, 202)
          }
          let caseIds = b.case_ids
          if (b.fraction && b.fraction < 1) {
            const all = (await loadSuite({ suite: b.suite, caseIds })).cases.map((x) => x.id)
            const rnd = mulberry32(b.seed ?? 1)
            const order = all.map((id) => ({ id, r: rnd() })).sort((x, y) => x.r - y.r)
            caseIds = order.slice(0, Math.max(1, Math.ceil(all.length * b.fraction))).map((x) => x.id)
          }
          const report = await EvalRunner.replay({ suite: b.suite, caseIds, seed: b.seed })
          const run_id = await Jobs.recordReplay(report, b.suite)
          return c.json({ run_id, report })
        } catch (err) {
          return handle(c, err)
        }
      },
    )
    .get(
      "/runs",
      describeRoute({
        summary: "List runs, newest first",
        operationId: "eval.run.list",
        responses: { 200: { description: "Runs" } },
      }),
      async (c) => {
        const q = RunsQuery.safeParse(c.req.query())
        if (!q.success) return invalid(c, q.error)
        try {
          const runs = await Jobs.list(q.data.suite)
          return c.json({
            runs: runs.map((r) => {
              const pr = Jobs.passRateOf(r)
              return { run_id: r.run_id, mode: r.mode, state: r.state, ...(pr !== undefined ? { pass_rate: pr } : {}), started_at: r.started_at }
            }),
          })
        } catch (err) {
          return handle(c, err)
        }
      },
    )
    .get(
      "/runs/:id",
      describeRoute({
        summary: "Get one run",
        operationId: "eval.run.get",
        responses: { 200: { description: "Run" } },
      }),
      async (c) => {
        try {
          const { suite: _s, ...v } = await Jobs.get(c.req.param("id"))
          return c.json(v)
        } catch (err) {
          return handle(c, err)
        }
      },
    )
    .post(
      "/runs/:id/cancel",
      describeRoute({
        summary: "Cancel a live run",
        operationId: "eval.run.cancel",
        responses: { 202: { description: "Cancelling" } },
      }),
      async (c) => {
        const id = c.req.param("id")
        try {
          if (!Jobs.cancel(id)) await Jobs.get(id)
        } catch (err) {
          return handle(c, err)
        }
        return c.body(null, 202)
      },
    ),
)
