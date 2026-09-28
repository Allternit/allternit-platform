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
      "/:requestID/reply",
      validator("param", z.object({ requestID: z.string() })),
      validator("json", PaneRender.Result),
      async (c) => {
        const { requestID } = c.req.valid("param")
        const found = await PaneRender.reply({ requestID, result: c.req.valid("json") })
        return found ? c.json(true) : c.json({ error: "Unknown request" }, 404)
      },
    ),
)
