import React, { useCallback, useEffect, useState } from "react";
import { HugeiconsIcon } from "@hugeicons/react";
import { AlertCircleIcon, MonitorIcon } from "@hugeicons/core-free-icons";
import { cn } from "@/lib/utils";
import { formatApiError, isNoRuntime } from "@/lib/api-client";
import { NoRuntimeState } from "@/components/console-ui/NoRuntimeState";
import {
  COMPUTER_KINDS,
  type ComputerKindFilter,
  type ComputerRecord,
  formatTime,
  listComputers,
} from "@/lib/managed-agents";
import { ComputeBillingPanel } from "@/components/settings/ComputeBillingPanel";
import {
  ListPage,
  EmptyState,
  MonoChip,
  Badge,
  SkeletonRow,
  SETTINGS_SELECT_CLASS,
} from "@/components/console-ui";

const TH_CLASS = "text-left text-[11px] font-semibold uppercase tracking-wide text-[var(--text-tertiary)] px-3 py-2";
const TD_CLASS = "px-3 py-2.5 text-[13px] text-[var(--text-primary)]";

function statusClass(status: string): string {
  switch (status) {
    case "running":
      return "text-[var(--status-success)]";
    case "creating":
      return "text-[var(--status-warning)]";
    case "error":
      return "text-[var(--status-error)]";
    default:
      return "text-[var(--text-secondary)]";
  }
}

/**
 * Managed computers — the relocated compute surface. The Compute page's
 * billing/hosting panel moves here unchanged, with the computers table and
 * its kind filter on top. /compute keeps working (same shared panel).
 */
export function ComputersPage(): React.ReactNode {
  const [computers, setComputers] = useState<ComputerRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [unavailable, setUnavailable] = useState(false);
  const [kindFilter, setKindFilter] = useState<"" | ComputerKindFilter>("");

  const load = useCallback(async (kind?: string) => {
    setLoading(true);
    setError(null);
    setUnavailable(false);
    try {
      setComputers(await listComputers(kind));
    } catch (err) {
      const message = formatApiError(err, "Unable to load computers");
      setError(message);
      // The computer routes 503 with "No VM driver is configured on this host"
      // when the gateway has no VM driver — say so instead of a bare error.
      setUnavailable(/no vm driver|503/i.test(message));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load(kindFilter || undefined);
  }, [load, kindFilter]);

  return (
    <div className="space-y-8">
      <ListPage
        title="Computers"
        subtitle="Managed cloud computers for agent use — provisioned on demand with a browser, shell, and filesystem."
        searchPlaceholder={undefined}
        onSearch={undefined}
        filters={
          <select
            aria-label="Kind filter"
            value={kindFilter}
            onChange={(e) => setKindFilter(e.target.value as "" | ComputerKindFilter)}
            className={SETTINGS_SELECT_CLASS}
          >
            <option value="">All kinds</option>
            {COMPUTER_KINDS.map((kind) => (
              <option key={kind} value={kind}>
                {kind}
              </option>
            ))}
          </select>
        }
        emptyState={
          <EmptyState
            icon={<HugeiconsIcon icon={MonitorIcon} size={32} />}
            title="No computers"
            caption="Provisioned computers appear here once created — through the API, a bot, or a session that requests one."
          />
        }
      >
        {unavailable && (
          <p className="mb-4 flex items-center gap-2 rounded-lg border border-solid border-[var(--status-warning)]/30 bg-[var(--status-warning)]/10 px-3 py-2 text-[13px] text-[var(--status-warning)]">
            <HugeiconsIcon icon={AlertCircleIcon} size={14} />
            Computer management is unavailable on this gateway — no VM driver is configured. Incus/Tart
            runs on the Computer Cloud VPS.
          </p>
        )}
        {error && !unavailable && (isNoRuntime(error) ? (
          <NoRuntimeState className="mb-4" />
        ) : (
          <p className="mb-4 flex items-center gap-2 rounded-lg border border-solid border-[var(--status-error)]/30 bg-[var(--status-error)]/10 px-3 py-2 text-[13px] text-[var(--status-error)]">
            <HugeiconsIcon icon={AlertCircleIcon} size={14} />
            {error}
          </p>
        ))}

        {loading ? (
          <SkeletonRow lines={5} />
        ) : computers.length > 0 ? (
          <div className="overflow-x-auto rounded-xl border border-solid border-[var(--border-subtle)]">
            <table className="w-full border-collapse">
              <thead>
                <tr className="border-b border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]">
                  <th className={TH_CLASS}>ID</th>
                  <th className={TH_CLASS}>Name</th>
                  <th className={TH_CLASS}>Kind</th>
                  <th className={TH_CLASS}>Status</th>
                  <th className={TH_CLASS}>Provider</th>
                  <th className={TH_CLASS}>Specs</th>
                  <th className={TH_CLASS}>Region</th>
                  <th className={TH_CLASS}>Last activity</th>
                </tr>
              </thead>
              <tbody>
                {computers.map((computer) => (
                  <tr
                    key={computer.id}
                    className="border-b border-solid border-[var(--border-subtle)] last:border-b-0"
                  >
                    <td className={TD_CLASS}>
                      <MonoChip>{computer.id.slice(0, 8)}</MonoChip>
                    </td>
                    <td className={cn(TD_CLASS, "font-medium")}>{computer.name}</td>
                    <td className={TD_CLASS}>
                      <Badge>{computer.kind}</Badge>
                    </td>
                    <td className={TD_CLASS}>
                      <Badge className={statusClass(computer.status)}>{computer.status}</Badge>
                    </td>
                    <td className={cn(TD_CLASS, "text-[var(--text-secondary)]")}>{computer.provider}</td>
                    <td className={cn(TD_CLASS, "text-[12px] text-[var(--text-secondary)]")}>
                      {computer.cpu_cores != null || computer.memory_mb != null
                        ? `${computer.cpu_cores ?? "?"} vCPU · ${computer.memory_mb != null ? `${Math.round(computer.memory_mb / 1024)} GB` : "?"}`
                        : "—"}
                    </td>
                    <td className={cn(TD_CLASS, "text-[var(--text-secondary)]")}>{computer.region ?? "—"}</td>
                    <td className={cn(TD_CLASS, "text-[var(--text-secondary)]")}>
                      {formatTime(computer.last_activity_at ?? computer.updated_at)}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ) : null}
      </ListPage>

      <section>
        <h2 className="m-0 mb-1 text-[16px] font-semibold text-[var(--text-primary)]">Compute settings</h2>
        <p className="m-0 mb-4 text-[13px] text-[var(--text-secondary)]">
          Local compute, managed hosting, and enterprise BYOC configuration — the former Compute page.
        </p>
        <ComputeBillingPanel />
      </section>
    </div>
  );
}

export default ComputersPage;
