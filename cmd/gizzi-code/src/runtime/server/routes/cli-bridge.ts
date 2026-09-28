import { Hono } from "hono"
import { CliBridge } from "@/runtime/integrations/cli-bridge"
import { lazy } from "@/shared/util/lazy"

/** MCP over HTTP for a CLI agent running a session turn (see CliBridge). */
export const CliBridgeRoutes = lazy(() =>
  new Hono()
    .post("/:sessionID/mcp", async (c) => {
      if (!CliBridge.authorized(c.req.header(CliBridge.TOKEN_HEADER))) return c.json({ error: "Unauthorized" }, 401)
      let body: any
      try {
        body = await c.req.json()
      } catch {
        return c.json({ jsonrpc: "2.0", id: null, error: { code: -32700, message: "Parse error" } }, 400)
      }
      const reply = await CliBridge.handle(c.req.param("sessionID"), body, c.req.raw.signal)
      return reply === null ? c.body(null, 202) : c.json(reply)
    })
    // Stateless server: no server-initiated stream, no sessions to end.
    .get("/:sessionID/mcp", (c) => c.body(null, 405))
    .delete("/:sessionID/mcp", (c) => c.body(null, 405)),
)
