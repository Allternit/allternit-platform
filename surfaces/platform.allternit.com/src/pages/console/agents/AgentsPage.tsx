import React, { useCallback, useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  BotIcon,
  AlertCircleIcon,
  ArrowRight01Icon,
} from "@hugeicons/core-free-icons";
import { cn } from "@/lib/utils";
import { formatApiError, isNoRuntime } from "@/lib/api-client";
import { NoRuntimeState } from "@/components/console-ui/NoRuntimeState";
import {
  type AgentRecord,
  formatTime,
  listAgents,
} from "@/lib/managed-agents";
import {
  ListPage,
  EmptyState,
  MonoChip,
  Badge,
  SkeletonRow,
  SETTINGS_SELECT_CLASS,
} from "@/components/console-ui";
import { QuickStartPanel } from "./QuickStartPanel";

const PAGE_SIZE = 25;

const STATUS_OPTIONS = ["", "active", "idle", "archived", "prototype", "disabled"];

function statusClass(status: string): string {
  switch (status) {
    case "archived":
      return "text-[var(--text-tertiary)]";
    case "prototype":
      return "text-[var(--status-warning)]";
    case "failed":
    case "error":
      return "text-[var(--status-error)]";
    default:
      return "text-[var(--status-success)]";
  }
}

const TH_CLASS = "text-left text-[11px] font-semibold uppercase tracking-wide text-[var(--text-tertiary)] px-3 py-2";
const TD_CLASS = "px-3 py-2.5 text-[13px] text-[var(--text-primary)]";

export function AgentsPage(): React.ReactNode {
  const navigate = useNavigate();
  const [agents, setAgents] = useState<AgentRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [statusFilter, setStatusFilter] = useState("");
  const [kindFilter, setKindFilter] = useState<"all" | "cloud" | "bot">("all");
  const [createdFrom, setCreatedFrom] = useState("");
  const [page, setPage] = useState(1);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setAgents(await listAgents());
    } catch (err) {
      setError(formatApiError(err, "Unable to load agents"));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    return agents.filter((agent) => {
      if (q && !agent.name.toLowerCase().includes(q) && !agent.id.toLowerCase().includes(q)) {
        return false;
      }
      if (statusFilter && agent.status !== statusFilter) return false;
      if (kindFilter === "bot" && !agent.is_bot) return false;
      if (kindFilter === "cloud" && agent.is_bot) return false;
      if (createdFrom) {
        const created = new Date(agent.created_at).getTime();
        const from = new Date(`${createdFrom}T00:00:00`).getTime();
        if (Number.isFinite(created) && Number.isFinite(from) && created < from) return false;
      }
      return true;
    });
  }, [agents, search, statusFilter, kindFilter, createdFrom]);

  useEffect(() => {
    setPage(1);
  }, [search, statusFilter, kindFilter, createdFrom]);

  const totalPages = Math.max(1, Math.ceil(filtered.length / PAGE_SIZE));
  const pageRows = filtered.slice((page - 1) * PAGE_SIZE, page * PAGE_SIZE);

  return (
    <div className="space-y-4">
      <ListPage
        title="Agents"
        subtitle="Managed agents — durable configurations with a model, system prompt, toolset, and skills. Bot agents additionally appear in the Hub."
        searchPlaceholder="Search by name or ID…"
        onSearch={setSearch}
        primaryAction={{ label: "+ Create agent", to: "/agents/new" }}
        filters={
          <>
            <select
              aria-label="Status filter"
              value={statusFilter}
              onChange={(e) => setStatusFilter(e.target.value)}
              className={SETTINGS_SELECT_CLASS}
            >
              {STATUS_OPTIONS.map((s) => (
                <option key={s} value={s}>
                  {s === "" ? "All statuses" : s}
                </option>
              ))}
            </select>
            <select
              aria-label="Kind filter"
              value={kindFilter}
              onChange={(e) => setKindFilter(e.target.value as "all" | "cloud" | "bot")}
              className={SETTINGS_SELECT_CLASS}
            >
              <option value="all">All kinds</option>
              <option value="cloud">Cloud</option>
              <option value="bot">Hub bots</option>
            </select>
            <label className="flex items-center gap-1.5 text-[12px] text-[var(--text-secondary)]">
              Created from
              <input
                type="date"
                value={createdFrom}
                onChange={(e) => setCreatedFrom(e.target.value)}
                className="rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] px-2 py-1.5 text-[12px] text-[var(--text-primary)] outline-none focus:border-[var(--accent-primary)]"
              />
            </label>
          </>
        }
        pagination={{ page, totalPages, onPage: setPage }}
        emptyState={
          <EmptyState
            icon={<HugeiconsIcon icon={BotIcon} size={32} />}
            title={search || statusFilter || kindFilter !== "all" ? "No agents match your filters" : "No agents yet"}
            caption={
              search || statusFilter || kindFilter !== "all"
                ? "Try widening the filters or clearing the search."
                : "Create your first managed agent — a durable configuration you can run in sessions, schedule as a deployment, and give tools and memory."
            }
            ctaLabel={search || statusFilter || kindFilter !== "all" ? undefined : "Create agent"}
            onCtaClick={
              search || statusFilter || kindFilter !== "all"
                ? undefined
                : () => navigate("/agents/new")
            }
          />
        }
      >
        {error && (isNoRuntime(error) ? (
          <NoRuntimeState className="mb-4" />
        ) : (
          <p className="mb-4 flex items-center gap-2 rounded-lg border border-solid border-[var(--status-error)]/30 bg-[var(--status-error)]/10 px-3 py-2 text-[13px] text-[var(--status-error)]">
            <HugeiconsIcon icon={AlertCircleIcon} size={14} />
            {error}
          </p>
        ))}

        {loading ? (
          <SkeletonRow lines={6} />
        ) : filtered.length > 0 ? (
          <div className="overflow-x-auto rounded-xl border border-solid border-[var(--border-subtle)]">
            <table className="w-full border-collapse">
              <thead>
                <tr className="border-b border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]">
                  <th className={cn(TH_CLASS, "w-8")}>
                    <input type="checkbox" aria-label="Select all rows" disabled title="Bulk actions are coming in a later phase" />
                  </th>
                  <th className={TH_CLASS}>ID</th>
                  <th className={TH_CLASS}>Name</th>
                  <th className={TH_CLASS}>Model</th>
                  <th className={TH_CLASS}>Status</th>
                  <th className={TH_CLASS}>Kind</th>
                  <th className={TH_CLASS}>Created</th>
                  <th className={TH_CLASS}>Last updated</th>
                  <th className={TH_CLASS} aria-label="Open" />
                </tr>
              </thead>
              <tbody>
                {pageRows.map((agent) => (
                  <tr
                    key={agent.id}
                    onClick={() => navigate(`/agents/${agent.id}`)}
                    className="cursor-pointer border-b border-solid border-[var(--border-subtle)] last:border-b-0 transition-colors hover:bg-[var(--surface-hover)]"
                  >
                    <td className={TD_CLASS} onClick={(e) => e.stopPropagation()}>
                      <input type="checkbox" aria-label={`Select ${agent.name}`} disabled title="Bulk actions are coming in a later phase" />
                    </td>
                    <td className={TD_CLASS}>
                      <MonoChip>{agent.id.slice(0, 8)}</MonoChip>
                    </td>
                    <td className={cn(TD_CLASS, "font-medium")}>{agent.name}</td>
                    <td className={cn(TD_CLASS, "font-mono text-[12px] text-[var(--text-secondary)]")}>
                      {agent.model}
                    </td>
                    <td className={TD_CLASS}>
                      <Badge className={statusClass(agent.status)}>{agent.status}</Badge>
                    </td>
                    <td className={TD_CLASS}>
                      {agent.is_bot ? (
                        <Badge className="text-[var(--accent-primary)]">Hub bot</Badge>
                      ) : (
                        <Badge>Cloud</Badge>
                      )}
                    </td>
                    <td className={cn(TD_CLASS, "text-[var(--text-secondary)]")}>
                      {formatTime(agent.created_at)}
                    </td>
                    <td className={cn(TD_CLASS, "text-[var(--text-secondary)]")}>
                      {formatTime(agent.updated_at)}
                    </td>
                    <td className={cn(TD_CLASS, "text-[var(--text-tertiary)]")}>
                      <HugeiconsIcon icon={ArrowRight01Icon} size={14} />
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ) : null}
      </ListPage>

      {!loading && agents.length > 0 && <QuickStartPanel />}
    </div>
  );
}

export default AgentsPage;
