import React, { useCallback, useEffect, useRef, useState } from "react";
import { HugeiconsIcon } from "@hugeicons/react";
import { Audit01Icon, AlertCircleIcon } from "@hugeicons/core-free-icons";
import { EmptyState, SkeletonRow, QUIET_BUTTON_CLASS } from "@/components/console-ui";
import { formatApiError } from "@/lib/api-client";
import { useProjectContext } from "@/lib/project-context";
import { downloadAuditCsv, listAuditLog, type AuditEvent, type AuditFilters } from "@/lib/governance";
import { cn } from "@/lib/utils";

const FIELD_CLASS =
  "rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] px-3 py-2 text-[13px] text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[var(--border-default)]";
const TH_CLASS = "px-3 py-2 text-left text-[11px] font-semibold uppercase tracking-wide text-[var(--text-tertiary)]";
const TD_CLASS = "px-3 py-2.5 align-top text-[13px] text-[var(--text-primary)]";

function formatTime(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString(undefined, { month: "short", day: "numeric", year: "numeric", hour: "numeric", minute: "2-digit" });
}

/**
 * Audit log: who changed keys, projects and organization settings. Reads
 * GET /api/v1/platform/audit-log for the selected project (or every project)
 * with cursor paging and a CSV download of the same filter.
 */
export function AuditLogPage(): React.ReactNode {
  const { project, projects } = useProjectContext();
  const [scope, setScope] = useState<"project" | "all">("project");
  const [action, setAction] = useState("");
  const [from, setFrom] = useState("");
  const [to, setTo] = useState("");
  const [events, setEvents] = useState<AuditEvent[]>([]);
  const [next, setNext] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [exporting, setExporting] = useState(false);
  const requestRef = useRef(0);

  const projectId = scope === "project" ? project?.id : undefined;
  // A project scope with no project picked yet falls back to every project.
  const filters: AuditFilters = {
    ...(projectId ? { project_id: projectId } : {}),
    ...(action.trim() ? { action: action.trim() } : {}),
    ...(from ? { from } : {}),
    ...(to ? { to } : {}),
  };
  const filterKey = JSON.stringify(filters);

  const load = useCallback(async () => {
    const request = ++requestRef.current;
    setLoading(true);
    setError(null);
    try {
      const page = await listAuditLog(JSON.parse(filterKey) as AuditFilters);
      if (request !== requestRef.current) return;
      setEvents(page.data);
      setNext(page.next);
    } catch (err) {
      if (request !== requestRef.current) return;
      setEvents([]);
      setNext(null);
      setError(formatApiError(err, "Unable to load the audit log."));
    } finally {
      if (request === requestRef.current) setLoading(false);
    }
  }, [filterKey]);

  useEffect(() => {
    void load();
  }, [load]);

  const loadMore = useCallback(async () => {
    if (!next) return;
    setLoadingMore(true);
    try {
      const page = await listAuditLog(JSON.parse(filterKey) as AuditFilters, next);
      setEvents((cur) => [...cur, ...page.data]);
      setNext(page.next);
    } catch (err) {
      setError(formatApiError(err, "Unable to load more events."));
    } finally {
      setLoadingMore(false);
    }
  }, [next, filterKey]);

  const exportCsv = useCallback(async () => {
    setExporting(true);
    setError(null);
    try {
      await downloadAuditCsv(JSON.parse(filterKey) as AuditFilters);
    } catch (err) {
      setError(formatApiError(err, "Unable to download the audit log."));
    } finally {
      setExporting(false);
    }
  }, [filterKey]);

  const filtered = Boolean(action.trim() || from || to);

  return (
    <div className="mx-auto w-full max-w-5xl px-4 py-6">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h1 className="m-0 text-[20px] font-semibold tracking-tight text-[var(--text-primary)]">Audit log</h1>
          <p className="m-0 mt-1 text-[13px] text-[var(--text-secondary)]">
            Who changed keys, projects and organization settings, and when.
          </p>
        </div>
        <button type="button" className={QUIET_BUTTON_CLASS} disabled={exporting || loading} onClick={() => void exportCsv()}>
          {exporting ? "Preparing…" : "Download CSV"}
        </button>
      </div>

      <div className="mt-4 grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-4">
        <label className="block text-[12px] font-medium text-[var(--text-secondary)]">
          Show
          <select
            value={scope}
            onChange={(e) => setScope(e.target.value as "project" | "all")}
            className={cn(FIELD_CLASS, "mt-1 w-full")}
          >
            <option value="project" disabled={!project}>
              {project ? `Project: ${project.name}` : "Selected project"}
            </option>
            <option value="all">All projects{projects.length > 1 ? ` (${projects.length})` : ""}</option>
          </select>
        </label>
        <label className="block text-[12px] font-medium text-[var(--text-secondary)]">
          Action
          <input value={action} onChange={(e) => setAction(e.target.value)} placeholder="key.created" className={cn(FIELD_CLASS, "mt-1 w-full")} />
        </label>
        <label className="block text-[12px] font-medium text-[var(--text-secondary)]">
          From
          <input type="date" value={from} max={to || undefined} onChange={(e) => setFrom(e.target.value)} className={cn(FIELD_CLASS, "mt-1 w-full")} />
        </label>
        <label className="block text-[12px] font-medium text-[var(--text-secondary)]">
          To
          <input type="date" value={to} min={from || undefined} onChange={(e) => setTo(e.target.value)} className={cn(FIELD_CLASS, "mt-1 w-full")} />
        </label>
      </div>

      {error && (
        <p role="alert" className="mb-0 mt-4 flex items-center gap-2 rounded-lg border border-solid border-[var(--status-error)]/30 bg-[var(--status-error)]/10 px-3 py-2 text-[13px] text-[var(--status-error)]">
          <HugeiconsIcon icon={AlertCircleIcon} size={14} />
          <span className="min-w-0 flex-1">{error}</span>
          <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => void load()}>
            Retry
          </button>
        </p>
      )}

      <div className="mt-4 rounded-xl border border-solid border-[var(--border-subtle)]">
        {loading ? (
          <div className="p-4">
            <SkeletonRow lines={5} />
          </div>
        ) : events.length === 0 && !error ? (
          <EmptyState
            icon={<HugeiconsIcon icon={Audit01Icon} size={32} />}
            title={filtered ? "No events match these filters" : "No audit events yet"}
            caption={
              filtered
                ? "Widen the date range or clear the action filter."
                : "Changes to API keys, projects and organization policy will be listed here."
            }
          />
        ) : events.length > 0 ? (
          <div className="overflow-x-auto">
            <table className="w-full min-w-[560px] border-collapse">
              <thead>
                <tr className="border-b border-solid border-[var(--border-subtle)]">
                  <th className={TH_CLASS}>Time</th>
                  <th className={TH_CLASS}>Actor</th>
                  <th className={TH_CLASS}>Action</th>
                  <th className={TH_CLASS}>Resource</th>
                  <th className={TH_CLASS}>IP</th>
                </tr>
              </thead>
              <tbody>
                {events.map((e) => (
                  <tr key={e.id} className="border-b border-solid border-[var(--border-subtle)] last:border-b-0">
                    <td className={cn(TD_CLASS, "whitespace-nowrap text-[var(--text-secondary)]")}>{formatTime(e.created_at)}</td>
                    <td className={cn(TD_CLASS, "break-all")}>{e.actor_email || "—"}</td>
                    <td className={cn(TD_CLASS, "font-mono text-[12px]")}>{e.action}</td>
                    <td className={cn(TD_CLASS, "break-all font-mono text-[12px]")}>{e.resource || "—"}</td>
                    <td className={cn(TD_CLASS, "font-mono text-[12px] text-[var(--text-secondary)]")}>{e.ip || "—"}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ) : null}
      </div>

      {next && !loading && (
        <div className="mt-4 flex justify-center">
          <button type="button" className={QUIET_BUTTON_CLASS} disabled={loadingMore} onClick={() => void loadMore()}>
            {loadingMore ? "Loading…" : "Load more"}
          </button>
        </div>
      )}
    </div>
  );
}

export default AuditLogPage;
