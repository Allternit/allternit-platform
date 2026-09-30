// POST /aai/call, GET /aai/providers, POST /aai/conformance/:adapterId — AAI v0.1 host.
// Auth: the gateway-wide bearer middleware (applied before this router) + scope checks below.
// Both AAI success and AAI failure answer HTTP 200 ({ok,value} | {ok:false,error}); only transport
// problems (bad body, unknown adapter) use non-200.
import { Router, type Request, type Response } from "express";
import { botExecutionBindingSchema } from "@allternit/subscription-fabric-contracts";
import { requireScope, type GatewayDeps } from "./server.js";
import { parseCredential, runWithCallScope } from "../aai/call-scope.js";
import { defaultAdaptersDir } from "../adapters/registry.js";

/**
 * allternit-api sends its binding rows as stored: SQL NULL columns arrive as JSON null (the schema has only
 * optional), and discovery (`agent.list` before a bot exists) sends a transient `{ type, vendor,
 * accountBindingId }` with no id/botId/mode/state and no adapterId (the vendor id is the adapter id).
 */
export function normalizeWireBinding(raw: unknown): unknown {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return raw;
  const b: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(raw as Record<string, unknown>)) if (v !== null) b[k] = v;
  b.id ??= "transient";
  b.botId ??= "";
  b.mode ??= "hosted";
  b.state ??= "READY";
  if (b.adapterId === undefined && typeof b.vendor === "string") b.adapterId = b.vendor;
  return b;
}

export function aaiRouter(deps: GatewayDeps): Router {
  const router = Router();
  const host = deps.aai;

  router.post("/aai/call", requireScope("tasks:submit"), async (req: Request, res: Response) => {
    if (!host) { res.status(503).json({ error: "aai_host_unavailable" }); return; }
    const body = (req.body ?? {}) as Record<string, unknown>;
    const binding = botExecutionBindingSchema.safeParse(normalizeWireBinding(body.binding));
    if (typeof body.op !== "string" || !binding.success ||
        (body.input !== undefined && (typeof body.input !== "object" || body.input === null || Array.isArray(body.input)))) {
      res.status(400).json({ error: "invalid_body", detail: "expected { op: string, binding: BotExecutionBinding, input: object }" });
      return;
    }
    // `credential` (user-owned vendor key, short-lived) is scoped to this call only and never persisted or logged.
    const credential = parseCredential(body.credential);
    const result = await runWithCallScope({ binding: binding.data, credential }, () =>
      host.call(body.op as string, binding.data, (body.input as Record<string, unknown> | undefined) ?? {}));
    res.status(200).json(result);
  });

  router.get("/aai/providers", requireScope("tasks:read"), async (_req: Request, res: Response) => {
    if (!host) { res.status(503).json({ error: "aai_host_unavailable" }); return; }
    res.json({ providers: await host.providers() });
  });

  router.post("/aai/conformance/:adapterId", requireScope("tasks:submit"), async (req: Request, res: Response) => {
    if (!host) { res.status(503).json({ error: "aai_host_unavailable" }); return; }
    // `?offline=1`: run against the adapter's shipped offline fixtures (no vendor needed).
    const offline = req.query.offline === "1" || req.query.offline === "true";
    const report = offline ? await host.offlineConformance(req.params.adapterId, defaultAdaptersDir()) : await host.conformance(req.params.adapterId);
    if (report === null) { res.status(404).json({ error: "no_offline_fixtures" }); return; }
    if (!report) { res.status(404).json({ error: "unknown_adapter" }); return; }
    res.json(report);
  });

  return router;
}
