import React, { useEffect, useMemo, useState } from "react";

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

const MONO = "ui-monospace, 'SF Mono', Menlo, monospace";

/** Card text per tier, on the engraved Gizzi art (same as the app's PlanCard). */
const TIER_INK: Record<PlanId, { text: string; sub: string; stamp: [string, string]; light: boolean }> = {
  free: { text: "#1b1b1a", sub: "rgba(0,0,0,.55)", stamp: ["#1b1b1a", "#fff"], light: false },
  plus: { text: "#F2F1EC", sub: "rgba(255,255,255,.6)", stamp: ["#F2F1EC", "#111"], light: true },
  super: { text: "#2B1C07", sub: "rgba(43,28,7,.62)", stamp: ["#2B1C07", "#E7C27A"], light: false },
  ultra: { text: "#1E2024", sub: "rgba(30,32,36,.6)", stamp: ["#1E2024", "#F1F1EF"], light: false },
};

/** The account's plan as a member card: tier art, member name, renewal date, payment stamp. */
function MemberCard({ plan, member, renewsAt, status }: { plan: Plan; member: string; renewsAt?: string | null; status?: string | null }) {
  const ink = TIER_INK[plan.id];
  const renews = renewsAt ? new Date(renewsAt).toLocaleDateString(undefined, { month: "short", day: "numeric" }).toUpperCase() : null;
  const stamp = status === "past_due" ? "PAYMENT DUE" : status === "unpaid" ? "PAUSED" : null;
  return (
    <div
      role="img"
      aria-label={`${plan.label} plan card for ${member}${renews ? `, renews ${renews}` : ""}`}
      className="relative w-[340px] max-w-full overflow-hidden rounded-[15px] bg-cover"
      style={{ aspectRatio: "1.586", backgroundImage: `url(${plan.art})`, color: ink.text, boxShadow: plan.id === "ultra" ? "0 18px 36px rgba(0,0,0,.28), inset 0 0 0 1px rgba(255,255,255,.5)" : "0 14px 30px rgba(0,0,0,.22)" }}
    >
      {plan.id === "ultra" && (
        <div
          aria-hidden="true"
          className="absolute inset-[5px] rounded-[12px] opacity-90"
          style={{ padding: 1, background: "conic-gradient(#c9ccd2, #f6e7b8, #bfe8dd, #c4d2ff, #e9c9f2, #c9ccd2)", WebkitMask: "linear-gradient(#000 0 0) content-box, linear-gradient(#000 0 0)", WebkitMaskComposite: "xor", maskComposite: "exclude" }}
        />
      )}
      <div className="absolute left-[5.5%] top-[8%] flex items-center gap-2">
        <img src={`/billing/wordmark-${ink.light ? "light" : "ink"}.svg`} alt="A://TERNIT" style={{ height: 11 }} />
        <span style={{ fontFamily: MONO, fontSize: 10, fontWeight: 700, letterSpacing: ".14em", padding: "2px 5px", borderRadius: 3, background: ink.stamp[0], color: ink.stamp[1] }}>{plan.label}</span>
      </div>
      {stamp && (
        <div className="absolute left-[5.5%] top-[32%] -rotate-6" style={{ fontFamily: MONO, fontWeight: 700, fontSize: 11, letterSpacing: ".1em", color: "#d4483a", border: "1.5px solid #d4483a", borderRadius: 4, padding: "2px 6px", background: "rgba(255,255,255,.55)" }}>
          {stamp}
        </div>
      )}
      <div className="absolute bottom-[7%] left-[5.5%]" style={{ fontFamily: MONO }}>
        <div style={{ fontSize: 9, letterSpacing: ".16em", color: ink.sub }}>MEMBER</div>
        <div className="truncate" style={{ maxWidth: 170, fontSize: 15, letterSpacing: ".05em", marginTop: 2 }}>{member.toUpperCase()}</div>
        {renews && plan.id !== "free" && (
          <>
            <div style={{ fontSize: 9, letterSpacing: ".16em", color: ink.sub, marginTop: 6 }}>RENEWS</div>
            <div style={{ fontSize: 12, letterSpacing: ".05em", marginTop: 1 }}>{renews}</div>
          </>
        )}
      </div>
    </div>
  );
}
const CORAL = "#D97757";

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
    art: "/billing/card-free.jpg",
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
    art: "/billing/card-plus.jpg",
  },
  {
    id: "super",
    label: "MAX",
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
    art: "/billing/card-max.jpg",
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
    art: "/billing/card-ultra.jpg",
  },
];

function planFromDisplayName(name?: string | null): PlanId {
  const n = (name || "").toLowerCase();
  if (n.includes("ultra")) return "ultra";
  if (n.includes("super") || n.includes("max")) return "super";
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
  memberName,
  renewsAt,
  status,
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
  /** Shown on the member card when the account has a paid plan. */
  memberName?: string | null;
  renewsAt?: string | null;
  status?: string | null;
}) {
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

      {subscribedPlanId && memberName ? (
        <MemberCard plan={PLANS.find((p) => p.id === subscribedPlanId) ?? PLANS[0]} member={memberName} renewsAt={renewsAt} status={status} />
      ) : null}

      <div className="grid grid-cols-1 gap-3 lg:grid-cols-2" role="list" aria-label="Plans">
        {PLANS.map((plan) => {
          const isCurrent = selected === plan.id;
          const ink = TIER_INK[plan.id];
          const livePlan = livePlans?.find((live) => live.id === plan.id);
          const buttonStyle = { backgroundColor: ink.stamp[0], color: ink.stamp[1] };
          const btn = "w-full rounded-[11px] px-2 py-2 text-[10.5px] font-semibold tracking-[0.12em] transition-opacity hover:opacity-90 disabled:cursor-not-allowed disabled:opacity-50";

          const action = onSelect ? (
            plan.id === "free" ? (
              <span className={`${btn} block text-center opacity-60`} style={buttonStyle}>FREE TIER</span>
            ) : plan.id === subscribedPlanId ? (
              <span className={`${btn} block border text-center`} style={{ borderColor: ink.text }}>CURRENT PLAN</span>
            ) : subscribedPlanId && onSwitch ? (
              <button type="button" onClick={onSwitch} className={btn} style={buttonStyle}>SWITCH IN MANAGE BILLING</button>
            ) : (
              <button type="button" onClick={() => onSelect(plan.id)} disabled={busyPlanId != null} className={btn} style={buttonStyle}>
                {busyPlanId === plan.id ? "REDIRECTING…" : `SUBSCRIBE — ${livePlan ? formatLivePrice(livePlan.price_usd) : plan.price}/MO`}
              </button>
            )
          ) : (
            <button
              type="button"
              onClick={() => {
                if (onSubscribe) onSubscribe(plan.id);
                else setSelected(plan.id);
              }}
              className={btn}
              style={buttonStyle}
            >
              {isCurrent && !onSubscribe ? "CURRENT" : "SUBSCRIBE"}
            </button>
          );

          return (
            <article
              key={plan.id}
              role="listitem"
              className="relative flex min-h-[170px] overflow-hidden rounded-2xl bg-no-repeat"
              style={{
                color: ink.text,
                backgroundImage: `url(${plan.art})`,
                backgroundSize: "160% auto",
                backgroundPosition: "0% 30%",
                border: plan.id === "free" ? "1px solid var(--border-subtle)" : undefined,
                boxShadow: plan.id === "ultra" ? "0 10px 22px rgba(0,0,0,.22), inset 0 0 0 1px rgba(255,255,255,.55)" : "0 8px 18px rgba(0,0,0,.16)",
                outline: plan.id === subscribedPlanId ? `2px solid ${CORAL}` : undefined,
                outlineOffset: 1,
              }}
            >
              <div className="min-w-0 flex-1 px-4 py-3.5">
                <div className="flex items-baseline gap-2">
                  <span style={{ fontFamily: MONO, fontSize: 11, fontWeight: 700, letterSpacing: ".14em", padding: "2px 6px", borderRadius: 3, background: ink.stamp[0], color: ink.stamp[1] }}>{plan.label}</span>
                  {plan.bonus && <span className="text-[9px] font-semibold tracking-[0.12em]" style={{ color: ink.sub }}>10% BONUS</span>}
                  <span className="ml-auto" style={{ fontFamily: MONO, fontSize: 24 }}>{livePlan ? formatLivePrice(livePlan.price_usd) : plan.price}</span>
                  <span className="text-[10px]" style={{ color: ink.sub }}>/MO</span>
                </div>
                <ul className="mt-3 space-y-1 text-[11px] font-medium tracking-[0.06em]">
                  {plan.features.map((feature) => (
                    <li key={feature}>{feature}</li>
                  ))}
                </ul>
              </div>
              <div className="relative flex w-[150px] shrink-0 items-end p-2.5" style={{ borderLeft: `1.5px dashed ${ink.light ? "rgba(255,255,255,.3)" : "rgba(0,0,0,.25)"}` }}>
                {action}
              </div>
              <i aria-hidden="true" className="absolute -top-[9px] size-[18px] rounded-full bg-[var(--bg-primary)]" style={{ right: 141 }} />
              <i aria-hidden="true" className="absolute -bottom-[9px] size-[18px] rounded-full bg-[var(--bg-primary)]" style={{ right: 141 }} />
            </article>
          );
        })}
      </div>

    </section>
  );
}
