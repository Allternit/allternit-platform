import { Hono } from "hono"
import { validator } from "@/runtime/server/openapi"
import { ScreenContext } from "@/runtime/integrations/screen-context"
import z from "zod/v4"
import { lazy } from "@/shared/util/lazy"

/** The app answers a screen context request (see ScreenContext). */
export const ScreenContextRoutes = lazy(() =>
  new Hono()
    .get("/", async (c) => c.json(await ScreenContext.list()))
    .post(
      "/:requestID/reply",
      validator("param", z.object({ requestID: z.string() })),
      validator("json", ScreenContext.Reply),
      async (c) => {
        const { requestID } = c.req.valid("param")
        const found = await ScreenContext.reply({ requestID, reply: c.req.valid("json") })
        return found ? c.json(true) : c.json({ error: "Unknown request" }, 404)
      },
    ),
)
