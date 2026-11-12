import React, { useCallback, useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  AlertCircleIcon,
  Analytics01Icon,
  Refresh01Icon,
  SourceCodeIcon,
} from "@hugeicons/core-free-icons";
import {
  Bar,
  BarChart,
  CartesianGrid,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import { cn } from "@/lib/utils";
import { AllternitApiError, formatApiError } from "@/lib/api-client";
import {
  EmptyState,
  ListPage,
  SkeletonRow,
  StatCard,
  QUIET_BUTTON_CLASS,
} from "@/components/console-ui";
import {
  downloadCsv,
  formatTokenCount,
  microdollarsToUsd,
} from "@/lib/gateway-analytics";
import {
  getGizziCodeUsage,
  gizziDateDaysAgo,
  gizziTomorrow,
  type GizziCodeBucketRow,
  type GizziCodeGranularity,
} from "@/lib/gizzi-usage";
import { usePlatformOrganization } from "@/lib/platform-auth-client";
import { isCloudGateway } from "@/lib/availability";
import { formatUsd, sumReportRows } from "@/lib/gateway-analytics";
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
} from "@/pages/console/analytics/cloudReport";

const RANGES: Array<{ label: string; days: number; granularity: GizziCodeGranularity }> = [
  { label: "7d", days: 7, granularity: "day" },
  { label: "30d", days: 30, granularity: "day" },
  { label: "90d", days: 90, granularity: "week" },
];

const MUTED = "#71717a";

/**
 * Telemetry is opt-in per client: gizzi-code only reports when
 * GIZZI_TELEMETRY=1 (cmd/gizzi-code/src/runtime/services/telemetry/
 * gizziUsageTelemetry.ts:8). Zero rows is the expected state, not a failure.
 */
const TELEMETRY_OFF_CAPTION =
  "Gizzi Code telemetry is off. Set GIZZI_TELEMETRY=1 on clients to enable opt-in usage stats.";

function bucketLabel(bucket: string, granularity: GizziCodeGranularity): string {
  if (granularity === "week") {
    // Backend week buckets are strftime('%Y-%W') — display as-is.
    return bucket;
  }
  if (granularity === "month") return bucket;
  const date = new Date(`${bucket}T00:00:00Z`);
  if (Number.isNaN(date.getTime())) return bucket;
  return date.toLocaleDateString("en-US", { month: "short", day: "numeric", timeZone: "UTC" });
}

function RuntimeGizziUsagePage(): React.ReactNode {
  const { organization, isLoaded: orgLoaded } = usePlatformOrganization();
  const navigate = useNavigate();

  const [rangeDays, setRangeDays] = useState(30);
  const [rows, setRows] = useState<GizziCodeBucketRow[] | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  /** 403 (no org / not admin) — a different honest state than "telemetry off". */
  const [forbidden, setForbidden] = useState<string | null>(null);

  const range = RANGES.find((r) => r.days === rangeDays) ?? RANGES[1];

  const load = useCallback(async () => {
    if (!organization) return;
    setLoading(true);
    setError(null);
    setForbidden(null);
    try {
      const response = await getGizziCodeUsage({
        organizationId: organization.id,
        start: gizziDateDaysAgo(range.days),
        end: gizziTomorrow(),
        granularity: range.granularity,
      });
      setRows(response.items ?? []);
    } catch (err) {
      setRows(null);
      if (err instanceof AllternitApiError && err.isAuthError()) {
        setForbidden(err.message);
      } else {
        setError(formatApiError(err, "Unable to load Gizzi Code usage"));
      }
    } finally {
      setLoading(false);
    }
  }, [organization, range.days, range.granularity]);

  useEffect(() => {
    if (orgLoaded) void load();
  }, [load, orgLoaded]);

  const totals = useMemo(() => {
    const acc = {
      linesAccepted: 0,
      toolCallsAccepted: 0,
      toolCallsRejected: 0,
      sessions: 0,
      costMicrodollars: 0,
      promptTokens: 0,
      completionTokens: 0,
    };
    for (const row of rows ?? []) {
      acc.linesAccepted += row.lines_accepted;
      acc.toolCallsAccepted += row.tool_calls_accepted;
      acc.toolCallsRejected += row.tool_calls_rejected;
      // unique_sessions is per-bucket; sum is an upper bound across buckets.
      acc.sessions += row.unique_sessions;
      acc.costMicrodollars += row.cost_microdollars;
      acc.promptTokens += row.prompt_tokens;
      acc.completionTokens += row.completion_tokens;
    }
    return acc;
  }, [rows]);

  const hasActivity =
    totals.linesAccepted > 0 ||
    totals.toolCallsAccepted > 0 ||
    totals.toolCallsRejected > 0 ||
    totals.costMicrodollars > 0 ||
    (rows ?? []).some((row) => row.events > 0);

  const chartData = useMemo(
    () =>
      (rows ?? [])
        .slice()
        .sort((a, b) => a.bucket.localeCompare(b.bucket))
        .map((row) => ({
          bucket: bucketLabel(row.bucket, range.granularity),
          lines: row.lines_accepted,
        })),
    [rows, range.granularity]
  );

  const acceptRate =
    totals.toolCallsAccepted + totals.toolCallsRejected > 0
      ? (totals.toolCallsAccepted /
          (totals.toolCallsAccepted + totals.toolCallsRejected)) *
        100
      : null;

  const exportCsv = useCallback(() => {
    if (!rows) return;
    downloadCsv(
      `allternit-gizzi-usage-${gizziDateDaysAgo(range.days)}-to-${gizziTomorrow()}.csv`,
      ["Bucket", "Sessions (unique/bucket)", "Events", "Lines accepted", "Tool calls accepted", "Tool calls rejected", "Tokens in", "Tokens out", "Spend (USD)"],
      rows.map((row) => [
        row.bucket,
        row.unique_sessions,
        row.events,
        row.lines_accepted,
        row.tool_calls_accepted,
        row.tool_calls_rejected,
        row.prompt_tokens,
        row.completion_tokens,
        (row.cost_microdollars / 1_000_000).toFixed(4),
      ])
    );
  }, [rows, range.days]);

  return (
    <ListPage
      title="Gizzi Code usage"
      subtitle="Opt-in usage and spend for Gizzi Code, the Allternit coding agent — sessions, accepted code, and cost estimates across your organization."
      filters={
        <div className="inline-flex items-center rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-0.5">
          {RANGES.map((r) => (
            <button
              key={r.days}
              type="button"
              onClick={() => setRangeDays(r.days)}
              className={cn(
                "rounded-lg px-3 py-1 text-[12px] font-semibold transition-colors",
                rangeDays === r.days
                  ? "bg-[var(--accent-primary)] text-[var(--ui-text-inverse)]"
                  : "text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
              )}
            >
              {r.label}
            </button>
          ))}
        </div>
      }
      primaryAction={{
        label: "Export CSV",
        onClick: exportCsv,
      }}
    >
      {error && (
        <p className="mb-4 flex items-center gap-2 rounded-lg border border-solid border-[var(--status-error)]/30 bg-[var(--status-error)]/10 px-3 py-2 text-[13px] text-[var(--status-error)]">
          <HugeiconsIcon icon={AlertCircleIcon} size={14} />
          {error}
        </p>
      )}

      {loading && rows === null ? (
        <SkeletonRow lines={6} />
      ) : forbidden ? (
        <EmptyState
          icon={<HugeiconsIcon icon={SourceCodeIcon} size={32} />}
          title="No access"
          caption={forbidden}
        />
      ) : !organization ? (
        <EmptyState
          icon={<HugeiconsIcon icon={SourceCodeIcon} size={32} />}
          title="No organization selected"
          caption="Gizzi Code usage is aggregated per organization. Select or create an organization to see it."
          ctaLabel="Organizations"
          onCtaClick={() => navigate("/organizations")}
          primaryCta
        />
      ) : !hasActivity ? (
        <EmptyState
          icon={<HugeiconsIcon icon={Analytics01Icon} size={32} />}
          title="No Gizzi Code usage in this range"
          caption={TELEMETRY_OFF_CAPTION}
        />
      ) : (
        <div className="space-y-4">
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-4">
            <StatCard
              label="Lines of code accepted"
              value={totals.linesAccepted.toLocaleString()}
              hint="Added lines from edits applied to disk"
            />
            <StatCard
              label="Suggestion accept rate"
              value={acceptRate === null ? "—" : `${acceptRate.toFixed(1)}%`}
              hint="Accepted vs declined edit/tool decisions"
            />
            <StatCard
              label="Sessions"
              value={totals.sessions.toLocaleString()}
              hint="Upper bound — bucket-unique sessions summed"
            />
            <StatCard
              label="Spend estimate"
              value={microdollarsToUsd(totals.costMicrodollars)}
              hint={`${formatTokenCount(totals.promptTokens)} in · ${formatTokenCount(totals.completionTokens)} out`}
            />
          </div>

          {chartData.some((d) => d.lines > 0) ? (
            <div className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4">
              <h3 className="mb-2 text-[11px] font-semibold uppercase tracking-wider text-[var(--text-tertiary)]">
                Lines accepted per {range.granularity}
              </h3>
              <div className="h-52">
                <ResponsiveContainer width="100%" height="100%">
                  <BarChart data={chartData} margin={{ left: -12, right: 8, top: 4, bottom: 0 }}>
                    <CartesianGrid stroke="var(--border-subtle)" vertical={false} />
                    <XAxis
                      dataKey="bucket"
                      axisLine={false}
                      tickLine={false}
                      tick={{ fill: MUTED, fontSize: 10 }}
                    />
                    <YAxis
                      axisLine={false}
                      tickLine={false}
                      tick={{ fill: MUTED, fontSize: 10 }}
                    />
                    <Tooltip
                      contentStyle={{
                        backgroundColor: "var(--bg-secondary)",
                        border: "1px solid var(--border-subtle)",
                        borderRadius: "8px",
                      }}
                    />
                    <Bar
                      dataKey="lines"
                      name="Lines accepted"
                      fill="var(--accent-primary)"
                      radius={[3, 3, 0, 0]}
                    />
                  </BarChart>
                </ResponsiveContainer>
              </div>
            </div>
          ) : null}

          <div className="overflow-x-auto rounded-xl border border-solid border-[var(--border-subtle)]">
            <table className="w-full border-collapse text-left text-[13px]">
              <thead>
                <tr className="border-b border-solid border-[var(--border-subtle)] text-[11px] uppercase tracking-wide text-[var(--text-tertiary)]">
                  <th className="px-3 py-2 font-medium">Period</th>
                  <th className="px-3 py-2 font-medium">Sessions</th>
                  <th className="px-3 py-2 font-medium">Events</th>
                  <th className="px-3 py-2 font-medium">Lines accepted</th>
                  <th className="px-3 py-2 font-medium">Tokens in</th>
                  <th className="px-3 py-2 font-medium">Tokens out</th>
                  <th className="px-3 py-2 font-medium">Spend</th>
                </tr>
              </thead>
              <tbody>
                {chartData.length === 0 ? (
                  <tr>
                    <td colSpan={7} className="px-3 py-6 text-center text-[var(--text-tertiary)]">
                      No rows in this range.
                    </td>
                  </tr>
                ) : (
                  (rows ?? [])
                    .slice()
                    .sort((a, b) => b.bucket.localeCompare(a.bucket))
                    .map((row) => (
                      <tr
                        key={row.bucket}
                        className="border-b border-solid border-[var(--border-subtle)] last:border-b-0 hover:bg-[var(--surface-hover)]"
                      >
                        <td className="px-3 py-2.5 text-[var(--text-primary)]">
                          {bucketLabel(row.bucket, range.granularity)}
                        </td>
                        <td className="px-3 py-2.5 text-[var(--text-secondary)]">
                          {row.unique_sessions.toLocaleString()}
                        </td>
                        <td className="px-3 py-2.5 text-[var(--text-secondary)]">
                          {row.events.toLocaleString()}
                        </td>
                        <td className="px-3 py-2.5 text-[var(--text-secondary)]">
                          {row.lines_accepted.toLocaleString()}
                        </td>
                        <td className="px-3 py-2.5 text-[var(--text-secondary)]">
                          {formatTokenCount(row.prompt_tokens)}
                        </td>
                        <td className="px-3 py-2.5 text-[var(--text-secondary)]">
                          {formatTokenCount(row.completion_tokens)}
                        </td>
                        <td className="px-3 py-2.5 text-[var(--text-secondary)]">
                          {microdollarsToUsd(row.cost_microdollars)}
                        </td>
                      </tr>
                    ))
                )}
              </tbody>
            </table>
          </div>

          <p className="m-0 text-[12px] text-[var(--text-tertiary)]">
            Telemetry is opt-in: clients report only when GIZZI_TELEMETRY=1 is
            set. Events carry no file paths, prompts, or tool arguments. Spend
            is the client-reported cost estimate.
          </p>
          <button
            type="button"
            onClick={() => void load()}
            disabled={loading}
            className={cn(QUIET_BUTTON_CLASS, "text-[12px]")}
          >
            <HugeiconsIcon icon={Refresh01Icon} size={12} className={cn(loading && "animate-spin")} />
            {loading ? "Refreshing…" : "Refresh"}
          </button>
        </div>
      )}
    </ListPage>
  );
}

/**
 * Allternit Cloud: Gizzi Code usage is the per-user report filtered to
 * client=gizzi-code (set by the X-Allternit-Client header). Grouped by day and
 * model; tool-call and lines columns are runtime-only and not shown.
 */
function CloudGizziUsagePage(): React.ReactNode {
  const filters = useReportFilters(30);
  const base = {
    from: filters.from,
    to: filters.to,
    projectId: filters.projectId,
    scope: filters.scope,
    client: "gizzi-code",
  };
  const daily = useUsageReport({ ...base, groupBy: "day" }, "Unable to load Gizzi Code usage");
  const byModel = useUsageReport({ ...base, groupBy: "model" }, "Unable to load Gizzi Code usage");
  const csv = useCsvDownload({ ...base, groupBy: "day" }, `allternit-gizzi-code-usage-${filters.from}.csv`);

  const dayRows = daily.rows ?? [];
  const totals = useMemo(() => sumReportRows(dayRows), [dayRows]);
  const chartData = useMemo(
    () =>
      dayRows
        .slice()
        .sort((a, b) => a.group.localeCompare(b.group))
        .map((r) => ({ day: dayLabel(r.group), tokens: r.prompt_tokens + r.completion_tokens })),
    [dayRows],
  );
  const loading = daily.loading || byModel.loading;
  const error = daily.error ?? byModel.error ?? csv.error;
  const reload = () => {
    daily.reload();
    byModel.reload();
  };

  return (
    <ListPage
      title="Gizzi Code usage"
      subtitle="Requests, tokens and spend from Gizzi Code signed in to Allternit Cloud."
      filters={<ReportFilters filters={filters} />}
      primaryAction={{ label: csv.busy ? "Preparing…" : "Download CSV", onClick: () => void csv.run() }}
    >
      {error && <ErrorRetry message={error} onRetry={reload} />}
      {loading && daily.rows === null ? (
        <SkeletonRow lines={6} />
      ) : daily.rows === null ? null : totals.requests === 0 ? (
        <EmptyState
          icon={<HugeiconsIcon icon={Analytics01Icon} size={32} />}
          title="No Gizzi Code usage in this range"
          caption="Sign in to Allternit Cloud in Gizzi Code and run a prompt. Billed requests from Gizzi Code show up here."
        />
      ) : (
        <div className="space-y-4">
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
            <StatCard
              label="Requests"
              value={totals.requests.toLocaleString()}
              hint={`${formatTokenCount(totals.prompt_tokens)} in · ${formatTokenCount(totals.completion_tokens)} out`}
            />
            <StatCard label="Tokens" value={formatTokenCount(totals.prompt_tokens + totals.completion_tokens)} />
            <StatCard label="Spend" value={formatUsd(totals.cost_usd)} hint="Billed requests only" />
          </div>
          <div className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4">
            <h3 className="mb-2 text-[11px] font-semibold uppercase tracking-wider text-[var(--text-tertiary)]">
              Tokens per day
            </h3>
            <div className="h-52">
              <ResponsiveContainer width="100%" height="100%">
                <BarChart data={chartData} margin={{ left: -12, right: 8, top: 4, bottom: 0 }}>
                  <CartesianGrid stroke="var(--border-subtle)" vertical={false} />
                  <XAxis dataKey="day" axisLine={false} tickLine={false} tick={{ fill: CLOUD_MUTED, fontSize: 10 }} />
                  <YAxis axisLine={false} tickLine={false} tick={{ fill: CLOUD_MUTED, fontSize: 10 }} tickFormatter={(v: number) => formatTokenCount(v)} />
                  <Tooltip
                    contentStyle={{
                      backgroundColor: "var(--bg-secondary)",
                      border: "1px solid var(--border-subtle)",
                      borderRadius: "8px",
                    }}
                    formatter={(v: number) => formatTokenCount(v)}
                  />
                  <Bar dataKey="tokens" name="Tokens" fill="var(--accent-primary)" radius={[3, 3, 0, 0]} />
                </BarChart>
              </ResponsiveContainer>
            </div>
          </div>
          <h3 className="m-0 text-[11px] font-semibold uppercase tracking-wider text-[var(--text-tertiary)]">By model</h3>
          <ReportTable rows={byModel.rows ?? []} groupBy="model" projectNames={filters.projectNames} />
          <h3 className="m-0 text-[11px] font-semibold uppercase tracking-wider text-[var(--text-tertiary)]">By day</h3>
          <ReportTable rows={dayRows} groupBy="day" projectNames={filters.projectNames} />
          <p className="m-0 text-[12px] text-[var(--text-tertiary)]">
            Gizzi Code marks its requests with X-Allternit-Client. Only billed requests are counted.
          </p>
          <RefreshButton loading={loading} onClick={reload} />
        </div>
      )}
    </ListPage>
  );
}

export function GizziUsagePage(): React.ReactNode {
  return isCloudGateway() ? <CloudGizziUsagePage /> : <RuntimeGizziUsagePage />;
}

export default GizziUsagePage;
