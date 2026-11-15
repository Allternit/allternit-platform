import React, { useCallback, useEffect, useMemo, useState } from "react";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  AlertCircleIcon,
  DollarIcon,
  Refresh01Icon,
} from "@hugeicons/core-free-icons";
import {
  Bar,
  BarChart,
  CartesianGrid,
  Cell,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import { cn } from "@/lib/utils";
import { formatApiError } from "@/lib/api-client";
import {
  ListPage,
  EmptyState,
  SkeletonRow,
  StatCard,
  QUIET_BUTTON_CLASS,
  SETTINGS_SELECT_CLASS,
} from "@/components/console-ui";
import {
  getCostBreakdown,
  getCostSummary,
  type CostBreakdownItem,
  type CostSummary,
} from "@/lib/usage";
import { downloadCsv, formatUsd, sumReportRows, type ReportGroupBy } from "@/lib/gateway-analytics";
import { isCloudGateway } from "@/lib/availability";
import {
  ErrorRetry,
  MUTED as CLOUD_MUTED,
  RefreshButton,
  ReportFilters,
  ReportTable,
  dayLabel,
  useCsvDownload,
  useReportFilters,
  useUsageReport,
} from "./cloudReport";

const GROUP_BYS = [
  { value: "provider", label: "By provider" },
  { value: "region", label: "By region" },
  { value: "instance_type", label: "By instance type" },
];

const BAR_COLORS = ["#f59e0b", "#B08D6E", "#22c55e", "#3b82f6", "#a855f7", "#ef4444"];
const MUTED = "#71717a";
const TREND_MONTHS = 6;

function monthLabel(month: string): string {
  const [year, monthNum] = month.split("-");
  const date = new Date(Number(year), Number(monthNum) - 1, 1);
  return date.toLocaleString("en-US", { month: "short" });
}

function shiftMonth(month: string, delta: number): string {
  const [year, monthNum] = month.split("-").map(Number);
  const date = new Date(year, monthNum - 1 + delta, 1);
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}`;
}

function currentMonth(): string {
  const now = new Date();
  return `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, "0")}`;
}

function formatCost(cost: number, currency = "USD"): string {
  return new Intl.NumberFormat("en-US", {
    style: "currency",
    currency,
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  }).format(cost);
}

function dimensionOf(row: CostBreakdownItem, groupBy: string): string {
  if (groupBy === "region") return row.region;
  if (groupBy === "instance_type") return row.instance_type;
  return row.provider;
}

function RuntimeCostPage(): React.ReactNode {
  const [month, setMonth] = useState(currentMonth());
  const [groupBy, setGroupBy] = useState("provider");
  const [summary, setSummary] = useState<CostSummary | null>(null);
  const [breakdown, setBreakdown] = useState<CostBreakdownItem[]>([]);
  const [trend, setTrend] = useState<Array<{ month: string; label: string; cost: number }>>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [summaryData, breakdownData] = await Promise.all([
        getCostSummary(month),
        getCostBreakdown(groupBy, month),
      ]);
      setSummary(summaryData);
      setBreakdown(breakdownData);

      // The costs API reports one month per call — assemble the trend by
      // fetching the trailing window in parallel.
      const trendMonths = Array.from({ length: TREND_MONTHS }, (_, i) =>
        shiftMonth(month, -(TREND_MONTHS - 1 - i))
      );
      const trendResults = await Promise.allSettled(
        trendMonths.map((m) => getCostSummary(m))
      );
      setTrend(
        trendMonths.map((m, i) => {
          const result = trendResults[i];
          return {
            month: m,
            label: monthLabel(m),
            cost: result.status === "fulfilled" ? result.value.current_month_cost : 0,
          };
        })
      );
    } catch (err) {
      setError(formatApiError(err, "Unable to load cost data"));
      setSummary(null);
      setBreakdown([]);
      setTrend([]);
    } finally {
      setLoading(false);
    }
  }, [month, groupBy]);

  useEffect(() => {
    void load();
  }, [load]);

  const chartData = useMemo(
    () =>
      breakdown.map((row) => ({
        name: dimensionOf(row, groupBy),
        cost: row.total_cost,
      })),
    [breakdown, groupBy]
  );

  const exportCsv = useCallback(() => {
    downloadCsv(
      `allternit-cost-${groupBy}-${month}.csv`,
      [
        "Provider",
        "Region",
        "Instance type",
        "Cost (USD)",
        "Runs",
        "Duration (hours)",
      ],
      breakdown.map((row) => [
        row.provider,
        row.region,
        row.instance_type,
        row.total_cost.toFixed(2),
        row.run_count,
        row.total_duration_hours.toFixed(2),
      ])
    );
  }, [breakdown, groupBy, month]);

  return (
    <ListPage
      title="Cost"
      subtitle="Hosted runtime spend for the selected month, from the cloud cost ledger."
      filters={
        <>
          <input
            type="month"
            value={month}
            onChange={(e) => e.target.value && setMonth(e.target.value)}
            aria-label="Month"
            className={SETTINGS_SELECT_CLASS}
          />
          <select
            value={groupBy}
            onChange={(e) => setGroupBy(e.target.value)}
            aria-label="Group by"
            className={SETTINGS_SELECT_CLASS}
          >
            {GROUP_BYS.map((option) => (
              <option key={option.value} value={option.value}>
                {option.label}
              </option>
            ))}
          </select>
        </>
      }
      primaryAction={{ label: "Export CSV", onClick: exportCsv }}
    >
      {error && (
        <p className="mb-4 flex items-center gap-2 rounded-lg border border-solid border-[var(--status-error)]/30 bg-[var(--status-error)]/10 px-3 py-2 text-[13px] text-[var(--status-error)]">
          <HugeiconsIcon icon={AlertCircleIcon} size={14} />
          {error}
        </p>
      )}

      {loading && !summary ? (
        <SkeletonRow lines={6} />
      ) : !summary ? null : (
        <div className="space-y-4">
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
            <StatCard
              label="Current cost"
              value={formatCost(summary.current_month_cost, summary.currency)}
              hint={month}
            />
            <StatCard
              label="Runtime hours"
              value={summary.total_duration_hours.toFixed(1)}
              hint={`${summary.run_count.toLocaleString()} runs`}
            />
            <StatCard
              label="Budget utilization"
              value={`${summary.budget_utilization_percent.toFixed(0)}%`}
              hint={`of ${formatCost(summary.monthly_budget, summary.currency)} monthly budget`}
            />
          </div>

          <div className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4">
            <h3 className="mb-2 text-[11px] font-semibold uppercase tracking-wider text-[var(--text-tertiary)]">
              Cost over the last {TREND_MONTHS} months
            </h3>
            <div className="h-48">
              <ResponsiveContainer width="100%" height="100%">
                <BarChart data={trend} margin={{ left: -12, right: 8, top: 4, bottom: 0 }}>
                  <CartesianGrid stroke="var(--border-subtle)" vertical={false} />
                  <XAxis
                    dataKey="label"
                    axisLine={false}
                    tickLine={false}
                    tick={{ fill: MUTED, fontSize: 10 }}
                  />
                  <YAxis
                    axisLine={false}
                    tickLine={false}
                    tick={{ fill: MUTED, fontSize: 10 }}
                    tickFormatter={(v: number) => `$${v}`}
                  />
                  <Tooltip
                    cursor={{ fill: "var(--surface-hover)" }}
                    contentStyle={{
                      backgroundColor: "var(--bg-secondary)",
                      border: "1px solid var(--border-subtle)",
                      borderRadius: "8px",
                    }}
                    formatter={(v: number) => formatCost(v, summary.currency)}
                  />
                  <Bar dataKey="cost" radius={[4, 4, 0, 0]} barSize={28}>
                    {trend.map((entry) => (
                      <Cell
                        key={entry.month}
                        fill={
                          entry.month === month
                            ? "var(--accent-primary)"
                            : BAR_COLORS[0]
                        }
                      />
                    ))}
                  </Bar>
                </BarChart>
              </ResponsiveContainer>
            </div>
          </div>

          {breakdown.length === 0 ? (
            <EmptyState
              icon={<HugeiconsIcon icon={DollarIcon} size={32} />}
              title="No cost data"
              caption={`No hosted runtime spend recorded for ${month}. Start a hosted runtime and its cost will appear here.`}
            />
          ) : (
            <div className="overflow-x-auto rounded-xl border border-solid border-[var(--border-subtle)]">
              <table className="w-full border-collapse text-left text-[13px]">
                <thead>
                  <tr className="border-b border-solid border-[var(--border-subtle)] text-[11px] uppercase tracking-wide text-[var(--text-tertiary)]">
                    <th className="px-3 py-2 font-medium">{GROUP_BYS.find((g) => g.value === groupBy)?.label.replace("By ", "")}</th>
                    <th className="px-3 py-2 font-medium">Runs</th>
                    <th className="px-3 py-2 font-medium">Duration (hours)</th>
                    <th className="px-3 py-2 font-medium">Cost</th>
                  </tr>
                </thead>
                <tbody>
                  {breakdown.map((row) => (
                    <tr
                      key={`${row.provider}|${row.region}|${row.instance_type}`}
                      className="border-b border-solid border-[var(--border-subtle)] last:border-b-0 hover:bg-[var(--surface-hover)]"
                    >
                      <td className="px-3 py-2.5 font-mono text-[12px] text-[var(--text-primary)]">
                        {dimensionOf(row, groupBy)}
                      </td>
                      <td className="px-3 py-2.5 text-[var(--text-secondary)]">
                        {row.run_count.toLocaleString()}
                      </td>
                      <td className="px-3 py-2.5 text-[var(--text-secondary)]">
                        {row.total_duration_hours.toFixed(1)}
                      </td>
                      <td className="px-3 py-2.5 text-[var(--text-secondary)]">
                        {formatCost(row.total_cost, summary.currency)}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}

          <p className="m-0 text-[12px] text-[var(--text-tertiary)]">
            Covers hosted compute runtime only. LLM gateway spend is on the{" "}
            <a href="/analytics/usage" className="text-[var(--accent-primary)]">
              Usage
            </a>{" "}
            page.
          </p>
          <button
            type="button"
            onClick={() => void load()}
            disabled={loading}
            className={cn(QUIET_BUTTON_CLASS, "text-[12px]")}
          >
            <HugeiconsIcon
              icon={Refresh01Icon}
              size={12}
              className={cn(loading && "animate-spin")}
            />
            {loading ? "Refreshing…" : "Refresh"}
          </button>
        </div>
      )}
    </ListPage>
  );
}

const COST_GROUPS: ReportGroupBy[] = ["model", "project", "key", "client", "day"];

/** Allternit Cloud: inference spend from the per-user report. */
function CloudCostPage(): React.ReactNode {
  const filters = useReportFilters(30);
  const [groupBy, setGroupBy] = useState<ReportGroupBy>("model");
  const base = { from: filters.from, to: filters.to, projectId: filters.projectId, scope: filters.scope };
  const daily = useUsageReport({ ...base, groupBy: "day" }, "Unable to load cost data");
  const grouped = useUsageReport(groupBy === "day" ? null : { ...base, groupBy }, "Unable to load cost data");
  const csv = useCsvDownload({ ...base, groupBy }, `allternit-cost-${groupBy}-${filters.from}.csv`);

  const dayRows = daily.rows ?? [];
  const tableRows = groupBy === "day" ? dayRows : grouped.rows ?? [];
  const totals = useMemo(() => sumReportRows(dayRows), [dayRows]);
  const chartData = useMemo(
    () =>
      dayRows
        .slice()
        .sort((a, b) => a.group.localeCompare(b.group))
        .map((r) => ({ day: dayLabel(r.group), cost: r.cost_usd })),
    [dayRows],
  );
  const loading = daily.loading || (groupBy !== "day" && grouped.loading);
  const error = daily.error ?? (groupBy !== "day" ? grouped.error : null) ?? csv.error;
  const reload = () => {
    daily.reload();
    if (groupBy !== "day") grouped.reload();
  };
  const perThousand = totals.requests > 0 ? (totals.cost_usd / totals.requests) * 1000 : null;

  return (
    <ListPage
      title="Cost"
      subtitle="Inference spend for billed requests, by day and by what drove it."
      filters={
        <ReportFilters filters={filters}>
          <select
            value={groupBy}
            onChange={(e) => setGroupBy(e.target.value as ReportGroupBy)}
            aria-label="Group by"
            className={SETTINGS_SELECT_CLASS}
          >
            {COST_GROUPS.map((g) => (
              <option key={g} value={g}>
                Group by {g === "key" ? "API key" : g}
              </option>
            ))}
          </select>
        </ReportFilters>
      }
      primaryAction={{ label: csv.busy ? "Preparing…" : "Download CSV", onClick: () => void csv.run() }}
    >
      {error && <ErrorRetry message={error} onRetry={reload} />}
      {loading && daily.rows === null ? (
        <SkeletonRow lines={6} />
      ) : daily.rows === null ? null : (
        <div className="space-y-4">
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
            <StatCard label="Spend" value={formatUsd(totals.cost_usd)} hint={`Last ${filters.rangeDays} days`} />
            <StatCard label="Requests" value={totals.requests.toLocaleString()} hint="Billed requests" />
            <StatCard
              label="Per 1,000 requests"
              value={perThousand === null ? "—" : formatUsd(perThousand)}
            />
          </div>
          {chartData.length === 0 ? (
            <EmptyState
              icon={<HugeiconsIcon icon={DollarIcon} size={32} />}
              title="No spend in this range"
              caption="Billed requests from your projects will show their cost here."
            />
          ) : (
            <>
              <div className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4">
                <h3 className="mb-2 text-[11px] font-semibold uppercase tracking-wider text-[var(--text-tertiary)]">
                  Spend per day
                </h3>
                <div className="h-48">
                  <ResponsiveContainer width="100%" height="100%">
                    <BarChart data={chartData} margin={{ left: -12, right: 8, top: 4, bottom: 0 }}>
                      <CartesianGrid stroke="var(--border-subtle)" vertical={false} />
                      <XAxis dataKey="day" axisLine={false} tickLine={false} tick={{ fill: CLOUD_MUTED, fontSize: 10 }} />
                      <YAxis
                        axisLine={false}
                        tickLine={false}
                        tick={{ fill: CLOUD_MUTED, fontSize: 10 }}
                        tickFormatter={(v: number) => `$${v}`}
                      />
                      <Tooltip
                        cursor={{ fill: "var(--surface-hover)" }}
                        contentStyle={{
                          backgroundColor: "var(--bg-secondary)",
                          border: "1px solid var(--border-subtle)",
                          borderRadius: "8px",
                        }}
                        formatter={(v: number) => formatUsd(v)}
                      />
                      <Bar dataKey="cost" name="Spend" fill="var(--accent-primary)" radius={[4, 4, 0, 0]} />
                    </BarChart>
                  </ResponsiveContainer>
                </div>
              </div>
              <ReportTable rows={tableRows} groupBy={groupBy} projectNames={filters.projectNames} />
            </>
          )}
          <p className="m-0 text-[12px] text-[var(--text-tertiary)]">
            Only billed requests are counted. Plan charges and credits are on the{" "}
            <a href="/platform/billing" className="text-[var(--accent-primary)]">
              Billing
            </a>{" "}
            page.
          </p>
          <RefreshButton loading={loading} onClick={reload} />
        </div>
      )}
    </ListPage>
  );
}

export function CostPage(): React.ReactNode {
  return isCloudGateway() ? <CloudCostPage /> : <RuntimeCostPage />;
}

export default CostPage;
