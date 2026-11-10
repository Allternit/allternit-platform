import React, { useCallback, useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { HugeiconsIcon } from "@hugeicons/react";
import { AlertCircleIcon, PlusSignIcon } from "@hugeicons/core-free-icons";
import { cn } from "@/lib/utils";
import { formatApiError } from "@/lib/api-client";
import {
  SESSION_COMPUTER_KINDS,
  type AgentRecord,
  type SessionComputerKind,
  createSession,
  listAgents,
} from "@/lib/managed-agents";
import { FormPage } from "@/components/console-ui";

const INPUT_CLASS =
  "w-full rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] px-3 py-2 text-[13px] text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[var(--border-default)]";

/**
 * Create a cloud session against POST /api/v1/sessions. The backend accepts
 * an agent reference ({id} or inline {model, instructions, …}), a computer
 * spec, a first input message, and an optional telemetry budget — that is
 * exactly what this form sends (cloud_agents_routes.rs CreateCloudSessionBody).
 */
export function SessionNewPage(): React.ReactNode {
  const navigate = useNavigate();
  const [agents, setAgents] = useState<AgentRecord[]>([]);
  const [agentId, setAgentId] = useState("");
  const [model, setModel] = useState("");
  const [instructions, setInstructions] = useState("");
  const [input, setInput] = useState("");
  const [computerKind, setComputerKind] = useState<SessionComputerKind>("none");
  const [maxCostUsd, setMaxCostUsd] = useState("");
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [unavailable, setUnavailable] = useState(false);

  useEffect(() => {
    let active = true;
    listAgents()
      .then((list) => {
        if (active) setAgents(list.filter((a) => a.status !== "archived"));
      })
      .catch(() => undefined);
    return () => {
      active = false;
    };
  }, []);

  const usingInlineAgent = agentId === "__inline__";

  const handleSubmit = useCallback(
    async (event: React.FormEvent<HTMLFormElement>) => {
      event.preventDefault();
      setCreating(true);
      setError(null);
      setUnavailable(false);
      try {
        const body: Record<string, unknown> = {
          computer: { kind: computerKind },
        };
        if (usingInlineAgent) {
          if (!model.trim()) throw new Error("Model is required for an inline agent.");
          body.agent = {
            model: model.trim(),
            instructions: instructions.trim() || undefined,
            name: "Console ad-hoc agent",
          };
        } else if (agentId) {
          body.agent = { id: agentId };
        }
        if (input.trim()) body.input = input.trim();
        if (maxCostUsd.trim()) {
          const cost = Number(maxCostUsd);
          if (Number.isFinite(cost) && cost > 0) body.budget = { max_cost_usd: cost };
        }
        const session = await createSession(body);
        navigate(`/sessions/${session.id}`);
      } catch (err) {
        const e = err as Error & { unavailable?: boolean };
        setUnavailable(Boolean(e.unavailable));
        setError(e.message);
      } finally {
        setCreating(false);
      }
    },
    [agentId, usingInlineAgent, model, instructions, input, computerKind, maxCostUsd, navigate]
  );

  return (
    <div className="space-y-6">
      {error && (
        <p
          className={cn(
            "flex items-center gap-2 rounded-lg border border-solid px-3 py-2 text-[13px]",
            unavailable
              ? "border-[var(--status-warning)]/30 bg-[var(--status-warning)]/10 text-[var(--status-warning)]"
              : "border-[var(--status-error)]/30 bg-[var(--status-error)]/10 text-[var(--status-error)]"
          )}
        >
          <HugeiconsIcon icon={AlertCircleIcon} size={14} />
          <span>
            {unavailable ? "Computer unavailable — " : ""}
            {error}
            {unavailable
              ? " Incus/Tart runs on the Computer Cloud VPS; this gateway host has no VM driver. Use kind none or local."
              : ""}
          </span>
        </p>
      )}
      <FormPage
        title="New session"
        breadcrumb={["Managed Agents", "Sessions", "New"]}
        cancelTo="/sessions"
        primaryLabel={creating ? "Creating…" : "Create session"}
        onSubmit={(e) => void handleSubmit(e)}
        sections={[
          {
            id: "agent",
            title: "Agent",
            description: "Run a saved managed agent, or define an inline agent just for this session.",
            children: (
              <div className="space-y-4">
                <div className="space-y-1">
                  <label htmlFor="session-agent" className="text-[12px] font-semibold text-[var(--text-secondary)]">
                    Agent
                  </label>
                  <select
                    id="session-agent"
                    value={agentId}
                    onChange={(e) => setAgentId(e.target.value)}
                    className={INPUT_CLASS}
                  >
                    <option value="">No agent (idle session, you drive it with follow-up messages)</option>
                    <option value="__inline__">Inline agent (ad-hoc model + instructions)</option>
                    {agents.map((agent) => (
                      <option key={agent.id} value={agent.id}>
                        {agent.name} ({agent.model})
                      </option>
                    ))}
                  </select>
                </div>
                {usingInlineAgent && (
                  <>
                    <div className="space-y-1">
                      <label htmlFor="session-model" className="text-[12px] font-semibold text-[var(--text-secondary)]">
                        Model
                      </label>
                      <input
                        id="session-model"
                        type="text"
                        value={model}
                        onChange={(e) => setModel(e.target.value)}
                        placeholder="model id, e.g. kimi-k3"
                        className={INPUT_CLASS}
                      />
                    </div>
                    <div className="space-y-1">
                      <label htmlFor="session-instructions" className="text-[12px] font-semibold text-[var(--text-secondary)]">
                        Instructions
                      </label>
                      <textarea
                        id="session-instructions"
                        value={instructions}
                        onChange={(e) => setInstructions(e.target.value)}
                        rows={3}
                        placeholder="Be terse. Report real output."
                        className={cn(INPUT_CLASS, "resize-y font-mono text-[12px]")}
                      />
                    </div>
                  </>
                )}
              </div>
            ),
          },
          {
            id: "input",
            title: "Input",
            description: "Optional first user message — enqueues a run immediately, like the API's input field.",
            children: (
              <textarea
                id="session-input"
                value={input}
                onChange={(e) => setInput(e.target.value)}
                rows={4}
                placeholder="Summarize my last 10 emails"
                className={cn(INPUT_CLASS, "resize-y font-mono text-[12px]")}
              />
            ),
          },
          {
            id: "environment",
            title: "Environment",
            description: "Computer kind for tool execution. VM-backed kinds need the Computer Cloud driver; none and local always work.",
            children: (
              <div className="grid gap-3 sm:grid-cols-2">
                <div className="space-y-1">
                  <label htmlFor="session-computer" className="text-[12px] font-semibold text-[var(--text-secondary)]">
                    Computer
                  </label>
                  <select
                    id="session-computer"
                    value={computerKind}
                    onChange={(e) => setComputerKind(e.target.value as SessionComputerKind)}
                    className={INPUT_CLASS}
                  >
                    {SESSION_COMPUTER_KINDS.map((kind) => (
                      <option key={kind} value={kind}>
                        {kind}
                      </option>
                    ))}
                  </select>
                </div>
                <div className="space-y-1">
                  <label htmlFor="session-max-cost" className="text-[12px] font-semibold text-[var(--text-secondary)]">
                    Max cost USD (telemetry)
                  </label>
                  <input
                    id="session-max-cost"
                    type="number"
                    min={0}
                    step={0.01}
                    value={maxCostUsd}
                    onChange={(e) => setMaxCostUsd(e.target.value)}
                    placeholder="optional"
                    className={INPUT_CLASS}
                  />
                </div>
              </div>
            ),
          },
        ]}
      />
      <p className="m-0 -mt-4 flex items-center gap-1.5 text-[12px] text-[var(--text-tertiary)]">
        <HugeiconsIcon icon={PlusSignIcon} size={12} />
        Budget and cost figures are telemetry only — Allternit does not charge sessions today.
      </p>
    </div>
  );
}

export default SessionNewPage;
