// Route-router store: typed row mappers + prepared-statement helpers over the
// 0012 tables. Every function takes the Database handle (dependency-injected,
// testable), matching store/queries.ts style.
import type { Db } from "../store/db.js";
import {
  CONTRACT_ITEM_IDS,
  type ContractItemId,
  type ContractResult,
} from "./contract.js";
import type { RouteId } from "./registry.js";

// ---------------------------------------------------------------------------
// route_conformance — §3 results per (vendor, route, app_version)
// ---------------------------------------------------------------------------

export interface ConformanceRow {
  vendor: string;
  route: string;
  app_version: string;
  item: string;
  passed: number;
  reason: string | null;
  evidence: string;
  checked_at: string;
}

/** Record one conformance run: upserts every contract item for the key. */
export function recordContractResult(
  db: Db,
  vendor: string,
  route: RouteId,
  appVersion: string,
  result: ContractResult
): void {
  const stmt = db.prepare(
    `INSERT INTO route_conformance (vendor, route, app_version, item, passed, reason, evidence, checked_at)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?)
     ON CONFLICT (vendor, route, app_version, item) DO UPDATE SET
       passed = excluded.passed,
       reason = excluded.reason,
       evidence = excluded.evidence,
       checked_at = excluded.checked_at`
  );
  const tx = db.transaction(() => {
    for (const item of result.items) {
      stmt.run(
        vendor,
        route,
        appVersion,
        item.item,
        item.passed ? 1 : 0,
        item.reason || null,
        JSON.stringify(item.evidence),
        result.checkedAt ?? new Date().toISOString()
      );
    }
  });
  tx();
}

/**
 * The contract result for exactly this app version. An app update makes the
 * old rows stop matching (they stay for history) — a route is ineligible for
 * the new version until the suite re-runs and rows exist for it. Returns null
 * when nothing was recorded for this version.
 */
export function contractForVersion(
  db: Db,
  vendor: string,
  route: RouteId,
  appVersion: string
): ContractResult | null {
  const rows = db
    .prepare(
      `SELECT * FROM route_conformance
       WHERE vendor = ? AND route = ? AND app_version = ?`
    )
    .all(vendor, route, appVersion) as ConformanceRow[];
  if (rows.length === 0) return null;
  const byItem = new Map(rows.map((r) => [r.item, r]));
  const items = CONTRACT_ITEM_IDS.map((id: ContractItemId) => {
    const row = byItem.get(id);
    return {
      item: id,
      passed: row?.passed === 1,
      reason: row?.reason ?? "",
      evidence: row ? (JSON.parse(row.evidence) as string[]) : [],
    };
  });
  const passed = items.filter((i) => i.passed).map((i) => i.item);
  const failed = items.filter((i) => !i.passed).map((i) => i.item);
  const checkedAt = rows.map((r) => r.checked_at).sort().at(-1) ?? null;
  return { appVersion, checkedAt, items, passed, failed, ok: failed.length === 0 };
}

/** Distinct vendors that have any recorded conformance. */
export function vendorsWithConformance(db: Db): string[] {
  const rows = db
    .prepare("SELECT DISTINCT vendor FROM route_conformance ORDER BY vendor ASC")
    .all() as Array<{ vendor: string }>;
  return rows.map((r) => r.vendor);
}

// ---------------------------------------------------------------------------
// route_bindings + route_overrides — §4.2/§4.6
// ---------------------------------------------------------------------------

export interface RouteBinding {
  vendor: string;
  boundRoute: RouteId | null;
  reason: string | null;
  updatedAt: string;
}

export function getBinding(db: Db, vendor: string): RouteBinding | null {
  const row = db
    .prepare("SELECT * FROM route_bindings WHERE vendor = ?")
    .get(vendor) as
    | { vendor: string; bound_route: string | null; reason: string | null; updated_at: string }
    | undefined;
  return row
    ? { vendor: row.vendor, boundRoute: row.bound_route as RouteId | null, reason: row.reason, updatedAt: row.updated_at }
    : null;
}

/** §4.2: the chosen route is written into the vendor's binding. */
export function setBinding(db: Db, vendor: string, route: RouteId | null, reason: string): void {
  db.prepare(
    `INSERT INTO route_bindings (vendor, bound_route, reason, updated_at)
     VALUES (?, ?, ?, ?)
     ON CONFLICT (vendor) DO UPDATE SET
       bound_route = excluded.bound_route,
       reason = excluded.reason,
       updated_at = excluded.updated_at`
  ).run(vendor, route, reason, new Date().toISOString());
}

export function getOverride(db: Db, vendor: string): RouteId | null {
  const row = db
    .prepare("SELECT route FROM route_overrides WHERE vendor = ?")
    .get(vendor) as { route: string } | undefined;
  return (row?.route as RouteId | undefined) ?? null;
}

/** null clears the pin (§4.6). */
export function setOverride(db: Db, vendor: string, route: RouteId | null): void {
  if (route === null) {
    db.prepare("DELETE FROM route_overrides WHERE vendor = ?").run(vendor);
    return;
  }
  db.prepare(
    `INSERT INTO route_overrides (vendor, route, updated_at) VALUES (?, ?, ?)
     ON CONFLICT (vendor) DO UPDATE SET route = excluded.route, updated_at = excluded.updated_at`
  ).run(vendor, route, new Date().toISOString());
}

/** Vendors with any binding or override row (for the API vendor list). */
export function vendorsWithRouteState(db: Db): string[] {
  const rows = db
    .prepare(
      `SELECT vendor FROM route_bindings
       UNION SELECT vendor FROM route_overrides
       ORDER BY vendor ASC`
    )
    .all() as Array<{ vendor: string }>;
  return rows.map((r) => r.vendor);
}

// ---------------------------------------------------------------------------
// route_probes — §2 launch-time probe (debug-mode rule)
// ---------------------------------------------------------------------------

export interface RouteProbe {
  vendor: string;
  route: RouteId;
  appVersion: string;
  applies: boolean;
  detail: string | null;
  probedAt: string;
}

export function recordProbe(
  db: Db,
  vendor: string,
  route: RouteId,
  appVersion: string,
  applies: boolean,
  detail?: string
): void {
  db.prepare(
    `INSERT INTO route_probes (vendor, route, app_version, applies, detail, probed_at)
     VALUES (?, ?, ?, ?, ?, ?)
     ON CONFLICT (vendor, route, app_version) DO UPDATE SET
       applies = excluded.applies,
       detail = excluded.detail,
       probed_at = excluded.probed_at`
  ).run(vendor, route, appVersion, applies ? 1 : 0, detail ?? null, new Date().toISOString());
}

/** null = no probe recorded for this app version (never assumed). */
export function probeForVersion(
  db: Db,
  vendor: string,
  route: RouteId,
  appVersion: string
): RouteProbe | null {
  const row = db
    .prepare(
      "SELECT * FROM route_probes WHERE vendor = ? AND route = ? AND app_version = ?"
    )
    .get(vendor, route, appVersion) as
    | { vendor: string; route: string; app_version: string; applies: number; detail: string | null; probed_at: string }
    | undefined;
  return row
    ? {
        vendor: row.vendor,
        route: row.route as RouteId,
        appVersion: row.app_version,
        applies: row.applies === 1,
        detail: row.detail,
        probedAt: row.probed_at,
      }
    : null;
}
