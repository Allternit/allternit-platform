import React, { useCallback, useEffect, useState } from "react";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  PlayIcon,
  PauseIcon,
  Cancel01Icon,
  TrashIcon,
  PlusSignIcon,
  Refresh01Icon,
  AlertCircleIcon,
} from "@hugeicons/core-free-icons";
import { cn } from "@/lib/utils";
import { formatApiError } from "@/lib/api-client";
import {
  type RunSummary,
  type RunMode,
  listRuns,
  createRun,
  startRun,
  pauseRun,
  resumeRun,
  cancelRun,
  deleteRun,
} from "@/lib/runs";
import { EmptyState } from "@/components/settings/EmptyState";
import { SkeletonRow } from "@/components/settings/SkeletonRow";
import { Badge } from "@/components/settings/Badge";
import { QUIET_BUTTON_CLASS, DESTRUCTIVE_BUTTON_CLASS } from "@/components/settings/buttonStyles";

function formatTime(iso?: string | null): string {
  if (!iso) return "—";
  const date = new Date(iso);
  const diff = Date.now() - date.getTime();
  if (diff < 60_000) return "Just now";
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} min ago`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} hr ago`;
  return date.toLocaleDateString();
}

function statusColor(status: string): string {
  switch (status) {
    case "running":
      return "text-[var(--status-success)] bg-[var(--status-success)]/10";
    case "pending":
    case "planning":
    case "queued":
      return "text-[var(--accent-highlight)] bg-[var(--accent-highlight)]/10";
    case "paused":
      return "text-[var(--status-warning)] bg-[var(--status-warning)]/10";
    case "completed":
      return "text-[var(--accent-primary)] bg-[var(--accent-primary)]/10";
    case "failed":
    case "cancelled":
      return "text-[var(--status-error)] bg-[var(--status-error)]/10";
    default:
      return "text-[var(--text-secondary)] bg-[var(--bg-primary)]";
  }
}

export function RunsPage() {
  const [runs, setRuns] = useState<RunSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [showCreate, setShowCreate] = useState(false);

  const [name, setName] = useState("");
  const [mode, setMode] = useState<RunMode>("local");
  const [command, setCommand] = useState("");
  const [description, setDescription] = useState("");
  const [creating, setCreating] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const data = await listRuns();
      setRuns(data);
    } catch (err) {
      setError(formatApiError(err, "Unable to load runs"));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const handleCreate = useCallback(async () => {
    if (!name.trim() || !command.trim()) return;
    setCreating(true);
    setError(null);
    try {
      await createRun({
        name: name.trim(),
        description: description.trim() || undefined,
        mode,
        config: { command: command.trim() },
        auto_start: true,
      });
      setName("");
      setCommand("");
      setDescription("");
      setShowCreate(false);
      await load();
    } catch (err) {
      setError(formatApiError(err, "Unable to create run"));
    } finally {
      setCreating(false);
    }
  }, [name, mode, command, description, load]);

  const action = useCallback(
    async (id: string, fn: (id: string) => Promise<unknown>) => {
      setBusyId(id);
      setError(null);
      try {
        await fn(id);
        await load();
      } catch (err) {
        setError(formatApiError(err, "Unable to update run"));
      } finally {
        setBusyId(null);
      }
    },
    [load]
  );

  return (
    <div className="space-y-6">
      <div className="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-4">
        <div>
          <h1 className="text-[22px] font-semibold tracking-tight text-[var(--text-primary)]">
            Runs
          </h1>
          <p className="text-[13px] text-[var(--text-secondary)] mt-1">
            View and control agent runs across local, remote, and cloud runtimes.
          </p>
        </div>
        <div className="flex items-center gap-2">
          <button
            type="button"
            onClick={() => void load()}
            disabled={loading}
            className={QUIET_BUTTON_CLASS}
          >
            <HugeiconsIcon
              icon={Refresh01Icon}
              size={13}
              className={cn(loading && "animate-spin")}
            />
            Refresh
          </button>
          <button
            type="button"
            onClick={() => setShowCreate((s) => !s)}
            className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-[13px] font-medium bg-[var(--accent-primary)] text-[var(--ui-text-inverse)] hover:brightness-110 transition-all disabled:opacity-50"
          >
            <HugeiconsIcon icon={PlusSignIcon} size={13} />
            New run
          </button>
        </div>
      </div>

      {showCreate && (
        <div className="rounded-2xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-5">
          <h2 className="text-[14px] font-semibold text-[var(--text-primary)] mb-4">Create run</h2>
          <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
            <div className="space-y-1">
              <label className="text-[11px] font-medium text-[var(--text-tertiary)]">Name</label>
              <input
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="my-run"
                className="w-full p-2 px-3 rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] text-[13px] text-[var(--text-primary)] outline-none focus:border-[var(--accent-primary)] placeholder:text-[var(--text-tertiary)]"
              />
            </div>
            <div className="space-y-1">
              <label className="text-[11px] font-medium text-[var(--text-tertiary)]">Mode</label>
              <select
                value={mode}
                onChange={(e) => setMode(e.target.value as RunMode)}
                className="w-full p-2 px-3 rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] text-[13px] text-[var(--text-primary)] outline-none focus:border-[var(--accent-primary)]"
              >
                <option value="local">local</option>
                <option value="remote">remote</option>
                <option value="cloud">cloud</option>
              </select>
            </div>
            <div className="sm:col-span-2 space-y-1">
              <label className="text-[11px] font-medium text-[var(--text-tertiary)]">Command</label>
              <input
                value={command}
                onChange={(e) => setCommand(e.target.value)}
                placeholder="e.g. allternit run --workspace ./project"
                className="w-full p-2 px-3 rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] text-[13px] text-[var(--text-primary)] outline-none focus:border-[var(--accent-primary)] placeholder:text-[var(--text-tertiary)]"
              />
            </div>
            <div className="sm:col-span-2 space-y-1">
              <label className="text-[11px] font-medium text-[var(--text-tertiary)]">
                Description (optional)
              </label>
              <input
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                placeholder="What is this run for?"
                className="w-full p-2 px-3 rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] text-[13px] text-[var(--text-primary)] outline-none focus:border-[var(--accent-primary)] placeholder:text-[var(--text-tertiary)]"
              />
            </div>
          </div>
          <div className="flex items-center justify-end gap-2 mt-4">
            <button
              type="button"
              onClick={() => setShowCreate(false)}
              className={QUIET_BUTTON_CLASS}
            >
              Cancel
            </button>
            <button
              type="button"
              onClick={() => void handleCreate()}
              disabled={creating || !name.trim() || !command.trim()}
              className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-[13px] font-medium bg-[var(--accent-primary)] text-[var(--ui-text-inverse)] hover:brightness-110 transition-all disabled:opacity-50"
            >
              {creating && <HugeiconsIcon icon={Refresh01Icon} size={13} className="animate-spin" />}
              Create run
            </button>
          </div>
        </div>
      )}

      {error && !loading && (
        <div className="rounded-xl border border-dashed border-[var(--status-error)]/30 bg-[var(--status-error)]/10 p-4">
          <div className="flex items-start gap-2 text-[13px] text-[var(--status-error)]">
            <HugeiconsIcon icon={AlertCircleIcon} size={16} className="shrink-0 mt-0.5" />
            <span>{error}</span>
          </div>
        </div>
      )}

      {loading ? (
        <div className="rounded-2xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-5">
          <SkeletonRow lines={5} />
        </div>
      ) : runs.length === 0 ? (
        <div className="rounded-2xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]">
          <EmptyState
            art="bell"
            size="panel"
            paper="var(--bg-secondary)"
            title="No runs yet"
            caption="Create a run to execute an agent workflow on your connected runtimes."
            ctaLabel="Refresh"
            onCtaClick={() => void load()}
          />
        </div>
      ) : (
        <div className="rounded-2xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] overflow-hidden">
          <div className="overflow-x-auto">
            <table className="w-full text-left text-[13px]">
              <thead>
                <tr className="border-b border-[var(--border-subtle)] text-[11px] uppercase tracking-wider text-[var(--text-tertiary)]">
                  <th className="px-4 py-3 font-semibold">Run</th>
                  <th className="px-4 py-3 font-semibold">Mode</th>
                  <th className="px-4 py-3 font-semibold">Status</th>
                  <th className="px-4 py-3 font-semibold">Progress</th>
                  <th className="px-4 py-3 font-semibold">Created</th>
                  <th className="px-4 py-3 font-semibold text-right">Actions</th>
                </tr>
              </thead>
              <tbody className="text-[var(--text-secondary)]">
                {runs.map((run) => (
                  <tr key={run.id} className="border-b border-[var(--border-subtle)] last:border-0">
                    <td className="px-4 py-3">
                      <div className="text-[var(--text-primary)] font-medium">{run.name}</div>
                    </td>
                    <td className="px-4 py-3 capitalize">{run.mode}</td>
                    <td className="px-4 py-3">
                      <Badge className={statusColor(run.status)}>{run.status}</Badge>
                    </td>
                    <td className="px-4 py-3">
                      {run.total_steps != null
                        ? `${run.completed_steps} / ${run.total_steps} steps`
                        : `${run.completed_steps} steps`}
                    </td>
                    <td className="px-4 py-3">{formatTime(run.created_at)}</td>
                    <td className="px-4 py-3">
                      <div className="flex items-center justify-end gap-2">
                        {run.status !== "running" && run.status !== "completed" && run.status !== "failed" && run.status !== "cancelled" && (
                          <button
                            type="button"
                            title="Start"
                            disabled={busyId === run.id}
                            onClick={() => void action(run.id, startRun)}
                            className="inline-flex items-center gap-1 px-2 py-1 rounded-md text-[12px] font-medium bg-[var(--status-success)]/10 text-[var(--status-success)] hover:bg-[var(--status-success)]/20 transition-colors disabled:opacity-50"
                          >
                            <HugeiconsIcon icon={PlayIcon} size={12} />
                            Start
                          </button>
                        )}
                        {run.status === "running" && (
                          <button
                            type="button"
                            title="Pause"
                            disabled={busyId === run.id}
                            onClick={() => void action(run.id, pauseRun)}
                            className="inline-flex items-center gap-1 px-2 py-1 rounded-md text-[12px] font-medium bg-[var(--status-warning)]/10 text-[var(--status-warning)] hover:bg-[var(--status-warning)]/20 transition-colors disabled:opacity-50"
                          >
                            <HugeiconsIcon icon={PauseIcon} size={12} />
                            Pause
                          </button>
                        )}
                        {run.status === "paused" && (
                          <button
                            type="button"
                            title="Resume"
                            disabled={busyId === run.id}
                            onClick={() => void action(run.id, resumeRun)}
                            className="inline-flex items-center gap-1 px-2 py-1 rounded-md text-[12px] font-medium bg-[var(--status-success)]/10 text-[var(--status-success)] hover:bg-[var(--status-success)]/20 transition-colors disabled:opacity-50"
                          >
                            <HugeiconsIcon icon={PlayIcon} size={12} />
                            Resume
                          </button>
                        )}
                        {(run.status === "running" || run.status === "paused" || run.status === "pending" || run.status === "planning" || run.status === "queued") && (
                          <button
                            type="button"
                            title="Cancel"
                            disabled={busyId === run.id}
                            onClick={() => void action(run.id, cancelRun)}
                            className="inline-flex items-center gap-1 px-2 py-1 rounded-md text-[12px] font-medium text-[var(--text-secondary)] hover:text-[var(--status-error)] hover:bg-[var(--status-error)]/10 transition-colors disabled:opacity-50"
                          >
                            <HugeiconsIcon icon={Cancel01Icon} size={12} />
                            Cancel
                          </button>
                        )}
                        <button
                          type="button"
                          title="Delete"
                          disabled={busyId === run.id}
                          onClick={() => void action(run.id, deleteRun)}
                          className={DESTRUCTIVE_BUTTON_CLASS}
                        >
                          <HugeiconsIcon icon={TrashIcon} size={12} />
                          Delete
                        </button>
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}
    </div>
  );
}
