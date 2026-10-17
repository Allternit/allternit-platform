// GET /v1/routes + PUT /v1/routes/:vendor/override — the Settings API for the
// deterministic route router (spec §6). Shape is a contract: another task
// builds the Settings UI against exactly this JSON. Auth: the gateway-wide
// bearer middleware plus the Settings scope (accounts:manage), same as
// /v1/accounts.
import { Router, type Request, type Response } from "express";
import { z } from "zod";
import { requireScope, type GatewayDeps } from "./server.js";

const overrideSchema = z
  .object({ route: z.string().nullable() })
  .strict();

export function routesRouter(deps: GatewayDeps): Router {
  const router = Router();
  const director = () => deps.routeDirector;

  router.get("/v1/routes", requireScope("accounts:manage"), async (_req: Request, res: Response) => {
    if (!director()) { res.status(503).json({ error: "route_router_unavailable" }); return; }
    res.json({ vendors: await director()!.status() });
  });

  // { route: <id> | null } — null clears the pin. 400 when the route is not
  // eligible for the vendor (§4.6: the pin still has to be eligible).
  router.put("/v1/routes/:vendor/override", requireScope("accounts:manage"), async (req: Request, res: Response) => {
    if (!director()) { res.status(503).json({ error: "route_router_unavailable" }); return; }
    const parsed = overrideSchema.safeParse(req.body ?? {});
    if (!parsed.success) {
      res.status(400).json({ error: "invalid_body", detail: parsed.error.issues });
      return;
    }
    const result = await director()!.setOverride(req.params.vendor, parsed.data.route);
    if (!result.ok) {
      res.status(result.status).json(result.body);
      return;
    }
    const view = await director()!.status([req.params.vendor]);
    res.json(view[0] ?? { vendor: req.params.vendor, routes: [] });
  });

  return router;
}
