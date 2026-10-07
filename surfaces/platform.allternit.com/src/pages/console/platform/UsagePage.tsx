import React, { useEffect, useState } from "react";
import { ChartBar } from "@phosphor-icons/react";
import { formatApiError } from "@/lib/api-client";
import { USAGE_METER_LABELS, type V1Client, type V1Usage } from "@/lib/platform-v1";
import { EmptyState, SkeletonRow } from "@/components/console-ui";
import { SETTINGS_SELECT_CLASS } from "@/components/console-ui/buttonStyles";
import { cn } from "@/lib/utils";
import { CARD_CLASS, ErrorBanner, PlatformHeader, ProjectGate, formatDate, usePlatformProjects } from "./shared";

type GroupBy = "meter" | "key" | "account";

const RANGES = [
  { days: 7, label: "Last 7 days" },
  { days: 30, label: "Last 30 days" },
  { days: 90, label: "Last 90 days" },
];

/** `GET /v1/usage` for the selected project: recorded usage by meter, key or account. */
export function PlatformUsagePage() {
  const state = usePlatformProjects();
  return (
    <div className="mx-auto w-full max-w-5xl">
      <PlatformHeader title="Usage" subtitle="Recorded usage for this project. Billing uses the same meters; see the pricing page for rates." state={state} />
      <ProjectGate state={state}>{({ project, client }) => <UsageBody key={project.id} client={client} />}</ProjectGate>
    </div>
  );
}

function formatQty(n: number): string {
  return n.toLocaleString(undefined, { maximumFractionDigits: 2 });
}

function groupLabel(groupBy: GroupBy, group: string | null): string {
  if (groupBy === "meter") return USAGE_METER_LABELS[group ?? ""] ?? group ?? "—";
  if (group === null) return groupBy === "account" ? "No account" : "—";
  if (groupBy === "key" && group.startsWith("console:")) return "Console";
  return group;
}

function UsageBody({ client }: { client: V1Client }) {
  const [groupBy, setGroupBy] = useState<GroupBy>("meter");
  const [days, setDays] = useState(30);
  const [usage, setUsage] = useState<V1Usage | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    setLoading(true);
    setError(null);
    const to = new Date();
    const from = new Date(to.getTime() - days * 86_400_000);
    client
      .usage({ group_by: groupBy, from: from.toISOString(), to: to.toISOString() })
      .then((u) => active && setUsage(u))
      .catch((err) => active && setError(formatApiError(err, "Unable to load usage")))
      .finally(() => active && setLoading(false));
    return () => {
      active = false;
    };
  }, [client, groupBy, days]);

  const rows = usage?.data ?? [];

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center gap-3">
        <label className="inline-flex items-center gap-2 text-[12px] font-medium text-[var(--text-secondary)]">
          Range
          <select value={days} onChange={(e) => setDays(Number(e.target.value))} className={SETTINGS_SELECT_CLASS}>
            {RANGES.map((r) => (
              <option key={r.days} value={r.days}>
                {r.label}
              </option>
            ))}
          </select>
        </label>
        <div role="radiogroup" aria-label="Group by" className="inline-flex rounded-lg border border-solid border-[var(--border-subtle)] p-0.5">
          {(["meter", "key", "account"] as const).map((g) => (
            <button
              key={g}
              type="button"
              role="radio"
              aria-checked={groupBy === g}
              onClick={() => setGroupBy(g)}
              className={cn(
                "rounded-md px-3 py-1 text-[12px] font-medium capitalize transition-colors",
                groupBy === g ? "bg-[var(--surface-hover)] text-[var(--text-primary)]" : "text-[var(--text-secondary)] hover:text-[var(--text-primary)]",
              )}
            >
              By {g}
            </button>
          ))}
        </div>
      </div>

      {error && <ErrorBanner message={error} />}

      <section className={cn(CARD_CLASS, "overflow-hidden")} aria-label="Usage rows" aria-busy={loading}>
        {loading ? (
          <div className="px-4" role="status" aria-label="Loading usage">
            <SkeletonRow lines={3} />
          </div>
        ) : rows.length === 0 ? (
          <EmptyState icon={<ChartBar size={32} aria-hidden />} title="No usage in this range" caption="Usage appears here as your agents talk, call, text and hold numbers." />
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full min-w-[480px] border-collapse text-left text-[13px]">
              <thead>
                <tr className="border-b border-solid border-[var(--border-subtle)] text-[12px] text-[var(--text-tertiary)]">
                  <th scope="col" className="px-4 py-2.5 font-medium">{groupBy === "meter" ? "Meter" : groupBy === "key" ? "API key" : "Account"}</th>
                  {groupBy !== "meter" && <th scope="col" className="px-4 py-2.5 font-medium">Meter</th>}
                  <th scope="col" className="px-4 py-2.5 text-right font-medium">Quantity</th>
                  <th scope="col" className="px-4 py-2.5 text-right font-medium">Events</th>
                </tr>
              </thead>
              <tbody>
                {rows.map((r, i) => (
                  <tr key={`${r.group}-${r.meter}-${i}`} className="border-b border-solid border-[var(--border-subtle)] last:border-0">
                    <td className={cn("px-4 py-2.5 text-[var(--text-primary)]", groupBy !== "meter" && "font-mono text-[12px]")}>{groupLabel(groupBy, r.group)}</td>
                    {groupBy !== "meter" && <td className="px-4 py-2.5 text-[var(--text-secondary)]">{USAGE_METER_LABELS[r.meter] ?? r.meter}</td>}
                    <td className="px-4 py-2.5 text-right tabular-nums text-[var(--text-primary)]">
                      {formatQty(r.quantity)} {r.unit ? <span className="text-[var(--text-tertiary)]">{r.unit}</span> : null}
                    </td>
                    <td className="px-4 py-2.5 text-right tabular-nums text-[var(--text-secondary)]">{r.events.toLocaleString()}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </section>
      {usage && (
        <p className="m-0 text-[12px] text-[var(--text-tertiary)]">
          {formatDate(usage.from)} to {formatDate(usage.to)}.
        </p>
      )}
    </div>
  );
}
