import { AllternitApiError, api } from "@/lib/api-client";

export interface FreeInferenceUsage {
  monthly_allowance_usd: number;
  used_usd: number;
  remaining_usd: number;
}

export interface CreditsTransaction {
  amount_usd: number;
  source: string;
  created_at: string;
}

export interface CreditsBalance {
  balance_usd: number;
  month_to_date_usage_usd: number;
  recent_transactions?: CreditsTransaction[];
  free_inference?: FreeInferenceUsage;
}

function billingApiBaseUrl(): string {
  return String(
    import.meta.env.VITE_ALLTERNIT_CLOUD_API_URL || "https://api.allternit.com",
  ).replace(/\/$/, "");
}

/**
 * Current credit balance for the signed-in user. Same endpoint and auth shape
 * as the BillingPage credits panel (allternit-cloud-api).
 */
export async function getCreditsBalance(
  token: string,
  signal?: AbortSignal,
): Promise<CreditsBalance> {
  const response = await fetch(`${billingApiBaseUrl()}/api/v1/billing/credits`, {
    headers: { Authorization: `Bearer ${token}` },
    signal,
  });
  if (!response.ok) {
    const payload = await response.json().catch(() => ({}));
    throw new Error(
      payload.message ||
        payload.error ||
        `Unable to load credit balance (${response.status})`,
    );
  }
  return (await response.json()) as CreditsBalance;
}

/**
 * The signed-in user's Allternit Cloud wallet (cloud-api
 * `/api/v1/billing/credits`, USD). Served by cloud-api itself, so it works
 * for every account, paired device or not. Goes through the shared api client
 * so it gets a fresh Clerk token and the 401 retry.
 */
export async function getCloudCreditsBalance(): Promise<CreditsBalance> {
  return api.get<CreditsBalance>("/api/v1/billing/credits");
}

export function formatCreditsUsd(value: number): string {
  return `$${value.toFixed(2)}`;
}

export interface OrgCreditsBalance {
  organization_id: string;
  balance_cents: number;
  available_cents: number;
  held_cents: number;
  currency: string;
}

export interface TransferFromWalletResult {
  organization_id: string;
  balance_cents: number;
  available_cents: number;
  wallet_balance_usd: number;
}

/**
 * Organization compute-credits balance (the fabric ledger in allternit-api on
 * the paired node, relayed by cloud-api). Returns null when there is no such
 * ledger to show: no active org (403/404) or no paired node (428).
 */
export async function getOrgCreditsBalance(): Promise<OrgCreditsBalance | null> {
  try {
    return await api.get<OrgCreditsBalance>("/api/v1/credits/balance");
  } catch (err) {
    if (err instanceof AllternitApiError) {
      const status = err.statusCode;
      if (status === 403 || status === 404 || status === 428) return null;
    }
    throw err;
  }
}

export interface ConsoleCredits {
  /** Cloud wallet (USD). null only when that call failed. */
  cloud: CreditsBalance | null;
  /** Device (org fabric) ledger. null unless a node is paired and answered. */
  device: OrgCreditsBalance | null;
}

/**
 * Both balances for the dashboard and the rail chip. The Cloud wallet is the
 * default; the device ledger is added only when its call succeeds. A failed
 * device call is never an error the user sees.
 */
export async function getConsoleCredits(): Promise<ConsoleCredits> {
  const [cloud, device] = await Promise.allSettled([
    getCloudCreditsBalance(),
    getOrgCreditsBalance(),
  ]);
  return {
    cloud: cloud.status === "fulfilled" ? cloud.value : null,
    device: device.status === "fulfilled" ? device.value : null,
  };
}

/**
 * Text for the rail Credits chip: the Cloud wallet balance, e.g. "$12.50".
 * Returns null when it cannot be loaded so the chip hides instead of
 * showing an error. Never calls the node-only `/api/v1/credits/balance`.
 */
export async function getCreditsChipLabel(): Promise<string | null> {
  try {
    const cloud = await getCloudCreditsBalance();
    return typeof cloud.balance_usd === "number" ? formatCreditsUsd(cloud.balance_usd) : null;
  } catch {
    return null;
  }
}

/**
 * Move credits from the signed-in user's Stripe-backed wallet into the
 * active organization's fabric compute ledger. Idempotent via idempotencyKey
 * (generate one per user action, reuse it on retry).
 */
export async function transferWalletToOrg(
  amountCents: number,
  idempotencyKey?: string,
): Promise<TransferFromWalletResult> {
  return api.post<TransferFromWalletResult>("/api/v1/credits/transfer_from_wallet", {
    amount_cents: amountCents,
    ...(idempotencyKey ? { idempotency_key: idempotencyKey } : {}),
  });
}
