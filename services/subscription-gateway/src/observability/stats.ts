// P4 Phase 2 — observability. Query-only: everything here is computed on read
// from task_attempts / adapter_breakers / route_rejections. No measurement
// events are emitted into the ledger on a stats read — stats are derivable
// from data the gateway already persists, and an event per query would spam
// the append-only log (see P4_PHASE_2_NOTES.md).
import type { FailureClass } from "@allternit/subscription-fabric-contracts";
import type { Db } from "../store/db.js";
import { listRouteRejections, type RouteRejectionRow } from "../store/queries.js";

// Attempt error classes that parked the task in needs_user rather than
// failing it (mirrors the worker's needsUserError mapping).
const NEEDS_USER_CLASSES: ReadonlySet<string> = new Set([
  "auth_required",
  "challenge_presented",
  "user_intervention_required",
]);

export interface AdapterStatsRow {
  adapter_id: string;
  adapter_version: string;
  attempts: number;
  completed: number;
  failed: number;
  needs_user: number;
  success_rate: number; // completed / attempts (0 when no attempts)
  latency_ms: { median: number | null; p95: number | null }; // ended attempts only
  failures_by_class: Partial<Record<FailureClass, number>>;
  ui_drift: { consecutive_failures: number; state: "closed" | "open" };
}

interface AttemptStatsRow {
  adapter_id: string;
  adapter_version: string;
  outcome: string;
  error: string | null;
  started_at: string;
  ended_at: string | null;
}

interface BreakerStatsRow {
  adapter_id: string;
  adapter_version: string;
  consecutive_ui_failures: number;
  state: string;
}

// Nearest-rank percentile over sorted values (deterministic, no interpolation).
function nearestRank(sorted: number[], p: number): number | null {
  if (sorted.length === 0) return null;
  const rank = Math.ceil((p / 100) * sorted.length);
  return sorted[Math.min(Math.max(rank, 1), sorted.length) - 1];
}

export function adapterStats(db: Db, opts: { since?: string } = {}): AdapterStatsRow[] {
  const rows = (
    opts.since
      ? db
          .prepare(
            `SELECT adapter_id, adapter_version, outcome, error, started_at, ended_at
             FROM task_attempts WHERE started_at >= ? ORDER BY adapter_id, adapter_version`
          )
          .all(opts.since)
      : db
          .prepare(
            `SELECT adapter_id, adapter_version, outcome, error, started_at, ended_at
             FROM task_attempts ORDER BY adapter_id, adapter_version`
          )
          .all()
  ) as AttemptStatsRow[];

  const breakers = new Map<string, BreakerStatsRow>();
  for (const b of db.prepare("SELECT * FROM adapter_breakers").all() as BreakerStatsRow[]) {
    breakers.set(`${b.adapter_id} ${b.adapter_version}`, b);
  }

  const groups = new Map<string, AttemptStatsRow[]>();
  for (const row of rows) {
    const key = `${row.adapter_id} ${row.adapter_version}`;
    const group = groups.get(key);
    if (group) group.push(row);
    else groups.set(key, [row]);
  }

  const out: AdapterStatsRow[] = [];
  for (const group of groups.values()) {
    const first = group[0];
    let completed = 0;
    let failed = 0;
    let needsUser = 0;
    const failuresByClass: Partial<Record<FailureClass, number>> = {};
    const latencies: number[] = [];
    for (const row of group) {
      if (row.outcome === "success" || row.outcome === "partial") completed += 1;
      if (row.ended_at) {
        const ms = Date.parse(row.ended_at) - Date.parse(row.started_at);
        if (Number.isFinite(ms)) latencies.push(ms);
      }
      if (row.error) {
        const parsed = JSON.parse(row.error) as { class?: FailureClass };
        const cls = parsed.class;
        if (cls) failuresByClass[cls] = (failuresByClass[cls] ?? 0) + 1;
        if (cls && NEEDS_USER_CLASSES.has(cls)) needsUser += 1;
        else if (row.outcome === "failed" || row.outcome === "ambiguous") failed += 1;
      } else if (row.outcome === "failed" || row.outcome === "ambiguous") {
        failed += 1;
      }
    }
    latencies.sort((a, b) => a - b);
    const breaker = breakers.get(`${first.adapter_id} ${first.adapter_version}`);
    out.push({
      adapter_id: first.adapter_id,
      adapter_version: first.adapter_version,
      attempts: group.length,
      completed,
      failed,
      needs_user: needsUser,
      success_rate: group.length === 0 ? 0 : completed / group.length,
      latency_ms: { median: nearestRank(latencies, 50), p95: nearestRank(latencies, 95) },
      failures_by_class: failuresByClass,
      ui_drift: {
        consecutive_failures: breaker?.consecutive_ui_failures ?? 0,
        state: breaker?.state === "open" ? "open" : "closed",
      },
    });
  }
  // Deterministic order: adapter_id, then version.
  return out.sort(
    (a, b) => a.adapter_id.localeCompare(b.adapter_id) || a.adapter_version.localeCompare(b.adapter_version)
  );
}

// Latest route-decision rejections, newest first (route_rejections, migration
// 0003 — written wherever a decision is persisted).
export function recentRejections(db: Db, limit = 50): RouteRejectionRow[] {
  return listRouteRejections(db, limit);
}

// Per-route telemetry for GET /v1/routes (the Settings → Connections view).
// Computed on read from what the gateway already persists, for the adapters
// that implement one (vendor, route): task_attempts (worker turns) and
// canary_runs (lane probes, pruned to a week). Every number is null/absent
// when there is no row behind it — nothing is estimated.
export interface RouteTelemetryNumbers {
  lastOkAt: string | null;
  lastUsedAt: string | null;
  latencyMs: number | null;
  successRate7d: number | null;
  tasks7d: number;
}

function latest(a: string | null, b: string | null | undefined): string | null {
  if (!b) return a;
  if (!a) return b;
  return b > a ? b : a;
}

export function routeTelemetry(db: Db, adapterIds: string[], since: string): RouteTelemetryNumbers {
  const out: RouteTelemetryNumbers = { lastOkAt: null, lastUsedAt: null, latencyMs: null, successRate7d: null, tasks7d: 0 };
  if (adapterIds.length === 0) return out;
  const marks = adapterIds.map(() => "?").join(", ");

  const attempts = db
    .prepare(
      `SELECT outcome, started_at, ended_at FROM task_attempts
       WHERE adapter_id IN (${marks}) AND started_at >= ?`
    )
    .all(...adapterIds, since) as Array<{ outcome: string; started_at: string; ended_at: string | null }>;
  let completed = 0;
  const latencies: number[] = [];
  for (const a of attempts) {
    const ok = a.outcome === "success" || a.outcome === "partial";
    if (ok) completed += 1;
    out.lastUsedAt = latest(out.lastUsedAt, a.ended_at ?? a.started_at);
    if (ok) out.lastOkAt = latest(out.lastOkAt, a.ended_at ?? a.started_at);
    if (a.ended_at) {
      const ms = Date.parse(a.ended_at) - Date.parse(a.started_at);
      if (Number.isFinite(ms) && ms >= 0) latencies.push(ms);
    }
  }
  out.tasks7d = attempts.length;
  out.successRate7d = attempts.length === 0 ? null : completed / attempts.length;

  // Lane probes (the same canary rows GET /v1/lanes reads) also count as use.
  const probes = db
    .prepare(
      `SELECT outcome, started_at, ended_at, latency_ms FROM canary_runs
       WHERE adapter_id IN (${marks}) AND started_at >= ?`
    )
    .all(...adapterIds, since) as Array<{ outcome: string; started_at: string; ended_at: string | null; latency_ms: number | null }>;
  const probeLatencies: number[] = [];
  for (const p of probes) {
    out.lastUsedAt = latest(out.lastUsedAt, p.ended_at ?? p.started_at);
    if (p.outcome === "pass") out.lastOkAt = latest(out.lastOkAt, p.ended_at ?? p.started_at);
    if (typeof p.latency_ms === "number") probeLatencies.push(p.latency_ms);
  }

  // Median time to a reply: task turns when there are any, else probe replies.
  const source = latencies.length > 0 ? latencies : probeLatencies;
  source.sort((a, b) => a - b);
  out.latencyMs = nearestRank(source, 50);
  return out;
}
