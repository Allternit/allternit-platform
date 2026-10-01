import React, { useCallback, useEffect, useState } from "react";
import {
  Receipt,
  ArrowSquareOut,
  Gauge,
  WarningCircle,
  ArrowsClockwise,
  Info,
  Coins,
  Lightning,
  Key,
  X,
} from "@phosphor-icons/react";
import {
  usePlatformOrganization,
  usePlatformUser,
  usePlatformAuth,
  useClerk,
} from "@/lib/platform-auth-client";
import { getHostedEntitlement, type HostedRuntimeEntitlement } from "@/lib/hosted-compute";
import { EmptyState } from "@/components/settings/EmptyState";
import { SkeletonRow } from "@/components/settings/SkeletonRow";
import { QUIET_BUTTON_CLASS } from "@/components/settings/buttonStyles";
import { formatApiError } from "@/lib/api-client";
import { PlanPicker, type LiveBillingPlan, type PlanId } from "@/components/PlanPicker";
import { TransferToOrgPanel } from "@/components/billing/TransferToOrgPanel";

function formatHours(seconds: number): string {
  if (seconds < 3600) return `${Math.max(0, Math.round(seconds / 60))} min`;
  return `${(seconds / 3600).toFixed(seconds < 36_000 ? 1 : 0)} hr`;
}

function safePortalUrl(value?: string) {
  if (!value) return null;
  try {
    const url = new URL(value);
    return url.protocol === "https:" ? url.toString() : null;
  } catch {
    return null;
  }
}

export interface CreditPack {
  id: string;
  price_usd: number;
  credits_usd: number;
  label: string;
}

export interface CreditTransaction {
  id?: string;
  source: string;
  created_at: string;
  amount_usd: number;
}

export interface FreeInferenceUsage {
  monthly_allowance_usd: number;
  used_usd: number;
  remaining_usd: number;
}

export interface BillingCredits {
  balance_usd: number;
  month_to_date_usage_usd: number;
  recent_transactions: CreditTransaction[];
  free_inference?: FreeInferenceUsage;
}

export interface InferenceProviderKey {
  provider_id: string;
  masked: string;
  status: string;
  last_validated_at?: string;
}

const INFERENCE_PROVIDERS: { id: string; label: string }[] = [
  { id: "together", label: "Together AI" },
  { id: "fireworks", label: "Fireworks" },
  { id: "deepinfra", label: "DeepInfra" },
  { id: "groq", label: "Groq" },
  { id: "openai", label: "OpenAI" },
  { id: "deepseek", label: "DeepSeek" },
  { id: "kimi", label: "Kimi" },
];

function billingApiBaseUrl() {
  return String(
    import.meta.env.VITE_ALLTERNIT_CLOUD_API_URL || "https://api.allternit.com",
  ).replace(/\/$/, "");
}

function formatSignedUsd(value: number): string {
  const sign = value > 0 ? "+" : value < 0 ? "-" : "";
  return `${sign}$${Math.abs(value).toFixed(2)}`;
}

function formatTransactionDate(value: string): string {
  const parsed = new Date(value);
  return Number.isNaN(parsed.getTime()) ? value : parsed.toLocaleDateString();
}

export function BillingPage() {
  const { organization } = usePlatformOrganization();
  const { user } = usePlatformUser();
  const { getToken, isSignedIn } = usePlatformAuth();
  const clerk = useClerk();
  const email = user?.primaryEmailAddress?.emailAddress || user?.userEmail || "—";

  // Signed out: sign in, then come back to this page with the plan chosen,
  // and checkout starts on its own (see the ?plan= handling below).
  const handleSubscribe = (planId?: PlanId) => {
    const target = planId && planId !== "free" ? `/billing?plan=${planId}` : "/billing";
    if (clerk?.openSignIn) {
      clerk.openSignIn({ redirectUrl: target });
    } else {
      window.location.href = `/sign-in?redirect_url=${encodeURIComponent(target)}`;
    }
  };

  if (!isSignedIn) {
    return (
      <div className="mx-auto max-w-6xl px-6 py-12">
        <div className="mb-8">
          <div className="mb-2 inline-flex items-center rounded-sm border border-[var(--text-primary)]/40 px-1.5 py-0.5 text-[9px] font-semibold tracking-[0.14em] text-[var(--text-primary)]">
            BETA
          </div>
          <h1 className="text-[32px] font-bold leading-none tracking-tight text-[var(--text-primary)]">
            Plans
          </h1>
          <p className="mt-2 max-w-2xl text-[15px] leading-relaxed text-[var(--text-secondary)]">
            Paid plans include monthly Allternit Cloud credits and an always-on cloud computer.
            Sign in to subscribe; checkout runs through Stripe.
          </p>
        </div>
        <PlanPicker currentPlanName={null} title="Choose a plan" onSubscribe={handleSubscribe} />
      </div>
    );
  }

  const [entitlement, setEntitlement] = useState<HostedRuntimeEntitlement | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [credits, setCredits] = useState<BillingCredits | null>(null);
  const [creditsError, setCreditsError] = useState<string | null>(null);
  const [billingAvailable, setBillingAvailable] = useState(true);
  const [packs, setPacks] = useState<CreditPack[]>([]);
  const [buyingPackId, setBuyingPackId] = useState<string | null>(null);
  const [checkoutError, setCheckoutError] = useState<string | null>(null);
  const [plans, setPlans] = useState<LiveBillingPlan[]>([]);
  const [subscribingPlanId, setSubscribingPlanId] = useState<PlanId | null>(null);
  const [subscribeError, setSubscribeError] = useState<string | null>(null);
  const [currentSubscription, setCurrentSubscription] = useState<{
    plan_id: string;
    status: string;
  } | null>(null);
  const [planNotice, setPlanNotice] = useState<string | null>(null);
  // `?plan=plus|super|ultra` (the website's Subscribe buttons) starts that
  // plan's checkout once the page knows the account has no plan yet.
  const [requestedPlan, setRequestedPlan] = useState<PlanId | null>(() => {
    if (typeof window === "undefined") return null;
    const params = new URLSearchParams(window.location.search);
    const value = params.get("plan");
    if (value !== "plus" && value !== "super" && value !== "ultra") return null;
    params.delete("plan");
    const search = params.toString();
    window.history.replaceState(
      null,
      "",
      `${window.location.pathname}${search ? `?${search}` : ""}${window.location.hash}`,
    );
    return value;
  });
  const [portalBusy, setPortalBusy] = useState(false);
  const [portalAvailable, setPortalAvailable] = useState(true);
  const [portalError, setPortalError] = useState<string | null>(null);
  const [keysAvailable, setKeysAvailable] = useState(true);
  const [inferenceKeys, setInferenceKeys] = useState<InferenceProviderKey[]>([]);
  const [keysError, setKeysError] = useState<string | null>(null);
  const [addingProvider, setAddingProvider] = useState<string | null>(null);
  const [newKeyValue, setNewKeyValue] = useState("");
  const [keySubmitting, setKeySubmitting] = useState(false);
  const [keySubmitError, setKeySubmitError] = useState<string | null>(null);
  const [removingProvider, setRemovingProvider] = useState<string | null>(null);
  const [checkoutNotice, setCheckoutNotice] = useState<"success" | "cancelled" | null>(() => {
    if (typeof window === "undefined") return null;
    const params = new URLSearchParams(window.location.search);
    const value = params.get("checkout");
    if (value !== "success" && value !== "cancelled") return null;
    params.delete("checkout");
    const search = params.toString();
    window.history.replaceState(
      null,
      "",
      `${window.location.pathname}${search ? `?${search}` : ""}${window.location.hash}`,
    );
    return value;
  });

  const load = useCallback(async () => {
    if (!isSignedIn) {
      setLoading(false);
      return;
    }
    setLoading(true);
    setError(null);
    try {
      const token = await getToken();
      if (!token) throw new Error("A web account session is required to view billing.");
      const data = await getHostedEntitlement(token);
      setEntitlement(data);

      // Credits + packs are a separate billing surface; a 503 from either endpoint means billing is not
      // configured in this environment, so show the disabled state rather than an error.
      try {
        const base = billingApiBaseUrl();
        const [packsResponse, creditsResponse] = await Promise.all([
          fetch(`${base}/api/v1/billing/packs`),
          fetch(`${base}/api/v1/billing/credits`, {
            headers: { Authorization: `Bearer ${token}` },
          }),
        ]);
        if (packsResponse.status === 503 || creditsResponse.status === 503) {
          setBillingAvailable(false);
          setCredits(null);
          setPacks([]);
          setCreditsError(null);
          return;
        }
        setBillingAvailable(true);
        const packsPayload = (packsResponse.ok
          ? await packsResponse.json().catch(() => ({}))
          : {}) as { packs?: CreditPack[] };
        setPacks(Array.isArray(packsPayload.packs) ? packsPayload.packs : []);
        if (!creditsResponse.ok) {
          const payload = await creditsResponse.json().catch(() => ({}));
          throw new Error(
            payload.message ||
              payload.error ||
              `Unable to load credit balance (${creditsResponse.status})`,
          );
        }
        const creditsPayload = (await creditsResponse.json()) as BillingCredits;
        setCredits(creditsPayload);
        setCreditsError(null);
      } catch (err) {
        setCredits(null);
        setCreditsError(formatApiError(err, "Unable to load credit balance"));
      }

      // The account's plan decides Subscribe vs Switch on the plan cards.
      try {
        const subscriptionResponse = await fetch(`${billingApiBaseUrl()}/api/v1/billing/subscription`, {
          headers: { Authorization: `Bearer ${token}` },
        });
        if (subscriptionResponse.ok) {
          const payload = (await subscriptionResponse.json().catch(() => null)) as {
            plan_id?: string;
            status?: string;
          } | null;
          setCurrentSubscription(
            payload?.plan_id && payload.status
              ? { plan_id: payload.plan_id, status: payload.status }
              : null,
          );
        }
      } catch {
        setCurrentSubscription(null);
      }

      // Plans feed the PlanPicker subscribe buttons; a failure here falls back to the static
      // prices in PlanPicker and must not break the page.
      try {
        const plansResponse = await fetch(`${billingApiBaseUrl()}/api/v1/billing/plans`);
        if (plansResponse.status === 503) {
          setBillingAvailable(false);
          setPlans([]);
        } else if (plansResponse.ok) {
          const plansPayload = (await plansResponse.json().catch(() => ({}))) as {
            plans?: LiveBillingPlan[];
          };
          setPlans(Array.isArray(plansPayload.plans) ? plansPayload.plans : []);
        }
      } catch {
        setPlans([]);
      }

      // Inference provider keys: a 503 means the feature isn't configured in this
      // environment — hide the card entirely.
      try {
        const keysResponse = await fetch(`${billingApiBaseUrl()}/api/v1/inference/keys`, {
          headers: { Authorization: `Bearer ${token}` },
        });
        if (keysResponse.status === 503) {
          setKeysAvailable(false);
          setInferenceKeys([]);
        } else if (keysResponse.ok) {
          const keysPayload = (await keysResponse.json().catch(() => ({}))) as {
            keys?: InferenceProviderKey[];
          };
          setKeysAvailable(true);
          setInferenceKeys(Array.isArray(keysPayload.keys) ? keysPayload.keys : []);
          setKeysError(null);
        } else {
          const payload = await keysResponse.json().catch(() => ({}));
          throw new Error(
            payload.error ||
              payload.message ||
              `Unable to load provider keys (${keysResponse.status})`,
          );
        }
      } catch (err) {
        setKeysError(formatApiError(err, "Unable to load provider keys"));
      }
    } catch (err) {
      setError(formatApiError(err, "Unable to load billing details"));
    } finally {
      setLoading(false);
    }
  }, [getToken, isSignedIn]);

  useEffect(() => {
    void load();
  }, [load]);

  const usagePercent = entitlement?.maxHoursMonthly
    ? Math.min(100, (entitlement.usedSecondsMonthly / (entitlement.maxHoursMonthly * 3600)) * 100)
    : 0;

  const handleBuyPack = async (packId: string) => {
    setBuyingPackId(packId);
    setCheckoutError(null);
    try {
      const token = await getToken();
      if (!token) throw new Error("A web account session is required to purchase credits.");
      const response = await fetch(`${billingApiBaseUrl()}/api/v1/billing/checkout`, {
        method: "POST",
        headers: {
          Authorization: `Bearer ${token}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({ pack_id: packId }),
      });
      if (response.status === 503) {
        setBillingAvailable(false);
        return;
      }
      const payload = await response.json().catch(() => ({}));
      if (!response.ok) {
        throw new Error(
          payload.message || payload.error || `Unable to start checkout (${response.status})`,
        );
      }
      const checkoutUrl = safePortalUrl(payload.checkout_url);
      if (!checkoutUrl) throw new Error("Checkout did not return a valid payment link.");
      window.location.href = checkoutUrl;
    } catch (err) {
      setCheckoutError(formatApiError(err, "Unable to start checkout"));
    } finally {
      setBuyingPackId(null);
    }
  };

  const handleSubscribePlan = async (planId: PlanId) => {
    setSubscribingPlanId(planId);
    setSubscribeError(null);
    try {
      const token = await getToken();
      if (!token) throw new Error("A web account session is required to subscribe.");
      const response = await fetch(`${billingApiBaseUrl()}/api/v1/billing/subscribe`, {
        method: "POST",
        headers: {
          Authorization: `Bearer ${token}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({ plan_id: planId }),
      });
      if (response.status === 503) {
        setBillingAvailable(false);
        return;
      }
      const payload = await response.json().catch(() => ({}));
      if (response.status === 409 && payload.error === "already_subscribed") {
        // One plan per account: changes happen in the Stripe billing portal.
        setSubscribingPlanId(null);
        await handleOpenPortal();
        return;
      }
      if (!response.ok) {
        throw new Error(
          payload.message || payload.error || `Unable to start subscription checkout (${response.status})`,
        );
      }
      const checkoutUrl = safePortalUrl(payload.checkout_url);
      if (!checkoutUrl) throw new Error("Checkout did not return a valid payment link.");
      window.location.href = checkoutUrl;
    } catch (err) {
      setSubscribeError(formatApiError(err, "Unable to start subscription checkout"));
    } finally {
      setSubscribingPlanId(null);
    }
  };

  const handleOpenPortal = async () => {
    setPortalBusy(true);
    setPortalError(null);
    try {
      const token = await getToken();
      if (!token) throw new Error("A web account session is required to manage billing.");
      const response = await fetch(`${billingApiBaseUrl()}/api/v1/billing/portal`, {
        method: "POST",
        headers: {
          Authorization: `Bearer ${token}`,
          "Content-Type": "application/json",
        },
        body: "{}",
      });
      if (response.status === 404) {
        // No Stripe customer yet — there is nothing to manage, so hide the button.
        setPortalAvailable(false);
        return;
      }
      const payload = await response.json().catch(() => ({}));
      if (!response.ok) {
        throw new Error(
          payload.message || payload.error || `Unable to open billing portal (${response.status})`,
        );
      }
      const portalUrl = safePortalUrl(payload.portal_url);
      if (!portalUrl) throw new Error("Portal did not return a valid link.");
      window.location.href = portalUrl;
    } catch (err) {
      setPortalError(formatApiError(err, "Unable to open billing portal"));
    } finally {
      setPortalBusy(false);
    }
  };

  const subscribedPlanId: PlanId | null =
    currentSubscription &&
    currentSubscription.status !== "none" &&
    (currentSubscription.plan_id === "plus" ||
      currentSubscription.plan_id === "super" ||
      currentSubscription.plan_id === "ultra")
      ? currentSubscription.plan_id
      : null;

  // Arrived from a website Subscribe button: start that plan's checkout, or
  // say where to switch when the account already has a plan.
  useEffect(() => {
    if (loading || !requestedPlan) return;
    const plan = requestedPlan;
    setRequestedPlan(null);
    const label = (id: string) => id.charAt(0).toUpperCase() + id.slice(1);
    if (subscribedPlanId === plan) {
      setPlanNotice(`You're already on ${label(plan)}.`);
    } else if (subscribedPlanId) {
      setPlanNotice(
        `You're on ${label(subscribedPlanId)}. Switch to ${label(plan)} in Manage billing.`,
      );
    } else if (billingAvailable) {
      void handleSubscribePlan(plan);
    }
    // handleSubscribePlan is recreated each render; run once per request.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [loading, requestedPlan, subscribedPlanId, billingAvailable]);

  const handleAddKey = async () => {
    const providerId = addingProvider;
    const apiKey = newKeyValue.trim();
    if (!providerId || !apiKey) return;
    setKeySubmitting(true);
    setKeySubmitError(null);
    try {
      const token = await getToken();
      if (!token) throw new Error("A web account session is required to manage provider keys.");
      const response = await fetch(`${billingApiBaseUrl()}/api/v1/inference/keys`, {
        method: "PUT",
        headers: {
          Authorization: `Bearer ${token}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({ provider_id: providerId, api_key: apiKey }),
      });
      if (response.status === 503) {
        setKeysAvailable(false);
        return;
      }
      const payload = await response.json().catch(() => ({}));
      if (!response.ok) {
        throw new Error(payload.error || payload.message || `Unable to save key (${response.status})`);
      }
      setAddingProvider(null);
      setNewKeyValue("");
      void load();
    } catch (err) {
      setKeySubmitError(formatApiError(err, "Unable to save provider key"));
    } finally {
      setKeySubmitting(false);
    }
  };

  const handleRemoveKey = async (providerId: string) => {
    setRemovingProvider(providerId);
    try {
      const token = await getToken();
      if (!token) throw new Error("A web account session is required to manage provider keys.");
      const response = await fetch(
        `${billingApiBaseUrl()}/api/v1/inference/keys/${encodeURIComponent(providerId)}`,
        {
          method: "DELETE",
          headers: { Authorization: `Bearer ${token}` },
        },
      );
      if (response.status === 503) {
        setKeysAvailable(false);
        return;
      }
      if (!response.ok) {
        const payload = await response.json().catch(() => ({}));
        throw new Error(
          payload.error || payload.message || `Unable to remove key (${response.status})`,
        );
      }
      void load();
    } catch (err) {
      setKeysError(formatApiError(err, "Unable to remove provider key"));
    } finally {
      setRemovingProvider(null);
    }
  };

  return (
    <div className="space-y-8">
      {checkoutNotice && (
        <div className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]/40 p-4">
          <div className="flex items-start gap-3">
            <Info
              size={18}
              className={`mt-0.5 shrink-0 ${
                checkoutNotice === "success"
                  ? "text-[var(--status-success)]"
                  : "text-[var(--status-warning)]"
              }`}
            />
            <p className="flex-1 text-[13px] leading-relaxed text-[var(--text-secondary)]">
              {checkoutNotice === "success"
                ? "Payment received. Your subscription or credit balance will update once Stripe confirms the checkout."
                : "Checkout cancelled. No charges were made — pick a plan or credit pack below to try again."}
            </p>
            <button
              type="button"
              aria-label="Dismiss"
              onClick={() => setCheckoutNotice(null)}
              className="shrink-0 text-[var(--text-tertiary)] hover:text-[var(--text-primary)] transition-colors"
            >
              <X size={14} />
            </button>
          </div>
        </div>
      )}

      <div>
        <h1 className="text-[22px] font-semibold tracking-tight text-[var(--text-primary)]">
          Credits
        </h1>
        <p className="text-[13px] text-[var(--text-secondary)] mt-1">
          Prepaid credits are consumed before plan-included allowances on paid usage.
        </p>
      </div>

      {!billingAvailable ? (
        <div className="rounded-xl border border-dashed border-[var(--border-subtle)] bg-[var(--bg-secondary)]/40 p-8 text-center">
          <div className="size-12 rounded-full bg-[var(--accent-primary)]/10 text-[var(--accent-primary)] flex items-center justify-center mx-auto mb-4">
            <Coins size={24} />
          </div>
          <h2 className="text-[16px] font-semibold text-[var(--text-primary)] mb-1">
            Billing not configured
          </h2>
          <p className="text-[13px] text-[var(--text-secondary)] max-w-md mx-auto leading-relaxed">
            Credit purchases are not enabled in this environment yet. Check back once billing is
            configured for your organization.
          </p>
        </div>
      ) : (
        <div className="space-y-3">
          {credits?.free_inference && (
            <div className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]/40 p-4">
              <div className="flex items-start gap-3 mb-4">
                <div className="size-9 shrink-0 rounded-lg bg-[var(--accent-primary)]/10 text-[var(--accent-primary)] flex items-center justify-center">
                  <Lightning size={18} />
                </div>
                <div className="min-w-0 flex-1">
                  <div className="text-[14px] font-semibold text-[var(--text-primary)]">Free inference</div>
                  <p className="text-[12px] text-[var(--text-secondary)] mt-1">
                    Monthly hosted-model allowance for free accounts.
                  </p>
                </div>
              </div>

              <div className="space-y-2">
                <div className="flex items-center justify-between gap-3 text-[12px]">
                  <span className="text-[var(--text-secondary)]">
                    ${credits.free_inference.used_usd.toFixed(2)} of $
                    {credits.free_inference.monthly_allowance_usd.toFixed(2)} used this month — resets monthly
                  </span>
                  <span className="font-mono text-[var(--text-primary)]">
                    {credits.free_inference.monthly_allowance_usd > 0
                      ? Math.min(
                          100,
                          (credits.free_inference.used_usd /
                            credits.free_inference.monthly_allowance_usd) *
                            100,
                        ).toFixed(0)
                      : "0"}
                    %
                  </span>
                </div>
                <div className="h-2 rounded-full bg-[var(--bg-primary)] overflow-hidden">
                  <div
                    className="h-full rounded-full bg-[var(--accent-primary)] transition-[width]"
                    style={{
                      width: `${
                        credits.free_inference.monthly_allowance_usd > 0
                          ? Math.min(
                              100,
                              (credits.free_inference.used_usd /
                                credits.free_inference.monthly_allowance_usd) *
                                100,
                            )
                          : 0
                      }%`,
                    }}
                  />
                </div>
                {credits.free_inference.remaining_usd <= 0 && (
                  <p className="text-[12px] text-[var(--status-warning)]">
                    Allowance used up — buy credits or subscribe to keep using hosted models.
                  </p>
                )}
              </div>
            </div>
          )}

          <div className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]/40 p-4">
            <div className="flex items-start gap-3 mb-4">
              <div className="size-9 shrink-0 rounded-lg bg-[var(--accent-primary)]/10 text-[var(--accent-primary)] flex items-center justify-center">
                <Coins size={18} />
              </div>
              <div className="min-w-0 flex-1">
                <div className="text-[14px] font-semibold text-[var(--text-primary)]">Credit balance</div>
                <p className="text-[12px] text-[var(--text-secondary)] mt-1">
                  Purchased credits and recent credit activity.
                </p>
              </div>
            </div>

            {creditsError ? (
              <div className="flex items-center justify-between gap-3">
                <span className="text-[13px] text-[var(--status-error)]">{creditsError}</span>
                <button
                  type="button"
                  onClick={() => void load()}
                  disabled={loading}
                  className={QUIET_BUTTON_CLASS}
                >
                  <ArrowsClockwise size={13} /> Retry
                </button>
              </div>
            ) : (
              <>
                <div className="grid grid-cols-1 sm:grid-cols-2 gap-3 mb-4">
                  <div className="rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)]/60 p-3">
                    <div className="text-[10px] uppercase tracking-wide text-[var(--text-tertiary)] mb-1">Balance</div>
                    <div className="text-[16px] font-semibold font-mono text-[var(--text-primary)]">
                      ${(credits?.balance_usd ?? 0).toFixed(2)}
                    </div>
                  </div>
                  <div className="rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)]/60 p-3">
                    <div className="text-[10px] uppercase tracking-wide text-[var(--text-tertiary)] mb-1">Month-to-date usage</div>
                    <div className="text-[16px] font-semibold font-mono text-[var(--text-primary)]">
                      ${(credits?.month_to_date_usage_usd ?? 0).toFixed(2)}
                    </div>
                  </div>
                </div>

                {(credits?.recent_transactions?.length ?? 0) > 0 ? (
                  <div className="divide-y divide-[var(--border-subtle)]">
                    {credits!.recent_transactions.map((tx, index) => (
                      <div
                        key={tx.id || `${tx.source}-${tx.created_at}-${index}`}
                        className="flex items-center justify-between gap-3 py-2 text-[12px]"
                      >
                        <div className="min-w-0">
                          <div className="truncate font-medium text-[var(--text-primary)]">
                            {tx.source || "—"}
                          </div>
                          <div className="text-[11px] text-[var(--text-tertiary)]">
                            {formatTransactionDate(tx.created_at)}
                          </div>
                        </div>
                        <span
                          className={`font-mono ${
                            tx.amount_usd >= 0
                              ? "text-[var(--status-success)]"
                              : "text-[var(--text-secondary)]"
                          }`}
                        >
                          {formatSignedUsd(tx.amount_usd)}
                        </span>
                      </div>
                    ))}
                  </div>
                ) : (
                  <div className="text-[13px] text-[var(--text-secondary)]">
                    No recent credit transactions.
                  </div>
                )}
              </>
            )}
          </div>

          <TransferToOrgPanel onTransferred={() => void load()} />

          <div className="grid grid-cols-2 lg:grid-cols-4 gap-3">
            {packs.map((pack) => (
              <div
                key={pack.id}
                className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]/40 p-4 flex flex-col"
              >
                <div className="text-[10px] uppercase tracking-wide text-[var(--text-tertiary)] mb-1">
                  {pack.label}
                </div>
                <div className="text-[16px] font-semibold text-[var(--text-primary)]">
                  ${pack.credits_usd.toFixed(2)} credits
                </div>
                <div className="text-[11px] text-[var(--text-secondary)] mt-1 mb-3">
                  Pay ${pack.price_usd.toFixed(2)}
                </div>
                <button
                  type="button"
                  onClick={() => void handleBuyPack(pack.id)}
                  disabled={buyingPackId !== null || !billingAvailable}
                  className={`${QUIET_BUTTON_CLASS} mt-auto justify-center`}
                >
                  {buyingPackId === pack.id ? "Redirecting…" : "Buy credits"}
                </button>
              </div>
            ))}
          </div>

          {checkoutError && (
            <p className="text-[13px] text-[var(--status-error)]">{checkoutError}</p>
          )}
        </div>
      )}

      <PlanPicker
        currentPlanName={entitlement?.planDisplayName}
        subscribedPlanId={subscribedPlanId}
        onSelect={billingAvailable ? (planId) => void handleSubscribePlan(planId) : undefined}
        onSwitch={() => void handleOpenPortal()}
        livePlans={billingAvailable && plans.length > 0 ? plans : undefined}
        busyPlanId={subscribingPlanId}
      />

      {planNotice && <p className="text-[13px] text-[var(--text-secondary)]">{planNotice}</p>}
      {subscribeError && (
        <p className="text-[13px] text-[var(--status-error)]">{subscribeError}</p>
      )}

      {keysAvailable && (
        <div className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]/40 p-4">
          <div className="flex items-start gap-3 mb-4">
            <div className="size-9 shrink-0 rounded-lg bg-[var(--accent-primary)]/10 text-[var(--accent-primary)] flex items-center justify-center">
              <Key size={18} />
            </div>
            <div className="min-w-0 flex-1">
              <div className="text-[14px] font-semibold text-[var(--text-primary)]">
                Provider API keys (bring your own)
              </div>
              <p className="text-[12px] text-[var(--text-secondary)] mt-1">
                Use your own provider keys for inference — usage bills directly to your provider
                account; we meter tokens but charge no credits.
              </p>
            </div>
          </div>

          {loading && inferenceKeys.length === 0 && !keysError ? (
            <SkeletonRow lines={3} />
          ) : keysError ? (
            <div className="flex items-center justify-between gap-3">
              <span className="text-[13px] text-[var(--status-error)]">{keysError}</span>
              <button
                type="button"
                onClick={() => void load()}
                disabled={loading}
                className={QUIET_BUTTON_CLASS}
              >
                <ArrowsClockwise size={13} /> Retry
              </button>
            </div>
          ) : (
            <div className="divide-y divide-[var(--border-subtle)]">
              {INFERENCE_PROVIDERS.map((provider) => {
                const stored = inferenceKeys.find((k) => k.provider_id === provider.id);
                const expanded = addingProvider === provider.id;
                return (
                  <div key={provider.id} className="py-2">
                    <div className="flex items-center justify-between gap-3 text-[12px]">
                      <div className="min-w-0">
                        <div className="truncate font-medium text-[var(--text-primary)]">
                          {provider.label}
                        </div>
                        {stored ? (
                          <div className="text-[11px] text-[var(--text-tertiary)] font-mono">
                            {stored.masked}
                            {stored.last_validated_at
                              ? ` · validated ${formatTransactionDate(stored.last_validated_at)}`
                              : ""}
                          </div>
                        ) : (
                          <div className="text-[11px] text-[var(--text-tertiary)]">
                            No key on file
                          </div>
                        )}
                      </div>
                      {stored ? (
                        <button
                          type="button"
                          onClick={() => void handleRemoveKey(provider.id)}
                          disabled={removingProvider !== null}
                          className={QUIET_BUTTON_CLASS}
                        >
                          {removingProvider === provider.id ? "Removing…" : "Remove"}
                        </button>
                      ) : (
                        <button
                          type="button"
                          onClick={() => {
                            if (expanded) {
                              setAddingProvider(null);
                            } else {
                              setAddingProvider(provider.id);
                            }
                            setNewKeyValue("");
                            setKeySubmitError(null);
                          }}
                          className={QUIET_BUTTON_CLASS}
                        >
                          {expanded ? "Close" : "Add key"}
                        </button>
                      )}
                    </div>
                    {expanded && (
                      <div className="mt-2">
                        <div className="flex items-center gap-2">
                          <input
                            type="password"
                            autoComplete="off"
                            value={newKeyValue}
                            onChange={(e) => setNewKeyValue(e.target.value)}
                            placeholder={`${provider.label} API key`}
                            className="flex-1 rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] px-3 py-1.5 text-[13px] text-[var(--text-primary)] outline-none focus:border-[var(--accent-primary)]"
                          />
                          <button
                            type="button"
                            onClick={() => void handleAddKey()}
                            disabled={keySubmitting || !newKeyValue.trim()}
                            className={QUIET_BUTTON_CLASS}
                          >
                            {keySubmitting ? "Saving…" : "Save"}
                          </button>
                          <button
                            type="button"
                            onClick={() => {
                              setAddingProvider(null);
                              setNewKeyValue("");
                              setKeySubmitError(null);
                            }}
                            className={QUIET_BUTTON_CLASS}
                          >
                            Cancel
                          </button>
                        </div>
                        {keySubmitError && (
                          <p className="mt-2 text-[12px] text-[var(--status-error)]">
                            {keySubmitError}
                          </p>
                        )}
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
          )}
        </div>
      )}

      <div className="rounded-xl border border-[var(--border-subtle)] bg-[var(--bg-secondary)]/40 p-4">
        <div className="flex items-start gap-3">
          <Info size={18} className="mt-0.5 shrink-0 text-[var(--accent-primary)]" />
          <p className="text-[13px] leading-relaxed text-[var(--text-secondary)]">
            During BETA, subscription tiers and model credits are UI-only. Cloud-model access is
            provided through upstream providers such as OpenRouter; paid bundled credits will not
            be sold until appropriate provider terms are in place.
          </p>
        </div>
      </div>

      <div>
        <h1 className="text-[22px] font-semibold tracking-tight text-[var(--text-primary)]">
          Usage & invoices
        </h1>
        <p className="text-[13px] text-[var(--text-secondary)] mt-1">
          Manage invoices, payment methods, and organization billing details.
        </p>
      </div>

      <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3">
        <div className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]/40 p-4">
          <div className="text-[10px] uppercase tracking-wide text-[var(--text-tertiary)] mb-1">Billed organization</div>
          <div className="text-[14px] font-semibold text-[var(--text-primary)]">
            {organization?.name || "Personal"}
          </div>
          <div className="text-[11px] text-[var(--text-secondary)] mt-1">{email}</div>
        </div>

        <div className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]/40 p-4">
          <div className="text-[10px] uppercase tracking-wide text-[var(--text-tertiary)] mb-1">Current plan</div>
          <div className="text-[14px] font-semibold text-[var(--text-primary)]">
            {entitlement?.planDisplayName || "Free"}
          </div>
          <div className="text-[11px] text-[var(--text-secondary)] mt-1">
            {entitlement?.canCreateHostedRuntime ? "Hosted compute enabled" : "No active subscription"}
          </div>
        </div>

        <div className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]/40 p-4">
          <div className="text-[10px] uppercase tracking-wide text-[var(--text-tertiary)] mb-1">Estimated cost</div>
          <div className="text-[14px] font-semibold text-[var(--text-primary)]">
            ${(entitlement?.estimatedCostUsdMonthly || 0).toFixed(2)}
          </div>
          <div className="text-[11px] text-[var(--text-secondary)] mt-1">Month-to-date infrastructure</div>
        </div>
      </div>

      {loading ? (
        <SkeletonRow lines={3} />
      ) : error ? (
        <EmptyState
          icon={<WarningCircle size={28} />}
          title="Billing details unavailable"
          caption={error}
          ctaLabel="Retry"
          onCtaClick={() => void load()}
        />
      ) : (
        <div className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]/40 p-4">
          <div className="flex items-start gap-3 mb-4">
            <div className="size-9 shrink-0 rounded-lg bg-[var(--accent-primary)]/10 text-[var(--accent-primary)] flex items-center justify-center">
              <Gauge size={18} />
            </div>
            <div className="min-w-0 flex-1">
              <div className="text-[14px] font-semibold text-[var(--text-primary)]">Monthly runtime usage</div>
              <p className="text-[12px] text-[var(--text-secondary)] mt-1">
                Managed compute hours accrue toward your plan limit.
              </p>
            </div>
          </div>

          {entitlement && entitlement.maxHoursMonthly > 0 ? (
            <div className="space-y-2">
              <div className="flex items-center justify-between gap-3 text-[12px]">
                <span className="text-[var(--text-secondary)]">
                  {formatHours(entitlement.usedSecondsMonthly)} / {entitlement.maxHoursMonthly} hr
                </span>
                <span className="font-mono text-[var(--text-primary)]">{usagePercent.toFixed(0)}%</span>
              </div>
              <div className="h-2 rounded-full bg-[var(--bg-primary)] overflow-hidden">
                <div
                  className="h-full rounded-full bg-[var(--accent-primary)] transition-[width]"
                  style={{ width: `${usagePercent}%` }}
                />
              </div>
              <div className="flex items-center justify-between gap-3 text-[11px] text-[var(--text-tertiary)]">
                <span>{formatHours(entitlement.remainingSecondsMonthly)} remaining</span>
                <span>${entitlement.estimatedCostUsdMonthly.toFixed(2)} estimated</span>
              </div>
            </div>
          ) : (
            <div className="text-[13px] text-[var(--text-secondary)]">
              No hosted runtime quota on the current plan. Pick a paid tier above to enable managed compute.
            </div>
          )}

          <div className="flex items-center flex-wrap gap-2 mt-5 pt-4 border-t border-[var(--border-subtle)]">
            {(() => {
              const tier = (entitlement?.planTierId || "").toLowerCase();
              return tier === "pro" || tier === "team" ? (
                <span className="inline-flex items-center rounded-sm border border-[var(--status-success)]/30 bg-[var(--status-success)]/10 px-1.5 py-0.5 text-[10px] font-semibold tracking-[0.08em] text-[var(--status-success)]">
                  Subscription active: {entitlement?.planTierId}
                </span>
              ) : null;
            })()}
            {portalAvailable && (
              <button
                type="button"
                onClick={() => void handleOpenPortal()}
                disabled={portalBusy}
                className={QUIET_BUTTON_CLASS}
              >
                {portalBusy ? "Opening…" : "Manage subscription"} <ArrowSquareOut size={13} />
              </button>
            )}
            {(() => {
              const portalUrl = safePortalUrl(entitlement?.billingPortalUrl);
              return portalUrl ? (
                <a
                  href={portalUrl}
                  target="_blank"
                  rel="noopener noreferrer"
                  className={QUIET_BUTTON_CLASS}
                >
                  Billing portal <ArrowSquareOut size={13} />
                </a>
              ) : null;
            })()}
            <button
              type="button"
              onClick={() => void load()}
              disabled={loading}
              className={QUIET_BUTTON_CLASS}
            >
              <ArrowsClockwise size={13} /> Refresh
            </button>
            {portalError && (
              <span className="text-[12px] text-[var(--status-error)]">{portalError}</span>
            )}
          </div>
        </div>
      )}

      <div className="rounded-xl border border-dashed border-[var(--border-subtle)] bg-[var(--bg-secondary)]/40 p-8 text-center">
        <div className="size-12 rounded-full bg-[var(--accent-primary)]/10 text-[var(--accent-primary)] flex items-center justify-center mx-auto mb-4">
          <Receipt size={24} />
        </div>
        <h2 className="text-[16px] font-semibold text-[var(--text-primary)] mb-1">
          Invoice history
        </h2>
        <p className="text-[13px] text-[var(--text-secondary)] max-w-md mx-auto leading-relaxed">
          Invoices and usage exports will appear here once billing is enabled for your organization.
        </p>
      </div>
    </div>
  );
}
