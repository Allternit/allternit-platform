// Eval loop case routes (CONTRACT.md section 6). Run routes land in a later step.
import { Hono } from "hono"
import { describeRoute } from "@/runtime/server/openapi"
import z from "zod/v4"
import { lazy } from "@/shared/util/lazy"
import { Log } from "@/shared/util/log"
import { EvalStore } from "@/runtime/eval/store"
import { Label, QuickCheck, Surface } from "@/runtime/eval/schema"

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
          return c.json({ suites: await EvalStore.listSuites() })
        } catch (err) {
          return handle(c, err)
        }
      },
    ),
)
