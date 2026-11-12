import React, { useCallback, useEffect, useMemo, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  AlertCircleIcon,
  PlusSignIcon,
  TrashIcon,
  SourceCodeIcon,
  LayoutIcon,
} from "@hugeicons/core-free-icons";
import { cn } from "@/lib/utils";
import { formatApiError } from "@/lib/api-client";
import {
  TOOL_PERMISSIONS,
  type AgentRecord,
  type AgentToolset,
  type CloudSkillRef,
  type SubagentRecord,
  type ToolPermission,
  createAgent,
  createSubagent,
  getAgent,
  getAgentToolset,
  listSkillRefs,
  listSubagents,
  patchAgent,
  putAgentToolset,
  toolNames,
} from "@/lib/managed-agents";
import { fetchConsoleModels, type ConsoleModel } from "@/lib/console-models";
import { FormPage } from "@/components/console-ui";

// Cloud default (the `default: true` row of GET /v1/models), sent when the form leaves the model blank.
const DEFAULT_MODEL_ID = "kimi-k3";

const INPUT_CLASS =
  "w-full rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] px-3 py-2 text-[13px] text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[var(--border-default)]";

function deriveProvider(model: string): string {
  const prefix = model.split("/")[0];
  return prefix && prefix !== model ? prefix : "allternit";
}

/**
 * Create (`/agents/new`) and edit (`/agents/:id/edit`) a managed agent.
 * General fields map to CreateAgentBody/UpdateAgentBody; tools, per-tool
 * permissions, MCP bindings, and allowed skills map to the toolset surface
 * (PUT /agents/:id/toolset) — the only backend route that accepts them.
 */
export function AgentFormPage(): React.ReactNode {
  const { id } = useParams<{ id: string }>();
  const isEdit = Boolean(id);
  const navigate = useNavigate();

  const [loading, setLoading] = useState(isEdit);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [agent, setAgent] = useState<AgentRecord | null>(null);

  // General section.
  const [view, setView] = useState<"form" | "code">("form");
  const [name, setName] = useState("");
  const [model, setModel] = useState("");
  const [description, setDescription] = useState("");
  const [systemPrompt, setSystemPrompt] = useState("");
  const [temperature, setTemperature] = useState("0.7");
  const [maxIterations, setMaxIterations] = useState("10");
  const [rawJson, setRawJson] = useState("");
  const [models, setModels] = useState<ConsoleModel[]>([]);
  const [modelsError, setModelsError] = useState<string | null>(null);

  // Tools section.
  const [tools, setTools] = useState<unknown[]>([]);
  const [toolPermissions, setToolPermissions] = useState<Record<string, ToolPermission>>({});
  const [mcpConnectors, setMcpConnectors] = useState<AgentToolset["mcp"]>([]);
  const [boundMcpIds, setBoundMcpIds] = useState<string[]>([]);
  const [newToolName, setNewToolName] = useState("");

  // Skills section.
  const [skills, setSkills] = useState<CloudSkillRef[]>([]);
  const [allowedSkills, setAllowedSkills] = useState<string[]>([]);

  // Manager section (edit only).
  const [subagents, setSubagents] = useState<SubagentRecord[]>([]);
  const [subName, setSubName] = useState("");
  const [subModel, setSubModel] = useState("");
  const [subPrompt, setSubPrompt] = useState("");
  const [addingSubagent, setAddingSubagent] = useState(false);

  useEffect(() => {
    let active = true;
    fetchConsoleModels()
      .then((list) => {
        if (active) setModels(list);
      })
      .catch((err) => {
        if (active) setModelsError(formatApiError(err, "model catalog"));
      });
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    if (!id) return;
    let active = true;
    void (async () => {
      try {
        const [loaded, toolset, skillList] = await Promise.all([
          getAgent(id),
          getAgentToolset(id),
          listSkillRefs(),
        ]);
        if (!active) return;
        setAgent(loaded);
        setName(loaded.name);
        setModel(loaded.model);
        setDescription(loaded.description ?? "");
        setSystemPrompt(loaded.system_prompt ?? "");
        setTemperature(String(loaded.temperature ?? 0.7));
        setMaxIterations(String(loaded.max_iterations ?? 10));
        const loadedTools = Array.isArray(loaded.tools) ? loaded.tools : toolNames(toolset.tools).map((n) => n);
        setTools(loadedTools);
        const perms: Record<string, ToolPermission> = {};
        for (const [tool, value] of Object.entries(toolset.tool_permissions ?? {})) {
          if ((TOOL_PERMISSIONS as readonly string[]).includes(value)) {
            perms[tool] = value as ToolPermission;
          }
        }
        setToolPermissions(perms);
        setMcpConnectors(toolset.mcp ?? []);
        setBoundMcpIds(toolset.mcp_connector_ids ?? []);
        setAllowedSkills(
          Array.isArray(toolset.allowed_skills)
            ? toolset.allowed_skills.filter((s): s is string => typeof s === "string")
            : []
        );
        setSkills(skillList);
        setSubagents(await listSubagents(id));
      } catch (err) {
        if (active) setError(formatApiError(err, "Unable to load agent"));
      } finally {
        if (active) setLoading(false);
      }
    })();
    return () => {
      active = false;
    };
  }, [id]);

  useEffect(() => {
    if (isEdit) return;
    let active = true;
    listSkillRefs()
      .then((list) => {
        if (active) setSkills(list);
      })
      .catch(() => undefined);
    return () => {
      active = false;
    };
  }, [isEdit]);

  // The raw JSON editor mirrors the whole create/patch payload.
  useEffect(() => {
    if (view !== "code") return;
    if (isEdit && agent) {
      setRawJson(
        JSON.stringify(
          {
            name,
            description: description || undefined,
            model,
            system_prompt: systemPrompt || undefined,
            temperature: Number(temperature) || 0.7,
            max_iterations: Number(maxIterations) || 10,
            tools,
            allowed_skills: allowedSkills,
            tool_permissions: toolPermissions,
            mcp_connector_ids: boundMcpIds,
          },
          null,
          2
        )
      );
    } else if (!isEdit) {
      setRawJson(
        JSON.stringify(
          {
            name: name || "my-agent",
            model: model || DEFAULT_MODEL_ID,
            description: description || undefined,
            system_prompt: systemPrompt || undefined,
            temperature: Number(temperature) || 0.7,
            max_iterations: Number(maxIterations) || 10,
            tools,
            allowed_skills: allowedSkills,
          },
          null,
          2
        )
      );
    }
  }, [view, isEdit, agent, name, model, description, systemPrompt, temperature, maxIterations, tools, allowedSkills, toolPermissions, boundMcpIds]);

  const addTool = useCallback(() => {
    const trimmed = newToolName.trim();
    if (!trimmed) return;
    setTools((prev) => (prev.some((t) => t === trimmed || (typeof t === "object" && t !== null && (t as { name?: string }).name === trimmed)) ? prev : [...prev, trimmed]));
    setNewToolName("");
  }, [newToolName]);

  const removeTool = useCallback((tool: string) => {
    setTools((prev) => prev.filter((t) => t !== tool && (typeof t !== "object" || t === null || (t as { name?: string }).name !== tool)));
    setToolPermissions((prev) => {
      const next = { ...prev };
      delete next[tool];
      return next;
    });
  }, []);

  const names = useMemo(() => toolNames(tools), [tools]);

  const handleAddSubagent = useCallback(async () => {
    if (!id || !subName.trim()) return;
    setAddingSubagent(true);
    setError(null);
    try {
      await createSubagent(id, {
        name: subName.trim(),
        model: subModel.trim() || model || DEFAULT_MODEL_ID,
        provider: deriveProvider(subModel.trim() || model),
        description: undefined,
        system_prompt: subPrompt.trim() || undefined,
      });
      setSubName("");
      setSubModel("");
      setSubPrompt("");
      setSubagents(await listSubagents(id));
    } catch (err) {
      setError(formatApiError(err, "Unable to add subagent"));
    } finally {
      setAddingSubagent(false);
    }
  }, [id, subName, subModel, subPrompt, model]);

  const handleSubmit = useCallback(
    async (event: React.FormEvent<HTMLFormElement>) => {
      event.preventDefault();
      setSaving(true);
      setError(null);
      try {
        let agentId = id;
        if (view === "code") {
          // Raw mode: the textarea IS the payload. tool_permissions and
          // mcp_connector_ids are not agent-body fields — they ride the
          // toolset PUT below.
          let payload: Record<string, unknown>;
          try {
            payload = JSON.parse(rawJson) as Record<string, unknown>;
          } catch (err) {
            throw new SyntaxError(`Config JSON is invalid: ${(err as Error).message}`);
          }
          const { tool_permissions, mcp_connector_ids, ...agentBody } = payload;
          if (isEdit && agentId) {
            await patchAgent(agentId, agentBody);
          } else {
            const created = await createAgent(agentBody as { name: string; model: string });
            agentId = created.id;
          }
          if (agentId && (tool_permissions !== undefined || mcp_connector_ids !== undefined)) {
            await putAgentToolset(agentId, {
              ...(tool_permissions !== undefined ? { tool_permissions: tool_permissions as Record<string, string> } : {}),
              ...(mcp_connector_ids !== undefined ? { mcp_connector_ids: mcp_connector_ids as string[] } : {}),
            });
          }
        } else {
          if (!name.trim()) throw new Error("Name is required.");
          if (!model.trim()) throw new Error("Model is required.");
          const agentBody: Record<string, unknown> = {
            name: name.trim(),
            model: model.trim(),
            provider: deriveProvider(model.trim()),
            description: description.trim() || undefined,
            system_prompt: systemPrompt.trim() || undefined,
            temperature: Number(temperature) || 0.7,
            max_iterations: Number(maxIterations) || 10,
            tools,
            allowed_skills: allowedSkills,
          };
          if (isEdit && agentId) {
            await patchAgent(agentId, agentBody);
          } else {
            const created = await createAgent(agentBody as { name: string; model: string });
            agentId = created.id;
          }
          // Toolset surface (edit or the post-create binding for new agents):
          // per-tool permissions and MCP bindings only exist on PUT toolset.
          if (agentId) {
            await putAgentToolset(agentId, {
              tools,
              allowed_skills: allowedSkills,
              mcp_connector_ids: boundMcpIds,
              tool_permissions: toolPermissions,
            });
          }
        }
        navigate(agentId ? `/agents/${agentId}` : "/agents");
      } catch (err) {
        setError(
          err instanceof SyntaxError
            ? err.message
            : formatApiError(err, isEdit ? "Unable to save agent" : "Unable to create agent")
        );
      } finally {
        setSaving(false);
      }
    },
    [
      view,
      rawJson,
      isEdit,
      id,
      name,
      model,
      description,
      systemPrompt,
      temperature,
      maxIterations,
      tools,
      allowedSkills,
      boundMcpIds,
      toolPermissions,
      navigate,
    ]
  );

  if (loading) {
    return (
      <div className="space-y-3 py-6">
        <div className="h-5 w-48 animate-pulse rounded-full bg-[var(--bg-secondary)]" />
        <div className="h-3.5 w-96 animate-pulse rounded-full bg-[var(--bg-secondary)]" />
        <div className="h-3.5 w-72 animate-pulse rounded-full bg-[var(--bg-secondary)]" />
      </div>
    );
  }

  const sections = [
    {
      id: "general",
      title: "General",
      description: "Identity, model, and instructions. Raw mode exposes the whole config as JSON.",
      children: (
        <div className="space-y-4">
          <div className="flex items-center gap-2">
            <div className="inline-flex rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-0.5">
              <button
                type="button"
                onClick={() => setView("form")}
                className={cn(
                  "inline-flex items-center gap-1 rounded-lg px-3 py-1.5 text-[12px] font-semibold transition-colors",
                  view === "form"
                    ? "bg-[var(--accent-primary)] text-[var(--ui-text-inverse)]"
                    : "text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
                )}
              >
                <HugeiconsIcon icon={LayoutIcon} size={13} />
                Rendered
              </button>
              <button
                type="button"
                onClick={() => setView("code")}
                className={cn(
                  "inline-flex items-center gap-1 rounded-lg px-3 py-1.5 text-[12px] font-semibold transition-colors",
                  view === "code"
                    ? "bg-[var(--accent-primary)] text-[var(--ui-text-inverse)]"
                    : "text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
                )}
              >
                <HugeiconsIcon icon={SourceCodeIcon} size={13} />
                Raw
              </button>
            </div>
          </div>

          {view === "code" ? (
            <div className="space-y-2">
              <textarea
                value={rawJson}
                onChange={(e) => setRawJson(e.target.value)}
                rows={18}
                spellCheck={false}
                className={cn(INPUT_CLASS, "resize-y font-mono text-[12px] leading-relaxed")}
              />
              <p className="m-0 text-[12px] text-[var(--text-tertiary)]">
                Raw JSON is the exact request body. <code className="font-mono">tool_permissions</code> and{" "}
                <code className="font-mono">mcp_connector_ids</code> are forwarded to the toolset endpoint
                after the agent save — they are not agent-body fields.
              </p>
            </div>
          ) : (
            <>
              <div className="grid gap-3 sm:grid-cols-2">
                <div className="space-y-1">
                  <label htmlFor="agent-name" className="text-[12px] font-semibold text-[var(--text-secondary)]">
                    Name
                  </label>
                  <input
                    id="agent-name"
                    type="text"
                    value={name}
                    onChange={(e) => setName(e.target.value)}
                    placeholder="Competitor scan"
                    className={INPUT_CLASS}
                  />
                </div>
                <div className="space-y-1">
                  <label htmlFor="agent-model" className="text-[12px] font-semibold text-[var(--text-secondary)]">
                    Model
                  </label>
                  {models.length > 0 ? (
                    <select
                      id="agent-model"
                      value={model}
                      onChange={(e) => setModel(e.target.value)}
                      className={INPUT_CLASS}
                    >
                      {!models.some((m) => m.id === model) && model && (
                        <option value={model}>{model} (current)</option>
                      )}
                      {models.map((m) => (
                        <option key={m.id} value={m.id}>
                          {m.display_name ?? m.id} ({m.id})
                        </option>
                      ))}
                    </select>
                  ) : (
                    <input
                      id="agent-model"
                      type="text"
                      value={model}
                      onChange={(e) => setModel(e.target.value)}
                      placeholder="model id, e.g. kimi-k3"
                      className={INPUT_CLASS}
                    />
                  )}
                  {modelsError && (
                    <p className="m-0 text-[12px] text-[var(--status-warning)]">
                      Live model catalog unavailable — enter a model id manually.
                    </p>
                  )}
                </div>
              </div>
              <div className="space-y-1">
                <label htmlFor="agent-description" className="text-[12px] font-semibold text-[var(--text-secondary)]">
                  Description
                </label>
                <input
                  id="agent-description"
                  type="text"
                  value={description}
                  onChange={(e) => setDescription(e.target.value)}
                  placeholder="What this agent does and when to use it"
                  className={INPUT_CLASS}
                />
              </div>
              <div className="space-y-1">
                <label htmlFor="agent-prompt" className="text-[12px] font-semibold text-[var(--text-secondary)]">
                  System prompt
                </label>
                <textarea
                  id="agent-prompt"
                  value={systemPrompt}
                  onChange={(e) => setSystemPrompt(e.target.value)}
                  rows={6}
                  className={cn(INPUT_CLASS, "resize-y font-mono text-[12px]")}
                />
              </div>
              <div className="grid gap-3 sm:grid-cols-2">
                <div className="space-y-1">
                  <label htmlFor="agent-temperature" className="text-[12px] font-semibold text-[var(--text-secondary)]">
                    Temperature
                  </label>
                  <input
                    id="agent-temperature"
                    type="number"
                    min={0}
                    max={2}
                    step={0.1}
                    value={temperature}
                    onChange={(e) => setTemperature(e.target.value)}
                    className={INPUT_CLASS}
                  />
                </div>
                <div className="space-y-1">
                  <label htmlFor="agent-max-iterations" className="text-[12px] font-semibold text-[var(--text-secondary)]">
                    Max iterations
                  </label>
                  <input
                    id="agent-max-iterations"
                    type="number"
                    min={1}
                    value={maxIterations}
                    onChange={(e) => setMaxIterations(e.target.value)}
                    className={INPUT_CLASS}
                  />
                </div>
              </div>
            </>
          )}
        </div>
      ),
    },
    {
      id: "tools",
      title: "Tools",
      description:
        "Built-in toolset with per-tool permissions (auto, always allow, always ask). MCP servers bind from your existing connectors.",
      children: (
        <div className="space-y-4">
          {names.length === 0 ? (
            <p className="m-0 rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] px-3 py-2 text-[12px] text-[var(--text-tertiary)]">
              No tools yet. Add a custom tool below, or attach tools via Raw mode for structured
              definitions.
            </p>
          ) : (
            <div className="divide-y divide-solid divide-[var(--border-subtle)] rounded-xl border border-solid border-[var(--border-subtle)]">
              {names.map((tool) => (
                <div key={tool} className="flex items-center gap-3 px-3 py-2">
                  <code className="flex-1 font-mono text-[12px] text-[var(--text-primary)]">{tool}</code>
                  <select
                    aria-label={`Permission for ${tool}`}
                    value={toolPermissions[tool] ?? "auto"}
                    onChange={(e) =>
                      setToolPermissions((prev) => ({ ...prev, [tool]: e.target.value as ToolPermission }))
                    }
                    className="rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] px-2 py-1 text-[12px] text-[var(--text-primary)] outline-none focus:border-[var(--accent-primary)]"
                  >
                    {TOOL_PERMISSIONS.map((p) => (
                      <option key={p} value={p}>
                        {p === "always_allow" ? "always allow" : p === "always_ask" ? "always ask" : "auto"}
                      </option>
                    ))}
                  </select>
                  <button
                    type="button"
                    onClick={() => removeTool(tool)}
                    className="text-[var(--text-tertiary)] transition-colors hover:text-[var(--status-error)]"
                    aria-label={`Remove ${tool}`}
                  >
                    <HugeiconsIcon icon={TrashIcon} size={13} />
                  </button>
                </div>
              ))}
            </div>
          )}

          <div className="flex flex-wrap items-center gap-2">
            <input
              type="text"
              value={newToolName}
              onChange={(e) => setNewToolName(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  addTool();
                }
              }}
              placeholder="Custom tool name"
              className={cn(INPUT_CLASS, "w-64")}
            />
            <button
              type="button"
              onClick={addTool}
              disabled={!newToolName.trim()}
              className="inline-flex items-center gap-1.5 rounded-lg border border-solid border-[var(--border-subtle)] bg-transparent px-3 py-2 text-[13px] font-medium text-[var(--text-primary)] transition-colors hover:bg-[var(--bg-secondary)] disabled:opacity-50"
            >
              <HugeiconsIcon icon={PlusSignIcon} size={13} />
              Add custom tool
            </button>
          </div>
          <p className="m-0 text-[12px] text-[var(--text-tertiary)]">
            Custom tools are stored as named entries on the agent's toolset. Tool definitions with
            parameters or schemas can be pasted into the <code className="font-mono">tools</code> array in
            Raw mode — the backend stores the full JSON.
          </p>

          {mcpConnectors.length > 0 && (
            <div className="space-y-2">
              <h3 className="m-0 text-[13px] font-semibold text-[var(--text-primary)]">MCP servers</h3>
              <p className="m-0 text-[12px] text-[var(--text-tertiary)]">
                Bound from your existing MCP connectors. Creating new connectors is not part of this
                surface yet.
              </p>
              <div className="space-y-1.5">
                {mcpConnectors.map((connector) => (
                  <label
                    key={connector.id}
                    className="flex items-center gap-2.5 rounded-lg border border-solid border-[var(--border-subtle)] px-3 py-2"
                  >
                    <input
                      type="checkbox"
                      checked={boundMcpIds.includes(connector.id)}
                      onChange={(e) =>
                        setBoundMcpIds((prev) =>
                          e.target.checked ? [...prev, connector.id] : prev.filter((c) => c !== connector.id)
                        )
                      }
                      className="accent-[var(--accent-primary)]"
                    />
                    <span className="text-[13px] font-medium text-[var(--text-primary)]">{connector.name}</span>
                    {connector.url && (
                      <span className="truncate font-mono text-[11px] text-[var(--text-tertiary)]">{connector.url}</span>
                    )}
                    {!connector.enabled && <span className="text-[11px] text-[var(--status-warning)]">disabled</span>}
                  </label>
                ))}
              </div>
            </div>
          )}
          {mcpConnectors.length === 0 && isEdit && (
            <p className="m-0 text-[12px] text-[var(--text-tertiary)]">
              No MCP connectors on this account yet — MCP servers you register will be listed here.
            </p>
          )}
        </div>
      ),
    },
    {
      id: "skills",
      title: "Skills",
      description: "Attach cloud skills this agent is allowed to run.",
      children:
        skills.length === 0 ? (
          <p className="m-0 rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] px-3 py-2 text-[12px] text-[var(--text-tertiary)]">
            No cloud skills yet. Skills are created on the Skills page and attached here by id.
          </p>
        ) : (
          <div className="space-y-1.5">
            {skills.map((skill) => (
              <label
                key={skill.id}
                className="flex items-center gap-2.5 rounded-lg border border-solid border-[var(--border-subtle)] px-3 py-2"
              >
                <input
                  type="checkbox"
                  checked={allowedSkills.includes(skill.id)}
                  onChange={(e) =>
                    setAllowedSkills((prev) =>
                      e.target.checked ? [...prev, skill.id] : prev.filter((s) => s !== skill.id)
                    )
                  }
                  className="accent-[var(--accent-primary)]"
                />
                <span className="text-[13px] font-medium text-[var(--text-primary)]">{skill.name}</span>
                {skill.description && (
                  <span className="truncate text-[12px] text-[var(--text-tertiary)]">{skill.description}</span>
                )}
              </label>
            ))}
          </div>
        ),
    },
    {
      id: "manager",
      title: "Manager",
      description: "Subagents this agent can delegate to.",
      children: !isEdit ? (
        <p className="m-0 rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] px-3 py-2 text-[12px] text-[var(--text-tertiary)]">
          Subagents are added after the agent exists — create the agent first, then attach
          subagents here.
        </p>
      ) : (
        <div className="space-y-3">
          {subagents.length === 0 ? (
            <p className="m-0 text-[13px] text-[var(--text-secondary)]">No subagents yet.</p>
          ) : (
            <div className="divide-y divide-solid divide-[var(--border-subtle)] rounded-xl border border-solid border-[var(--border-subtle)]">
              {subagents.map((sub) => (
                <div key={sub.id} className="flex items-center gap-3 px-3 py-2">
                  <span className="flex-1 text-[13px] font-medium text-[var(--text-primary)]">{sub.name}</span>
                  <code className="font-mono text-[11px] text-[var(--text-tertiary)]">{sub.model}</code>
                  <span className="text-[11px] text-[var(--text-secondary)]">{sub.status}</span>
                </div>
              ))}
            </div>
          )}
          <div className="grid gap-3 sm:grid-cols-2">
            <input
              type="text"
              value={subName}
              onChange={(e) => setSubName(e.target.value)}
              placeholder="Subagent name"
              className={INPUT_CLASS}
            />
            <input
              type="text"
              value={subModel}
              onChange={(e) => setSubModel(e.target.value)}
              placeholder="Model (defaults to parent's)"
              className={INPUT_CLASS}
            />
          </div>
          <textarea
            value={subPrompt}
            onChange={(e) => setSubPrompt(e.target.value)}
            rows={3}
            placeholder="System prompt (optional)"
            className={cn(INPUT_CLASS, "resize-y font-mono text-[12px]")}
          />
          <button
            type="button"
            onClick={() => void handleAddSubagent()}
            disabled={addingSubagent || !subName.trim()}
            className="inline-flex items-center gap-1.5 rounded-lg border border-solid border-[var(--border-subtle)] bg-transparent px-3 py-2 text-[13px] font-medium text-[var(--text-primary)] transition-colors hover:bg-[var(--bg-secondary)] disabled:opacity-50"
          >
            <HugeiconsIcon icon={PlusSignIcon} size={13} />
            {addingSubagent ? "Adding…" : "Add subagent"}
          </button>
        </div>
      ),
    },
  ];

  return (
    <div className="space-y-6">
      {error && (
        <p className="flex items-center gap-2 rounded-lg border border-solid border-[var(--status-error)]/30 bg-[var(--status-error)]/10 px-3 py-2 text-[13px] text-[var(--status-error)]">
          <HugeiconsIcon icon={AlertCircleIcon} size={14} />
          {error}
        </p>
      )}
      <FormPage
        title={isEdit ? `Edit ${agent?.name ?? "agent"}` : "Create agent"}
        breadcrumb={["Managed Agents", "Agents", isEdit ? "Edit" : "New"]}
        sections={sections}
        cancelTo={isEdit && id ? `/agents/${id}` : "/agents"}
        primaryLabel={saving ? "Saving…" : isEdit ? "Save" : "Create agent"}
        onSubmit={(e) => void handleSubmit(e)}
      />
    </div>
  );
}

export default AgentFormPage;
