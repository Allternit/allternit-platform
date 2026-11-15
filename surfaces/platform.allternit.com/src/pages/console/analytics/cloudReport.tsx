/**
 * Shared pieces for the cloud Usage, Cost and Gizzi Code usage pages. They all
 * read GET /api/v1/me/usage/report; this file holds the filter bar, the data
 * hook and the table so the three pages stay thin.
 */
import React, { useCallback, useEffect, useMemo, useState } from "react";
import { HugeiconsIcon } from "@hugeicons/react";
import { AlertCircleIcon, Refresh01Icon } from "@hugeicons/core-free-icons";
import { cn } from "@/lib/utils";
import { formatApiError } from "@/lib/api-client";
import { QUIET_BUTTON_CLASS, SETTINGS_SELECT_CLASS } from "@/components/console-ui";
import { hasOrganizationAdminAccess } from "@/components/settings/OrganizationAccessPanel";
import { useProjectContext } from "@/lib/project-context";
import { usePlatformAuth, usePlatformOrganization } from "@/lib/platform-auth-client";
import {
  dateDaysAgo,
  downloadUsageReportCsv,
  formatTokenCount,
  formatUsd,
  getUsageReport,
  sortReportRows,
  tomorrowDate,
  type ReportGroupBy,
  type ReportScope,
  type UsageReportParams,
  type UsageReportRow,
} from "@/lib/gateway-analytics";

export const REPORT_RANGES = [7, 30, 90] as const;

export const GROUP_LABELS: Record<ReportGroupBy, string> = {
  day: "Day",
  model: "Model",
  project: "Project",
  key: "API key",
  client: "Client",
};

export const MUTED = "#71717a";

export interface ReportFilterState {
  rangeDays: number;
  setRangeDays: (d: number) => void;
  /** "" = all projects. */
  projectId: string;
  setProjectId: (id: string) => void;
  scope: ReportScope;
  setScope: (s: ReportScope) => void;
  isOrgAdmin: boolean;
  projectNames: Map<string, string>;
  from: string;
  to: string;
}

/** Range, project (defaults to the console's selected project) and org scope. */
export function useReportFilters(defaultDays = 30): ReportFilterState {
  const ctx = useProjectContext();
  const auth = usePlatformAuth();
  const { organization, membership } = usePlatformOrganization();
  const [rangeDays, setRangeDays] = useState(defaultDays);
  // undefined = follow the console's selected project.
  const [override, setOverride] = useState<string | undefined>(undefined);
  const [scope, setScope] = useState<ReportScope>("me");
  const isOrgAdmin = Boolean(organization) && hasOrganizationAdminAccess(auth.orgRole ?? membership?.role);
  const projectId = override ?? ctx.project?.id ?? "";
  const projectNames = useMemo(() => new Map(ctx.projects.map((p) => [p.id, p.name])), [ctx.projects]);
  const from = useMemo(() => dateDaysAgo(rangeDays), [rangeDays]);
  const to = useMemo(() => tomorrowDate(), []);
  return {
    rangeDays,
    setRangeDays,
    projectId,
    setProjectId: setOverride,
    scope: isOrgAdmin ? scope : "me",
    setScope,
    isOrgAdmin,
    projectNames,
    from,
    to,
  };
}

export function ReportFilters({
  filters,
  showProject = true,
  children,
}: {
  filters: ReportFilterState;
  showProject?: boolean;
  children?: React.ReactNode;
}): React.ReactNode {
  const ctx = useProjectContext();
  return (
    <>
      <div className="inline-flex items-center rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-0.5">
        {REPORT_RANGES.map((d) => (
          <button
            key={d}
            type="button"
            onClick={() => filters.setRangeDays(d)}
            aria-pressed={filters.rangeDays === d}
            className={cn(
              "rounded-lg px-3 py-1 text-[12px] font-semibold transition-colors",
              filters.rangeDays === d
                ? "bg-[var(--accent-primary)] text-[var(--ui-text-inverse)]"
                : "text-[var(--text-secondary)] hover:text-[var(--text-primary)]",
            )}
          >
            {d}d
          </button>
        ))}
      </div>
      {showProject && (
        <select
          value={filters.projectId}
          onChange={(e) => filters.setProjectId(e.target.value)}
          aria-label="Project"
          className={cn(SETTINGS_SELECT_CLASS, "min-w-0 max-w-[200px]")}
        >
          <option value="">All projects</option>
          {ctx.projects.map((p) => (
            <option key={p.id} value={p.id}>
              {p.name} ({p.env === "live" ? "live" : "sandbox"})
            </option>
          ))}
        </select>
      )}
      {children}
      {filters.isOrgAdmin && (
        <select
          value={filters.scope}
          onChange={(e) => filters.setScope(e.target.value as ReportScope)}
          aria-label="Scope"
          className={SETTINGS_SELECT_CLASS}
        >
          <option value="me">Just me</option>
          <option value="org">Whole organization</option>
        </select>
      )}
    </>
  );
}

export interface ReportState {
  rows: UsageReportRow[] | null;
  loading: boolean;
  error: string | null;
  reload: () => void;
}

/** Loads the report; re-runs when any param changes. null params = skip. */
export function useUsageReport(params: UsageReportParams | null, fallback = "Unable to load usage"): ReportState {
  const [rows, setRows] = useState<UsageReportRow[] | null>(null);
  const key = params ? JSON.stringify(params) : "";
  const [loading, setLoading] = useState(Boolean(key));
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (!key) {
      setRows(null);
      setError(null);
      setLoading(false);
      return;
    }
    const p = JSON.parse(key) as UsageReportParams;
    setLoading(true);
    setError(null);
    try {
      setRows(await getUsageReport(p));
    } catch (err) {
      setRows(null);
      setError(formatApiError(err, fallback));
    } finally {
      setLoading(false);
    }
  }, [key, fallback]);

  useEffect(() => {
    void load();
  }, [load]);

  return { rows, loading, error, reload: () => void load() };
}

export function ErrorRetry({ message, onRetry }: { message: string; onRetry: () => void }): React.ReactNode {
  return (
    <div
      role="alert"
      className="mb-4 flex flex-wrap items-center gap-2 rounded-lg border border-solid border-[var(--status-error)]/30 bg-[var(--status-error)]/10 px-3 py-2 text-[13px] text-[var(--status-error)]"
    >
      <HugeiconsIcon icon={AlertCircleIcon} size={14} />
      <span className="min-w-0 flex-1">{message}</span>
      <button type="button" onClick={onRetry} className={cn(QUIET_BUTTON_CLASS, "px-2.5 py-1 text-[12px]")}>
        <HugeiconsIcon icon={Refresh01Icon} size={12} />
        Retry
      </button>
    </div>
  );
}

/** Day rows as "Nov 12", keeping other group values as they are. */
export function dayLabel(day: string): string {
  const d = new Date(`${day}T00:00:00Z`);
  return Number.isNaN(d.getTime())
    ? day
    : d.toLocaleDateString("en-US", { month: "short", day: "numeric", timeZone: "UTC" });
}

export function groupDisplay(
  groupBy: ReportGroupBy,
  group: string,
  projectNames: Map<string, string>,
): string {
  if (!group) return "(none)";
  if (groupBy === "project") return projectNames.get(group) ?? group;
  return group;
}

export function ReportTable({
  rows,
  groupBy,
  projectNames,
  showCost = true,
}: {
  rows: UsageReportRow[];
  groupBy: ReportGroupBy;
  projectNames: Map<string, string>;
  showCost?: boolean;
}): React.ReactNode {
  const sorted = useMemo(() => sortReportRows(rows, groupBy), [rows, groupBy]);
  const th = "px-3 py-2 font-medium whitespace-nowrap";
  const td = "px-3 py-2.5 text-[var(--text-secondary)] whitespace-nowrap";
  return (
    <div className="overflow-x-auto rounded-xl border border-solid border-[var(--border-subtle)]">
      <table className="w-full border-collapse text-left text-[13px]">
        <thead>
          <tr className="border-b border-solid border-[var(--border-subtle)] text-[11px] uppercase tracking-wide text-[var(--text-tertiary)]">
            <th className={th}>{GROUP_LABELS[groupBy]}</th>
            <th className={th}>Requests</th>
            <th className={th}>Tokens in</th>
            <th className={th}>Tokens out</th>
            {showCost && <th className={th}>Cost</th>}
          </tr>
        </thead>
        <tbody>
          {sorted.map((r) => (
            <tr
              key={r.group}
              className="border-b border-solid border-[var(--border-subtle)] last:border-b-0 hover:bg-[var(--surface-hover)]"
            >
              <td className="px-3 py-2.5 font-mono text-[12px] text-[var(--text-primary)]">
                {groupBy === "day" ? r.group : groupDisplay(groupBy, r.group, projectNames)}
              </td>
              <td className={td}>{r.requests.toLocaleString()}</td>
              <td className={td}>{formatTokenCount(r.prompt_tokens)}</td>
              <td className={td}>{formatTokenCount(r.completion_tokens)}</td>
              {showCost && <td className={td}>{formatUsd(r.cost_usd)}</td>}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export function RefreshButton({ loading, onClick }: { loading: boolean; onClick: () => void }): React.ReactNode {
  return (
    <button type="button" onClick={onClick} disabled={loading} className={cn(QUIET_BUTTON_CLASS, "text-[12px]")}>
      <HugeiconsIcon icon={Refresh01Icon} size={12} className={cn(loading && "animate-spin")} />
      {loading ? "Refreshing…" : "Refresh"}
    </button>
  );
}

/** "Download CSV": fetch format=csv with auth and save it. */
export function useCsvDownload(params: UsageReportParams, filename: string) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const key = JSON.stringify(params);
  const run = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      await downloadUsageReportCsv(JSON.parse(key) as UsageReportParams, filename);
    } catch (err) {
      setError(formatApiError(err, "Unable to download the CSV"));
    } finally {
      setBusy(false);
    }
  }, [key, filename]);
  return { busy, error, run };
}
