import { Hono } from "hono"
import { validator } from "@/runtime/server/openapi"
import { ShellBridge, ShellResult } from "@/runtime/tools/builtins/shell"
import z from "zod/v4"
import { lazy } from "@/shared/util/lazy"

/**
 * The app answers a `shell` tool request (ShellBridge, AllternitOS step 13):
 * the ShellCommandV1 result or a ShellStateV1. Same shape as PaneBrowserRoutes.
 * A body that is not a contract result still settles the request as
 * `bad_request`, so the tool does not wait out its 30 s timeout.
 */
export const ShellRoutes = lazy(() =>
  new Hono().post(
    "/:requestID/reply",
    validator("param", z.object({ requestID: z.string() })),
    async (c) => {
      const { requestID } = c.req.valid("param")
      const body = await c.req.json().catch(() => undefined)
      const parsed = ShellResult.safeParse(body)
      const result: ShellResult = parsed.success ? parsed.data : { ok: false, error: "bad_request" }
      const found = await ShellBridge.reply({ requestID, result })
      if (!found) return c.json({ error: "Unknown request" }, 404)
      return parsed.success ? c.json(true) : c.json({ error: "Not a ShellCommandV1 result" }, 400)
    },
  ),
)
