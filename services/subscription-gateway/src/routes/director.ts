// RouteDirector (spec §4): turns run on the vendor's bound route only;
// failover to the next eligible route happens at the START of the next turn,
// never mid-stream (selection only ever happens here, at turn start); every
// choice is logged with its reason; with no eligible route the turn fails
// with the typed NO_ROUTE error. Eligibility (§3 + §4): signed in AND online
// AND the full capability contract passed for the CURRENT app version AND
// (desktop lanes) the launch-time probe applies.
import type { Account } from "@allternit/subscription-fabric-contracts";
import type { ConformanceReport } from "@allternit/agent-gateway";
import { contractFromReport, contractUnchecked, type ContractResult } from "./contract.js";
import {
  ROUTE_ORDER,
  isRouteId,
  phoneDeviceOnline,
  genericPhoneAdapterId,
  routeForPhoneDevice,
  routeReliability,
  type LinkedPhoneDeviceLike,
  type RouteId,
  type RouteReliability,
} from "./registry.js";
import { routeTelemetry } from "../observability/stats.js";
import { selectRoute, type RouteCandidateState } from "./select.js";
import {
  contractForVersion,
  getBinding,
  getOverride,
  probeForVersion,
  recordContractResult,
  recordProbe as storeRecordProbe,
  setBinding,
  setOverride,
  vendorsWithConformance,
  vendorsWithRouteState,
} from "./store.js";
import type { Db } from "../store/db.js";
import { listAccounts } from "../store/queries.js";
import { browserLanesRoutable } from "../local_desktop.js";

// ---------------------------------------------------------------------------
// Inputs (seams implemented in main.ts, faked in tests)
// ---------------------------------------------------------------------------

/** One adapter's lane declaration: which routes it implements for a vendor. */
export interface VendorLaneInfo {
  vendor: string;
  adapterId: string;
  /** The app/adapter version conformance is keyed by. */
  appVersion: string;
  routes: RouteId[];
  /** Lane reachable right now (not kill-switched / down). */
  online: boolean;
  /** How the lane reads the app; absent = its own written adapter spec ("exact"). */
  reliability?: RouteReliability;
}

export interface VendorLaneSource {
  lanes(): Promise<VendorLaneInfo[]>;
}

export interface PhoneDevicesSource {
  /** Mirrors Desktop's `phone.devices` linked entries. */
  linkedDevices(): Promise<LinkedPhoneDeviceLike[]>;
}

export type RouteChoiceKind = "bind" | "failover" | "override" | "no_route" | "turn";

export interface RouteChoiceLogEntry {
  vendor: string;
  route: RouteId | null;
  kind: RouteChoiceKind;
  reason: string;
  at: string;
}

export interface RouteDirectorDeps {
  db: Db;
  lanes?: VendorLaneSource;
  phones?: PhoneDevicesSource;
  /** Choice sink (§4.3): wired to the EventLog in main.ts, a list in tests. */
  onChoice?: (entry: RouteChoiceLogEntry) => void;
  now?: () => Date;
}

const EMPTY_LANES: VendorLaneSource = { lanes: async () => [] };
const EMPTY_PHONES: PhoneDevicesSource = { linkedDevices: async () => [] };

// The §4.5 typed error shape. Through the AAI surface the gateway surfaces it
// with the reason preserved (see AaiHost.call).
export interface NoRouteError {
  error: "NO_ROUTE";
  reason: string;
}

export type TurnRoute =
  | { ok: true; route: RouteId; adapterId: string | null; rebound: boolean; reason: string }
  | ({ ok: false } & NoRouteError);

export interface RouteStatusView {
  route: RouteId;
  eligible: boolean;
  signedIn: boolean;
  online: boolean;
  contract: { passed: string[]; failed: string[]; appVersion?: string; checkedAt?: string };
  /** Absent when nothing serves the route (no adapter, no linked phone). */
  telemetry?: RouteTelemetryView;
}

/** Mirrors the UI's VendorRouteTelemetry (allternit-ai sessions-computer-api.ts). */
export interface RouteTelemetryView {
  lastOkAt: string | null;
  lastUsedAt: string | null;
  latencyMs: number | null;
  successRate7d: number | null;
  tasks7d: number;
  reliability?: RouteReliability;
}

const PHONE_ROUTES: ReadonlySet<RouteId> = new Set(["android-app", "iphone-app-ictrl", "iphone-app-usb-agent"]);
const TELEMETRY_WINDOW_MS = 7 * 24 * 60 * 60 * 1000;

export interface VendorRoutesView {
  vendor: string;
  boundRoute?: RouteId;
  override?: RouteId;
  routes: RouteStatusView[];
}

interface RouteEligibility {
  route: RouteId;
  signedIn: boolean;
  online: boolean;
  eligible: boolean;
  contract: ContractResult;
}

export class RouteDirector {
  private readonly deps: Required<RouteDirectorDeps>;

  constructor(deps: RouteDirectorDeps) {
    this.deps = {
      lanes: deps.lanes ?? EMPTY_LANES,
      phones: deps.phones ?? EMPTY_PHONES,
      onChoice: deps.onChoice ?? (() => {}),
      now: deps.now ?? (() => new Date()),
      db: deps.db,
    };
  }

  private log(entry: Omit<RouteChoiceLogEntry, "at">): void {
    try {
      this.deps.onChoice({ ...entry, at: this.deps.now().toISOString() });
    } catch {
      // Logging never breaks routing.
    }
  }

  /** Adapters implementing (vendor, route), deterministic order. */
  private async adaptersFor(vendor: string, route: RouteId): Promise<VendorLaneInfo[]> {
    const lanes = await this.deps.lanes.lanes();
    return lanes
      .filter((l) => l.vendor === vendor && l.routes.includes(route))
      .sort((a, b) => a.adapterId.localeCompare(b.adapterId));
  }

  // The app version eligibility is checked against: the implementing
  // adapter's version (deterministic lowest adapter_id wins when several
  // implement one route). Phone routes have no adapter yet → null.
  private async currentAppVersion(vendor: string, route: RouteId): Promise<string | null> {
    const adapters = await this.adaptersFor(vendor, route);
    return adapters[0]?.appVersion ?? null;
  }

  private async eligibilityFor(vendor: string, route: RouteId, accounts: Account[]): Promise<RouteEligibility> {
    const appVersion = await this.currentAppVersion(vendor, route);
    const contract =
      appVersion === null ? contractUnchecked() : contractForVersion(this.deps.db, vendor, route, appVersion) ?? contractUnchecked();

    let signedIn = false;
    let online = false;

    if (route === "android-app" || route === "iphone-app-ictrl" || route === "iphone-app-usb-agent") {
      // Phone routes (§2): registered now, adapters in a follow-up task. The
      // device being linked and reachable is real; the vendor app's sign-in
      // and the §3 contract are NOT proven — eligible stays false with the
      // failing items visible (task item 7: never assume).
      const devices = await this.deps.phones.linkedDevices();
      const mine = devices.filter((d) => routeForPhoneDevice(d) === route);
      signedIn = mine.length > 0;
      online = mine.some((d) => phoneDeviceOnline(d));
    } else {
      const adapters = await this.adaptersFor(vendor, route);
      // signed in: an enabled, servable account for the vendor (ready or
      // degraded — the same servable set the fabric router uses).
      signedIn = accounts.some(
        (a) => a.provider === vendor && a.enabled && (a.session_health === "ready" || a.session_health === "degraded")
      );
      online = adapters.some((a) => a.online);
      // The person's own computer with browser sign-ins off (local_desktop.ts).
      if (route === "web" && !browserLanesRoutable()) online = false;
    }

    let eligible = signedIn && online && contract.ok;
    // §2 debug-mode rule: a desktop lane is eligible only when the
    // launch-time probe recorded that this lane applies to the current app
    // version (cdp: the shipped app accepted its debug flag unmodified).
    if (eligible && (route === "desktop-app-cdp" || route === "desktop-app-ax") && appVersion !== null) {
      const probe = probeForVersion(this.deps.db, vendor, route, appVersion);
      eligible = probe?.applies === true;
    }

    return { route, signedIn, online, eligible, contract };
  }

  private async eligibilityAll(vendor: string): Promise<RouteEligibility[]> {
    const accounts = listAccounts(this.deps.db);
    const out: RouteEligibility[] = [];
    for (const route of ROUTE_ORDER) {
      out.push(await this.eligibilityFor(vendor, route, accounts));
    }
    return out;
  }

  // Settings telemetry for one route: the implementing adapters' last 7 days
  // of turns and probes, plus how the route reads the app. Absent when
  // nothing serves the route — never a row of zeros for a route with no lane.
  private async telemetryFor(vendor: string, route: RouteId, deviceLinked: boolean): Promise<RouteTelemetryView | undefined> {
    const adapters = await this.adaptersFor(vendor, route);
    const reliability = routeReliability(route, {
      hasAdapter: adapters.length > 0,
      declared: adapters[0]?.reliability,
      deviceLinked,
    });
    if (adapters.length === 0 && reliability === null) return undefined;
    const since = new Date(this.deps.now().getTime() - TELEMETRY_WINDOW_MS).toISOString();
    const ids = adapters.map((a) => a.adapterId);
    // A linked phone with no written adapter: its generic turns' attempts.
    if (ids.length === 0 && PHONE_ROUTES.has(route)) ids.push(genericPhoneAdapterId(vendor, route));
    const numbers = routeTelemetry(this.deps.db, ids, since);
    return reliability ? { ...numbers, reliability } : numbers;
  }

  /** The implementing adapter for a route (deterministic pick). */
  async adapterFor(vendor: string, route: RouteId): Promise<string | null> {
    return (await this.adaptersFor(vendor, route))[0]?.adapterId ?? null;
  }

  // -------------------------------------------------------------------------
  // Turn start (§4.2/§4.4): the ONLY selection point — mid-stream turns are
  // never re-routed because this is never called mid-stream.
  // -------------------------------------------------------------------------
  async routeForTurn(vendor: string): Promise<TurnRoute> {
    const candidates: RouteCandidateState[] = (await this.eligibilityAll(vendor)).map((e) => ({
      route: e.route,
      eligible: e.eligible,
      signedIn: e.signedIn,
      online: e.online,
    }));
    const override = getOverride(this.deps.db, vendor);
    const selection = selectRoute({ vendor, candidates, override });
    const bound = getBinding(this.deps.db, vendor)?.boundRoute ?? null;

    if (!selection.ok) {
      // §4.5: typed NO_ROUTE. The binding row is cleared so the state reads
      // honestly; the reason is logged with the choice (§4.3).
      setBinding(this.deps.db, vendor, null, selection.reason);
      this.log({ vendor, route: null, kind: "no_route", reason: selection.reason });
      return { ok: false, error: "NO_ROUTE", reason: selection.reason };
    }

    const rebound = bound !== selection.route;
    const kind: RouteChoiceKind = override != null ? "override" : rebound ? (bound === null ? "bind" : "failover") : "turn";
    if (rebound) {
      setBinding(this.deps.db, vendor, selection.route, selection.reason);
    }
    this.log({ vendor, route: selection.route, kind, reason: selection.reason });
    return {
      ok: true,
      route: selection.route,
      adapterId: await this.adapterFor(vendor, selection.route),
      rebound,
      reason: selection.reason,
    };
  }

  /** §4.6: Settings pin. The pin still has to be eligible. */
  async setOverride(vendor: string, route: string | null): Promise<{ ok: true } | { ok: false; status: number; body: Record<string, unknown> }> {
    if (route !== null) {
      if (!isRouteId(route)) {
        return { ok: false, status: 400, body: { error: "unknown_route", route, allowed: [...ROUTE_ORDER] } };
      }
      const eligibility = await this.eligibilityFor(vendor, route, listAccounts(this.deps.db));
      if (!eligibility.eligible) {
        return {
          ok: false,
          status: 400,
          body: {
            error: "route_not_eligible",
            vendor,
            route,
            detail: `route is not eligible for ${vendor} (signedIn=${eligibility.signedIn}, online=${eligibility.online}, contract=${eligibility.contract.ok ? "passed" : "not passed"})`,
            failed: eligibility.contract.failed,
          },
        };
      }
    }
    setOverride(this.deps.db, vendor, route);
    this.log({ vendor, route, kind: "override", reason: route === null ? "override cleared" : `override pinned to ${route}` });
    return { ok: true };
  }

  /** GET /v1/routes view, spec shape. */
  async status(vendors?: string[]): Promise<VendorRoutesView[]> {
    const wanted = vendors ? new Set(vendors) : null;
    const all = await this.vendors();
    const out: VendorRoutesView[] = [];
    for (const vendor of all) {
      if (wanted && !wanted.has(vendor)) continue;
      const eligibility = await this.eligibilityAll(vendor);
      const binding = getBinding(this.deps.db, vendor);
      const override = getOverride(this.deps.db, vendor);
      const routes: RouteStatusView[] = [];
      for (const e of eligibility) {
        const contract: RouteStatusView["contract"] = {
          passed: e.contract.passed,
          failed: e.contract.failed,
        };
        if (e.contract.appVersion !== null) contract.appVersion = e.contract.appVersion;
        if (e.contract.checkedAt !== null) contract.checkedAt = e.contract.checkedAt;
        const row: RouteStatusView = { route: e.route, eligible: e.eligible, signedIn: e.signedIn, online: e.online, contract };
        // Phone routes: signedIn means a phone serving the route is linked.
        const telemetry = await this.telemetryFor(vendor, e.route, PHONE_ROUTES.has(e.route) && e.signedIn);
        if (telemetry) row.telemetry = telemetry;
        routes.push(row);
      }
      const view: VendorRoutesView = { vendor, routes };
      if (binding?.boundRoute) view.boundRoute = binding.boundRoute;
      if (override) view.override = override;
      out.push(view);
    }
    return out;
  }

  /** Every vendor the router knows about (union, sorted). */
  async vendors(): Promise<string[]> {
    const fromLanes = (await this.deps.lanes.lanes()).map((l) => l.vendor);
    const fromAccounts = listAccounts(this.deps.db).map((a) => a.provider);
    const fromState = vendorsWithRouteState(this.deps.db);
    const fromConformance = vendorsWithConformance(this.deps.db);
    return [...new Set([...fromLanes, ...fromAccounts, ...fromState, ...fromConformance])].sort();
  }

  /** Record a per-route conformance run into the (vendor, route, appVersion) store. */
  recordConformance(vendor: string, route: RouteId, appVersion: string, report: ConformanceReport): ContractResult {
    const result = contractFromReport(report, { appVersion });
    recordContractResult(this.deps.db, vendor, route, appVersion, result);
    return result;
  }

  /** §2 launch-time probe record (called at boot from the transport probe). */
  recordProbe(vendor: string, route: RouteId, appVersion: string, applies: boolean, detail?: string): void {
    storeRecordProbe(this.deps.db, vendor, route, appVersion, applies, detail);
  }
}
