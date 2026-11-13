/**
 * Model = auto policy — client-side resolution of `model: "auto"`.
 *
 * Ported from the ai surface's model gateway view
 * (`surfaces/ai.allternit.com/src/lib/model-gateway-api.ts`). The policy is
 * persisted in localStorage under the same key so both surfaces share one
 * setting. Resolution happens in the browser: the playground substitutes the
 * concrete model id before calling the gateway, exactly like the donor.
 */

export type AutoPolicyStrategy =
  | "manual"
  | "cheapest"
  | "fastest"
  | "strongest"
  | "balanced";

export interface ModelAutoPolicy {
  strategy: AutoPolicyStrategy;
  allowedProviders: string[];
  maxInputCentsPer1m: number | null;
  maxOutputCentsPer1m: number | null;
}

export interface PolicyCatalogModel {
  id: string;
  owned_by: string;
  quality_tier?: string;
  pricing?: {
    input_cents_per_1m?: number;
    output_cents_per_1m?: number;
  } | null;
}

const POLICY_STORAGE_KEY = "allternit:model-gateway:auto-policy";

/**
 * Used when nothing is saved (every new account). It must resolve to a model
 * from any non-empty catalog: "balanced" takes the highest quality tier and the
 * cheapest model within it, and with no tier or price data it still returns
 * the first catalog entry. A "manual" default made `model: "auto"` resolve to
 * nothing for every new account. Saved policies are read as saved.
 */
export const DEFAULT_AUTO_POLICY: ModelAutoPolicy = {
  strategy: "balanced",
  allowedProviders: [],
  maxInputCentsPer1m: null,
  maxOutputCentsPer1m: null,
};

function isAutoPolicyStrategy(value: unknown): value is AutoPolicyStrategy {
  return (
    typeof value === "string" &&
    ["manual", "cheapest", "fastest", "strongest", "balanced"].includes(value)
  );
}

export function loadModelAutoPolicy(): ModelAutoPolicy {
  try {
    const raw = window.localStorage.getItem(POLICY_STORAGE_KEY);
    if (!raw) return DEFAULT_AUTO_POLICY;
    const parsed = JSON.parse(raw) as unknown;
    if (!parsed || typeof parsed !== "object") return DEFAULT_AUTO_POLICY;
    const p = parsed as Partial<ModelAutoPolicy>;
    return {
      strategy: isAutoPolicyStrategy(p.strategy)
        ? p.strategy
        : DEFAULT_AUTO_POLICY.strategy,
      allowedProviders: Array.isArray(p.allowedProviders)
        ? p.allowedProviders.filter((x): x is string => typeof x === "string")
        : DEFAULT_AUTO_POLICY.allowedProviders,
      maxInputCentsPer1m:
        typeof p.maxInputCentsPer1m === "number" || p.maxInputCentsPer1m === null
          ? p.maxInputCentsPer1m
          : DEFAULT_AUTO_POLICY.maxInputCentsPer1m,
      maxOutputCentsPer1m:
        typeof p.maxOutputCentsPer1m === "number" || p.maxOutputCentsPer1m === null
          ? p.maxOutputCentsPer1m
          : DEFAULT_AUTO_POLICY.maxOutputCentsPer1m,
    };
  } catch {
    return DEFAULT_AUTO_POLICY;
  }
}

export function saveModelAutoPolicy(policy: ModelAutoPolicy): void {
  try {
    window.localStorage.setItem(POLICY_STORAGE_KEY, JSON.stringify(policy));
  } catch {
    // Storage unavailable — policy stays session-local.
  }
}

const TIER_RANK: Record<string, number> = {
  reasoning: 3,
  high: 2,
  fast: 1,
};

function tierRank(tier: string): number {
  return TIER_RANK[tier.toLowerCase()] ?? 0;
}

/**
 * Resolve a concrete model id from the catalog using the active auto policy.
 * Returns `null` when the policy is manual or no model satisfies the filters.
 */
export function resolveAutoModel(
  models: PolicyCatalogModel[],
  policy: ModelAutoPolicy
): string | null {
  if (policy.strategy === "manual" || models.length === 0) return null;

  let candidates = models.slice();

  if (policy.allowedProviders.length > 0) {
    candidates = candidates.filter((m) =>
      policy.allowedProviders.includes(m.owned_by)
    );
  }
  if (policy.maxInputCentsPer1m != null) {
    candidates = candidates.filter(
      (m) => (m.pricing?.input_cents_per_1m ?? 0) <= policy.maxInputCentsPer1m!
    );
  }
  if (policy.maxOutputCentsPer1m != null) {
    candidates = candidates.filter(
      (m) => (m.pricing?.output_cents_per_1m ?? 0) <= policy.maxOutputCentsPer1m!
    );
  }
  if (candidates.length === 0) return null;

  const inputPrice = (m: PolicyCatalogModel) => m.pricing?.input_cents_per_1m ?? 0;
  const outputPrice = (m: PolicyCatalogModel) => m.pricing?.output_cents_per_1m ?? 0;

  switch (policy.strategy) {
    case "cheapest":
      candidates.sort(
        (a, b) =>
          inputPrice(a) + outputPrice(a) - (inputPrice(b) + outputPrice(b))
      );
      break;
    case "fastest":
      candidates.sort(
        (a, b) =>
          inputPrice(a) - inputPrice(b) ||
          tierRank(b.quality_tier ?? "") - tierRank(a.quality_tier ?? "")
      );
      break;
    case "strongest":
      candidates.sort(
        (a, b) =>
          tierRank(b.quality_tier ?? "") - tierRank(a.quality_tier ?? "") ||
          inputPrice(a) - inputPrice(b)
      );
      break;
    case "balanced":
      candidates.sort(
        (a, b) =>
          tierRank(b.quality_tier ?? "") - tierRank(a.quality_tier ?? "") ||
          inputPrice(a) + outputPrice(a) - (inputPrice(b) + outputPrice(b))
      );
      break;
  }

  return candidates[0]?.id ?? null;
}
