import React, { useCallback, useEffect, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { CheckCircle, CircleNotch, CreditCard, ArrowSquareOut } from "@phosphor-icons/react";
import { formatApiError } from "@/lib/api-client";
import type { PlatformProject } from "@/lib/platform-projects";
import {
  ACCEPTABLE_USE_URL,
  DEVELOPER_TERMS_URL,
  getProjectBilling,
  openBillingPortal,
  startPlanCheckout,
  type PlatformPlanId,
  type ProjectBilling,
} from "@/lib/platform-billing";
import { SkeletonCard } from "@/components/console-ui";
import { QUIET_BUTTON_CLASS } from "@/components/console-ui/buttonStyles";
import { cn } from "@/lib/utils";
import { CARD_CLASS, ErrorBanner, PlatformHeader, PRIMARY_BUTTON_CLASS, ProjectGate, formatDate, usePlatformProjects } from "./shared";

/**
 * `/platform/billing`: add a payment method and choose a plan for a project.
 * The API's 402 `payment_method_required` links here with `?project=`.
 */
export function PlatformBillingPage() {
  const state = usePlatformProjects();
  const [params] = useSearchParams();
  const wanted = params.get("project");
  const { select, projects } = state;

  useEffect(() => {
    if (wanted && projects.some((p) => p.id === wanted)) select(wanted);
  }, [wanted, projects, select]);

  return (
    <div className="mx-auto w-full max-w-5xl">
      <PlatformHeader
        title="Billing"
        subtitle="Every project needs a card on file before it can run agents, place calls, text, buy numbers or use computers. There is no free usage."
        state={state}
      />
      <ProjectGate state={state}>{({ project }) => <BillingBody key={project.id} project={project} outcome={params.get("checkout")} />}</ProjectGate>
    </div>
  );
}

function statusLabel(b: ProjectBilling): { text: string; tone: "ok" | "warn" | "none" } {
  if (b.payment_method && b.status === "past_due") return { text: "Card on file, last payment failed. Stripe is retrying; update your card.", tone: "warn" };
  if (b.payment_method) return { text: `Card on file since ${formatDate(b.payment_method_added_at)}.`, tone: "ok" };
  if (b.status === "unpaid") return { text: "Payment failed and Stripe stopped retrying. Billable requests are refused until you update your card.", tone: "warn" };
  if (b.status === "canceled") return { text: "The plan was cancelled. Choose a plan to continue.", tone: "warn" };
  return { text: "No payment method yet. Billable requests answer 402 payment_method_required.", tone: "none" };
}

function BillingBody({ project, outcome }: { project: PlatformProject; outcome: string | null }) {
  const [billing, setBilling] = useState<ProjectBilling | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<PlatformPlanId | "portal" | null>(null);
  const [accepted, setAccepted] = useState(false);

  const load = useCallback(async () => {
    setError(null);
    try {
      setBilling(await getProjectBilling(project.id));
    } catch (err) {
      setError(formatApiError(err, "Unable to load billing"));
    } finally {
      setLoading(false);
    }
  }, [project.id]);

  useEffect(() => {
    void load();
  }, [load]);

  // Back from Checkout: the webhook can land a few seconds after the redirect.
  useEffect(() => {
    if (outcome !== "success" || billing?.payment_method) return;
    let tries = 0;
    const timer = window.setInterval(() => {
      tries += 1;
      void load();
      if (tries >= 10) window.clearInterval(timer);
    }, 3000);
    return () => window.clearInterval(timer);
  }, [outcome, billing?.payment_method, load]);

  const choose = useCallback(
    async (plan: PlatformPlanId) => {
      setBusy(plan);
      setError(null);
      try {
        window.location.assign(await startPlanCheckout(project.id, plan));
      } catch (err) {
        setError(formatApiError(err, "Unable to start checkout"));
        setBusy(null);
      }
    },
    [project.id],
  );

  const portal = useCallback(async () => {
    setBusy("portal");
    setError(null);
    try {
      window.location.assign(await openBillingPortal(project.id));
    } catch (err) {
      setError(formatApiError(err, "Unable to open the billing portal"));
      setBusy(null);
    }
  }, [project.id]);

  if (loading) {
    return (
      <div className="space-y-3" role="status" aria-label="Loading billing">
        <SkeletonCard />
        <SkeletonCard rows={2} />
      </div>
    );
  }
  if (!billing) return <ErrorBanner message={error ?? "Unable to load billing"} />;

  const status = statusLabel(billing);
  const waiting = outcome === "success" && !billing.payment_method;

  return (
    <div className="space-y-4">
      {error && <ErrorBanner message={error} />}
      {outcome === "cancelled" && !billing.payment_method && (
        <p className="m-0 text-[13px] text-[var(--text-secondary)]">Checkout was cancelled. Nothing was charged.</p>
      )}

      <section className={cn(CARD_CLASS, "p-4")} aria-label="Payment method">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div className="flex min-w-0 items-start gap-2">
            {billing.payment_method && billing.status === "active" ? (
              <CheckCircle size={18} className="mt-0.5 shrink-0 text-[var(--status-success)]" aria-hidden />
            ) : (
              <CreditCard size={18} className="mt-0.5 shrink-0 text-[var(--text-tertiary)]" aria-hidden />
            )}
            <div className="min-w-0">
              <div className="text-[14px] font-semibold text-[var(--text-primary)]">
                {billing.payment_method ? `Plan: ${billing.plan === "growth" ? "Growth" : billing.env === "sandbox" ? "Pay as you go (sandbox)" : "Pay as you go"}` : "Add a payment method"}
              </div>
              <div role="status" className={cn("mt-0.5 text-[13px]", status.tone === "warn" ? "text-[var(--status-warning)]" : "text-[var(--text-secondary)]")}>
                {waiting ? (
                  <span className="inline-flex items-center gap-1.5">
                    <CircleNotch size={14} className="animate-spin" aria-hidden /> Confirming your payment with Stripe…
                  </span>
                ) : (
                  status.text
                )}
              </div>
              <div className="mt-1 text-[12px] text-[var(--text-tertiary)]">
                Monthly spend cap ${(billing.spend_cap_cents / 100).toLocaleString()} (change it on the Projects page).
              </div>
            </div>
          </div>
          {billing.portal_available && (
            <button type="button" className={QUIET_BUTTON_CLASS} disabled={busy !== null} onClick={() => void portal()}>
              {busy === "portal" ? <CircleNotch size={14} className="animate-spin" aria-hidden /> : <ArrowSquareOut size={14} aria-hidden />} Manage billing
            </button>
          )}
        </div>
      </section>

      <section aria-label="Plans" className="grid grid-cols-1 gap-3 md:grid-cols-2">
        {billing.plans.map((plan) => {
          const current = billing.payment_method && (billing.plan === plan.id || (billing.env === "sandbox" && plan.id === "payg"));
          return (
            <div key={plan.id} className={cn(CARD_CLASS, "flex flex-col gap-2 p-4", current && "border-[var(--accent-primary)]/40")}>
              <div className="flex items-baseline justify-between gap-2">
                <div className="text-[15px] font-semibold text-[var(--text-primary)]">{plan.name}</div>
                <div className="text-[13px] tabular-nums text-[var(--text-secondary)]">
                  {plan.monthly_cents === 0 ? "$0 / month + usage" : `$${(plan.monthly_cents / 100).toLocaleString()} / month`}
                </div>
              </div>
              <p className="m-0 flex-1 text-[13px] text-[var(--text-secondary)]">{plan.summary}</p>
              {current ? (
                <div className="text-[12px] font-semibold text-[var(--accent-primary)]">Current plan</div>
              ) : (
                <button
                  type="button"
                  className={cn(PRIMARY_BUTTON_CLASS, "self-start")}
                  disabled={!plan.available || !billing.checkout_available || !accepted || busy !== null}
                  onClick={() => void choose(plan.id)}
                >
                  {busy === plan.id ? <CircleNotch size={14} className="animate-spin" aria-hidden /> : <CreditCard size={14} aria-hidden />}
                  {billing.payment_method ? `Switch to ${plan.name}` : `Add card · ${plan.name}`}
                </button>
              )}
              {!plan.available && <div className="text-[12px] text-[var(--text-tertiary)]">Available on live projects.</div>}
            </div>
          );
        })}
      </section>

      <label className="flex items-start gap-2 text-[13px] text-[var(--text-primary)]">
        <input type="checkbox" className="mt-0.5" checked={accepted} onChange={(e) => setAccepted(e.target.checked)} />
        <span>
          I accept the{" "}
          <a href={billing.terms_url || DEVELOPER_TERMS_URL} target="_blank" rel="noreferrer" className="font-semibold text-[var(--accent-primary)] hover:underline">
            Developer Terms
          </a>{" "}
          and the{" "}
          <a href={billing.acceptable_use_url || ACCEPTABLE_USE_URL} target="_blank" rel="noreferrer" className="font-semibold text-[var(--accent-primary)] hover:underline">
            Acceptable Use Policy
          </a>
          .
        </span>
      </label>
      {!billing.checkout_available && <ErrorBanner message="Billing isn't set up on this deployment yet." />}
      <p className="m-0 text-[12px] text-[var(--text-tertiary)]">
        Payments are handled by Stripe. Usage is billed monthly at the prices on the{" "}
        <a href="https://docs.allternit.com/api/platform/pricing" target="_blank" rel="noreferrer" className="text-[var(--accent-primary)] hover:underline">
          pricing page
        </a>
        . Sandbox usage is never billed, but sandbox projects still need a card.
      </p>
    </div>
  );
}
