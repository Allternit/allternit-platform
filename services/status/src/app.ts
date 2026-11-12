// Worker logic: cron tick, ingest, HTTP routing. index.ts is the entry (workerd only accepts handlers as exports there).
import type { IngestPayload, ProbeResult, Status } from "../contract/types";
import { handleAdmin } from "./admin";
import { type Mailer, bindingMailer, composeEmail, planAlerts } from "./alerts";
import { type Fetcher, probeAll } from "./check";
import { renderFeed } from "./feed";
import { renderPage } from "./page";
import { type ComponentRecord, step } from "./state";
import {
  type Db,
  type StoredComponent,
  activeMaintenance,
  autoIncidentWrites,
  componentStateWrites,
  getHealth,
  getHistoryDoc,
  getIncidentsDoc,
  getStatusDoc,
  ingestWrite,
  loadTickContext,
  maintenanceExpiryWrites,
  rollupWrites,
  todayWrites,
  tickInsert,
} from "./store";
import { TARGETS, cloudflareTargets } from "./targets";

export interface Env {
  DB: Db;
  ASSETS?: { fetch(req: Request): Promise<Response> };
  ADMIN_TOKEN?: string;
  INGEST_HMAC_SECRET?: string;
  AUTO_INCIDENTS?: string;
  ALERT_BINDING?: string;
  ALERT_TO?: string;
  ALERT_FROM?: string;
  SITE_URL?: string;
  [k: string]: unknown;
}

export const INGEST_MAX_SKEW_MS = 5 * 60_000;
const INGEST_MAX_BYTES = 256 * 1024;

// ---- cron tick ----------------------------------------------------------------
export interface TickDeps {
  fetcher?: Fetcher;
  mailer?: Mailer;
}

export interface TickSummary {
  probes: number;
  transitions: { component_id: string; from: string; to: string }[];
  alerts: string | null;
  rollup: string | null;
  writes: number;
}

export async function runTick(env: Env, now: Date, deps: TickDeps = {}): Promise<TickSummary> {
  const db = env.DB;
  const ctx = await loadTickContext(db, now);
  const targets = cloudflareTargets(TARGETS);
  const results = await probeAll(targets, deps.fetcher ?? fetch);

  const prev = new Map<string, ComponentRecord>();
  for (const [id, c] of ctx.components) prev.set(id, { status: c.status, last_change: c.last_change, machine: c.machine });
  const out = step({ now, targets: TARGETS, cf: results, standby: ctx.standby, prev });

  const inMaint = activeMaintenance(ctx.openIncidents, now);
  const names = new Map(TARGETS.components.map((c) => [c.id, c.name]));
  const states: Record<string, Status> = {};
  for (const [id, rec] of out.next) {
    const s: Status = inMaint.has(id) ? "maintenance" : rec.status;
    if (s !== "unknown") states[id] = s;
  }

  // Alerts (always on).
  const alertStates = new Map(
    [...out.next].map(([id, r]) => [id, { status: r.status, alerted_at: ctx.components.get(id)?.alerted_at ?? null }]),
  );
  const plan = planAlerts(out.transitions, alertStates, names, inMaint, now);
  const autoIncidents = autoIncidentsOn(env.AUTO_INCIDENTS, now);
  let alertSubject: string | null = null;
  let applyAlerted = false;
  if (plan.items.length) {
    const mail = composeEmail(plan.items, { now, siteUrl: env.SITE_URL ?? "https://status.allternit.com", autoIncidents });
    alertSubject = mail.subject;
    const res = await (deps.mailer ?? bindingMailer(env))(mail.subject, mail.text, now);
    applyAlerted = res !== "failed"; // failed: alerted_at unchanged, so the next tick retries as a reminder
  }

  const next = new Map<string, StoredComponent>();
  for (const [id, r] of out.next) {
    const prevAlerted = ctx.components.get(id)?.alerted_at ?? null;
    const alerted_at = applyAlerted && plan.alertedAt.has(id) ? plan.alertedAt.get(id)! : prevAlerted;
    next.set(id, { ...r, alerted_at });
  }

  const writes = [
    tickInsert(db, "cloudflare", now, now.toISOString(), results, states),
    ...componentStateWrites(db, ctx.components, next, now),
    ...autoIncidentWrites(db, {
      transitions: out.transitions,
      open: ctx.openIncidents,
      next: out.next,
      names,
      inMaintenance: inMaint,
      autoIncidents,
      now,
    }),
    ...maintenanceExpiryWrites(db, ctx.openIncidents, now),
    ...(ctx.rollupDue ? rollupWrites(db, ctx.rollupDue, now) : []),
    ...todayWrites(db, now, states), // after the rollup so uptime_90d includes today
  ];
  await db.batch(writes); // one D1 batch per tick

  return { probes: results.length, transitions: out.transitions, alerts: alertSubject, rollup: ctx.rollupDue, writes: writes.length };
}

// ---- ingest (standby vantage) ---------------------------------------------------
function hexToBytes(hex: string): Uint8Array {
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  return out;
}

/** HMAC-SHA256 check of the raw body. crypto.subtle.verify compares in constant time. */
export async function verifySignature(secret: string, raw: Uint8Array, header: string | null): Promise<boolean> {
  const m = /^sha256=([0-9a-fA-F]{64})$/.exec((header ?? "").trim());
  if (!m) return false;
  const key = await crypto.subtle.importKey("raw", new TextEncoder().encode(secret), { name: "HMAC", hash: "SHA-256" }, false, [
    "verify",
  ]);
  return crypto.subtle.verify("HMAC", key, hexToBytes(m[1].toLowerCase()), raw);
}

const intOrNull = (v: unknown) => (v === null || v === undefined ? null : Number.isInteger(v) ? (v as number) : undefined);

function cleanResults(v: unknown): ProbeResult[] | null {
  if (!Array.isArray(v) || v.length > 200) return null;
  const out: ProbeResult[] = [];
  for (const r of v) {
    if (!r || typeof r !== "object") return null;
    const o = r as Record<string, unknown>;
    const sc = intOrNull(o.status_code);
    const lat = intOrNull(o.latency_ms);
    if (typeof o.target_id !== "string" || o.target_id.length > 100 || typeof o.ok !== "boolean") return null;
    if (sc === undefined || lat === undefined) return null;
    out.push({
      target_id: o.target_id,
      ok: o.ok,
      status_code: sc,
      latency_ms: lat,
      marker_ok: typeof o.marker_ok === "boolean" ? o.marker_ok : null,
      error: typeof o.error === "string" ? o.error.slice(0, 200) : null,
    });
  }
  return out;
}

export async function handleIngest(req: Request, env: Env, now: Date = new Date()): Promise<Response> {
  if (!env.INGEST_HMAC_SECRET) return plain(503, "ingest is not configured");
  const raw = new Uint8Array(await req.arrayBuffer());
  if (raw.byteLength > INGEST_MAX_BYTES) return plain(413, "body too large");
  if (!(await verifySignature(env.INGEST_HMAC_SECRET, raw, req.headers.get("x-allternit-signature")))) {
    return plain(401, "bad signature");
  }
  let p: Partial<IngestPayload>;
  try {
    p = JSON.parse(new TextDecoder().decode(raw));
  } catch {
    return plain(400, "body is not JSON");
  }
  if (p.schema !== "allternit.ingest/v1" || p.vantage !== "allternit-standby") return plain(400, "bad schema or vantage");
  const ts = typeof p.ts === "string" ? Date.parse(p.ts) : NaN;
  if (!Number.isFinite(ts) || Math.abs(now.getTime() - ts) > INGEST_MAX_SKEW_MS) return plain(400, "ts missing or more than 5 min off");
  const results = cleanResults(p.results);
  if (!results) return plain(400, "bad results");
  await ingestWrite(env.DB, new Date(ts).toISOString(), results).run();
  return json(202, { ok: true, results: results.length }, false);
}

// ---- HTTP helpers ---------------------------------------------------------------
const CORS = { "access-control-allow-origin": "*" };
function json(status: number, data: unknown, publicGet = true, cache = "public, max-age=30"): Response {
  return new Response(JSON.stringify(data), {
    status,
    headers: {
      "content-type": "application/json; charset=utf-8",
      ...(publicGet ? { ...CORS, "cache-control": cache } : { "cache-control": "no-store" }),
    },
  });
}
function plain(status: number, text: string): Response {
  return new Response(text, { status, headers: { "content-type": "text/plain; charset=utf-8", "cache-control": "no-store" } });
}

// History changes once a day; keep it in the isolate for a few minutes.
const historyMemo = new Map<string, { at: number; doc: unknown }>();
async function history(env: Env, days: number, now: Date) {
  const key = `${days}:${now.toISOString().slice(0, 10)}`;
  const hit = historyMemo.get(key);
  if (hit && now.getTime() - hit.at < 5 * 60_000) return hit.doc as Awaited<ReturnType<typeof getHistoryDoc>>;
  const doc = await getHistoryDoc(env.DB, TARGETS, days, now);
  historyMemo.set(key, { at: now.getTime(), doc });
  return doc;
}

async function publicGet(req: Request, env: Env, now: Date): Promise<Response | null> {
  const url = new URL(req.url);
  switch (url.pathname) {
    case "/api/v1/status.json":
      return json(200, await getStatusDoc(env.DB, TARGETS, now));
    case "/api/v1/history.json": {
      const d = Number(url.searchParams.get("days") ?? 90);
      if (!Number.isInteger(d) || d < 1 || d > 400) return json(400, { error: "days must be 1..400" });
      return json(200, await history(env, d, now));
    }
    case "/api/v1/incidents.json":
      return json(200, await getIncidentsDoc(env.DB));
    case "/api/v1/targets.json":
      return json(200, TARGETS);
    case "/api/health": {
      const h = await getHealth(env.DB, now);
      return json(h.status === "ok" ? 200 : 503, h, true, "no-store");
    }
    case "/feed.xml": {
      const xml = renderFeed(await getIncidentsDoc(env.DB), { now: now.toISOString(), siteUrl: env.SITE_URL });
      return new Response(xml, {
        headers: { "content-type": "application/rss+xml; charset=utf-8", "cache-control": "public, max-age=30", ...CORS },
      });
    }
    case "/":
    case "/index.html": {
      const [status, hist, inc] = await Promise.all([
        getStatusDoc(env.DB, TARGETS, now),
        history(env, 90, now),
        getIncidentsDoc(env.DB),
      ]);
      return new Response(renderPage(status, hist, inc, { now: now.toISOString() }), {
        headers: { "content-type": "text/html; charset=utf-8", "cache-control": "public, max-age=30" },
      });
    }
  }
  return null;
}

export async function handleFetch(req: Request, env: Env, ctx?: ExecutionContext, now: Date = new Date()): Promise<Response> {
  const url = new URL(req.url);
  const path = url.pathname;

  if (path === "/api/v1/ingest") {
    return req.method === "POST" ? handleIngest(req, env, now) : plain(405, "POST only");
  }
  if (path.startsWith("/api/admin/")) return handleAdmin(req, env, TARGETS, now);

  if (req.method === "OPTIONS") {
    return new Response(null, {
      status: 204,
      headers: { ...CORS, "access-control-allow-methods": "GET, HEAD, OPTIONS", "access-control-max-age": "86400" },
    });
  }
  if (req.method !== "GET" && req.method !== "HEAD") return plain(405, "method not allowed");

  // Edge cache for public GETs (30 s), except the freshness check.
  const cache = typeof caches !== "undefined" && path !== "/api/health" ? (caches as unknown as { default: Cache }).default : null;
  if (cache) {
    const hit = await cache.match(req);
    if (hit) return hit;
  }
  const res = await publicGet(req, env, now);
  if (res) {
    if (cache && res.status === 200 && ctx) ctx.waitUntil(cache.put(req, res.clone()));
    return res;
  }
  if (path === "/admin" && env.ASSETS) {
    return env.ASSETS.fetch(new Request(new URL("/admin.html", url), req));
  }
  return plain(404, "not found");
}

/**
 * AUTO_INCIDENTS: "on", "off", or an ISO 8601 time from which it is on (the end of a soak),
 * so the switch flips on schedule without a redeploy. Anything else counts as off.
 */
export function autoIncidentsOn(value: string | undefined, now: Date): boolean {
  const v = (value ?? "off").trim();
  if (v.toLowerCase() === "on") return true;
  if (!/^\d{4}-\d{2}-\d{2}T/.test(v)) return false;
  const from = Date.parse(v);
  return Number.isFinite(from) && now.getTime() >= from;
}
