import React, { useCallback, useEffect, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  AlertCircleIcon,
  Archive02Icon,
  ArrowLeft01Icon,
  BotIcon,
  PencilEdit01Icon,
} from "@hugeicons/core-free-icons";
import { cn } from "@/lib/utils";
import { formatApiError, isNoRuntime } from "@/lib/api-client";
import { NoRuntimeState } from "@/components/console-ui/NoRuntimeState";
import {
  type AgentRecord,
  type AgentToolset,
  type SubagentRecord,
  archiveAgent,
  formatDate,
  getAgent,
  getAgentToolset,
  listSubagents,
  toolNames,
} from "@/lib/managed-agents";
import { MonoChip, Badge, SkeletonCard } from "@/components/console-ui";

function StatusBadge({ status }: { status: string }): React.ReactNode {
  const cls =
    status === "archived"
      ? "text-[var(--text-tertiary)]"
      : status === "prototype"
        ? "text-[var(--status-warning)]"
        : "text-[var(--status-success)]";
  return <Badge className={cls}>{status}</Badge>;
}

export function AgentDetailPage(): React.ReactNode {
  const { id } = useParams<{ id: string }>();
  const navigate = useNavigate();
  const [agent, setAgent] = useState<AgentRecord | null>(null);
  const [toolset, setToolset] = useState<AgentToolset | null>(null);
  const [subagents, setSubagents] = useState<SubagentRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [archiving, setArchiving] = useState(false);

  const load = useCallback(async () => {
    if (!id) return;
    setLoading(true);
    setError(null);
    try {
      const [loaded, ts, subs] = await Promise.all([
        getAgent(id),
        getAgentToolset(id).catch(() => null),
        listSubagents(id).catch(() => [] as SubagentRecord[]),
      ]);
      setAgent(loaded);
      setToolset(ts);
      setSubagents(subs);
    } catch (err) {
      setError(formatApiError(err, "Unable to load agent"));
    } finally {
      setLoading(false);
    }
  }, [id]);

  useEffect(() => {
    void load();
  }, [load]);

  const handleArchive = useCallback(async () => {
    if (!id || !agent) return;
    if (!window.confirm(`Archive agent "${agent.name}"? Archived agents stop accepting new runs.`)) {
      return;
    }
    setArchiving(true);
    setError(null);
    try {
      await archiveAgent(id);
      await load();
    } catch (err) {
      setError(formatApiError(err, "Archive failed"));
    } finally {
      setArchiving(false);
    }
  }, [id, agent, load]);

  if (loading) {
    return (
      <div className="space-y-4">
        <SkeletonCard rows={3} />
        <SkeletonCard rows={4} />
      </div>
    );
  }

  if (error && !agent) {
    return (
      <div className="space-y-4">
        <Link
          to="/agents"
          className="inline-flex items-center gap-1.5 text-[13px] text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
        >
          <HugeiconsIcon icon={ArrowLeft01Icon} size={14} />
          Back to agents
        </Link>
        {isNoRuntime(error) ? (
          <NoRuntimeState />
        ) : (
          <p className="flex items-center gap-2 rounded-lg border border-solid border-[var(--status-error)]/30 bg-[var(--status-error)]/10 px-3 py-2 text-[13px] text-[var(--status-error)]">
            <HugeiconsIcon icon={AlertCircleIcon} size={14} />
            {error}
          </p>
        )}
      </div>
    );
  }

  if (!agent) return null;

  const toolNamesList = toolNames(toolset?.tools ?? agent.tools);
  const permissionLabel = (value: string): string =>
    value === "always_allow" ? "always allow" : value === "always_ask" ? "always ask" : "auto";
  const allowedSkills = Array.isArray(toolset?.allowed_skills)
    ? toolset.allowed_skills.filter((s): s is string => typeof s === "string")
    : [];

  return (
    <div className="space-y-6">
      <Link
        to="/agents"
        className="inline-flex items-center gap-1.5 text-[13px] text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
      >
        <HugeiconsIcon icon={ArrowLeft01Icon} size={14} />
        Back to agents
      </Link>

      {error && (isNoRuntime(error) ? (
        <NoRuntimeState />
      ) : (
        <p className="flex items-center gap-2 rounded-lg border border-solid border-[var(--status-error)]/30 bg-[var(--status-error)]/10 px-3 py-2 text-[13px] text-[var(--status-error)]">
          <HugeiconsIcon icon={AlertCircleIcon} size={14} />
          {error}
        </p>
      ))}

      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0 space-y-2">
          <div className="flex flex-wrap items-center gap-2">
            <h1 className="m-0 text-[20px] font-semibold tracking-tight text-[var(--text-primary)]">
              {agent.name}
            </h1>
            <StatusBadge status={agent.status} />
            {agent.is_bot ? (
              <Badge className="text-[var(--accent-primary)]">Hub bot</Badge>
            ) : (
              <Badge>Cloud</Badge>
            )}
            {agent.mode && agent.mode !== "primary" ? <Badge>{agent.mode}</Badge> : null}
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <MonoChip>{agent.id}</MonoChip>
            <span className="font-mono text-[12px] text-[var(--text-secondary)]">{agent.model}</span>
          </div>
          {agent.description && (
            <p className="m-0 text-[13px] text-[var(--text-secondary)]">{agent.description}</p>
          )}
        </div>
        <div className="flex shrink-0 gap-2">
          <Link
            to={`/agents/${agent.id}/edit`}
            className="inline-flex items-center gap-1.5 rounded-lg border border-solid border-[var(--border-subtle)] px-3.5 py-2 text-[13px] font-medium text-[var(--text-primary)] transition-colors hover:bg-[var(--bg-secondary)]"
          >
            <HugeiconsIcon icon={PencilEdit01Icon} size={13} />
            Edit
          </Link>
          <button
            type="button"
            onClick={() => void handleArchive()}
            disabled={archiving || agent.status === "archived"}
            className="inline-flex items-center gap-1.5 rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] px-3.5 py-2 text-[13px] font-medium text-[var(--text-secondary)] transition-colors hover:text-[var(--status-error)] hover:border-[var(--status-error)]/30 disabled:opacity-50 disabled:cursor-not-allowed"
          >
            <HugeiconsIcon icon={Archive02Icon} size={13} />
            {archiving ? "Archiving…" : "Archive"}
          </button>
        </div>
      </div>

      <div className="grid gap-4 lg:grid-cols-2">
        <section className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4">
          <h2 className="m-0 mb-3 text-[14px] font-semibold text-[var(--text-primary)]">General</h2>
          <dl className="m-0 space-y-2 text-[13px]">
            <div className="flex justify-between gap-4">
              <dt className="text-[var(--text-tertiary)]">Provider</dt>
              <dd className="m-0 font-mono text-[12px] text-[var(--text-primary)]">{agent.provider}</dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt className="text-[var(--text-tertiary)]">Temperature</dt>
              <dd className="m-0 text-[var(--text-primary)]">{agent.temperature}</dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt className="text-[var(--text-tertiary)]">Max iterations</dt>
              <dd className="m-0 text-[var(--text-primary)]">{agent.max_iterations}</dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt className="text-[var(--text-tertiary)]">Version</dt>
              <dd className="m-0 text-[var(--text-primary)]">{agent.version ?? 1}</dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt className="text-[var(--text-tertiary)]">Created</dt>
              <dd className="m-0 text-[var(--text-primary)]">{formatDate(agent.created_at)}</dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt className="text-[var(--text-tertiary)]">Last updated</dt>
              <dd className="m-0 text-[var(--text-primary)]">{formatDate(agent.updated_at)}</dd>
            </div>
            {agent.last_run_at && (
              <div className="flex justify-between gap-4">
                <dt className="text-[var(--text-tertiary)]">Last run</dt>
                <dd className="m-0 text-[var(--text-primary)]">{formatDate(agent.last_run_at)}</dd>
              </div>
            )}
          </dl>
          {agent.system_prompt && (
            <div className="mt-4">
              <h3 className="m-0 mb-1.5 text-[12px] font-semibold uppercase tracking-wide text-[var(--text-tertiary)]">
                System prompt
              </h3>
              <pre className="m-0 whitespace-pre-wrap rounded-lg bg-[var(--bg-primary)] p-3 font-mono text-[12px] leading-relaxed text-[var(--text-secondary)]">
                {agent.system_prompt}
              </pre>
            </div>
          )}
        </section>

        <section className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4">
          <h2 className="m-0 mb-3 text-[14px] font-semibold text-[var(--text-primary)]">Tools</h2>
          {toolNamesList.length === 0 ? (
            <p className="m-0 flex items-center gap-2 text-[13px] text-[var(--text-tertiary)]">
              <HugeiconsIcon icon={BotIcon} size={14} />
              No tools on this agent's toolset.
            </p>
          ) : (
            <div className="space-y-1.5">
              {toolNamesList.map((tool) => {
                const perm = toolset?.tool_permissions?.[tool] ?? "auto";
                return (
                  <div key={tool} className="flex items-center gap-3">
                    <code className="flex-1 font-mono text-[12px] text-[var(--text-primary)]">{tool}</code>
                    <Badge
                      className={cn(
                        perm === "always_allow" && "text-[var(--status-success)]",
                        perm === "always_ask" && "text-[var(--status-warning)]"
                      )}
                    >
                      {permissionLabel(perm)}
                    </Badge>
                  </div>
                );
              })}
            </div>
          )}
          {(toolset?.mcp_connector_ids?.length ?? 0) > 0 && (
            <p className="m-0 mt-3 text-[12px] text-[var(--text-tertiary)]">
              MCP connectors bound: {toolset?.mcp_connector_ids.join(", ")}
            </p>
          )}
          {allowedSkills.length > 0 && (
            <div className="mt-4">
              <h3 className="m-0 mb-1.5 text-[12px] font-semibold uppercase tracking-wide text-[var(--text-tertiary)]">
                Allowed skills
              </h3>
              <div className="flex flex-wrap gap-1.5">
                {allowedSkills.map((skill) => (
                  <Badge key={skill}>{skill}</Badge>
                ))}
              </div>
            </div>
          )}
        </section>
      </div>

      <section className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4">
        <h2 className="m-0 mb-3 text-[14px] font-semibold text-[var(--text-primary)]">
          Subagents
          {subagents.length > 0 && <span className="ml-1.5 text-[var(--text-tertiary)]">({subagents.length})</span>}
        </h2>
        {subagents.length === 0 ? (
          <p className="m-0 text-[13px] text-[var(--text-tertiary)]">
            No subagents. Subagents are attached from the agent's edit page.
          </p>
        ) : (
          <div className="divide-y divide-solid divide-[var(--border-subtle)] rounded-lg border border-solid border-[var(--border-subtle)]">
            {subagents.map((sub) => (
              <div key={sub.id} className="flex items-center gap-3 px-3 py-2">
                <span className="flex-1 text-[13px] font-medium text-[var(--text-primary)]">{sub.name}</span>
                <code className="font-mono text-[11px] text-[var(--text-tertiary)]">{sub.model}</code>
                <StatusBadge status={sub.status} />
              </div>
            ))}
          </div>
        )}
      </section>
    </div>
  );
}

export default AgentDetailPage;
