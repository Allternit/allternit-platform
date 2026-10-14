// POST /aai/call, GET /aai/events/stream (SSE), GET /aai/providers, POST /aai/conformance/:adapterId — AAI v0.1 host.
// Auth: the gateway-wide bearer middleware (applied before this router) + scope checks below.
// Both AAI success and AAI failure answer HTTP 200 ({ok,value} | {ok:false,error}); only transport
// problems (bad body, unknown adapter) use non-200.
import { Router, type Request, type Response } from "express";
import { botExecutionBindingSchema } from "@allternit/subscription-fabric-contracts";
import { requireScope, type GatewayDeps } from "./server.js";
import { parseCredential, runWithCallScope } from "../aai/call-scope.js";
import { defaultAdaptersDir } from "../adapters/registry.js";

/**
 * Which adapter serves a vendor when a binding names none. Accounts and wizard bindings carry the vendor
 * library id (`xai`, `anthropic`, `openai`, ...), never an adapter id, so without this every such call
 * answered UNSUPPORTED "no provider registered for adapter xai". Anthropic has two adapters: a user's own
 * API key means Claude Managed Agents, otherwise the Claude desktop app.
 */
export function adapterForVendor(vendor: string, hasCredential: boolean): string {
  switch (vendor) {
    case "xai": case "grok": return "grok-bot";
    case "openai": case "chatgpt": return "chatgpt-dots";
    case "anthropic": case "claude": return hasCredential ? "claude-managed-agents" : "claude-desktop";
    default: return vendor; // openclaw, and bindings that already name their adapter as the vendor
  }
}

/**
 * allternit-api sends its binding rows as stored: SQL NULL columns arrive as JSON null (the schema has only
 * optional), and discovery (`agent.list` before a bot exists) sends a transient `{ type, vendor,
 * accountBindingId }` with no id/botId/mode/state and no adapterId (resolved from the vendor above).
 */
export function normalizeWireBinding(raw: unknown, hasCredential = false): unknown {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return raw;
  const b: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(raw as Record<string, unknown>)) if (v !== null) b[k] = v;
  b.id ??= "transient";
  b.botId ??= "";
  b.mode ??= "hosted";
  b.state ??= "READY";
  if (b.adapterId === undefined && typeof b.vendor === "string") b.adapterId = adapterForVendor(b.vendor, hasCredential);
  return b;
}

export function aaiRouter(deps: GatewayDeps): Router {
  const router = Router();
  const host = deps.aai;

  router.post("/aai/call", requireScope("tasks:submit"), async (req: Request, res: Response) => {
    if (!host) { res.status(503).json({ error: "aai_host_unavailable" }); return; }
    const body = (req.body ?? {}) as Record<string, unknown>;
    const binding = botExecutionBindingSchema.safeParse(normalizeWireBinding(body.binding, body.credential != null));
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

  /**
   * SSE over `agent.events`: `event: aai` / `id: <cursor>` / `data: <wire event>` per event, resuming after the
   * `Last-Event-ID` header (else `?cursor`). The provider is polled every pollMs; `: hb` heartbeats keep proxies open.
   * A failed read is sent as `event: error` (`data: AAIError`); a retryable one keeps polling, any other ends the stream.
   * Binding: `?binding=<json>` (same shape as /aai/call), else the binding the context was opened with, else
   * `?adapterId` alone.
   */
  router.get("/aai/events/stream", requireScope("tasks:read"), async (req: Request, res: Response) => {
    if (!host) { res.status(503).json({ error: "aai_host_unavailable" }); return; }
    const q = req.query as Record<string, unknown>;
    const contextId = typeof q.contextId === "string" ? q.contextId : "";
    let rawBinding: unknown;
    if (typeof q.binding === "string") {
      try { rawBinding = JSON.parse(q.binding); } catch { rawBinding = undefined; }
    } else if (typeof q.adapterId === "string" && q.adapterId) {
      rawBinding = host.contexts?.get(q.adapterId, contextId)?.binding ?? { id: "sse", type: "vendor", adapterId: q.adapterId };
    }
    const binding = botExecutionBindingSchema.safeParse(normalizeWireBinding(rawBinding));
    if (!contextId || !binding.success || (typeof q.adapterId === "string" && binding.data.adapterId !== q.adapterId)) {
      res.status(400).json({ error: "invalid_query", detail: "expected ?contextId and ?adapterId (or ?binding=<BotExecutionBinding json>)" });
      return;
    }
    const lastEventId = req.header("last-event-id");
    let cursor: string | undefined = lastEventId || (typeof q.cursor === "string" && q.cursor ? q.cursor : undefined);
    const pollMs = deps.aaiStream?.pollMs ?? 500;
    const heartbeatMs = deps.aaiStream?.heartbeatMs ?? 15_000;

    res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache", connection: "keep-alive", "x-accel-buffering": "no" });
    res.write(`retry: ${Math.max(1000, pollMs * 2)}\n: connected\n\n`);
    let closed = false;
    let timer: NodeJS.Timeout | undefined;
    const heartbeat = setInterval(() => { if (!closed) res.write(": hb\n\n"); }, heartbeatMs);
    const stop = () => { if (closed) return; closed = true; clearInterval(heartbeat); if (timer) clearTimeout(timer); res.end(); };
    req.on("close", stop);

    const poll = async (): Promise<void> => {
      if (closed) return;
      const r = await host.call("agent.events", binding.data, { contextId, ...(cursor !== undefined ? { cursor } : {}) });
      if (closed) return;
      let wait = pollMs;
      if (r.ok) {
        const v = r.value as { events: Array<Record<string, unknown> & { cursor: string }>; cursor?: string };
        for (const e of v.events) res.write(`id: ${e.cursor}\nevent: aai\ndata: ${JSON.stringify(e)}\n\n`);
        if (v.events.length) cursor = v.events[v.events.length - 1].cursor;
        else if (typeof v.cursor === "string" && v.cursor) cursor = v.cursor;
      } else {
        res.write(`event: error\ndata: ${JSON.stringify(r.error)}\n\n`);
        if (!r.error.retryable) { stop(); return; }
        wait = Math.max(pollMs, r.error.retryAfterMs ?? 0);
      }
      timer = setTimeout(() => { void poll(); }, wait);
    };
    void poll();
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
