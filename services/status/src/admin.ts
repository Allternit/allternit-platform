// Admin API: bearer token (ADMIN_TOKEN secret), constant-time compare, failed-login
// rate limit per IP, an audit row for every action.
//
//   GET   /api/admin/incidents              all incidents incl. drafts (admin view)
//   POST  /api/admin/incidents              AdminIncidentCreate      -> open an incident
//   PATCH /api/admin/incidents/:id          AdminIncidentPatch       -> update / resolve (sets auto=false)
//   POST  /api/admin/incidents/:id/publish  publish an auto-incident draft
//   POST  /api/admin/maintenance            AdminMaintenanceCreate   -> schedule maintenance
import type { AdminIncidentCreate, AdminIncidentPatch, IncidentStatus, TargetsDoc } from "../contract/types";
import {
  type Db,
  auditWrite,
  countAuthFails,
  createIncidentWrites,
  getAdminIncidents,
  getIncidentRow,
  latestUpdateStatus,
  newIncidentId,
  patchIncidentWrites,
  publishDraftWrite,
} from "./store";
import { ADMIN_DEFAULT_BODY } from "./wording";

/** Not in contract v1: schedule a maintenance window. */
export interface AdminMaintenanceCreate {
  schema: "allternit.admin.maintenance.create/v1";
  title: string;
  component_ids: string[];
  body: string;
  starts_at: string;
  ends_at: string;
}

export const AUTH_FAIL_LIMIT = 10;
export const AUTH_FAIL_WINDOW_MS = 15 * 60_000;
const MAX_BODY_BYTES = 32 * 1024;

export interface AdminEnv {
  DB: Db;
  ADMIN_TOKEN?: string;
}

const json = (status: number, data: unknown) =>
  new Response(JSON.stringify(data), {
    status,
    headers: { "content-type": "application/json; charset=utf-8", "cache-control": "no-store" },
  });
const bad = (msg: string) => json(400, { error: msg });

async function digest(s: string): Promise<Uint8Array> {
  return new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(s)));
}

/** Constant-time token check (compares fixed-length digests, every byte). */
export async function tokenMatches(given: string, expected: string): Promise<boolean> {
  const [a, b] = await Promise.all([digest(given), digest(expected)]);
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a[i] ^ b[i];
  return diff === 0;
}

const STATUSES: IncidentStatus[] = ["investigating", "identified", "monitoring", "resolved"];
const isStr = (v: unknown, max: number): v is string => typeof v === "string" && v.trim().length > 0 && v.length <= max;
const isIso = (v: unknown): v is string => typeof v === "string" && /^\d{4}-\d{2}-\d{2}T/.test(v) && Number.isFinite(Date.parse(v));

function validComponents(ids: unknown, targets: TargetsDoc): ids is string[] {
  const known = new Set(targets.components.map((c) => c.id));
  return Array.isArray(ids) && ids.length > 0 && ids.length <= 20 && ids.every((x) => typeof x === "string" && known.has(x));
}

export async function handleAdmin(req: Request, env: AdminEnv, targets: TargetsDoc, now: Date = new Date()): Promise<Response> {
  const url = new URL(req.url);
  const ip = req.headers.get("cf-connecting-ip") ?? "unknown";
  if (!env.ADMIN_TOKEN) return json(503, { error: "admin is not configured" });

  const fails = await countAuthFails(env.DB, ip, new Date(now.getTime() - AUTH_FAIL_WINDOW_MS));
  if (fails >= AUTH_FAIL_LIMIT) return json(429, { error: "too many failed attempts, try again later" });

  const auth = req.headers.get("authorization") ?? "";
  const m = /^Bearer\s+(.+)$/i.exec(auth);
  if (!m || !(await tokenMatches(m[1].trim(), env.ADMIN_TOKEN))) {
    await auditWrite(env.DB, now, "auth_fail", url.pathname, { method: req.method }, ip, "anonymous").run();
    return json(401, { error: "unauthorized" });
  }

  const path = url.pathname.replace(/\/+$/, "");
  const parts = path.split("/").filter(Boolean); // api, admin, ...

  let body: unknown = undefined;
  if (req.method === "POST" || req.method === "PATCH") {
    const raw = await req.text();
    if (raw.length > MAX_BODY_BYTES) return json(413, { error: "body too large" });
    if (raw.trim()) {
      try {
        body = JSON.parse(raw);
      } catch {
        return bad("body is not JSON");
      }
    }
  }
  const b = (body ?? {}) as Record<string, unknown>;

  // GET /api/admin/incidents
  if (req.method === "GET" && parts.length === 3 && parts[2] === "incidents") {
    return json(200, { incidents: await getAdminIncidents(env.DB) });
  }

  // POST /api/admin/incidents
  if (req.method === "POST" && parts.length === 3 && parts[2] === "incidents") {
    const c = b as Partial<AdminIncidentCreate>;
    if (c.schema !== "allternit.admin.incident.create/v1") return bad("schema must be allternit.admin.incident.create/v1");
    if (!isStr(c.title, 200)) return bad("title is required (max 200)");
    if (!c.impact || !["degraded", "down", "maintenance"].includes(c.impact)) return bad("impact must be degraded, down or maintenance");
    if (!validComponents(c.component_ids, targets)) return bad("component_ids must list known components");
    if (!isStr(c.body, 5000)) return bad("body is required (max 5000)");
    if (c.status && !["investigating", "identified", "monitoring"].includes(c.status)) return bad("bad status");
    const id = newIncidentId("inc", now);
    await env.DB.batch([
      ...createIncidentWrites(env.DB, {
        id,
        title: c.title.trim(),
        impact: c.impact,
        component_ids: c.component_ids,
        status: c.status ?? "investigating",
        body: c.body.trim(),
        now,
        kind: c.impact === "maintenance" ? "maintenance" : "incident",
      }),
      auditWrite(env.DB, now, "incident.create", id, c, ip),
    ]);
    return json(201, { id });
  }

  // POST /api/admin/maintenance
  if (req.method === "POST" && parts.length === 3 && parts[2] === "maintenance") {
    const c = b as Partial<AdminMaintenanceCreate>;
    if (c.schema !== "allternit.admin.maintenance.create/v1") return bad("schema must be allternit.admin.maintenance.create/v1");
    if (!isStr(c.title, 200)) return bad("title is required (max 200)");
    if (!validComponents(c.component_ids, targets)) return bad("component_ids must list known components");
    if (!isStr(c.body, 5000)) return bad("body is required (max 5000)");
    if (!isIso(c.starts_at) || !isIso(c.ends_at)) return bad("starts_at and ends_at must be ISO 8601 times");
    const starts = new Date(c.starts_at).toISOString();
    const ends = new Date(c.ends_at).toISOString();
    if (ends <= starts) return bad("ends_at must be after starts_at");
    if (ends <= now.toISOString()) return bad("ends_at is in the past");
    const id = newIncidentId("mnt", now);
    await env.DB.batch([
      ...createIncidentWrites(env.DB, {
        id,
        title: c.title.trim(),
        impact: "maintenance",
        component_ids: c.component_ids,
        status: "identified",
        body: c.body.trim(),
        now,
        kind: "maintenance",
        started_at: starts,
        ends_at: ends,
      }),
      auditWrite(env.DB, now, "maintenance.create", id, c, ip),
    ]);
    return json(201, { id });
  }

  // /api/admin/incidents/:id[/publish]
  if (parts.length >= 4 && parts[2] === "incidents") {
    const id = decodeURIComponent(parts[3]);
    const row = await getIncidentRow(env.DB, id);
    if (!row) return json(404, { error: "no such incident" });

    if (req.method === "POST" && parts.length === 5 && parts[4] === "publish") {
      if (!row.draft) return json(409, { error: "incident is not a draft" });
      await env.DB.batch([publishDraftWrite(env.DB, id, now), auditWrite(env.DB, now, "incident.publish", id, undefined, ip)]);
      return json(200, { id, published: true });
    }

    if (req.method === "PATCH" && parts.length === 4) {
      const p = b as Partial<AdminIncidentPatch>;
      if (p.schema !== "allternit.admin.incident.patch/v1") return bad("schema must be allternit.admin.incident.patch/v1");
      if (p.title !== undefined && !isStr(p.title, 200)) return bad("title must be 1-200 characters");
      if (p.status !== undefined && !STATUSES.includes(p.status)) return bad("bad status");
      if (p.body !== undefined && !isStr(p.body, 5000)) return bad("body must be 1-5000 characters");
      if (p.component_ids !== undefined && !validComponents(p.component_ids, targets)) return bad("component_ids must list known components");
      if (p.title === undefined && p.status === undefined && p.body === undefined && p.component_ids === undefined) return bad("nothing to change");
      let text: string | null = p.body?.trim() ?? null;
      let status = p.status;
      if (text !== null && !status) status = (await latestUpdateStatus(env.DB, id)) ?? "investigating";
      if (text === null && p.status) text = ADMIN_DEFAULT_BODY[p.status];
      await env.DB.batch([
        ...patchIncidentWrites(env.DB, row, { title: p.title?.trim(), component_ids: p.component_ids, status }, text, now),
        auditWrite(env.DB, now, "incident.update", id, p, ip),
      ]);
      return json(200, { id, auto: false });
    }
  }

  return json(404, { error: "not found" });
}
