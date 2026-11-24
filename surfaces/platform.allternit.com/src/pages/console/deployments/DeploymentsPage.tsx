import React, { useCallback, useEffect, useState } from "react";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  AlertCircleIcon,
  ArrowRight01Icon,
  PlayIcon,
  PlusSignIcon,
  RocketIcon,
  TrashIcon,
  XIcon,
} from "@hugeicons/core-free-icons";
import { cn } from "@/lib/utils";
import { formatApiError, isNoRuntime } from "@/lib/api-client";
import { NoRuntimeState } from "@/components/console-ui/NoRuntimeState";
import {
  type AgentRecord,
  type ComputerRecord,
  type DeploymentRecord,
  type DeploymentRun,
  type VaultRecord,
  createDeployment,
  deleteDeployment,
  deploymentName,
  formatDate,
  formatTime,
  listAgents,
  listComputers,
  listDeploymentRuns,
  listDeployments,
  listVaults,
  patchDeployment,
  triggerDeployment,
} from "@/lib/managed-agents";
import {
  ListPage,
  EmptyState,
  MonoChip,
  Badge,
  SkeletonRow,
  QUIET_BUTTON_CLASS,
  DESTRUCTIVE_BUTTON_CLASS,
  SETTINGS_SELECT_CLASS,
} from "@/components/console-ui";

const INPUT_CLASS =
  "w-full rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] px-3 py-2 text-[13px] text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[var(--border-default)]";

const TH_CLASS = "text-left text-[11px] font-semibold uppercase tracking-wide text-[var(--text-tertiary)] px-3 py-2";
const TD_CLASS = "px-3 py-2.5 text-[13px] text-[var(--text-primary)]";

const CRON_HINTS = [
  { expr: "*/15 * * * *", hint: "every 15 minutes" },
  { expr: "0 * * * *", hint: "hourly" },
  { expr: "0 9 * * 1", hint: "Mondays at 9am" },
  { expr: "0 0 1 * *", hint: "first of the month" },
];

function statusClass(status: string): string {
  switch (status) {
    case "active":
      return "text-[var(--status-success)]";
    case "paused":
      return "text-[var(--status-warning)]";
    case "archived":
      return "text-[var(--text-tertiary)]";
    default:
      return "text-[var(--text-secondary)]";
  }
}

/**
 * Deployments — the beta schedule surface. A deployment binds an agent to a
 * cron schedule; the only backend body fields are agent_id, cron, and
 * metadata (beta_deployment_routes.rs CreateDeploymentBody), so name,
 * environment, and vault selections persist in metadata and round-trip
 * through PATCH.
 */
export function DeploymentsPage(): React.ReactNode {
  const [deployments, setDeployments] = useState<DeploymentRecord[]>([]);
  const [agents, setAgents] = useState<AgentRecord[]>([]);
  const [computers, setComputers] = useState<ComputerRecord[]>([]);
  const [vaults, setVaults] = useState<VaultRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [statusFilter, setStatusFilter] = useState("");
  const [showCreate, setShowCreate] = useState(false);

  // Create form state.
  const [name, setName] = useState("");
  const [agentId, setAgentId] = useState("");
  const [cron, setCron] = useState("0 9 * * *");
  const [computerId, setComputerId] = useState("");
  const [vaultIds, setVaultIds] = useState<string[]>([]);
  const [creating, setCreating] = useState(false);

  // Detail drawer state.
  const [selected, setSelected] = useState<DeploymentRecord | null>(null);
  const [runs, setRuns] = useState<DeploymentRun[] | null>(null);
  const [drawerError, setDrawerError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [list, agentList, computerList, vaultList] = await Promise.all([
        listDeployments(statusFilter || undefined),
        listAgents().catch(() => [] as AgentRecord[]),
        listComputers().catch(() => [] as ComputerRecord[]),
        listVaults().catch(() => [] as VaultRecord[]),
      ]);
      setDeployments(list);
      setAgents(agentList);
      setComputers(computerList);
      setVaults(vaultList);
    } catch (err) {
      setError(formatApiError(err, "Unable to load deployments"));
    } finally {
      setLoading(false);
    }
  }, [statusFilter]);

  useEffect(() => {
    void load();
  }, [load]);

  const openDrawer = useCallback(async (deployment: DeploymentRecord) => {
    setSelected(deployment);
    setRuns(null);
    setDrawerError(null);
    try {
      setRuns(await listDeploymentRuns(deployment.id));
    } catch (err) {
      setDrawerError(formatApiError(err, "Unable to load runs"));
    }
  }, []);

  const refreshDrawer = useCallback(
    async (deploymentId: string) => {
      try {
        setRuns(await listDeploymentRuns(deploymentId));
        setDeployments(await listDeployments(statusFilter || undefined));
      } catch (err) {
        setDrawerError(formatApiError(err, "Refresh failed"));
      }
    },
    [statusFilter]
  );

  const handleCreate = useCallback(async () => {
    setCreating(true);
    setError(null);
    try {
      if (!agentId) throw new Error("Pick an agent.");
      const metadata: Record<string, unknown> = {};
      if (name.trim()) metadata.name = name.trim();
      if (computerId) metadata.computer_id = computerId;
      if (vaultIds.length > 0) metadata.vault_ids = vaultIds;
      await createDeployment({ agent_id: agentId, cron: cron.trim(), metadata });
      setShowCreate(false);
      setName("");
      setAgentId("");
      setComputerId("");
      setVaultIds([]);
      await load();
    } catch (err) {
      setError(
        err instanceof Error && !("statusCode" in err)
          ? err.message
          : formatApiError(err, "Unable to create deployment")
      );
    } finally {
      setCreating(false);
    }
  }, [agentId, name, cron, computerId, vaultIds, load]);

  const handleToggle = useCallback(
    async (deployment: DeploymentRecord) => {
      setBusy(true);
      setDrawerError(null);
      try {
        const next = await patchDeployment(deployment.id, {
          status: deployment.status === "active" ? "paused" : "active",
        });
        setSelected(next);
        await load();
      } catch (err) {
        setDrawerError(formatApiError(err, "Status change failed"));
      } finally {
        setBusy(false);
      }
    },
    [load]
  );

  const handleTrigger = useCallback(
    async (deployment: DeploymentRecord) => {
      setBusy(true);
      setDrawerError(null);
      try {
        await triggerDeployment(deployment.id);
        await refreshDrawer(deployment.id);
      } catch (err) {
        setDrawerError(formatApiError(err, "Trigger failed"));
      } finally {
        setBusy(false);
      }
    },
    [refreshDrawer]
  );

  const handleDelete = useCallback(
    async (deployment: DeploymentRecord) => {
      if (!window.confirm(`Delete deployment "${deploymentName(deployment)}"? Its run history goes with it.`)) {
        return;
      }
      setBusy(true);
      setDrawerError(null);
      try {
        await deleteDeployment(deployment.id);
        setSelected(null);
        await load();
      } catch (err) {
        setDrawerError(formatApiError(err, "Delete failed"));
      } finally {
        setBusy(false);
      }
    },
    [load]
  );

  const agentName = (id?: string | null): string =>
    id ? (agents.find((a) => a.id === id)?.name ?? id.slice(0, 8)) : "—";

  return (
    <div className="relative">
      <ListPage
        title="Deployments"
        subtitle="Bind an agent to a cron schedule — the deployment runs it on cadence and records every run."
        searchPlaceholder={undefined}
        onSearch={undefined}
        filters={
          <select
            aria-label="Status filter"
            value={statusFilter}
            onChange={(e) => setStatusFilter(e.target.value)}
            className={SETTINGS_SELECT_CLASS}
          >
            <option value="">All statuses</option>
            <option value="active">active</option>
            <option value="paused">paused</option>
            <option value="archived">archived</option>
          </select>
        }
        primaryAction={{ label: "+ New deployment", onClick: () => setShowCreate((v) => !v) }}
        emptyState={
          <EmptyState
            icon={<HugeiconsIcon icon={RocketIcon} size={32} />}
            title="No deployments"
            caption="A deployment is the binding of an agent to credentials, an environment, and a schedule — create one and the scheduler runs it for you."
            ctaLabel="New deployment"
            onCtaClick={() => setShowCreate(true)}
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

        {showCreate && (
          <div className="mb-4 space-y-3 rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4">
            <div className="grid gap-3 sm:grid-cols-2">
              <div className="space-y-1">
                <label htmlFor="dep-name" className="text-[12px] font-semibold text-[var(--text-secondary)]">
                  Name
                </label>
                <input
                  id="dep-name"
                  type="text"
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  placeholder="Morning briefing"
                  className={INPUT_CLASS}
                />
              </div>
              <div className="space-y-1">
                <label htmlFor="dep-agent" className="text-[12px] font-semibold text-[var(--text-secondary)]">
                  Agent
                </label>
                <select
                  id="dep-agent"
                  value={agentId}
                  onChange={(e) => setAgentId(e.target.value)}
                  className={INPUT_CLASS}
                >
                  <option value="">Pick an agent…</option>
                  {agents
                    .filter((a) => a.status !== "archived")
                    .map((agent) => (
                      <option key={agent.id} value={agent.id}>
                        {agent.name}
                      </option>
                    ))}
                </select>
              </div>
            </div>
            <div className="space-y-1">
              <label htmlFor="dep-cron" className="text-[12px] font-semibold text-[var(--text-secondary)]">
                Schedule (cron)
              </label>
              <input
                id="dep-cron"
                type="text"
                value={cron}
                onChange={(e) => setCron(e.target.value)}
                placeholder="0 9 * * *"
                className={cn(INPUT_CLASS, "font-mono")}
              />
              <div className="flex flex-wrap gap-1.5 pt-1">
                {CRON_HINTS.map((h) => (
                  <button
                    key={h.expr}
                    type="button"
                    onClick={() => setCron(h.expr)}
                    className="rounded-full border border-solid border-[var(--border-subtle)] px-2.5 py-1 text-[11px] text-[var(--text-secondary)] transition-colors hover:text-[var(--text-primary)]"
                    title={h.hint}
                  >
                    <code className="font-mono">{h.expr}</code> · {h.hint}
                  </button>
                ))}
              </div>
            </div>
            <div className="grid gap-3 sm:grid-cols-2">
              <div className="space-y-1">
                <label htmlFor="dep-computer" className="text-[12px] font-semibold text-[var(--text-secondary)]">
                  Environment (computer) — stored in metadata
                </label>
                <select
                  id="dep-computer"
                  value={computerId}
                  onChange={(e) => setComputerId(e.target.value)}
                  className={INPUT_CLASS}
                >
                  <option value="">None</option>
                  {computers.map((computer) => (
                    <option key={computer.id} value={computer.id}>
                      {computer.name} ({computer.kind})
                    </option>
                  ))}
                </select>
              </div>
              <div className="space-y-1">
                <span className="text-[12px] font-semibold text-[var(--text-secondary)]">
                  Vaults — stored in metadata
                </span>
                <div className="max-h-28 space-y-1 overflow-y-auto rounded-lg border border-solid border-[var(--border-subtle)] p-2">
                  {vaults.length === 0 ? (
                    <p className="m-0 text-[12px] text-[var(--text-tertiary)]">No vaults in this organization.</p>
                  ) : (
                    vaults.map((vault) => (
                      <label key={vault.id} className="flex items-center gap-2 text-[12px] text-[var(--text-primary)]">
                        <input
                          type="checkbox"
                          checked={vaultIds.includes(vault.id)}
                          onChange={(e) =>
                            setVaultIds((prev) =>
                              e.target.checked ? [...prev, vault.id] : prev.filter((v) => v !== vault.id)
                            )
                          }
                          className="accent-[var(--accent-primary)]"
                        />
                        {vault.name}
                      </label>
                    ))
                  )}
                </div>
              </div>
            </div>
            <div className="flex items-center gap-2">
              <button
                type="button"
                onClick={() => void handleCreate()}
                disabled={creating || !agentId || !cron.trim()}
                className="inline-flex items-center gap-1.5 rounded-lg bg-[var(--accent-primary)] px-3.5 py-2 text-[13px] font-semibold text-[var(--ui-text-inverse)] transition-colors hover:brightness-110 disabled:opacity-50 disabled:cursor-not-allowed"
              >
                <HugeiconsIcon icon={PlusSignIcon} size={13} />
                {creating ? "Creating…" : "Create deployment"}
              </button>
              <button type="button" onClick={() => setShowCreate(false)} className={QUIET_BUTTON_CLASS}>
                Cancel
              </button>
            </div>
          </div>
        )}

        {loading ? (
          <SkeletonRow lines={5} />
        ) : deployments.length > 0 ? (
          <div className="overflow-x-auto rounded-xl border border-solid border-[var(--border-subtle)]">
            <table className="w-full border-collapse">
              <thead>
                <tr className="border-b border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]">
                  <th className={TH_CLASS}>ID</th>
                  <th className={TH_CLASS}>Name</th>
                  <th className={TH_CLASS}>Status</th>
                  <th className={TH_CLASS}>Agent</th>
                  <th className={TH_CLASS}>Trigger</th>
                  <th className={TH_CLASS}>Next run</th>
                  <th className={TH_CLASS}>Created</th>
                  <th className={TH_CLASS} aria-label="Open" />
                </tr>
              </thead>
              <tbody>
                {deployments.map((deployment) => (
                  <tr
                    key={deployment.id}
                    onClick={() => void openDrawer(deployment)}
                    className="cursor-pointer border-b border-solid border-[var(--border-subtle)] last:border-b-0 transition-colors hover:bg-[var(--surface-hover)]"
                  >
                    <td className={TD_CLASS}>
                      <MonoChip>{deployment.id.slice(0, 8)}</MonoChip>
                    </td>
                    <td className={cn(TD_CLASS, "font-medium")}>{deploymentName(deployment)}</td>
                    <td className={TD_CLASS}>
                      <Badge className={statusClass(deployment.status)}>{deployment.status}</Badge>
                    </td>
                    <td className={cn(TD_CLASS, "text-[var(--text-secondary)]")}>
                      {agentName(deployment.agent_id)}
                    </td>
                    <td className={cn(TD_CLASS, "font-mono text-[12px] text-[var(--text-secondary)]")}>
                      {deployment.cron}
                    </td>
                    <td className={cn(TD_CLASS, "text-[var(--text-secondary)]")}>
                      {deployment.next_run_at ? formatTime(deployment.next_run_at) : "—"}
                    </td>
                    <td className={cn(TD_CLASS, "text-[var(--text-secondary)]")}>
                      {formatTime(deployment.created_at)}
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

      {selected && (
        <div className="fixed inset-0 z-40 flex justify-end bg-black/40" onClick={() => setSelected(null)}>
          <div
            className="h-full w-full max-w-md overflow-y-auto border-l border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] p-5"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="flex items-start justify-between gap-3">
              <div className="min-w-0">
                <h2 className="m-0 text-[16px] font-semibold text-[var(--text-primary)]">
                  {deploymentName(selected)}
                </h2>
                <div className="mt-1.5 flex items-center gap-2">
                  <MonoChip>{selected.id.slice(0, 8)}</MonoChip>
                  <Badge className={statusClass(selected.status)}>{selected.status}</Badge>
                </div>
              </div>
              <button
                type="button"
                onClick={() => setSelected(null)}
                className="text-[var(--text-tertiary)] transition-colors hover:text-[var(--text-primary)]"
                aria-label="Close"
              >
                <HugeiconsIcon icon={XIcon} size={16} />
              </button>
            </div>

            <dl className="m-0 mt-4 space-y-2 text-[13px]">
              <div className="flex justify-between gap-4">
                <dt className="text-[var(--text-tertiary)]">Agent</dt>
                <dd className="m-0 text-[var(--text-primary)]">{agentName(selected.agent_id)}</dd>
              </div>
              <div className="flex justify-between gap-4">
                <dt className="text-[var(--text-tertiary)]">Cron</dt>
                <dd className="m-0 font-mono text-[12px] text-[var(--text-primary)]">{selected.cron}</dd>
              </div>
              <div className="flex justify-between gap-4">
                <dt className="text-[var(--text-tertiary)]">Next run</dt>
                <dd className="m-0 text-[var(--text-primary)]">
                  {selected.next_run_at ? formatDate(selected.next_run_at) : "—"}
                </dd>
              </div>
              <div className="flex justify-between gap-4">
                <dt className="text-[var(--text-tertiary)]">Last run</dt>
                <dd className="m-0 text-[var(--text-primary)]">
                  {selected.last_run_at ? formatDate(selected.last_run_at) : "—"}
                </dd>
              </div>
              {typeof selected.metadata?.computer_id === "string" && (
                <div className="flex justify-between gap-4">
                  <dt className="text-[var(--text-tertiary)]">Environment</dt>
                  <dd className="m-0 text-[var(--text-primary)]">
                    {computers.find((c) => c.id === selected.metadata.computer_id)?.name ??
                      (selected.metadata.computer_id as string).slice(0, 8)}
                  </dd>
                </div>
              )}
              {Array.isArray(selected.metadata?.vault_ids) && (selected.metadata.vault_ids as unknown[]).length > 0 && (
                <div className="flex justify-between gap-4">
                  <dt className="text-[var(--text-tertiary)]">Vaults</dt>
                  <dd className="m-0 text-[var(--text-primary)]">
                    {(selected.metadata.vault_ids as string[])
                      .map((v) => vaults.find((vault) => vault.id === v)?.name ?? v.slice(0, 8))
                      .join(", ")}
                  </dd>
                </div>
              )}
            </dl>

            <div className="mt-4 flex flex-wrap gap-2">
              <button
                type="button"
                onClick={() => void handleTrigger(selected)}
                disabled={busy}
                className="inline-flex items-center gap-1.5 rounded-lg bg-[var(--accent-primary)] px-3 py-1.5 text-[12px] font-semibold text-[var(--ui-text-inverse)] transition-colors hover:brightness-110 disabled:opacity-50"
              >
                <HugeiconsIcon icon={PlayIcon} size={12} />
                Trigger now
              </button>
              <button
                type="button"
                onClick={() => void handleToggle(selected)}
                disabled={busy}
                className={cn(QUIET_BUTTON_CLASS, "px-3 py-1.5 text-[12px]")}
              >
                {selected.status === "active" ? "Pause" : "Resume"}
              </button>
              <button
                type="button"
                onClick={() => void handleDelete(selected)}
                disabled={busy}
                className={cn(DESTRUCTIVE_BUTTON_CLASS, "px-3 py-1.5 text-[12px]")}
              >
                <HugeiconsIcon icon={TrashIcon} size={12} />
                Delete
              </button>
            </div>

            {drawerError && (
              <p className="mt-3 flex items-center gap-2 rounded-lg border border-solid border-[var(--status-error)]/30 bg-[var(--status-error)]/10 px-3 py-2 text-[12px] text-[var(--status-error)]">
                <HugeiconsIcon icon={AlertCircleIcon} size={13} />
                {drawerError}
              </p>
            )}

            <h3 className="m-0 mt-6 mb-2 text-[13px] font-semibold text-[var(--text-primary)]">Runs</h3>
            {runs === null ? (
              <p className="text-[12px] text-[var(--text-tertiary)]">Loading runs…</p>
            ) : runs.length === 0 ? (
              <p className="text-[12px] text-[var(--text-tertiary)]">
                No runs yet — trigger the deployment or wait for the schedule.
              </p>
            ) : (
              <div className="space-y-1.5">
                {runs.map((run) => (
                  <div
                    key={run.id}
                    className="rounded-lg border border-solid border-[var(--border-subtle)] px-3 py-2 text-[12px]"
                  >
                    <div className="flex items-center gap-2">
                      <MonoChip>{run.id.slice(0, 8)}</MonoChip>
                      <Badge
                        className={
                          run.status === "succeeded"
                            ? "text-[var(--status-success)]"
                            : run.status === "failed"
                              ? "text-[var(--status-error)]"
                              : "text-[var(--status-warning)]"
                        }
                      >
                        {run.status}
                      </Badge>
                      <span className="ml-auto text-[var(--text-tertiary)]">
                        {run.triggered_by ?? "schedule"}
                      </span>
                    </div>
                    <div className="mt-1 text-[var(--text-tertiary)]">
                      {formatDate(run.started_at)}
                      {run.finished_at ? ` → ${formatTime(run.finished_at)}` : " → running"}
                    </div>
                    {run.error && <div className="mt-1 text-[var(--status-error)]">{run.error}</div>}
                  </div>
                ))}
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

export default DeploymentsPage;
