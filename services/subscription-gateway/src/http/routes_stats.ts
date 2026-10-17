// GET /v1/stats/adapters and GET /v1/stats/rejections?limit= — P4 Phase 2
// observability. Query-only, computed on read. Scoped tasks:read (the data is
// attempt/routing history — same sensitivity class as task reads).
import { Router, type Request, type Response } from "express";
import { getBreaker } from "../breakers.js";
import { adapterStats, recentRejections } from "../observability/stats.js";
import { latestCanaryRun, listAccounts } from "../store/queries.js";
import { latestEvidenceId } from "./routes_evidence.js";
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

  // Per connected account: session health, the canary's last probe and last pass,
  // and the adapter's drift breaker. `state` is what a person should see:
  // green (ready), amber (slow / busy), red (needs a person or the provider is down).
  router.get("/v1/lanes", requireScope("tasks:read"), (_req: Request, res: Response) => {
    const lanes = listAccounts(deps.db)
      .filter((a) => a.enabled)
      .map((a) => {
        const adapter = deps.adapterRegistry?.adapters.find((l) => l.manifest.provider === a.provider)?.manifest;
        const breaker = adapter ? getBreaker(deps.db, adapter.adapter_id, adapter.adapter_version) : null;
        const lastProbe = latestCanaryRun(deps.db, a.account_id);
        const lastOk = latestCanaryRun(deps.db, a.account_id, "pass");
        const breakerOpen = breaker?.state === "open";
        const health = a.session_health;
        const state =
          breakerOpen || (health !== "ready" && health !== "degraded" && health !== "profile_locked")
            ? "red"
            : health === "ready"
              ? "green"
              : "amber";
        return {
          account_id: a.account_id,
          provider: a.provider,
          label: a.label,
          health,
          state,
          breaker: breaker ? { state: breaker.state, opened_at: breaker.opened_at } : null,
          lastProbeAt: lastProbe?.ended_at ?? null,
          lastOkAt: lastOk?.ended_at ?? null,
          lastProbeOutcome: lastProbe?.outcome ?? null,
          latency_ms: lastProbe?.latency_ms ?? null,
          evidence_ref: state === "red" && adapter ? latestEvidenceId(deps.db, adapter.adapter_id) : null,
        };
      });
    res.json({ lanes });
  });

  return router;
}
