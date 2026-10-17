// §4 deterministic routing — the pure selection core. Same inputs → same
// output: the choice depends only on (vendor, eligible routes, fixed order,
// user override). No clocks, no randomness, no I/O. A route that is not
// eligible can never be chosen, by any path — there is no weaker fallback.
import { ROUTE_ORDER, type RouteId } from "./registry.js";

export interface RouteCandidateState {
  route: RouteId;
  /** signed in AND online AND passed the vendor's full §3 contract. */
  eligible: boolean;
  signedIn: boolean;
  online: boolean;
}

export type RouteSelection =
  | { ok: true; route: RouteId; reason: string }
  | { ok: false; error: "NO_ROUTE"; reason: string };

export interface SelectRouteInput {
  vendor: string;
  /** Every route the vendor can use, in any order; eligibility flags rule. */
  candidates: RouteCandidateState[];
  /** Fixed order override (tests); defaults to the §4.1 order. */
  order?: readonly RouteId[];
  /** §4.6 Settings pin. Honoured only when the pinned route is eligible. */
  override?: RouteId | null;
}

// §4.5 — the typed no-route reason, composed from the actual per-route state
// (e.g. "android-app offline; iphone-app-ictrl not signed in; no eligible
// route"). Plain words, logged with every choice (§4.3).
function noRouteReason(candidates: RouteCandidateState[], order: readonly RouteId[]): string {
  const byRoute = new Map(candidates.map((c) => [c.route, c]));
  const parts = order.map((route) => {
    const c = byRoute.get(route);
    if (!c) return `${route}: not offered`;
    if (!c.signedIn) return `${route}: not signed in`;
    if (!c.online) return `${route}: offline`;
    return `${route}: contract not passed`;
  });
  return `${parts.join("; ")}; no eligible route`;
}

export function selectRoute(input: SelectRouteInput): RouteSelection {
  const order = input.order ?? ROUTE_ORDER;
  const byRoute = new Map(input.candidates.map((c) => [c.route, c]));
  const eligible = (route: RouteId): RouteCandidateState | undefined => {
    const c = byRoute.get(route);
    return c?.eligible ? c : undefined;
  };

  // §4.6: the pin wins only when eligible; an ineligible pin is reported
  // honestly and falls through to the default order (never a silent bump to
  // a weaker route without the reason saying so).
  if (input.override != null) {
    if (eligible(input.override)) {
      return { ok: true, route: input.override, reason: `user override pins ${input.override} (eligible)` };
    }
    const c = byRoute.get(input.override);
    const why = !c ? "not offered for this vendor" : !c.signedIn ? "not signed in" : !c.online ? "offline" : "contract not passed";
    const fallback = order.map(eligible).find((c): c is RouteCandidateState => c !== undefined);
    if (fallback) {
      return {
        ok: true,
        route: fallback.route,
        reason: `override ${input.override} ineligible (${why}); default order selects ${fallback.route}`,
      };
    }
    return { ok: false, error: "NO_ROUTE", reason: `override ${input.override} ineligible (${why}); ${noRouteReason(input.candidates, order)}` };
  }

  // §4.1: first eligible route in the fixed order.
  for (const route of order) {
    if (eligible(route)) {
      return { ok: true, route, reason: `first eligible route in fixed order: ${route}` };
    }
  }
  return { ok: false, error: "NO_ROUTE", reason: noRouteReason(input.candidates, order) };
}
