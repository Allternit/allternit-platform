import { Hono } from "hono"
import { validator } from "@/runtime/server/openapi"
import { PaneArtifact } from "@/runtime/integrations/pane-artifact"
import z from "zod/v4"
import { lazy } from "@/shared/util/lazy"

/** The app answers a pane artifact request (see PaneArtifact). */
export const PaneArtifactRoutes = lazy(() =>
  new Hono()
    .get("/", async (c) => c.json(await PaneArtifact.list()))
    .post(
      "/:requestID/reply",
      validator("param", z.object({ requestID: z.string() })),
      validator("json", PaneArtifact.Result),
      async (c) => {
        const { requestID } = c.req.valid("param")
        const found = await PaneArtifact.reply({ requestID, result: c.req.valid("json") })
        return found ? c.json(true) : c.json({ error: "Unknown request" }, 404)
      },
    ),
)
