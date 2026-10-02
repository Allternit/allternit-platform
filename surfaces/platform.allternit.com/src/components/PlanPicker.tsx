import React, { useEffect, useMemo, useState } from "react";
import { useTheme } from "@/lib/theme";

export type PlanId = "free" | "plus" | "super" | "ultra";

export interface LiveBillingPlan {
  id: string;
  label: string;
  price_usd: number;
  monthly_credits_usd: number;
  rollover_cap_usd: number;
  plan_tier: string;
}

function formatLivePrice(value: number): string {
  return `$${Number.isInteger(value) ? value.toFixed(0) : value.toFixed(2)}`;
}

interface Plan {
  id: PlanId;
  label: string;
  price: string;
  bonus: boolean;
  hero: boolean;
  features: string[];
  art: string;
}

const IVORY = "#FDF8F3";
const CORAL = "#D97757";
const GRAPHITE = "#111111";

const PLANS: Plan[] = [
  {
    id: "free",
    label: "FREE",
    price: "$0",
    bonus: false,
    hero: false,
    features: [
      "LOCAL MODELS ONLY",
      "STANDARD RATE LIMITS",
      "$0 MONTHLY CREDITS",
    ],
    art: "/billing/free.png",
  },
  {
    id: "plus",
    label: "PLUS",
    price: "$20",
    bonus: true,
    hero: false,
    features: [
      "$22 MONTHLY CREDITS",
      "$10 ROLLOVER CAP",
      "LOCAL + CLOUD MODELS",
      "HOSTED TOOL USAGE",
      "HIGH RATE LIMITS",
    ],
    art: "/billing/plus.png",
  },
  {
    id: "super",
    label: "SUPER",
    price: "$100",
    bonus: true,
    hero: true,
    features: [
      "$110 MONTHLY CREDITS",
      "$50 ROLLOVER CAP",
      "LOCAL + CLOUD MODELS",
      "HOSTED TOOL USAGE",
      "HIGH RATE LIMITS",
    ],
    art: "/billing/super.png",
  },
  {
    id: "ultra",
    label: "ULTRA",
    price: "$200",
    bonus: true,
    hero: false,
    features: [
      "$220 MONTHLY CREDITS",
      "$100 ROLLOVER CAP",
      "LOCAL + CLOUD MODELS",
      "HOSTED TOOL USAGE",
      "HIGH RATE LIMITS",
    ],
    art: "/billing/ultra.png",
  },
];

function planFromDisplayName(name?: string | null): PlanId {
  const n = (name || "").toLowerCase();
  if (n.includes("ultra")) return "ultra";
  if (n.includes("super")) return "super";
  if (n.includes("plus")) return "plus";
  return "free";
}

export function PlanPicker({
  currentPlanName,
  title = "Manage Subscription",
  subscribedPlanId,
  onSubscribe,
  onSelect,
  onSwitch,
  livePlans,
  busyPlanId,
}: {
  currentPlanName?: string | null;
  /** The account's paid plan, when it has one: its card says Current and
   * the other paid cards switch in the billing portal instead of starting a
   * second subscription. */
  subscribedPlanId?: PlanId | null;
  title?: string;
  onSubscribe?: (planId: PlanId) => void;
  onSelect?: (planId: PlanId) => void;
  onSwitch?: () => void;
  livePlans?: LiveBillingPlan[];
  busyPlanId?: PlanId | null;
}) {
  const { resolved } = useTheme();
  const isDark = resolved === "dark";
  const inferred = useMemo(
    () => subscribedPlanId ?? planFromDisplayName(currentPlanName),
    [subscribedPlanId, currentPlanName],
  );
  const [selected, setSelected] = useState<PlanId>(inferred);

  useEffect(() => {
    setSelected(inferred);
  }, [inferred]);

  return (
    <section className="space-y-5">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <div className="mb-2 inline-flex items-center rounded-sm border border-[var(--text-primary)]/40 px-1.5 py-0.5 text-[9px] font-semibold tracking-[0.14em] text-[var(--text-primary)]">
            BETA
          </div>
          <h2 className="text-[28px] font-bold leading-none tracking-tight text-[var(--text-primary)]">
            {title}
          </h2>
          <p className="mt-2 max-w-2xl text-[13px] leading-relaxed text-[var(--text-secondary)]">
            Paid plans include monthly Allternit Cloud credits, local + cloud models, built-in
            tool use and an always-on cloud computer.
          </p>
        </div>
      </div>

      <div className="grid grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-4">
        {PLANS.map((plan) => {
          const isCurrent = selected === plan.id;
          const hero = plan.hero;
          const bg = hero ? CORAL : isDark ? GRAPHITE : "var(--bg-secondary)";
          const text = hero ? IVORY : isDark ? IVORY : "var(--text-primary)";
          const mutedText = hero ? IVORY : isDark ? IVORY : "var(--text-secondary)";
          const buttonBg = hero ? "#0A0A0A" : isDark ? IVORY : "var(--text-primary)";
          const buttonText = hero ? IVORY : isDark ? "#0A0A0A" : "var(--bg-primary)";
          const livePlan = livePlans?.find((live) => live.id === plan.id);

          return (
            <article
              key={plan.id}
              className="flex h-full flex-col rounded-2xl border border-[var(--border-subtle)] p-4"
              style={{ backgroundColor: bg, color: text }}
            >
              <div className="flex items-start justify-between gap-2">
                <span
                  className="rounded-sm border px-1.5 py-0.5 text-[9px] font-semibold tracking-[0.14em]"
                  style={{ borderColor: `${text}40` }}
                >
                  {plan.label}
                </span>
                {plan.bonus && (
                  <span
                    className="rounded-sm border px-1.5 py-0.5 text-[9px] font-semibold tracking-[0.12em]"
                    style={{ borderColor: `${text}40` }}
                  >
                    10% BONUS
                  </span>
                )}
              </div>

              <div className="mt-3">
                <div className="text-[34px] font-bold leading-none tracking-tight">{plan.price}</div>
                <div className="mt-1 text-[10px] font-semibold tracking-[0.16em] opacity-80">
                  PER MONTH
                </div>
              </div>

              <div className="mt-4 overflow-hidden rounded-md">
                <img
                  src={plan.art}
                  alt=""
                  className="aspect-[2.4/1] w-full object-cover"
                />
              </div>

              <ul className="mt-4 flex-1 space-y-1.5 text-[11px] font-medium tracking-[0.08em]" style={{ color: mutedText }}>
                {plan.features.map((feature) => (
                  <li key={feature} className="flex items-start gap-2">
                    <span className="mt-1 inline-block size-1.5 shrink-0 bg-current opacity-80" />
                    <span className="leading-snug">{feature}</span>
                  </li>
                ))}
              </ul>

              {onSelect ? (
                plan.id === "free" ? (
                  <span
                    className="mt-5 self-start rounded-md px-4 py-2 text-[11px] font-semibold tracking-[0.14em] opacity-50"
                    style={{
                      backgroundColor: buttonBg,
                      color: buttonText,
                    }}
                  >
                    FREE TIER
                  </span>
                ) : plan.id === subscribedPlanId ? (
                  <span
                    className="mt-5 self-start rounded-md border px-4 py-2 text-[11px] font-semibold tracking-[0.14em]"
                    style={{ borderColor: `${text}60` }}
                  >
                    CURRENT PLAN
                  </span>
                ) : subscribedPlanId && onSwitch ? (
                  <button
                    type="button"
                    onClick={onSwitch}
                    className="mt-5 self-start rounded-md px-4 py-2 text-[11px] font-semibold tracking-[0.14em] transition-opacity hover:opacity-90"
                    style={{
                      backgroundColor: buttonBg,
                      color: buttonText,
                    }}
                  >
                    SWITCH IN MANAGE BILLING
                  </button>
                ) : (
                  <button
                    type="button"
                    onClick={() => onSelect(plan.id)}
                    disabled={busyPlanId != null}
                    className="mt-5 self-start rounded-md px-4 py-2 text-[11px] font-semibold tracking-[0.14em] transition-opacity hover:opacity-90 disabled:opacity-50 disabled:cursor-not-allowed"
                    style={{
                      backgroundColor: buttonBg,
                      color: buttonText,
                    }}
                  >
                    {busyPlanId === plan.id
                      ? "REDIRECTING…"
                      : `SUBSCRIBE — ${livePlan ? formatLivePrice(livePlan.price_usd) : plan.price}/MO`}
                  </button>
                )
              ) : (
                <button
                  type="button"
                  onClick={() => {
                    if (onSubscribe) {
                      onSubscribe(plan.id);
                    } else {
                      setSelected(plan.id);
                    }
                  }}
                  className="mt-5 self-start rounded-md px-4 py-2 text-[11px] font-semibold tracking-[0.14em] transition-opacity hover:opacity-90"
                  style={{
                    backgroundColor: buttonBg,
                    color: buttonText,
                  }}
                >
                  {isCurrent && !onSubscribe ? "CURRENT" : "SUBSCRIBE"}
                </button>
              )}
            </article>
          );
        })}
      </div>

    </section>
  );
}
