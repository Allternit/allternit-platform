/**
 * One registry for "is this feature available here".
 *
 * Allternit Cloud (gateway host api.allternit.com) does not serve every
 * runtime console route. Pages, nav and the command palette all read this
 * module so they never disagree.
 */

export const CLOUD_GATEWAY_HOST = "api.allternit.com";
const DEFAULT_GATEWAY_URL = `https://${CLOUD_GATEWAY_HOST}`;

export type GatedFeature =
  | "batches"
  | "files"
  | "serviceAccounts"
  | "security"
  | "spendLimits"
  | "caching"
  | "tags"
  | "orgWebhooks"
  | "routingPolicy"
  | "routeCredentials";

/** "hidden" = not on cloud (honest state). "redirect" = send to another page. */
export const CLOUD_AVAILABILITY: Record<
  GatedFeature,
  { available: false; redirectTo?: string }
> = {
  batches: { available: false },
  files: { available: false },
  serviceAccounts: { available: false },
  security: { available: false },
  spendLimits: { available: false, redirectTo: "/platform/billing" },
  // /api/v1/gateway/caching and /api/v1/tags are runtime routes; cloud relays
  // them to a paired runtime, so a cloud-only user gets an error.
  caching: { available: false },
  tags: { available: false },
  // Cloud webhooks are per project (Platform API /v1/webhooks).
  orgWebhooks: { available: false, redirectTo: "/platform/webhooks" },
  // /api/v1/gateway/provider-routing and /api/v1/gateway/route-credentials are
  // relayed to a paired runtime (src/lib/gateway-routing-policy.ts).
  routingPolicy: { available: false },
  routeCredentials: { available: false },
};

export function isCloudHost(gatewayUrl: string | undefined | null): boolean {
  const configured = String(gatewayUrl || DEFAULT_GATEWAY_URL).trim();
  try {
    return new URL(configured).hostname.toLowerCase() === CLOUD_GATEWAY_HOST;
  } catch {
    return false;
  }
}

/** Same source as api-client gatewayBase(): VITE_ALLTERNIT_GATEWAY_URL, default api.allternit.com. */
export function isCloudGateway(): boolean {
  return isCloudHost(import.meta.env.VITE_ALLTERNIT_GATEWAY_URL);
}

export function fabricEnabled(): boolean {
  return Boolean(String(import.meta.env.VITE_ALLTERNITOS_URL ?? "").trim());
}

/** True when the feature is unavailable on the current gateway and has no redirect. */
export function isHiddenOnCloud(feature: GatedFeature, cloud = isCloudGateway()): boolean {
  return cloud && !CLOUD_AVAILABILITY[feature].available;
}
