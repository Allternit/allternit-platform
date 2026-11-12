import React, { useCallback, useEffect, useMemo, useState } from "react";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  AlertCircleIcon,
  ArrowLeft01Icon,
  ArrowRight01Icon,
  DocumentValidationIcon,
} from "@hugeicons/core-free-icons";
import { useNavigate } from "react-router-dom";
import { cn } from "@/lib/utils";
import { isCloudGateway } from "@/lib/availability";
import { useProjectContext } from "@/lib/project-context";
import { ErrorRetry, REPORT_RANGES, useReportFilters, type ReportScope } from "./cloudReport";
import { formatApiError } from "@/lib/api-client";
import {
  ListPage,
  EmptyState,
  MonoChip,
  SkeletonRow,
  CodeTemplateBlock,
  QUIET_BUTTON_CLASS,
  SETTINGS_SELECT_CLASS,
} from "@/components/console-ui";
import {
  dateDaysAgo,
  formatUsd,
  getGatewayLogs,
  getUsageRequestDetail,
  getUsageRequests,
  tomorrowDate,
  type UsageRequestDetail,
  type UsageRequestRow,
  listTags,
  microdollarsToUsd,
  type GatewayLogRow,
  type ResourceTag,
} from "@/lib/gateway-analytics";

const PAGE_LIMIT = 50;
const RANGE_OPTIONS = [
  { label: "All time", days: null },
  { label: "Last 24 hours", days: 1 },
  { label: "Last 7 days", days: 7 },
  { label: "Last 30 days", days: 30 },
];

function formatTime(iso: string): string {
  const date = new Date(iso.replace(" ", "T") + "Z");
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleString();
}

function statusBadgeClass(status: string): string {
  return status === "success"
    ? "bg-[var(--status-success)]/10 text-[var(--status-success)]"
    : "bg-[var(--status-error)]/10 text-[var(--status-error)]";
}

function RuntimeLogsPage(): React.ReactNode {
  const [logs, setLogs] = useState<GatewayLogRow[]>([]);
  const [tags, setTags] = useState<ResourceTag[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [statusFilter, setStatusFilter] = useState("");
  const [tagFilter, setTagFilter] = useState("");
  const [rangeDays, setRangeDays] = useState<number | null>(null);
  const [nextCursor, setNextCursor] = useState<string | null>(null);
  const [cursorStack, setCursorStack] = useState<string[]>([]);
  const [selected, setSelected] = useState<GatewayLogRow | null>(null);

  const load = useCallback(
    async (cursor?: string) => {
      setLoading(true);
      setError(null);
      try {
        const data = await getGatewayLogs({
          limit: PAGE_LIMIT,
          cursor,
          status: statusFilter || undefined,
          tag: tagFilter || undefined,
        });
        setLogs(data.logs);
        setNextCursor(data.next_cursor);
      } catch (err) {
        setError(formatApiError(err, "Unable to load logs"));
        setLogs([]);
        setNextCursor(null);
      } finally {
        setLoading(false);
      }
    },
    [statusFilter, tagFilter]
  );

  useEffect(() => {
    setCursorStack([]);
    void load();
  }, [load]);

  useEffect(() => {
    void listTags()
      .then(setTags)
      .catch(() => setTags([]));
  }, []);

  const goNext = useCallback(() => {
    if (!nextCursor) return;
    setCursorStack((stack) => [...stack, nextCursor]);
    void load(nextCursor);
  }, [load, nextCursor]);

  const goBack = useCallback(() => {
    setCursorStack((stack) => {
      const next = stack.slice(0, -1);
      void load(next[next.length - 1]);
      return next;
    });
  }, [load]);

  const tagOptions = useMemo(() => {
    const seen = new Map<string, string>();
    for (const tag of tags) {
      if (!seen.has(tag.value)) seen.set(tag.value, `${tag.key}: ${tag.value}`);
    }
    return Array.from(seen.entries()).sort((a, b) => a[1].localeCompare(b[1]));
  }, [tags]);

  const filtered = useMemo(() => {
    const query = search.trim().toLowerCase();
    const cutoff =
      rangeDays === null
        ? null
        : Date.now() - rangeDays * 86_400_000;
    return logs.filter((row) => {
      if (query) {
        const haystack = `${row.id} ${row.model_id ?? ""} ${row.provider_id ?? ""}`.toLowerCase();
        if (!haystack.includes(query)) return false;
      }
      if (cutoff !== null) {
        const ts = new Date(row.created_at.replace(" ", "T") + "Z").getTime();
        if (!Number.isNaN(ts) && ts < cutoff) return false;
      }
      return true;
    });
  }, [logs, search, rangeDays]);

  return (
    <ListPage
      title="Logs"
      subtitle="Request logs usually appear within a minute of completing. Batch requests are not included."
      searchPlaceholder="Search by ID, model, or provider"
      onSearch={setSearch}
      filters={
        <>
          <select
            value={statusFilter}
            onChange={(e) => setStatusFilter(e.target.value)}
            aria-label="Filter by status"
            className={SETTINGS_SELECT_CLASS}
          >
            <option value="">All statuses</option>
            <option value="success">Success</option>
            <option value="error">Error</option>
          </select>
          <select
            value={tagFilter}
            onChange={(e) => setTagFilter(e.target.value)}
            aria-label="Filter by tag"
            className={SETTINGS_SELECT_CLASS}
          >
            <option value="">All tags</option>
            {tagOptions.map(([value, label]) => (
              <option key={value} value={value}>
                {label}
              </option>
            ))}
          </select>
          <select
            value={rangeDays === null ? "" : String(rangeDays)}
            onChange={(e) =>
              setRangeDays(e.target.value === "" ? null : Number(e.target.value))
            }
            aria-label="Filter by range"
            className={SETTINGS_SELECT_CLASS}
          >
            {RANGE_OPTIONS.map((range) => (
              <option key={range.label} value={range.days === null ? "" : String(range.days)}>
                {range.label}
              </option>
            ))}
          </select>
        </>
      }
    >
      {error && (
        <p className="mb-4 flex items-center gap-2 rounded-lg border border-solid border-[var(--status-error)]/30 bg-[var(--status-error)]/10 px-3 py-2 text-[13px] text-[var(--status-error)]">
          <HugeiconsIcon icon={AlertCircleIcon} size={14} />
          {error}
        </p>
      )}

      {loading && logs.length === 0 ? (
        <SkeletonRow lines={8} />
      ) : filtered.length === 0 ? (
        <EmptyState
          icon={<HugeiconsIcon icon={DocumentValidationIcon} size={32} />}
          title="No logs found"
          caption={
            search || statusFilter || tagFilter
              ? "No logs match the current filters. Try widening the search."
              : "Requests sent through the gateway will appear here within a minute of completing."
          }
        />
      ) : (
        <>
          <div className="overflow-x-auto rounded-xl border border-solid border-[var(--border-subtle)]">
            <table className="w-full border-collapse text-left text-[13px]">
              <thead>
                <tr className="border-b border-solid border-[var(--border-subtle)] text-[11px] uppercase tracking-wide text-[var(--text-tertiary)]">
                  <th className="px-3 py-2 font-medium">ID</th>
                  <th className="px-3 py-2 font-medium">Status</th>
                  <th className="px-3 py-2 font-medium">Time</th>
                  <th className="px-3 py-2 font-medium">Model</th>
                  <th className="px-3 py-2 font-medium">Input tokens</th>
                  <th className="px-3 py-2 font-medium">Output tokens</th>
                  <th className="px-3 py-2 font-medium">Latency</th>
                </tr>
              </thead>
              <tbody>
                {filtered.map((row) => (
                  <tr
                    key={row.id}
                    onClick={() => setSelected(row)}
                    className="cursor-pointer border-b border-solid border-[var(--border-subtle)] last:border-b-0 hover:bg-[var(--surface-hover)]"
                  >
                    <td className="px-3 py-2.5">
                      <MonoChip>{row.id.slice(0, 14)}…</MonoChip>
                    </td>
                    <td className="px-3 py-2.5">
                      <span
                        className={cn(
                          "inline-flex items-center rounded-md px-1.5 py-0.5 text-[11px] font-medium",
                          statusBadgeClass(row.status)
                        )}
                      >
                        {row.status}
                      </span>
                      {row.error_type && (
                        <span className="ml-1.5 text-[11px] text-[var(--text-tertiary)]">
                          {row.error_type}
                        </span>
                      )}
                    </td>
                    <td className="px-3 py-2.5 whitespace-nowrap text-[var(--text-secondary)]">
                      {formatTime(row.created_at)}
                    </td>
                    <td className="px-3 py-2.5 font-mono text-[12px] text-[var(--text-secondary)]">
                      {row.model_id ?? "—"}
                    </td>
                    <td className="px-3 py-2.5 text-[var(--text-secondary)]">
                      {row.prompt_tokens.toLocaleString()}
                    </td>
                    <td className="px-3 py-2.5 text-[var(--text-secondary)]">
                      {row.completion_tokens.toLocaleString()}
                    </td>
                    <td className="px-3 py-2.5 text-[var(--text-secondary)]">
                      {row.latency_ms >= 1000
                        ? `${(row.latency_ms / 1000).toFixed(1)}s`
                        : `${row.latency_ms}ms`}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          <div className="mt-4 flex items-center justify-end gap-2 text-[12px] text-[var(--text-secondary)]">
            <button
              type="button"
              aria-label="Previous page"
              disabled={cursorStack.length === 0 || loading}
              onClick={goBack}
              className="inline-flex size-7 items-center justify-center rounded-md border border-solid border-[var(--border-subtle)] transition-colors hover:bg-[var(--surface-hover)] disabled:cursor-not-allowed disabled:opacity-40"
            >
              <HugeiconsIcon icon={ArrowLeft01Icon} size={14} />
            </button>
            <button
              type="button"
              aria-label="Next page"
              disabled={!nextCursor || loading}
              onClick={goNext}
              className="inline-flex size-7 items-center justify-center rounded-md border border-solid border-[var(--border-subtle)] transition-colors hover:bg-[var(--surface-hover)] disabled:cursor-not-allowed disabled:opacity-40"
            >
              <HugeiconsIcon icon={ArrowRight01Icon} size={14} />
            </button>
          </div>
        </>
      )}

      {selected && (
        <div
          className="fixed inset-0 z-50 flex justify-end bg-black/40"
          onClick={() => setSelected(null)}
        >
          <div
            className="h-full w-full max-w-lg overflow-y-auto border-l border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] p-5"
            onClick={(e) => e.stopPropagation()}
            role="dialog"
            aria-label={`Log ${selected.id} details`}
          >
            <div className="mb-4 flex items-start justify-between gap-3">
              <div>
                <h2 className="m-0 text-[16px] font-semibold text-[var(--text-primary)]">
                  Log details
                </h2>
                <p className="m-0 mt-1 font-mono text-[12px] text-[var(--text-tertiary)]">
                  {selected.id}
                </p>
              </div>
              <button
                type="button"
                onClick={() => setSelected(null)}
                className={cn(QUIET_BUTTON_CLASS, "px-2.5 py-1 text-[12px]")}
              >
                Close
              </button>
            </div>

            <dl className="m-0 mb-5 space-y-2 text-[13px]">
              {(
                [
                  ["Status", selected.status],
                  ["Error type", selected.error_type],
                  ["Time", formatTime(selected.created_at)],
                  ["Provider", selected.provider_id],
                  ["Model", selected.model_id],
                  ["Fallback from", selected.fallback_from],
                  ["Routing policy", selected.policy],
                  ["Input tokens", selected.prompt_tokens.toLocaleString()],
                  ["Output tokens", selected.completion_tokens.toLocaleString()],
                  ["Reasoning tokens", selected.reasoning_tokens.toLocaleString()],
                  ["Cached tokens", selected.cached_tokens.toLocaleString()],
                  ["Latency", `${selected.latency_ms}ms`],
                  ["Time to first token", selected.ttft_ms === null ? null : `${selected.ttft_ms}ms`],
                  ["Cost", microdollarsToUsd(selected.cost_microdollars)],
                  ["Key prefix", selected.key_prefix],
                  ["Gizzi session", selected.gizzi_session_id],
                  ["Batch", selected.batch_id],
                  ["Context cache", selected.context_cache_id],
                ] as Array<[string, string | null]>
              )
                .filter(([, value]) => value !== null)
                .map(([label, value]) => (
                  <div key={label} className="flex justify-between gap-4">
                    <dt className="text-[var(--text-tertiary)]">{label}</dt>
                    <dd className="m-0 text-right font-mono text-[12px] text-[var(--text-secondary)]">
                      {value}
                    </dd>
                  </div>
                ))}
            </dl>

            <CodeTemplateBlock language="json" code={JSON.stringify(selected, null, 2)} />
          </div>
        </div>
      )}
    </ListPage>
  );
}

const CLIENT_OPTIONS = [
  { value: "", label: "All clients" },
  { value: "api", label: "API" },
  { value: "playground", label: "Playground" },
  { value: "gizzi-code", label: "Gizzi Code" },
  { value: "runtime", label: "Runtime" },
];

function formatLatency(ms: number | null): string {
  if (ms === null) return "—";
  return ms >= 1000 ? `${(ms / 1000).toFixed(1)}s` : `${ms}ms`;
}

function formatCloudTime(iso: string): string {
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString();
}

function BodyBlock({ label, value }: { label: string; value: unknown }): React.ReactNode {
  return (
    <div className="mt-4">
      <h3 className="m-0 mb-1.5 text-[11px] font-semibold uppercase tracking-wider text-[var(--text-tertiary)]">
        {label}
      </h3>
      <CodeTemplateBlock
        language="json"
        code={typeof value === "string" ? value : JSON.stringify(value, null, 2)}
      />
    </div>
  );
}

function RequestDrawer({
  row,
  scope,
  onClose,
}: {
  row: UsageRequestRow;
  scope: ReportScope;
  onClose: () => void;
}): React.ReactNode {
  const navigate = useNavigate();
  const [detail, setDetail] = useState<UsageRequestDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setDetail(await getUsageRequestDetail(row.request_id, scope));
    } catch (err) {
      setDetail(null);
      setError(formatApiError(err, "Unable to load this request"));
    } finally {
      setLoading(false);
    }
  }, [row.request_id]);

  useEffect(() => {
    void load();
  }, [load]);

  const d = detail ?? { ...row, request: null, response: null };
  const hasBodies = d.request !== null || d.response !== null;

  return (
    <div className="fixed inset-0 z-50 flex justify-end bg-black/40" onClick={onClose}>
      <div
        className="h-full w-full max-w-lg overflow-y-auto border-l border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] p-4 sm:p-5"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-label={`Request ${row.request_id} details`}
      >
        <div className="mb-4 flex items-start justify-between gap-3">
          <div className="min-w-0">
            <h2 className="m-0 text-[16px] font-semibold text-[var(--text-primary)]">Request details</h2>
            <p className="m-0 mt-1 break-all font-mono text-[12px] text-[var(--text-tertiary)]">{row.request_id}</p>
          </div>
          <button type="button" onClick={onClose} className={cn(QUIET_BUTTON_CLASS, "px-2.5 py-1 text-[12px]")}>
            Close
          </button>
        </div>

        <button
          type="button"
          onClick={() => navigate(`/playground?replay=${encodeURIComponent(row.request_id)}${scope === "org" ? "&scope=org" : ""}`)}
          className="mb-4 inline-flex items-center rounded-lg bg-[var(--accent-primary)] px-3.5 py-2 text-[13px] font-semibold text-[var(--ui-text-inverse)] transition-colors hover:brightness-110"
        >
          Replay in Playground
        </button>

        {error && <ErrorRetry message={error} onRetry={() => void load()} />}

        <dl className="m-0 space-y-2 text-[13px]">
          {(
            [
              ["Time", formatCloudTime(d.created_at)],
              ["Model", d.model],
              ["Tokens in", d.prompt_tokens.toLocaleString()],
              ["Tokens out", d.completion_tokens.toLocaleString()],
              ["Cost", formatUsd(d.cost_usd)],
              ["Latency", d.latency_ms === null ? null : formatLatency(d.latency_ms)],
              ["Client", d.client],
              ["Key", d.key],
              ["Request ID", d.request_id],
            ] as Array<[string, string | null]>
          )
            .filter(([, v]) => v !== null)
            .map(([label, value]) => (
              <div key={label} className="flex justify-between gap-4">
                <dt className="shrink-0 text-[var(--text-tertiary)]">{label}</dt>
                <dd className="m-0 min-w-0 break-all text-right font-mono text-[12px] text-[var(--text-secondary)]">
                  {value}
                </dd>
              </div>
            ))}
        </dl>

        {loading && !detail ? (
          <div className="mt-4">
            <SkeletonRow lines={3} />
          </div>
        ) : hasBodies ? (
          <>
            {d.request !== null && <BodyBlock label="Request" value={d.request} />}
            {d.response !== null && <BodyBlock label="Response" value={d.response} />}
          </>
        ) : !error ? (
          <p className="mt-4 rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-3 text-[12px] text-[var(--text-secondary)]">
            Request and response bodies are only kept when the project turns on request logging.
          </p>
        ) : null}
      </div>
    </div>
  );
}

/** Allternit Cloud: billed requests from GET /api/v1/me/usage/requests. */
function CloudLogsPage(): React.ReactNode {
  const ctx = useProjectContext();
  const [override, setOverride] = useState<string | undefined>(undefined);
  const projectId = override ?? ctx.project?.id ?? "";
  const { isOrgAdmin, scope, setScope } = useReportFilters();
  const [rangeDays, setRangeDays] = useState<number>(7);
  const [client, setClient] = useState("");
  const [search, setSearch] = useState("");
  const [model, setModel] = useState("");
  const [rows, setRows] = useState<UsageRequestRow[]>([]);
  const [nextCursor, setNextCursor] = useState<string | null>(null);
  const [cursorStack, setCursorStack] = useState<string[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<UsageRequestRow | null>(null);
  const projectNames = useMemo(() => new Map(ctx.projects.map((p) => [p.id, p.name])), [ctx.projects]);

  // Debounce the model filter so each keystroke is not a request.
  useEffect(() => {
    const t = window.setTimeout(() => setModel(search.trim()), 350);
    return () => window.clearTimeout(t);
  }, [search]);

  const load = useCallback(
    async (cursor?: string) => {
      setLoading(true);
      setError(null);
      try {
        const page = await getUsageRequests({
          from: dateDaysAgo(rangeDays),
          to: tomorrowDate(),
          projectId,
          client,
          model,
          cursor,
          limit: PAGE_LIMIT,
          scope,
        });
        setRows(page.data);
        setNextCursor(page.next_cursor);
      } catch (err) {
        setRows([]);
        setNextCursor(null);
        setError(formatApiError(err, "Unable to load logs"));
      } finally {
        setLoading(false);
      }
    },
    [rangeDays, projectId, client, model, scope],
  );

  useEffect(() => {
    setCursorStack([]);
    void load();
  }, [load]);

  const goNext = () => {
    if (!nextCursor) return;
    setCursorStack((stack) => [...stack, nextCursor]);
    void load(nextCursor);
  };
  const goBack = () => {
    const next = cursorStack.slice(0, -1);
    setCursorStack(next);
    void load(next[next.length - 1]);
  };

  return (
    <ListPage
      title="Logs"
      subtitle="Billed requests only. Calls that failed before billing are not listed."
      searchPlaceholder="Filter by model"
      onSearch={setSearch}
      filters={
        <>
          <select
            value={projectId}
            onChange={(e) => setOverride(e.target.value)}
            aria-label="Project"
            className={cn(SETTINGS_SELECT_CLASS, "min-w-0 max-w-[200px]")}
          >
            <option value="">All projects</option>
            {ctx.projects.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
          </select>
          <select
            value={client}
            onChange={(e) => setClient(e.target.value)}
            aria-label="Filter by client"
            className={SETTINGS_SELECT_CLASS}
          >
            {CLIENT_OPTIONS.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
          <select
            value={String(rangeDays)}
            onChange={(e) => setRangeDays(Number(e.target.value))}
            aria-label="Filter by range"
            className={SETTINGS_SELECT_CLASS}
          >
            {REPORT_RANGES.map((d) => (
              <option key={d} value={d}>
                Last {d} days
              </option>
            ))}
          </select>
          {isOrgAdmin && (
            <select
              value={scope}
              onChange={(e) => setScope(e.target.value as ReportScope)}
              aria-label="Scope"
              className={SETTINGS_SELECT_CLASS}
            >
              <option value="me">Just me</option>
              <option value="org">Whole organization</option>
            </select>
          )}
        </>
      }
    >
      {error && <ErrorRetry message={error} onRetry={() => void load(cursorStack[cursorStack.length - 1])} />}

      {loading && rows.length === 0 ? (
        <SkeletonRow lines={8} />
      ) : rows.length === 0 ? (
        !error && (
          <EmptyState
            icon={<HugeiconsIcon icon={DocumentValidationIcon} size={32} />}
            title="No billed requests found"
            caption={
              model || client
                ? "No requests match these filters. Try widening them."
                : "Only billed requests are listed. Send a request with a project API key and it appears here."
            }
          />
        )
      ) : (
        <>
          <div className="overflow-x-auto rounded-xl border border-solid border-[var(--border-subtle)]">
            <table className="w-full border-collapse text-left text-[13px]">
              <thead>
                <tr className="border-b border-solid border-[var(--border-subtle)] text-[11px] uppercase tracking-wide text-[var(--text-tertiary)]">
                  <th className="px-3 py-2 font-medium">Request</th>
                  <th className="px-3 py-2 font-medium">Time</th>
                  <th className="px-3 py-2 font-medium">Model</th>
                  <th className="px-3 py-2 font-medium">Client</th>
                  <th className="px-3 py-2 font-medium">Project</th>
                  <th className="px-3 py-2 font-medium">Tokens in</th>
                  <th className="px-3 py-2 font-medium">Tokens out</th>
                  <th className="px-3 py-2 font-medium">Cost</th>
                  <th className="px-3 py-2 font-medium">Latency</th>
                </tr>
              </thead>
              <tbody>
                {rows.map((row) => (
                  <tr
                    key={row.request_id}
                    onClick={() => setSelected(row)}
                    className="cursor-pointer border-b border-solid border-[var(--border-subtle)] last:border-b-0 hover:bg-[var(--surface-hover)]"
                  >
                    <td className="px-3 py-2.5">
                      <MonoChip>{row.request_id.slice(0, 14)}…</MonoChip>
                    </td>
                    <td className="whitespace-nowrap px-3 py-2.5 text-[var(--text-secondary)]">
                      {formatCloudTime(row.created_at)}
                    </td>
                    <td className="px-3 py-2.5 font-mono text-[12px] text-[var(--text-secondary)]">{row.model ?? "—"}</td>
                    <td className="px-3 py-2.5 text-[var(--text-secondary)]">{row.client ?? "—"}</td>
                    <td className="px-3 py-2.5 text-[var(--text-secondary)]">
                      {row.project_id ? projectNames.get(row.project_id) ?? row.project_id : "—"}
                    </td>
                    <td className="px-3 py-2.5 text-[var(--text-secondary)]">{row.prompt_tokens.toLocaleString()}</td>
                    <td className="px-3 py-2.5 text-[var(--text-secondary)]">{row.completion_tokens.toLocaleString()}</td>
                    <td className="whitespace-nowrap px-3 py-2.5 text-[var(--text-secondary)]">{formatUsd(row.cost_usd)}</td>
                    <td className="px-3 py-2.5 text-[var(--text-secondary)]">{formatLatency(row.latency_ms)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <div className="mt-4 flex items-center justify-end gap-2 text-[12px] text-[var(--text-secondary)]">
            <button
              type="button"
              aria-label="Previous page"
              disabled={cursorStack.length === 0 || loading}
              onClick={goBack}
              className="inline-flex size-7 items-center justify-center rounded-md border border-solid border-[var(--border-subtle)] transition-colors hover:bg-[var(--surface-hover)] disabled:cursor-not-allowed disabled:opacity-40"
            >
              <HugeiconsIcon icon={ArrowLeft01Icon} size={14} />
            </button>
            <button
              type="button"
              aria-label="Next page"
              disabled={!nextCursor || loading}
              onClick={goNext}
              className="inline-flex size-7 items-center justify-center rounded-md border border-solid border-[var(--border-subtle)] transition-colors hover:bg-[var(--surface-hover)] disabled:cursor-not-allowed disabled:opacity-40"
            >
              <HugeiconsIcon icon={ArrowRight01Icon} size={14} />
            </button>
          </div>
        </>
      )}

      {selected && <RequestDrawer row={selected} scope={scope} onClose={() => setSelected(null)} />}
    </ListPage>
  );
}

export function LogsPage(): React.ReactNode {
  return isCloudGateway() ? <CloudLogsPage /> : <RuntimeLogsPage />;
}

export default LogsPage;
