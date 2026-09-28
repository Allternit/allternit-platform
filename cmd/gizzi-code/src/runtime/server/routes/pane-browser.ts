import { Hono } from "hono"
import { validator } from "@/runtime/server/openapi"
import { PaneBrowser } from "@/runtime/integrations/pane-browser"
import z from "zod/v4"
import { lazy } from "@/shared/util/lazy"

/** The app answers a pane browser request (see PaneBrowser). */
export const PaneBrowserRoutes = lazy(() =>
  new Hono()
    .get("/", async (c) => c.json(await PaneBrowser.list()))
    .post(
      "/:requestID/reply",
      validator("param", z.object({ requestID: z.string() })),
      validator("json", PaneBrowser.Result),
      async (c) => {
        const { requestID } = c.req.valid("param")
        const found = await PaneBrowser.reply({ requestID, result: c.req.valid("json") })
        return found ? c.json(true) : c.json({ error: "Unknown request" }, 404)
      },
    ),
)
