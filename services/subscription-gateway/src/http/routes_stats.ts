// GET /v1/stats/adapters and GET /v1/stats/rejections?limit= — P4 Phase 2
// observability. Query-only, computed on read. Scoped tasks:read (the data is
// attempt/routing history — same sensitivity class as task reads).
import { Router, type Request, type Response } from "express";
import { adapterStats, recentRejections } from "../observability/stats.js";
import { requireScope, type GatewayDeps } from "./server.js";

const DEFAULT_REJECTIONS_LIMIT = 50;
const MAX_REJECTIONS_LIMIT = 500;

export function statsRouter(deps: GatewayDeps): Router {
  const router = Router();

  router.get("/v1/stats/adapters", requireScope("tasks:read"), (req: Request, res: Response) => {
    const since = typeof req.query.since === "string" ? req.query.since : undefined;
    res.json(adapterStats(deps.db, since ? { since } : {}));
  });

  router.get("/v1/stats/rejections", requireScope("tasks:read"), (req: Request, res: Response) => {
    const raw = typeof req.query.limit === "string" ? Number(req.query.limit) : NaN;
    const limit = Number.isFinite(raw) && raw > 0
      ? Math.min(Math.floor(raw), MAX_REJECTIONS_LIMIT)
      : DEFAULT_REJECTIONS_LIMIT;
    res.json(recentRejections(deps.db, limit));
  });

  return router;
}
