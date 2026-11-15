// D1 access. All SQL lives here. Reads are batched; each cron tick writes one batch.
import type {
  HealthDoc,
  HistoryDay,
  HistoryDoc,
  Incident,
  IncidentImpact,
  IncidentStatus,
  IncidentsDoc,
  IncidentUpdate,
  ProbeResult,
  Status,
  StatusDoc,
  TargetsDoc,
} from "../contract/types";
import type { ComponentRecord, MachineStatus, StandbyReport, Transition } from "./state";
import { emptyMachine, overallOf } from "./state";
import { AUTO_INCIDENT, MAINTENANCE } from "./wording";

// ---- minimal D1 surface (structurally compatible with D1Database) ----------
export interface Stmt {
  bind(...values: unknown[]): Stmt;
  all<T = Record<string, unknown>>(): Promise<{ results: T[] }>;
  first<T = Record<string, unknown>>(): Promise<T | null>;
  run(): Promise<unknown>;
}
export interface Db {
  prepare(sql: string): Stmt;
  batch<T = Record<string, unknown>>(stmts: Stmt[]): Promise<{ results: T[] }[]>;
}

export const TICKS_KEEP_DAYS = 14;
export const DAILY_KEEP_DAYS = 400;
export const ROLLUP_AFTER_MIN = 5; // 00:05 UTC
export const HEALTH_STALE_S = 300;
export const NEVER = "1970-01-01T00:00:00.000Z";

export const minuteKey = (d: Date) => d.toISOString().slice(0, 16);
export const dayKey = (d: Date) => d.toISOString().slice(0, 10);
export const addDays = (d: Date, n: number) => new Date(d.getTime() + n * 86_400_000);

// ---- rows -------------------------------------------------------------------
export interface IncidentRow {
  id: string;
  title: string;
  state: "open" | "resolved";
  impact: IncidentImpact;
  component_ids: string;
  started_at: string;
  resolved_at: string | null;
  auto: number;
  origin: "auto" | "admin";
  draft: number;
  kind: "incident" | "maintenance";
  ends_at: string | null;
  created_at: string;
  updated_at: string;
}
interface UpdateRow {
  id: number;
  incident_id: string;
  ts: string;
  status: IncidentStatus;
  body: string;
}
interface StateRow {
  component_id: string;
  status: MachineStatus;
  last_change: string | null;
  machine: string;
  alerted_at: string | null;
  uptime_90d: number | null;
}

export interface StoredComponent extends ComponentRecord {
  alerted_at: string | null;
}

export function newIncidentId(prefix: "inc" | "mnt", now: Date): string {
  const hex = Array.from(crypto.getRandomValues(new Uint8Array(3)), (b) => b.toString(16).padStart(2, "0")).join("");
  return `${prefix}-${dayKey(now).replace(/-/g, "")}-${hex}`;
}

/** Maintenance windows in effect at `now` (published, started, not ended). */
export function activeMaintenance(rows: IncidentRow[], now: Date): Set<string> {
  const ids = new Set<string>();
  const t = now.toISOString();
  for (const r of rows) {
    if (r.kind !== "maintenance" || r.state !== "open" || r.draft) continue;
    if (r.started_at > t || (r.ends_at && r.ends_at <= t)) continue;
    for (const c of JSON.parse(r.component_ids) as string[]) ids.add(c);
  }
  return ids;
}

// ---- tick: read ---------------------------------------------------------------
export interface TickContext {
  components: Map<string, StoredComponent>;
  standby: StandbyReport[];
  openIncidents: IncidentRow[];
  rollupDue: string | null; // the date to roll up, or null
}

export async function loadTickContext(db: Db, now: Date): Promise<TickContext> {
  const yesterday = dayKey(addDays(now, -1));
  const [states, standby, open, rolled] = await db.batch([
    db.prepare("SELECT component_id, status, last_change, machine, alerted_at, uptime_90d FROM component_state"),
    db.prepare("SELECT ts, results FROM ticks WHERE vantage = 'allternit-standby' ORDER BY minute DESC LIMIT 3"),
    db.prepare("SELECT * FROM incidents WHERE state = 'open'"),
    db.prepare("SELECT count(*) AS n FROM audit WHERE action = 'rollup' AND target = ?").bind(yesterday),
  ]);
  const components = new Map<string, StoredComponent>();
  for (const r of states.results as unknown as StateRow[]) {
    let machine = emptyMachine();
    try {
      machine = { ...emptyMachine(), ...JSON.parse(r.machine) };
    } catch {
      /* corrupt row: start over */
    }
    components.set(r.component_id, { status: r.status, last_change: r.last_change, machine, alerted_at: r.alerted_at });
  }
  const sb: StandbyReport[] = [];
  for (const r of standby.results as unknown as { ts: string; results: string }[]) {
    try {
      sb.push({ ts: r.ts, results: JSON.parse(r.results) as ProbeResult[] });
    } catch {
      /* skip */
    }
  }
  const minutesToday = now.getUTCHours() * 60 + now.getUTCMinutes();
  const done = Number((rolled.results[0] as { n?: number } | undefined)?.n ?? 0) > 0;
  return {
    components,
    standby: sb,
    openIncidents: open.results as unknown as IncidentRow[],
    rollupDue: !done && minutesToday >= ROLLUP_AFTER_MIN ? yesterday : null,
  };
}

// ---- tick: write --------------------------------------------------------------
export function tickInsert(db: Db, vantage: string, now: Date, ts: string, results: ProbeResult[], states: Record<string, Status> | null): Stmt {
  return db
    .prepare("INSERT OR REPLACE INTO ticks (vantage, minute, ts, results, states) VALUES (?, ?, ?, ?, ?)")
    .bind(vantage, minuteKey(now), ts, JSON.stringify(results), states ? JSON.stringify(states) : null);
}

/** Upserts only rows that changed (keeps D1 writes near one row per tick when all is well). */
export function componentStateWrites(
  db: Db,
  prev: Map<string, StoredComponent>,
  next: Map<string, StoredComponent>,
  now: Date,
): Stmt[] {
  const out: Stmt[] = [];
  for (const [id, n] of next) {
    const p = prev.get(id);
    const same =
      p &&
      p.status === n.status &&
      p.last_change === n.last_change &&
      p.alerted_at === n.alerted_at &&
      JSON.stringify(p.machine) === JSON.stringify(n.machine);
    if (same) continue;
    out.push(
      db
        .prepare(
          `INSERT INTO component_state (component_id, status, last_change, machine, alerted_at, updated_at)
           VALUES (?, ?, ?, ?, ?, ?)
           ON CONFLICT (component_id) DO UPDATE SET status = excluded.status, last_change = excluded.last_change,
             machine = excluded.machine, alerted_at = excluded.alerted_at, updated_at = excluded.updated_at`,
        )
        .bind(id, n.status, n.last_change, JSON.stringify(n.machine), n.alerted_at, now.toISOString()),
    );
  }
  return out;
}

function addUpdate(db: Db, incidentId: string, ts: string, status: IncidentStatus, body: string, source: "auto" | "admin"): Stmt {
  return db
    .prepare("INSERT INTO incident_updates (incident_id, ts, status, body, source) VALUES (?, ?, ?, ?, ?)")
    .bind(incidentId, ts, status, body, source);
}

/**
 * Automatic incidents from this tick's transitions.
 * - auto=1 incidents get created, escalated, improved and resolved by automation.
 * - auto=0 (a human edited it): automation only appends the "resolved" update.
 * - Incidents opened by a human (origin=admin) are never touched by automation.
 * - AUTO_INCIDENTS off: new incidents are drafts (admin-only) until published.
 */
export function autoIncidentWrites(
  db: Db,
  args: {
    transitions: Transition[];
    open: IncidentRow[];
    next: Map<string, ComponentRecord>;
    names: Map<string, string>;
    inMaintenance: Set<string>;
    autoIncidents: boolean;
    now: Date;
  },
): Stmt[] {
  const { transitions, open, next, names, inMaintenance, autoIncidents, now } = args;
  const ts = now.toISOString();
  const out: Stmt[] = [];
  const autoOpen = open.filter((r) => r.origin === "auto" && r.kind === "incident");
  const comps = (r: IncidentRow) => JSON.parse(r.component_ids) as string[];
  const resolved = new Set<string>();
  const name = (id: string) => names.get(id) ?? id;

  for (const t of transitions) {
    if (inMaintenance.has(t.component_id)) continue;
    const mine = autoOpen.filter((r) => comps(r).includes(t.component_id) && !resolved.has(r.id));
    if (t.to === "down" || t.to === "degraded") {
      if (mine.length === 0) {
        const id = newIncidentId("inc", now);
        const n = name(t.component_id);
        const title = t.to === "down" ? AUTO_INCIDENT.downTitle(n) : AUTO_INCIDENT.degradedTitle(n);
        const body = t.to === "down" ? AUTO_INCIDENT.downBody(n) : AUTO_INCIDENT.degradedBody(n);
        out.push(
          db
            .prepare(
              `INSERT INTO incidents (id, title, state, impact, component_ids, started_at, resolved_at, auto, origin, draft, kind, ends_at, created_at, updated_at)
               VALUES (?, ?, 'open', ?, ?, ?, NULL, 1, 'auto', ?, 'incident', NULL, ?, ?)`,
            )
            .bind(id, title, t.to, JSON.stringify([t.component_id]), ts, autoIncidents ? 0 : 1, ts, ts),
          addUpdate(db, id, ts, "investigating", body, "auto"),
        );
        continue;
      }
      for (const r of mine) {
        if (!r.auto) continue;
        if (t.to === "down" && r.impact !== "down") {
          out.push(
            db.prepare("UPDATE incidents SET impact = 'down', updated_at = ? WHERE id = ?").bind(ts, r.id),
            addUpdate(db, r.id, ts, "investigating", AUTO_INCIDENT.escalatedBody(name(t.component_id)), "auto"),
          );
        } else if (t.to === "degraded" && t.from === "down") {
          out.push(addUpdate(db, r.id, ts, "monitoring", AUTO_INCIDENT.improvedBody(name(t.component_id)), "auto"));
        }
      }
    } else if (t.to === "operational") {
      for (const r of mine) {
        const all = comps(r);
        if (!all.every((c) => next.get(c)?.status === "operational" || next.get(c)?.status === "unknown")) continue;
        resolved.add(r.id);
        const names2 = all.map(name).join(" and ");
        out.push(
          db.prepare("UPDATE incidents SET state = 'resolved', resolved_at = ?, updated_at = ? WHERE id = ?").bind(ts, ts, r.id),
          addUpdate(db, r.id, ts, "resolved", AUTO_INCIDENT.resolvedBody(names2), "auto"),
        );
      }
    }
  }
  return out;
}

/** Close maintenance windows whose end time has passed. */
export function maintenanceExpiryWrites(db: Db, open: IncidentRow[], now: Date): Stmt[] {
  const ts = now.toISOString();
  const out: Stmt[] = [];
  for (const r of open) {
    if (r.kind !== "maintenance" || !r.ends_at || r.ends_at > ts) continue;
    out.push(
      db.prepare("UPDATE incidents SET state = 'resolved', resolved_at = ?, updated_at = ? WHERE id = ?").bind(r.ends_at, ts, r.id),
      addUpdate(db, r.id, r.ends_at, "resolved", MAINTENANCE.completeBody, "auto"),
    );
  }
  return out;
}

/**
 * Daily rollup for `date` (YYYY-MM-DD, UTC) from the Cloudflare ticks, done in SQL so the
 * Worker spends no CPU on it. Day status = worst status seen. Uptime = ticks not down /
 * ticks counted (maintenance and unknown are not counted). Also refreshes uptime_90d and
 * prunes ticks (14 d), daily (400 d) and audit (400 d).
 */
export function rollupWrites(db: Db, date: string, now: Date): Stmt[] {
  const start = `${date}T00:00`;
  const end = `${dayKey(addDays(new Date(`${date}T00:00:00Z`), 1))}T00:00`;
  const from90 = dayKey(addDays(new Date(`${date}T00:00:00Z`), -89));
  return [
    db
      .prepare(
        `INSERT OR REPLACE INTO daily (component_id, date, status, uptime, checks, up_checks)
         SELECT j.key, ?1,
           CASE MAX(CASE j.value WHEN 'down' THEN 4 WHEN 'degraded' THEN 3 WHEN 'maintenance' THEN 2 WHEN 'operational' THEN 1 ELSE 0 END)
             WHEN 4 THEN 'down' WHEN 3 THEN 'degraded' WHEN 2 THEN 'maintenance' WHEN 1 THEN 'operational' ELSE NULL END,
           CASE WHEN SUM(j.value IN ('operational', 'degraded', 'down')) = 0 THEN NULL
             ELSE CAST(SUM(j.value IN ('operational', 'degraded')) AS REAL) / SUM(j.value IN ('operational', 'degraded', 'down')) END,
           SUM(j.value IN ('operational', 'degraded', 'down')),
           SUM(j.value IN ('operational', 'degraded'))
         FROM ticks AS t, json_each(t.states) AS j
         WHERE t.vantage = 'cloudflare' AND t.states IS NOT NULL AND t.minute >= ?2 AND t.minute < ?3
         GROUP BY j.key`,
      )
      .bind(date, start, end),
    db
      .prepare(
        `UPDATE component_state SET uptime_90d = (
           SELECT CASE WHEN SUM(d.checks) > 0 THEN CAST(SUM(d.up_checks) AS REAL) / SUM(d.checks) ELSE NULL END
           FROM daily AS d WHERE d.component_id = component_state.component_id AND d.date >= ?1 AND d.date <= ?2)`,
      )
      .bind(from90, date),
    db.prepare("DELETE FROM ticks WHERE minute < ?").bind(minuteKey(addDays(now, -TICKS_KEEP_DAYS))),
    db.prepare("DELETE FROM daily WHERE date < ?").bind(dayKey(addDays(now, -DAILY_KEEP_DAYS))),
    db.prepare("DELETE FROM audit WHERE ts < ?").bind(addDays(now, -DAILY_KEEP_DAYS).toISOString()),
    db
      .prepare("INSERT INTO audit (ts, actor, action, target, detail, ip) VALUES (?, 'system', 'rollup', ?, NULL, NULL)")
      .bind(now.toISOString(), date),
  ];
}

/**
 * Today's partial daily row, upserted on every tick from this tick's states so the 90-day
 * bar and uptime figure fill in on launch day instead of waiting for the 00:05 rollup.
 * Same counting as the rollup (maintenance counts nothing, down is not up). The rollup
 * later REPLACEs yesterday's row from the ticks, so finished days stay exact. Also
 * refreshes uptime_90d through today.
 */
export function todayWrites(db: Db, now: Date, states: Record<string, Status>): Stmt[] {
  const date = dayKey(now);
  const rank = (col: string) =>
    `CASE ${col} WHEN 'down' THEN 4 WHEN 'degraded' THEN 3 WHEN 'maintenance' THEN 2 WHEN 'operational' THEN 1 ELSE 0 END`;
  const name = (r: string) => `CASE ${r} WHEN 4 THEN 'down' WHEN 3 THEN 'degraded' WHEN 2 THEN 'maintenance' WHEN 1 THEN 'operational' ELSE NULL END`;
  return [
    db
      .prepare(
        `INSERT INTO daily (component_id, date, status, uptime, checks, up_checks)
         SELECT j.key, ?1, j.value,
           CASE WHEN j.value IN ('operational', 'degraded', 'down') THEN CASE WHEN j.value = 'down' THEN 0.0 ELSE 1.0 END ELSE NULL END,
           j.value IN ('operational', 'degraded', 'down'),
           j.value IN ('operational', 'degraded')
         FROM json_each(?2) AS j WHERE true
         ON CONFLICT (component_id, date) DO UPDATE SET
           status = ${name(`MAX(${rank("daily.status")}, ${rank("excluded.status")})`)},
           checks = daily.checks + excluded.checks,
           up_checks = daily.up_checks + excluded.up_checks,
           uptime = CASE WHEN daily.checks + excluded.checks > 0
             THEN CAST(daily.up_checks + excluded.up_checks AS REAL) / (daily.checks + excluded.checks) ELSE NULL END`,
      )
      .bind(date, JSON.stringify(states)),
    db
      .prepare(
        `UPDATE component_state SET uptime_90d = (
           SELECT CASE WHEN SUM(d.checks) > 0 THEN CAST(SUM(d.up_checks) AS REAL) / SUM(d.checks) ELSE NULL END
           FROM daily AS d WHERE d.component_id = component_state.component_id AND d.date >= ?1 AND d.date <= ?2)`,
      )
      .bind(dayKey(addDays(now, -89)), date),
  ];
}

// ---- ingest -------------------------------------------------------------------
export function ingestWrite(db: Db, ts: string, results: ProbeResult[]): Stmt {
  return tickInsert(db, "allternit-standby", new Date(ts), ts, results, null);
}

// ---- public reads -------------------------------------------------------------
function toIncident(r: IncidentRow, updates: UpdateRow[]): Incident {
  return {
    id: r.id,
    title: r.title,
    state: r.state,
    impact: r.impact,
    component_ids: JSON.parse(r.component_ids) as string[],
    started_at: r.started_at,
    resolved_at: r.resolved_at,
    auto: !!r.auto,
    updates: updates.map<IncidentUpdate>((u) => ({ id: u.id, ts: u.ts, status: u.status, body: u.body })),
  };
}

function join(rows: IncidentRow[], updates: UpdateRow[]): Incident[] {
  const by = new Map<string, UpdateRow[]>();
  for (const u of updates) (by.get(u.incident_id) ?? by.set(u.incident_id, []).get(u.incident_id)!).push(u);
  return rows.filter((r) => by.has(r.id)).map((r) => toIncident(r, by.get(r.id)!));
}

export async function getStatusDoc(db: Db, targets: TargetsDoc, now: Date): Promise<StatusDoc> {
  const [states, last, open, updates] = await db.batch([
    db.prepare("SELECT component_id, status, last_change, uptime_90d FROM component_state"),
    db.prepare("SELECT ts FROM ticks WHERE vantage = 'cloudflare' ORDER BY minute DESC LIMIT 1"),
    db.prepare("SELECT * FROM incidents WHERE state = 'open' AND draft = 0 ORDER BY started_at DESC"),
    db.prepare(
      `SELECT u.id, u.incident_id, u.ts, u.status, u.body FROM incident_updates AS u
       JOIN incidents AS i ON i.id = u.incident_id WHERE i.state = 'open' AND i.draft = 0 ORDER BY u.id`,
    ),
  ]);
  const byId = new Map((states.results as unknown as StateRow[]).map((r) => [r.component_id, r]));
  const openRows = open.results as unknown as IncidentRow[];
  const maint = activeMaintenance(openRows, now);
  const components = targets.components.map((c) => {
    const r = byId.get(c.id);
    const status: Status = maint.has(c.id) ? "maintenance" : (r?.status ?? "unknown");
    return {
      id: c.id,
      name: c.name,
      status,
      uptime_90d: r?.uptime_90d ?? null,
      last_change: r?.last_change ?? null,
      critical: c.critical,
    };
  });
  const nowIso = now.toISOString();
  // Upcoming maintenance is listed in incidents.json, not here (clients would show it as current).
  const current = openRows.filter((r) => r.started_at <= nowIso);
  return {
    schema: "allternit.status/v1",
    last_checked_at: (last.results[0] as { ts?: string } | undefined)?.ts ?? NEVER,
    overall: overallOf(components.map((c) => c.status)),
    components,
    incidents: join(current, updates.results as unknown as UpdateRow[]),
  };
}

export async function getIncidentsDoc(db: Db, opts: { includeDrafts?: boolean; limit?: number } = {}): Promise<IncidentsDoc & { drafts?: string[] }> {
  const draftCond = opts.includeDrafts ? "1 = 1" : "draft = 0";
  const limit = Math.min(Math.max(opts.limit ?? 200, 1), 500);
  const [rows, updates] = await db.batch([
    db.prepare(`SELECT * FROM incidents WHERE ${draftCond} ORDER BY started_at DESC LIMIT ?`).bind(limit),
    db.prepare(
      `SELECT u.id, u.incident_id, u.ts, u.status, u.body FROM incident_updates AS u
       WHERE u.incident_id IN (SELECT id FROM incidents WHERE ${draftCond} ORDER BY started_at DESC LIMIT ?) ORDER BY u.id`,
    ).bind(limit),
  ]);
  const list = rows.results as unknown as IncidentRow[];
  return { schema: "allternit.incidents/v1", incidents: join(list, updates.results as unknown as UpdateRow[]) };
}

/** Admin view: incidents with their draft/origin/kind flags. */
export async function getAdminIncidents(db: Db): Promise<(Incident & { draft: boolean; origin: string; kind: string; ends_at: string | null })[]> {
  const [rows, updates] = await db.batch([
    db.prepare("SELECT * FROM incidents ORDER BY state = 'open' DESC, started_at DESC LIMIT 100"),
    db.prepare(
      `SELECT id, incident_id, ts, status, body FROM incident_updates
       WHERE incident_id IN (SELECT id FROM incidents ORDER BY state = 'open' DESC, started_at DESC LIMIT 100) ORDER BY id`,
    ),
  ]);
  const list = rows.results as unknown as IncidentRow[];
  const inc = join(list, updates.results as unknown as UpdateRow[]);
  const meta = new Map(list.map((r) => [r.id, r]));
  return inc.map((i) => {
    const r = meta.get(i.id)!;
    return { ...i, draft: !!r.draft, origin: r.origin, kind: r.kind, ends_at: r.ends_at };
  });
}

export async function getHistoryDoc(db: Db, targets: TargetsDoc, days: number, now: Date): Promise<HistoryDoc> {
  const n = Math.min(Math.max(Math.trunc(days) || 90, 1), 400);
  const last = now; // today is a partial day, filled in by todayWrites
  const first = addDays(last, -(n - 1));
  const { results } = await db
    .prepare("SELECT component_id, date, status, uptime FROM daily WHERE date >= ? AND date <= ?")
    .bind(dayKey(first), dayKey(last))
    .all<{ component_id: string; date: string; status: Status | null; uptime: number | null }>();
  const map = new Map(results.map((r) => [`${r.component_id}|${r.date}`, r]));
  const dates: string[] = [];
  for (let i = 0; i < n; i++) dates.push(dayKey(addDays(first, i)));
  return {
    schema: "allternit.history/v1",
    days: n,
    generated_at: now.toISOString(),
    components: targets.components.map((c) => ({
      id: c.id,
      days: dates.map<HistoryDay>((date) => {
        const r = map.get(`${c.id}|${date}`);
        return { date, status: r?.status ?? null, uptime: r?.uptime ?? null };
      }),
    })),
  };
}

export async function getHealth(db: Db, now: Date): Promise<HealthDoc> {
  const r = await db
    .prepare("SELECT ts FROM ticks WHERE vantage = 'cloudflare' ORDER BY minute DESC LIMIT 1")
    .first<{ ts: string }>();
  if (!r) return { status: "stale", last_checked_at: null, age_seconds: null };
  const age = Math.max(0, Math.round((now.getTime() - Date.parse(r.ts)) / 1000));
  return { status: age <= HEALTH_STALE_S ? "ok" : "stale", last_checked_at: r.ts, age_seconds: age };
}

// ---- admin writes -------------------------------------------------------------
export async function getIncidentRow(db: Db, id: string): Promise<IncidentRow | null> {
  return db.prepare("SELECT * FROM incidents WHERE id = ?").bind(id).first<IncidentRow>();
}

export function auditWrite(db: Db, now: Date, action: string, target: string | null, detail: unknown, ip: string | null, actor = "admin"): Stmt {
  return db
    .prepare("INSERT INTO audit (ts, actor, action, target, detail, ip) VALUES (?, ?, ?, ?, ?, ?)")
    .bind(now.toISOString(), actor, action, target, detail === undefined ? null : JSON.stringify(detail), ip);
}

export async function countAuthFails(db: Db, ip: string, since: Date): Promise<number> {
  const r = await db
    .prepare("SELECT count(*) AS n FROM audit WHERE action = 'auth_fail' AND ip = ? AND ts >= ?")
    .bind(ip, since.toISOString())
    .first<{ n: number }>();
  return Number(r?.n ?? 0);
}

export function createIncidentWrites(
  db: Db,
  args: {
    id: string;
    title: string;
    impact: IncidentImpact;
    component_ids: string[];
    status: IncidentStatus;
    body: string;
    now: Date;
    kind?: "incident" | "maintenance";
    started_at?: string;
    ends_at?: string | null;
  },
): Stmt[] {
  const ts = args.now.toISOString();
  const started = args.started_at ?? ts;
  return [
    db
      .prepare(
        `INSERT INTO incidents (id, title, state, impact, component_ids, started_at, resolved_at, auto, origin, draft, kind, ends_at, created_at, updated_at)
         VALUES (?, ?, 'open', ?, ?, ?, NULL, 0, 'admin', 0, ?, ?, ?, ?)`,
      )
      .bind(args.id, args.title, args.impact, JSON.stringify(args.component_ids), started, args.kind ?? "incident", args.ends_at ?? null, ts, ts),
    addUpdate(db, args.id, ts, args.status, args.body, "admin"),
  ];
}

/** Any manual edit sets auto = 0. */
export function patchIncidentWrites(
  db: Db,
  row: IncidentRow,
  patch: { title?: string; component_ids?: string[]; status?: IncidentStatus; body?: string },
  body: string | null,
  now: Date,
): Stmt[] {
  const ts = now.toISOString();
  const out: Stmt[] = [
    db
      .prepare("UPDATE incidents SET title = ?, component_ids = ?, auto = 0, updated_at = ? WHERE id = ?")
      .bind(patch.title ?? row.title, JSON.stringify(patch.component_ids ?? JSON.parse(row.component_ids)), ts, row.id),
  ];
  if (patch.status === "resolved" && row.state === "open") {
    out.push(db.prepare("UPDATE incidents SET state = 'resolved', resolved_at = ? WHERE id = ?").bind(ts, row.id));
  } else if (patch.status && patch.status !== "resolved" && row.state === "resolved") {
    out.push(db.prepare("UPDATE incidents SET state = 'open', resolved_at = NULL WHERE id = ?").bind(row.id));
  }
  if (body !== null) out.push(addUpdate(db, row.id, ts, patch.status ?? "investigating", body, "admin"));
  return out;
}

export async function latestUpdateStatus(db: Db, id: string): Promise<IncidentStatus | null> {
  const r = await db
    .prepare("SELECT status FROM incident_updates WHERE incident_id = ? ORDER BY id DESC LIMIT 1")
    .bind(id)
    .first<{ status: IncidentStatus }>();
  return r?.status ?? null;
}

export function publishDraftWrite(db: Db, id: string, now: Date): Stmt {
  return db.prepare("UPDATE incidents SET draft = 0, updated_at = ? WHERE id = ?").bind(now.toISOString(), id);
}
