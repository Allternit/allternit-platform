import React, { useCallback, useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  AlertCircleIcon,
  ComputerIcon,
  ArrowRight01Icon,
} from "@hugeicons/core-free-icons";
import { cn } from "@/lib/utils";
import { formatApiError, isNoRuntime } from "@/lib/api-client";
import { NoRuntimeState } from "@/components/console-ui/NoRuntimeState";
import {
  type AgentRecord,
  type SessionRecord,
  formatTime,
  listAgents,
  listSessions,
} from "@/lib/managed-agents";
import {
  ListPage,
  EmptyState,
  MonoChip,
  Badge,
  SkeletonRow,
  SETTINGS_SELECT_CLASS,
} from "@/components/console-ui";

const PAGE_SIZE = 25;

const STATUS_OPTIONS = ["", "idle", "running", "waiting", "failed", "archived"];

function statusClass(status?: string): string {
  switch (status) {
    case "running":
      return "text-[var(--status-success)]";
    case "waiting":
      return "text-[var(--status-warning)]";
    case "failed":
      return "text-[var(--status-error)]";
    case "archived":
      return "text-[var(--text-tertiary)]";
    default:
      return "text-[var(--text-secondary)]";
  }
}

const TH_CLASS = "text-left text-[11px] font-semibold uppercase tracking-wide text-[var(--text-tertiary)] px-3 py-2";
const TD_CLASS = "px-3 py-2.5 text-[13px] text-[var(--text-primary)]";

function agentName(agents: AgentRecord[], id?: string | null): string {
  if (!id) return "—";
  return agents.find((a) => a.id === id)?.name ?? id.slice(0, 8);
}

export function SessionsPage(): React.ReactNode {
  const navigate = useNavigate();
  const [sessions, setSessions] = useState<SessionRecord[]>([]);
  const [agents, setAgents] = useState<AgentRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [statusFilter, setStatusFilter] = useState("");
  const [agentFilter, setAgentFilter] = useState("");
  const [createdFrom, setCreatedFrom] = useState("");
  const [page, setPage] = useState(1);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [sessionList, agentList] = await Promise.all([
        listSessions(),
        listAgents().catch(() => [] as AgentRecord[]),
      ]);
      setSessions(sessionList);
      setAgents(agentList);
    } catch (err) {
      setError(formatApiError(err, "Unable to load sessions"));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    return sessions.filter((session) => {
      if (q) {
        const name = (session.name ?? "").toLowerCase();
        if (!name.includes(q) && !session.id.toLowerCase().includes(q)) return false;
      }
      if (statusFilter && session.status !== statusFilter) return false;
      if (agentFilter && session.agent_id !== agentFilter) return false;
      if (createdFrom) {
        const created = new Date(session.created_at).getTime();
        const from = new Date(`${createdFrom}T00:00:00`).getTime();
        if (Number.isFinite(created) && Number.isFinite(from) && created < from) return false;
      }
      return true;
    });
  }, [sessions, search, statusFilter, agentFilter, createdFrom]);

  useEffect(() => {
    setPage(1);
  }, [search, statusFilter, agentFilter, createdFrom]);

  const totalPages = Math.max(1, Math.ceil(filtered.length / PAGE_SIZE));
  const pageRows = filtered.slice((page - 1) * PAGE_SIZE, page * PAGE_SIZE);

  // Budget fields are derived server-side (cloud_agents_routes.rs public_budget):
  // estimated_cost_usd is always present, tokens_used only once work ran.
  const hasAnyTokens = filtered.some((s) => s.budget?.tokens_used != null);

  return (
    <ListPage
      title="Sessions"
      subtitle="Stateful conversations between an agent, its tools, and a computer. Sessions appear here once created through the API, the playground, or the console."
      searchPlaceholder="Search by name or ID…"
      onSearch={setSearch}
      primaryAction={{ label: "+ New session", to: "/sessions/new" }}
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
            aria-label="Agent filter"
            value={agentFilter}
            onChange={(e) => setAgentFilter(e.target.value)}
            className={SETTINGS_SELECT_CLASS}
          >
            <option value="">All agents</option>
            {agents.map((agent) => (
              <option key={agent.id} value={agent.id}>
                {agent.name}
              </option>
            ))}
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
          icon={<HugeiconsIcon icon={ComputerIcon} size={32} />}
          title={search || statusFilter || agentFilter ? "No sessions match your filters" : "No sessions yet"}
          caption={
            search || statusFilter || agentFilter
              ? "Try widening the filters or clearing the search."
              : "Sessions appear here once created through the API or the playground. Start one from “New session”, or copy the curl template and run it anywhere."
          }
          ctaLabel={search || statusFilter || agentFilter ? undefined : "New session"}
          onCtaClick={
            search || statusFilter || agentFilter ? undefined : () => navigate("/sessions/new")
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
                <th className={TH_CLASS}>ID</th>
                <th className={TH_CLASS}>Name</th>
                <th className={TH_CLASS}>Status</th>
                <th className={TH_CLASS}>Agent</th>
                <th className={TH_CLASS}>Cost (est.)</th>
                {hasAnyTokens && <th className={TH_CLASS}>Tokens</th>}
                <th className={TH_CLASS}>Computer</th>
                <th className={TH_CLASS}>Created</th>
                <th className={TH_CLASS} aria-label="Open" />
              </tr>
            </thead>
            <tbody>
              {pageRows.map((session) => (
                <tr
                  key={session.id}
                  onClick={() => navigate(`/sessions/${session.id}`)}
                  className="cursor-pointer border-b border-solid border-[var(--border-subtle)] last:border-b-0 transition-colors hover:bg-[var(--surface-hover)]"
                >
                  <td className={TD_CLASS}>
                    <MonoChip>{session.id.slice(0, 8)}</MonoChip>
                  </td>
                  <td className={cn(TD_CLASS, "font-medium")}>{session.name ?? session.id.slice(0, 8)}</td>
                  <td className={TD_CLASS}>
                    <Badge className={statusClass(session.status)}>{session.status ?? "—"}</Badge>
                  </td>
                  <td className={cn(TD_CLASS, "text-[var(--text-secondary)]")}>
                    {agentName(agents, session.agent_id)}
                  </td>
                  <td className={cn(TD_CLASS, "text-[var(--text-secondary)]")}>
                    {session.budget?.estimated_cost_usd != null
                      ? `$${Number(session.budget.estimated_cost_usd).toFixed(4)}`
                      : "—"}
                  </td>
                  {hasAnyTokens && (
                    <td className={cn(TD_CLASS, "text-[var(--text-secondary)]")}>
                      {session.budget?.tokens_used != null ? session.budget.tokens_used : "—"}
                    </td>
                  )}
                  <td className={cn(TD_CLASS, "text-[var(--text-secondary)]")}>
                    {session.computer?.kind ?? "none"}
                  </td>
                  <td className={cn(TD_CLASS, "text-[var(--text-secondary)]")}>
                    {formatTime(session.created_at)}
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
  );
}

export default SessionsPage;
