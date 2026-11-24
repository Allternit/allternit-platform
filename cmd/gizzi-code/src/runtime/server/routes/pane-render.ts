import { Hono } from "hono"
import { validator } from "@/runtime/server/openapi"
import { PaneRender } from "@/runtime/integrations/pane-render"
import z from "zod/v4"
import { lazy } from "@/shared/util/lazy"

/** The app answers a pane render request (see PaneRender). */
export const PaneRenderRoutes = lazy(() =>
  new Hono()
    .get("/", async (c) => c.json(await PaneRender.list()))
    .post(
      "/:requestID/claim",
      validator("param", z.object({ requestID: z.string() })),
      validator("json", z.object({ executorId: z.string().regex(/^[A-Za-z0-9_-]{16,128}$/) })),
      async (c) => {
        const found = await PaneRender.claim({ requestID: c.req.valid("param").requestID, executorID: c.req.valid("json").executorId })
        return found ? c.json(true) : c.json({ error: "Request unavailable or already claimed" }, 409)
      },
    )
    .post(
      "/:requestID/reply",
      validator("param", z.object({ requestID: z.string() })),
      validator("json", PaneRender.Result.extend({ executorId: z.string().optional() })),
      async (c) => {
        const { requestID } = c.req.valid("param")
        const { executorId, ...result } = c.req.valid("json")
        const found = await PaneRender.reply({ requestID, result, executorID: executorId })
        return found ? c.json(true) : c.json({ error: "Unknown request" }, 404)
      },
    )
    .post(
      "/:requestID/progress",
      validator("param", z.object({ requestID: z.string() })),
      validator("json", PaneRender.ProgressDetail.extend({ executorId: z.string().optional() })),
      async (c) => {
        const { requestID } = c.req.valid("param")
        const { executorId, ...detail } = c.req.valid("json")
        const found = await PaneRender.progress({ requestID, detail, executorID: executorId })
        return found ? c.json(true) : c.json({ error: "Unknown request" }, 404)
      },
    )
    .post(
      "/:requestID/progress",
      validator("param", z.object({ requestID: z.string() })),
      validator("json", PaneRender.ProgressDetail),
      async (c) => {
        const { requestID } = c.req.valid("param")
        const found = await PaneRender.progress({ requestID, detail: c.req.valid("json") })
        return found ? c.json(true) : c.json({ error: "Unknown request" }, 404)
      },
    ),
)
