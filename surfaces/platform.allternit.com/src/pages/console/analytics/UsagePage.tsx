import React, { useCallback, useEffect, useMemo, useState } from "react";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  AlertCircleIcon,
  Analytics01Icon,
  Refresh01Icon,
} from "@hugeicons/core-free-icons";
import {
  Area,
  AreaChart,
  CartesianGrid,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import { cn } from "@/lib/utils";
import { formatApiError } from "@/lib/api-client";
import {
  StatCard,
  ListPage,
  EmptyState,
  SkeletonRow,
  QUIET_BUTTON_CLASS,
  SETTINGS_SELECT_CLASS,
} from "@/components/console-ui";
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
import {
  dateDaysAgo,
  downloadCsv,
  formatUsd,
  sumReportRows,
  type ReportGroupBy,
  formatTokenCount,
  getGatewayUsage,
  listGatewayKeys,
  microdollarsToUsd,
  tomorrowDate,
  type GatewayKeyRef,
  type UsageDayRow,
  type UsageResponse,
} from "@/lib/gateway-analytics";

const RANGES = [
  { label: "Last 7 days", days: 7 },
  { label: "Last 30 days", days: 30 },
  { label: "Last 90 days", days: 90 },
];

type GroupMode = "model" | "tag";

const MUTED = "#71717a";

interface ModelRow {
  model: string;
  requests: number;
  prompt_tokens: number;
  completion_tokens: number;
  cached_tokens: number;
  spend_microdollars: number;
}

function RuntimeUsagePage(): React.ReactNode {
  const [data, setData] = useState<UsageResponse | null>(null);
  const [keys, setKeys] = useState<GatewayKeyRef[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [rangeDays, setRangeDays] = useState(30);
  const [groupBy, setGroupBy] = useState<GroupMode>("model");
  const [modelFilter, setModelFilter] = useState("");
  const [keyFilter, setKeyFilter] = useState("");

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [usage, keyList] = await Promise.all([
        getGatewayUsage({
          from: dateDaysAgo(rangeDays),
          to: tomorrowDate(),
          groupBy: groupBy === "tag" ? "tag" : undefined,
        }),
        listGatewayKeys().catch(() => [] as GatewayKeyRef[]),
      ]);
      setData(usage);
      setKeys(keyList);
    } catch (err) {
      setError(formatApiError(err, "Unable to load usage"));
      setData(null);
    } finally {
      setLoading(false);
    }
  }, [rangeDays, groupBy]);

  useEffect(() => {
    void load();
  }, [load]);

  const dayRows = useMemo(
    () => (data?.group_by === "day" ? (data.usage as UsageDayRow[]) : []),
    [data]
  );

  const models = useMemo(
    () => Array.from(new Set(dayRows.map((row) => row.model_id).filter(Boolean))).sort(),
    [dayRows]
  );

  const filteredRows = useMemo(
    () =>
      dayRows.filter(
        (row) =>
          (!modelFilter || row.model_id === modelFilter) &&
          (!keyFilter || row.virtual_key_id === keyFilter)
      ),
    [dayRows, modelFilter, keyFilter]
  );

  const tagRows = useMemo(
    () =>
      data?.group_by === "tag"
        ? (data.usage as UsageResponse["usage"]).map((row) => row as unknown as {
            tag_key: string;
            tag_value: string;
            requests: number;
            prompt_tokens: number;
            completion_tokens: number;
            cached_tokens: number;
            spend_microdollars: number;
          })
        : [],
    [data]
  );

  const filteredTagRows = useMemo(
    () => tagRows.filter((row) => !modelFilter || row.tag_value === modelFilter),
    [tagRows, modelFilter]
  );

  // Model-grouped rollup (what the table shows in "model" mode).
  const modelRollup = useMemo(() => {
    const map = new Map<string, ModelRow>();
    for (const row of filteredRows) {
      const existing = map.get(row.model_id) ?? {
        model: row.model_id || "(unknown)",
        requests: 0,
        prompt_tokens: 0,
        completion_tokens: 0,
        cached_tokens: 0,
        spend_microdollars: 0,
      };
      existing.requests += row.requests;
      existing.prompt_tokens += row.prompt_tokens;
      existing.completion_tokens += row.completion_tokens;
      existing.cached_tokens += row.cached_tokens;
      existing.spend_microdollars += row.spend_microdollars;
      map.set(row.model_id, existing);
    }
    return Array.from(map.values()).sort((a, b) => b.prompt_tokens - a.prompt_tokens);
  }, [filteredRows]);

  // Daily token series for the chart (model mode only — tag mode has no dates).
  const chartData = useMemo(() => {
    const byDay = new Map<string, { day: string; input: number; output: number }>();
    for (const row of filteredRows) {
      const existing = byDay.get(row.day) ?? { day: row.day, input: 0, output: 0 };
      existing.input += row.prompt_tokens;
      existing.output += row.completion_tokens;
      byDay.set(row.day, existing);
    }
    return Array.from(byDay.values()).sort((a, b) => a.day.localeCompare(b.day));
  }, [filteredRows]);

  const totals = useMemo(() => {
    const rows = data?.group_by === "tag" ? filteredTagRows : filteredRows;
    return rows.reduce(
      (acc, row) => ({
        requests: acc.requests + (row.requests ?? 0),
        prompt_tokens: acc.prompt_tokens + (row.prompt_tokens ?? 0),
        completion_tokens: acc.completion_tokens + (row.completion_tokens ?? 0),
        spend_microdollars: acc.spend_microdollars + (row.spend_microdollars ?? 0),
      }),
      { requests: 0, prompt_tokens: 0, completion_tokens: 0, spend_microdollars: 0 }
    );
  }, [data, filteredRows, filteredTagRows]);

  const exportCsv = useCallback(() => {
    if (!data) return;
    if (data.group_by === "tag") {
      downloadCsv(
        `allternit-usage-tags-${data.from}.csv`,
        ["Tag key", "Tag value", "Requests", "Tokens in", "Tokens out", "Cached tokens", "Spend (USD)"],
        filteredTagRows.map((row) => [
          row.tag_key,
          row.tag_value,
          row.requests,
          row.prompt_tokens,
          row.completion_tokens,
          row.cached_tokens,
          (row.spend_microdollars / 1_000_000).toFixed(4),
        ])
      );
    } else {
      downloadCsv(
        `allternit-usage-${data.from}.csv`,
        ["Day", "Provider", "Model", "Key prefix", "Requests", "Tokens in", "Tokens out", "Cached tokens", "Spend (USD)"],
        filteredRows.map((row) => [
          row.day,
          row.provider_id,
          row.model_id,
          row.key_prefix ?? "",
          row.requests,
          row.prompt_tokens,
          row.completion_tokens,
          row.cached_tokens,
          (row.spend_microdollars / 1_000_000).toFixed(4),
        ])
      );
    }
  }, [data, filteredRows, filteredTagRows]);

  return (
    <ListPage
      title="Usage"
      subtitle="Token and request volume across every model and API key on this account."
      filters={
        <>
          <div className="inline-flex items-center rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-0.5">
            {RANGES.map((range) => (
              <button
                key={range.days}
                type="button"
                onClick={() => setRangeDays(range.days)}
                className={cn(
                  "rounded-lg px-3 py-1 text-[12px] font-semibold transition-colors",
                  rangeDays === range.days
                    ? "bg-[var(--accent-primary)] text-[var(--ui-text-inverse)]"
                    : "text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
                )}
              >
                {range.days}d
              </button>
            ))}
          </div>
          <select
            value={groupBy}
            onChange={(e) => setGroupBy(e.target.value as GroupMode)}
            aria-label="Group by"
            className={SETTINGS_SELECT_CLASS}
          >
            <option value="model">Group by model</option>
            <option value="tag">Group by tag</option>
          </select>
          <select
            value={modelFilter}
            onChange={(e) => setModelFilter(e.target.value)}
            aria-label="Filter by model or tag"
            className={SETTINGS_SELECT_CLASS}
          >
            <option value="">
              {groupBy === "tag" ? "All tag values" : "All models"}
            </option>
            {(groupBy === "tag"
              ? Array.from(new Set(tagRows.map((row) => row.tag_value))).sort()
              : models
            ).map((value) => (
              <option key={value} value={value}>
                {value}
              </option>
            ))}
          </select>
          {groupBy === "model" && (
            <select
              value={keyFilter}
              onChange={(e) => setKeyFilter(e.target.value)}
              aria-label="Filter by API key"
              className={SETTINGS_SELECT_CLASS}
            >
              <option value="">All API keys</option>
              {keys.map((key) => (
                <option key={key.id} value={key.id}>
                  {key.name ?? key.key_prefix ?? key.id}
                </option>
              ))}
            </select>
          )}
        </>
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

      {loading && !data ? (
        <SkeletonRow lines={6} />
      ) : !data ? null : (
        <div className="space-y-4">
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
            <StatCard
              label="Total tokens in"
              value={formatTokenCount(totals.prompt_tokens)}
              hint={`${totals.requests.toLocaleString()} requests`}
            />
            <StatCard
              label="Total tokens out"
              value={formatTokenCount(totals.completion_tokens)}
            />
            <StatCard
              label="Spend"
              value={microdollarsToUsd(totals.spend_microdollars)}
              hint="Recomputed from the models.dev price cache when available"
            />
          </div>

          {groupBy === "model" ? (
            <div className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4">
              <h3 className="mb-2 text-[11px] font-semibold uppercase tracking-wider text-[var(--text-tertiary)]">
                Token usage per day
              </h3>
              {chartData.length === 0 ? (
                <div className="flex h-44 items-center justify-center text-[12px] text-[var(--text-tertiary)]">
                  No usage in this range yet.
                </div>
              ) : (
                <div className="h-52">
                  <ResponsiveContainer width="100%" height="100%">
                    <AreaChart data={chartData} margin={{ left: -12, right: 8, top: 4, bottom: 0 }}>
                      <CartesianGrid stroke="var(--border-subtle)" vertical={false} />
                      <XAxis
                        dataKey="day"
                        axisLine={false}
                        tickLine={false}
                        tick={{ fill: MUTED, fontSize: 10 }}
                      />
                      <YAxis
                        axisLine={false}
                        tickLine={false}
                        tick={{ fill: MUTED, fontSize: 10 }}
                        tickFormatter={(v: number) => formatTokenCount(v)}
                      />
                      <Tooltip
                        contentStyle={{
                          backgroundColor: "var(--bg-secondary)",
                          border: "1px solid var(--border-subtle)",
                          borderRadius: "8px",
                        }}
                        formatter={(v: number) => formatTokenCount(v)}
                      />
                      <Area
                        type="monotone"
                        dataKey="input"
                        name="Tokens in"
                        stroke="var(--accent-primary)"
                        fill="var(--accent-primary)"
                        fillOpacity={0.18}
                        strokeWidth={1.5}
                      />
                      <Area
                        type="monotone"
                        dataKey="output"
                        name="Tokens out"
                        stroke="var(--accent-highlight)"
                        fill="var(--accent-highlight)"
                        fillOpacity={0.18}
                        strokeWidth={1.5}
                      />
                    </AreaChart>
                  </ResponsiveContainer>
                </div>
              )}
            </div>
          ) : null}

          {groupBy === "model" ? (
            modelRollup.length === 0 ? (
              <EmptyState
                icon={<HugeiconsIcon icon={Analytics01Icon} size={32} />}
                title="No usage yet"
                caption="Send a request through the gateway with any API key and it will show up here within a minute of completing."
              />
            ) : (
              <div className="overflow-x-auto rounded-xl border border-solid border-[var(--border-subtle)]">
                <table className="w-full border-collapse text-left text-[13px]">
                  <thead>
                    <tr className="border-b border-solid border-[var(--border-subtle)] text-[11px] uppercase tracking-wide text-[var(--text-tertiary)]">
                      <th className="px-3 py-2 font-medium">Model</th>
                      <th className="px-3 py-2 font-medium">Requests</th>
                      <th className="px-3 py-2 font-medium">Tokens in</th>
                      <th className="px-3 py-2 font-medium">Tokens out</th>
                      <th className="px-3 py-2 font-medium">Cached tokens</th>
                      <th className="px-3 py-2 font-medium">Spend</th>
                    </tr>
                  </thead>
                  <tbody>
                    {modelRollup.map((row) => (
                      <tr
                        key={row.model}
                        className="border-b border-solid border-[var(--border-subtle)] last:border-b-0 hover:bg-[var(--surface-hover)]"
                      >
                        <td className="px-3 py-2.5 font-mono text-[12px] text-[var(--text-primary)]">
                          {row.model}
                        </td>
                        <td className="px-3 py-2.5 text-[var(--text-secondary)]">
                          {row.requests.toLocaleString()}
                        </td>
                        <td className="px-3 py-2.5 text-[var(--text-secondary)]">
                          {formatTokenCount(row.prompt_tokens)}
                        </td>
                        <td className="px-3 py-2.5 text-[var(--text-secondary)]">
                          {formatTokenCount(row.completion_tokens)}
                        </td>
                        <td className="px-3 py-2.5 text-[var(--text-secondary)]">
                          {formatTokenCount(row.cached_tokens)}
                        </td>
                        <td className="px-3 py-2.5 text-[var(--text-secondary)]">
                          {microdollarsToUsd(row.spend_microdollars)}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )
          ) : filteredTagRows.length === 0 ? (
            <EmptyState
              icon={<HugeiconsIcon icon={Analytics01Icon} size={32} />}
              title="No tagged usage"
              caption="Tag gateway keys with x-allternit-tag (or the Tags API) and tagged spend will appear here. Untagged requests are not listed in this mode."
            />
          ) : (
            <div className="overflow-x-auto rounded-xl border border-solid border-[var(--border-subtle)]">
              <table className="w-full border-collapse text-left text-[13px]">
                <thead>
                  <tr className="border-b border-solid border-[var(--border-subtle)] text-[11px] uppercase tracking-wide text-[var(--text-tertiary)]">
                    <th className="px-3 py-2 font-medium">Tag</th>
                    <th className="px-3 py-2 font-medium">Requests</th>
                    <th className="px-3 py-2 font-medium">Tokens in</th>
                    <th className="px-3 py-2 font-medium">Tokens out</th>
                    <th className="px-3 py-2 font-medium">Spend</th>
                  </tr>
                </thead>
                <tbody>
                  {filteredTagRows.map((row) => (
                    <tr
                      key={`${row.tag_key}:${row.tag_value}`}
                      className="border-b border-solid border-[var(--border-subtle)] last:border-b-0 hover:bg-[var(--surface-hover)]"
                    >
                      <td className="px-3 py-2.5">
                        <span className="font-mono text-[12px] text-[var(--text-primary)]">
                          {row.tag_key}: {row.tag_value}
                        </span>
                      </td>
                      <td className="px-3 py-2.5 text-[var(--text-secondary)]">
                        {row.requests.toLocaleString()}
                      </td>
                      <td className="px-3 py-2.5 text-[var(--text-secondary)]">
                        {formatTokenCount(row.prompt_tokens)}
                      </td>
                      <td className="px-3 py-2.5 text-[var(--text-secondary)]">
                        {formatTokenCount(row.completion_tokens)}
                      </td>
                      <td className="px-3 py-2.5 text-[var(--text-secondary)]">
                        {microdollarsToUsd(row.spend_microdollars)}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}

          <p className="m-0 text-[12px] text-[var(--text-tertiary)]">
            Includes usage from both API and Console requests. Figures come
            from the gateway's usage events; spend is recomputed from the
            models.dev price cache when available.
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

const CLOUD_GROUPS: ReportGroupBy[] = ["model", "project", "key", "client", "day"];

/** Allternit Cloud: the per-user report (GET /api/v1/me/usage/report). */
function CloudUsagePage(): React.ReactNode {
  const filters = useReportFilters(30);
  const [groupBy, setGroupBy] = useState<ReportGroupBy>("model");
  const base = {
    from: filters.from,
    to: filters.to,
    projectId: filters.projectId,
    scope: filters.scope,
  };
  const daily = useUsageReport({ ...base, groupBy: "day" });
  const grouped = useUsageReport(groupBy === "day" ? null : { ...base, groupBy });
  const csv = useCsvDownload(
    { ...base, groupBy },
    `allternit-usage-${groupBy}-${filters.from}.csv`,
  );

  const dayRows = daily.rows ?? [];
  const tableRows = groupBy === "day" ? dayRows : grouped.rows ?? [];
  const totals = useMemo(() => sumReportRows(dayRows), [dayRows]);
  const chartData = useMemo(
    () =>
      dayRows
        .slice()
        .sort((a, b) => a.group.localeCompare(b.group))
        .map((r) => ({ day: dayLabel(r.group), input: r.prompt_tokens, output: r.completion_tokens })),
    [dayRows],
  );
  const loading = daily.loading || (groupBy !== "day" && grouped.loading);
  const error = daily.error ?? (groupBy !== "day" ? grouped.error : null) ?? csv.error;
  const reload = () => {
    daily.reload();
    if (groupBy !== "day") grouped.reload();
  };
  const initial = loading && daily.rows === null;

  return (
    <ListPage
      title="Usage"
      subtitle="Requests and tokens across your projects and API keys."
      filters={
        <ReportFilters filters={filters}>
          <select
            value={groupBy}
            onChange={(e) => setGroupBy(e.target.value as ReportGroupBy)}
            aria-label="Group by"
            className={SETTINGS_SELECT_CLASS}
          >
            {CLOUD_GROUPS.map((g) => (
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
      {initial ? (
        <SkeletonRow lines={6} />
      ) : daily.rows === null ? null : (
        <div className="space-y-4">
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
            <StatCard
              label="Tokens in"
              value={formatTokenCount(totals.prompt_tokens)}
              hint={`${totals.requests.toLocaleString()} requests`}
            />
            <StatCard label="Tokens out" value={formatTokenCount(totals.completion_tokens)} />
            <StatCard label="Spend" value={formatUsd(totals.cost_usd)} hint="Billed requests only" />
          </div>

          {chartData.length === 0 ? (
            <EmptyState
              icon={<HugeiconsIcon icon={Analytics01Icon} size={32} />}
              title="No usage in this range"
              caption="Send a request with a project API key and it appears here once it is billed."
            />
          ) : (
            <>
              <div className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4">
                <h3 className="mb-2 text-[11px] font-semibold uppercase tracking-wider text-[var(--text-tertiary)]">
                  Token usage per day
                </h3>
                <div className="h-52">
                  <ResponsiveContainer width="100%" height="100%">
                    <AreaChart data={chartData} margin={{ left: -12, right: 8, top: 4, bottom: 0 }}>
                      <CartesianGrid stroke="var(--border-subtle)" vertical={false} />
                      <XAxis dataKey="day" axisLine={false} tickLine={false} tick={{ fill: CLOUD_MUTED, fontSize: 10 }} />
                      <YAxis
                        axisLine={false}
                        tickLine={false}
                        tick={{ fill: CLOUD_MUTED, fontSize: 10 }}
                        tickFormatter={(v: number) => formatTokenCount(v)}
                      />
                      <Tooltip
                        contentStyle={{
                          backgroundColor: "var(--bg-secondary)",
                          border: "1px solid var(--border-subtle)",
                          borderRadius: "8px",
                        }}
                        formatter={(v: number) => formatTokenCount(v)}
                      />
                      <Area type="monotone" dataKey="input" name="Tokens in" stroke="var(--accent-primary)" fill="var(--accent-primary)" fillOpacity={0.18} strokeWidth={1.5} />
                      <Area type="monotone" dataKey="output" name="Tokens out" stroke="var(--accent-highlight)" fill="var(--accent-highlight)" fillOpacity={0.18} strokeWidth={1.5} />
                    </AreaChart>
                  </ResponsiveContainer>
                </div>
              </div>
              <ReportTable rows={tableRows} groupBy={groupBy} projectNames={filters.projectNames} />
            </>
          )}
          <p className="m-0 text-[12px] text-[var(--text-tertiary)]">
            Only billed requests are counted. Requests that failed before billing do not appear.
          </p>
          <RefreshButton loading={loading} onClick={reload} />
        </div>
      )}
    </ListPage>
  );
}

export function UsagePage(): React.ReactNode {
  return isCloudGateway() ? <CloudUsagePage /> : <RuntimeUsagePage />;
}

export default UsagePage;
